#!/usr/bin/env python3
"""LongMemEval harness that exposes Layer-1 structured-fact extracts to the reader.

The bench's `wg-benchmarks longmemeval` now emits a `structured` field
per retrieval (currency / duration / event_date / count, deterministically
parsed from raw text by `wg-core::extract_structured`). This harness
turns those typed slots into an aggregation hint block the reader sees
alongside raw snippets.

Example for "how much total bike-related expenses?":
  STRUCTURED HINTS (computed from the 12 retrieved facts):
    - Currency mentions: $40, $50, $95 (sum = $185 USD)
    - Total: $185 USD
  RAW SNIPPETS: <session blocks as before>

Reader can use the hint when it matches the question's intent. For
non-aggregation questions the hint is informational and ignored.

Compares against:
  v9 baseline (no structured hint):  83.9% ± 2.7 (4-run mean, 120q)

Usage:
  python3 scripts/longmemeval_structured.py \
      --retrievals /tmp/wg_retrievals_60bal_structured.jsonl \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --reader MiniMax-M2.7-highspeed --judge MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 --reader-api-key-env MINIMAX_API_KEY \
      --judge-base-url https://api.minimax.io/v1 --judge-api-key-env MINIMAX_API_KEY \
      --workers 16 \
      --out /tmp/wg_structured_60bal
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import (  # noqa: E402
    _CATEGORY_CONFIG,
    _DEFAULT_CONFIG,
    _build_reader_prompt,
    _call_openai,
    _extract_text,
    _filter_and_sort,
    _format_session_block,
    _epoch_ms_to_iso,
    _grade,
    _is_counting_question,
)


def _aggregate_structured(retrievals: list, relevance_threshold: float = 0.0) -> dict:
    """Walk every retrieval's structured values and compute totals.

    `relevance_threshold` filters out structured values from facts whose
    semantic similarity to the question (computed by the bench at emit
    time, stored in `relevance`) is below the threshold. Defaults to 0
    (no filtering — backwards compat). Set to ~0.4 (median observed)
    to drop off-topic facts that BM25 surfaced but are semantically
    unrelated to the question — e.g., $400 currency mention in an
    unrelated travel-quote fact bleeding into a "bike expenses" sum.

    Returns a dict with summary stats per kind. Empty if no structured
    values are present (or all filtered out).
    """
    by_kind = defaultdict(list)
    for hit in retrievals:
        rel = hit.get("relevance")
        # Skip when below threshold AND we have a relevance signal.
        # Missing relevance (legacy retrievals from before the bench
        # gained the field) passes through unfiltered.
        if rel is not None and rel < relevance_threshold:
            continue
        for v in hit.get("structured", []):
            by_kind[v["kind"]].append(v)
    out = {}
    if by_kind.get("currency"):
        # Group by ISO unit, sum minor units, format back to whole units.
        by_unit = defaultdict(list)
        for v in by_kind["currency"]:
            by_unit[v["unit"]].append(v)
        currency_sums = {}
        for unit, vs in by_unit.items():
            total_minor = sum(v["value"] for v in vs)
            # USD/EUR/GBP store cents → divide; KRW/JPY are already whole.
            divisor = 1.0 if unit in ("KRW", "JPY") else 100.0
            currency_sums[unit] = {
                "total": total_minor / divisor,
                "count": len(vs),
                "raws": [v["raw"] for v in vs],
            }
        out["currency"] = currency_sums
    if by_kind.get("duration"):
        # Sum seconds, then express in the most useful unit (auto-pick).
        secs = [v["value"] for v in by_kind["duration"]]
        total_sec = sum(secs)
        out["duration"] = {
            "total_seconds": total_sec,
            "total_days": total_sec / 86400,
            "total_weeks": total_sec / (86400 * 7),
            "count": len(secs),
            "raws": [v["raw"] for v in by_kind["duration"]],
        }
    if by_kind.get("count"):
        nums = [v["value"] for v in by_kind["count"]]
        out["count"] = {
            "total": sum(nums),
            "count": len(nums),
            "raws": [v["raw"] for v in by_kind["count"]],
        }
    if by_kind.get("event_date"):
        # Distinct sorted dates.
        seen = set()
        dates = []
        for v in by_kind["event_date"]:
            ms = int(v["value"])
            d = datetime.fromtimestamp(ms / 1000.0, tz=timezone.utc).date().isoformat()
            if d not in seen:
                seen.add(d)
                dates.append(d)
        dates.sort()
        out["event_date"] = {
            "distinct_dates": dates,
            "count": len(dates),
        }
    return out


def _format_structured_hint(agg: dict) -> str:
    """Render the aggregate as a compact "STRUCTURED HINTS" block for
    the reader prompt. Uses cap on raws to bound prompt size."""
    if not agg:
        return ""
    lines = ["# STRUCTURED HINTS (deterministically extracted from retrieved facts)"]
    if "currency" in agg:
        for unit, info in agg["currency"].items():
            raws_preview = ", ".join(info["raws"][:8])
            if len(info["raws"]) > 8:
                raws_preview += f", … ({len(info['raws']) - 8} more)"
            symbol = {"USD": "$", "KRW": "₩", "EUR": "€", "GBP": "£", "JPY": "¥"}.get(unit, unit)
            lines.append(
                f"- Currency ({unit}): {info['count']} mentions [{raws_preview}]; "
                f"sum = {symbol}{info['total']:.2f}"
            )
    if "duration" in agg:
        d = agg["duration"]
        raws_preview = ", ".join(d["raws"][:8])
        if len(d["raws"]) > 8:
            raws_preview += f", … ({len(d['raws']) - 8} more)"
        lines.append(
            f"- Duration: {d['count']} mentions [{raws_preview}]; "
            f"sum = {d['total_days']:.1f} days ({d['total_weeks']:.2f} weeks, "
            f"{d['total_seconds']/3600:.1f} hours)"
        )
    if "count" in agg:
        c = agg["count"]
        raws_preview = ", ".join(c["raws"][:8])
        if len(c["raws"]) > 8:
            raws_preview += f", … ({len(c['raws']) - 8} more)"
        lines.append(
            f"- Explicit counts: {c['count']} mentions [{raws_preview}]; "
            f"sum = {int(c['total'])} (use ONLY if the question asks 'how many items/things')"
        )
    if "event_date" in agg:
        e = agg["event_date"]
        dates_preview = ", ".join(e["distinct_dates"][:8])
        if len(e["distinct_dates"]) > 8:
            dates_preview += f", … ({len(e['distinct_dates']) - 8} more)"
        lines.append(
            f"- Event dates: {e['count']} distinct dates [{dates_preview}]"
        )
    lines.append("")
    lines.append(
        "Use the hints above when the question asks for sums, counts, totals, "
        "or date ranges. Cross-reference with the raw snippets below to "
        "verify which mentions are relevant to the question (some hints "
        "may be from off-topic facts)."
    )
    lines.append("")
    return "\n".join(lines)


def _build_structured_prompt(
    question_data: dict,
    retrievals: list,
    relevance_threshold: float = 0.0,
) -> tuple:
    """Build a reader prompt that prepends the structured-hints block
    before the standard category-aware prompt."""
    qtype = question_data["question_type"]
    cfg = dict(_CATEGORY_CONFIG.get(qtype, _DEFAULT_CONFIG))
    if qtype == "multi-session" and _is_counting_question(question_data["question"]):
        cfg["max_res"] = min(30, int(cfg["max_res"] * 1.5))

    # Filter retrievals exactly like the omega harness, then aggregate
    # ONLY over the filtered set (so off-topic facts don't pollute the
    # hint). This keeps hints aligned with what the reader sees.
    # Relevance threshold further drops facts whose semantic
    # similarity to the question is too low.
    retr_copy = [dict(r) for r in retrievals]
    filtered = _filter_and_sort(retr_copy, cfg)
    agg = _aggregate_structured(filtered, relevance_threshold=relevance_threshold)
    hint_block = _format_structured_hint(agg)

    # Build the rest exactly like the omega harness.
    prompt, max_tokens, n_used = _build_reader_prompt(question_data, retrievals)
    if hint_block:
        prompt = hint_block + "\n" + prompt
    return prompt, max_tokens, n_used


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", required=True, type=Path)
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--reader", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--judge", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--reader-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--reader-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--judge-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--judge-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=16)
    ap.add_argument(
        "--relevance-threshold",
        type=float,
        default=0.0,
        help="Filter structured values from facts whose question-similarity "
        "is below this. 0 = no filter (default). Empirically observed "
        "median ~0.37 on 60q balanced, p25 ~0.28 — try 0.4 to drop "
        "off-topic facts.",
    )
    args = ap.parse_args()

    reader_key = os.environ.get(args.reader_api_key_env, "")
    judge_key = os.environ.get(args.judge_api_key_env, "")
    if not reader_key or not judge_key:
        print("error: API keys not set", file=sys.stderr)
        return 2

    args.out.mkdir(parents=True, exist_ok=True)
    hyp_path = args.out / f"hypotheses_{args.reader}.jsonl"
    judg_path = args.out / f"judgements_{args.reader}_judge_{args.judge}.jsonl"

    rows = [json.loads(l) for l in open(args.retrievals)]
    gold = {q["question_id"]: q for q in json.load(open(args.gold))}
    for r in rows:
        g = gold.get(r["question_id"])
        if g is not None:
            r["question"] = g["question"]
            r["question_date"] = g.get("question_date")
    print(f"Structured-hint harness: {len(rows)} questions, reader={args.reader}")

    done = set()
    if hyp_path.exists():
        for line in open(hyp_path):
            done.add(json.loads(line)["question_id"])
    todo = [r for r in rows if r["question_id"] not in done]
    print(f"  reader: {len(todo)} new, workers={args.workers}")

    def _read_one(row):
        qdata = {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "question": row["question"],
            "question_date": row.get("question_date"),
        }
        prompt, max_tokens, n_used = _build_structured_prompt(
            qdata, row.get("retrievals", []),
            relevance_threshold=args.relevance_threshold,
        )
        try:
            resp = _call_openai(
                reader_key, args.reader,
                [{"role": "user", "content": prompt}],
                max_tokens, args.reader_base_url,
            )
            hypothesis = _extract_text(resp)
        except Exception as e:
            print(f"  ! reader fail {row['question_id']}: {e}", file=sys.stderr)
            hypothesis = ""
        return {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "question": row["question"],
            "hypothesis": hypothesis,
            "n_snippets_used": n_used,
        }

    write_lock = threading.Lock()
    with open(hyp_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_read_one, r): r for r in todo}
        i = 0
        for fut in as_completed(futures):
            i += 1
            r = fut.result()
            with write_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 10 == 0 or i == len(todo):
                print(f"    [{i:>4}/{len(todo)}] {r['question_id']}", file=sys.stderr)

    judged = set()
    if judg_path.exists():
        for line in open(judg_path):
            judged.add(json.loads(line)["question_id"])
    hyps = [json.loads(l) for l in open(hyp_path)]
    todo_j = [h for h in hyps if h["question_id"] not in judged]
    print(f"  judge: {len(todo_j)} new")

    def _judge_one(hyp):
        qid = hyp["question_id"]
        qdata = gold.get(qid)
        if qdata is None:
            return None
        try:
            raw, correct = _grade(qdata, hyp["hypothesis"], args.judge, judge_key, args.judge_base_url)
        except Exception as e:
            print(f"  ! judge fail {qid}: {e}", file=sys.stderr)
            raw, correct = "", None
        return {
            "question_id": qid,
            "question_type": hyp["question_type"],
            "verdict_raw": raw,
            "correct": correct,
        }

    judge_lock = threading.Lock()
    with open(judg_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_judge_one, h): h for h in todo_j}
        i = 0
        for fut in as_completed(futures):
            i += 1
            r = fut.result()
            if r is None:
                continue
            with judge_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 10 == 0 or i == len(todo_j):
                print(f"    [{i:>4}/{len(todo_j)}] {r['question_id']}", file=sys.stderr)

    js = [json.loads(l) for l in open(judg_path)]
    ok = sum(1 for j in js if j["correct"] is True)
    bad = sum(1 for j in js if j["correct"] is False)
    unk = sum(1 for j in js if j["correct"] is None)
    n = ok + bad + unk
    print()
    print(f"Result (structured-hint harness): reader={args.reader}, judge={args.judge}")
    print(f"  total: {n}")
    print(f"  CORRECT:    {ok}  ({ok/n:.3f})")
    print(f"  INCORRECT:  {bad}  ({bad/n:.3f})")
    print(f"  unparseable {unk}  ({unk/n:.3f})")
    by_type = {}
    for j in js:
        by_type.setdefault(j["question_type"], []).append(j["correct"])
    print("\n  By question_type:")
    for qt in sorted(by_type):
        vs = by_type[qt]
        okc = sum(1 for v in vs if v)
        print(f"    {qt:30}  {okc/len(vs):.3f}  ({okc}/{len(vs)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
