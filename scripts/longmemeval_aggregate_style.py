#!/usr/bin/env python3
"""LongMemEval multi-session test for the wg_aggregate primitive.

Pulls the agent out of the synthesis loop for counting / aggregation
questions. Instead of feeding the reader 30 raw snippets and asking
it to count or sum (the failure mode our 60q hybrid bench surfaced
at 50% multi-session), we:

  1. Pre-process each question's retrievals into an `aggregate` output
     (deduped by fact_id, with a count + per-entity grouping).
  2. Send the reader a SHORTER, STRUCTURED prompt: count first,
     enumerated items second, with strict instructions to USE the
     count if asked "how many" / "how much".

Compares to the baseline omega-style harness on the same retrievals.
Only multi-session questions are affected (other categories pass
through unchanged for parity).

Usage:
  python3 scripts/longmemeval_aggregate_style.py \
      --retrievals /tmp/wg_retrievals_60bal_hybrid_ingest.jsonl \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --reader MiniMax-M2.7-highspeed --judge MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 \
      --reader-api-key-env MINIMAX_API_KEY \
      --judge-base-url https://api.minimax.io/v1 \
      --judge-api-key-env MINIMAX_API_KEY \
      --workers 4 \
      --out /tmp/wg_aggregate_test
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
from collections import OrderedDict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

# Reuse the omega-style call/judge plumbing.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import (  # noqa: E402
    GRADE_PROMPTS,
    _call_openai,
    _extract_text,
    _grade,
    _is_counting_question,
)


# Multi-session aggregate-style reader prompt — bypasses OMEGA's
# generic multi-session rules and feeds a deterministic items list +
# explicit "count is N, use it" instruction.
AGGREGATE_PROMPT = """\
You answered a user's question by aggregating relevant items from \
their past conversations. The retrieval layer already deduplicated \
by fact and grouped by entity; you only need to read the structured \
output below and write the final answer.

# Items found ({matched_count} matching, deduped)

{items_block}

# Per-entity grouping

{groups_block}

# Question
{question}

CRITICAL — apply these rules in order:
1. If the question is "how many X": the answer is the per-entity \
group count for X (or the total matched_count if no entity narrowing \
makes sense). Do NOT recount the items list — use the count above.
2. If the question is "how much / total X" with monetary or duration \
values: extract the value from each item, write the addition \
arithmetic ("$40 + $50 + $95 = $185"), then state the total as the \
last sentence.
3. If the question asks "list / enumerate", reproduce the items \
above with their counts, deduped.
4. For non-counting questions, treat the items as the relevant \
context and answer normally.

Final answer:"""


def build_aggregate_view(retrievals: list, preview_chars: int = 160) -> dict:
    """Mirror what wg_aggregate(op=enumerate) + by_entity would emit.

    Pure post-process on the retrieval list — no actual MCP call.
    Validates whether feeding the reader a structured deduped view
    helps over feeding raw snippets, in isolation from any new
    daemon / tool plumbing.
    """
    seen: OrderedDict = OrderedDict()
    for hit in retrievals:
        fid = hit.get("fact_id")
        if not fid or fid in seen:
            continue
        seen[fid] = hit
    items = list(seen.values())

    # Per-entity grouping (primary entity = first session_id-ish or
    # source label). Use entity_names if the bench surfaced them;
    # otherwise fall back to session_id.
    groups: OrderedDict = OrderedDict()
    for hit in items:
        # In the bench retrieval JSONL, entity_names isn't surfaced
        # per-hit (the bench writes session_id + source). Use
        # session_id as the grouping key — closest stable analog.
        key = hit.get("session_id") or "(no session)"
        groups.setdefault(key, []).append(hit)

    return {
        "matched_count": len(items),
        "items": items,
        "groups": groups,
    }


def render_aggregate_prompt(view: dict, question: str, preview_chars: int = 160) -> str:
    items = view["items"]
    groups = view["groups"]
    items_block = "\n".join(
        f"- [{i+1}] (sess {hit.get('session_id') or '?'}) {hit['content'][:preview_chars]}"
        for i, hit in enumerate(items)
    )
    groups_block = "\n".join(
        f"- {key}: {len(hits)} item{'s' if len(hits) != 1 else ''}"
        for key, hits in groups.items()
    )
    return AGGREGATE_PROMPT.format(
        matched_count=view["matched_count"],
        items_block=items_block,
        groups_block=groups_block,
        question=question,
    )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", required=True, type=Path)
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--reader", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--judge", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--out", default=Path("/tmp/wg_aggregate_test"), type=Path)
    ap.add_argument("--reader-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--reader-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--judge-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--judge-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument(
        "--only-counting",
        action="store_true",
        help="Apply aggregate-style only to counting questions; pass "
        "non-counting through to the standard omega harness path. "
        "Default: aggregate-style on every multi-session question.",
    )
    args = ap.parse_args()

    reader_key = os.environ.get(args.reader_api_key_env, "")
    judge_key = os.environ.get(args.judge_api_key_env, "")
    if not reader_key or not judge_key:
        print(f"error: {args.reader_api_key_env} or {args.judge_api_key_env} not set", file=sys.stderr)
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

    # Limit to multi-session for this experiment.
    rows = [r for r in rows if r["question_type"] == "multi-session"]
    if args.only_counting:
        rows = [r for r in rows if _is_counting_question(r["question"])]
    print(f"Aggregate-style harness: {len(rows)} multi-session questions | reader={args.reader}")

    # ---- Stage A: reader ----
    done = set()
    if hyp_path.exists():
        for line in open(hyp_path):
            done.add(json.loads(line)["question_id"])
    todo = [r for r in rows if r["question_id"] not in done]
    print(f"  reader: {len(todo)} new questions, workers={args.workers}")

    def _read_one(row):
        view = build_aggregate_view(row.get("retrievals", []))
        prompt = render_aggregate_prompt(view, row["question"])
        try:
            resp = _call_openai(
                reader_key, args.reader,
                [{"role": "user", "content": prompt}],
                2048, args.reader_base_url,
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
            "matched_count": view["matched_count"],
        }

    write_lock = threading.Lock()
    with open(hyp_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_read_one, row): row for row in todo}
        i = 0
        for fut in as_completed(futures):
            i += 1
            r = fut.result()
            with write_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 5 == 0 or i == len(todo):
                print(f"    [{i:>3}/{len(todo)}] {r['question_id']}", file=sys.stderr)

    # ---- Stage B: judge ----
    judged = set()
    if judg_path.exists():
        for line in open(judg_path):
            judged.add(json.loads(line)["question_id"])
    hyps = [json.loads(l) for l in open(hyp_path)]
    todo_j = [h for h in hyps if h["question_id"] not in judged]
    print(f"  judge: {len(todo_j)} new judgements, workers={args.workers}")

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
            if i % 5 == 0 or i == len(todo_j):
                print(f"    [{i:>3}/{len(todo_j)}] {r['question_id']}", file=sys.stderr)

    # ---- Aggregate ----
    js = [json.loads(l) for l in open(judg_path)]
    ok = sum(1 for j in js if j["correct"] is True)
    bad = sum(1 for j in js if j["correct"] is False)
    unk = sum(1 for j in js if j["correct"] is None)
    n = ok + bad + unk
    print()
    print(f"Aggregate-style result: {args.reader} reader, {args.judge} judge, multi-session only")
    print(f"  total: {n}")
    print(f"  CORRECT:    {ok}  ({ok/n:.3f})")
    print(f"  INCORRECT:  {bad}  ({bad/n:.3f})")
    print(f"  unparseable {unk}  ({unk/n:.3f})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
