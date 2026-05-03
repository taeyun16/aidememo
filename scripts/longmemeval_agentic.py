#!/usr/bin/env python3
"""Agentic-loop harness for LongMemEval.

Tests whether the multi-session ceiling (~60% across all single-call
interventions) lifts when the reader can call deterministic
aggregation tools instead of doing arithmetic in its head. Mirrors
the RLM (Recursive Language Models) pattern from
https://alexzhang13.github.io/blog/2026/longcot-rlm/ — decomposition
+ deterministic aggregator tool, not deeper internal CoT.

Architecture per question:
  1. Reader sees question + retrieved session blocks + tool catalogue.
  2. Reader emits either a tool call (JSON) or a final answer.
  3. Tool executes on the retrievals' structured field (currency /
     duration / event_date pre-extracted by wg_core::extract_structured
     at bench emit time). Returns deterministic numeric result.
  4. Reader sees tool result, may call again or commit answer.
  5. Loop bounded by max_iterations (default 3).

Available tools:
  * aggregate_sum_currency(query) — sum of $/etc. across matching facts
  * aggregate_sum_duration(query) — sum of durations in seconds
  * aggregate_count_distinct_dates(query) — distinct date count
  * aggregate_timeline(query) — ordered list of dated events
  * aggregate_count_facts(query, fact_type) — fact count by type
  * dump_more_context(query) — surface additional snippets

Usage:
  python3 scripts/longmemeval_agentic.py \
      --retrievals /tmp/wg_retrievals_60bal_relevance_full.jsonl \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --reader MiniMax-M2.7-highspeed --judge MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 --reader-api-key-env MINIMAX_API_KEY \
      --judge-base-url https://api.minimax.io/v1 --judge-api-key-env MINIMAX_API_KEY \
      --workers 8 --only-multi-session \
      --out /tmp/wg_agentic_60bal
"""
from __future__ import annotations

import argparse
import json
import os
import re
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
    _call_openai,
    _extract_text,
    _filter_and_sort,
    _format_session_block,
    _epoch_ms_to_iso,
    _grade,
    _is_counting_question,
)


# ------------------------ Tool implementations ---------------------------

def _filter_for_query(retrievals: list, sub_query: str, threshold: float = 0.0) -> list:
    """Filter retrievals to those whose content contains any token of
    the sub-query (case-insensitive). Cheap BM25-flavoured prefilter so
    the agent's narrow tool calls don't aggregate over off-topic facts."""
    if not sub_query.strip():
        return retrievals
    tokens = [t.lower() for t in re.findall(r"\w+", sub_query) if len(t) > 2]
    if not tokens:
        return retrievals
    out = []
    for hit in retrievals:
        text = hit.get("content", "").lower()
        if any(tok in text for tok in tokens):
            out.append(hit)
    return out


def tool_sum_currency(retrievals: list, query: str) -> dict:
    matched = _filter_for_query(retrievals, query)
    by_unit: dict = defaultdict(float)
    samples: dict = defaultdict(list)
    for hit in matched:
        for v in hit.get("structured", []):
            if v["kind"] != "currency":
                continue
            divisor = 1.0 if v["unit"] in ("KRW", "JPY") else 100.0
            by_unit[v["unit"]] += v["value"] / divisor
            samples[v["unit"]].append(v["raw"])
    return {
        "tool": "aggregate_sum_currency",
        "query": query,
        "facts_matched": len(matched),
        "by_unit": [
            {"unit": u, "total": by_unit[u], "samples": samples[u][:10]}
            for u in by_unit
        ],
    }


def tool_sum_duration(retrievals: list, query: str) -> dict:
    matched = _filter_for_query(retrievals, query)
    total_secs = 0.0
    samples = []
    for hit in matched:
        for v in hit.get("structured", []):
            if v["kind"] != "duration":
                continue
            total_secs += v["value"]
            samples.append(v["raw"])
    return {
        "tool": "aggregate_sum_duration",
        "query": query,
        "facts_matched": len(matched),
        "total_seconds": total_secs,
        "total_minutes": total_secs / 60.0,
        "total_hours": total_secs / 3600.0,
        "total_days": total_secs / 86400.0,
        "total_weeks": total_secs / (86400.0 * 7.0),
        "samples": samples[:10],
    }


def tool_count_distinct_dates(retrievals: list, query: str) -> dict:
    matched = _filter_for_query(retrievals, query)
    seen = set()
    dates = []
    for hit in matched:
        for v in hit.get("structured", []):
            if v["kind"] != "event_date":
                continue
            try:
                dt = datetime.fromtimestamp(int(v["value"]) / 1000, tz=timezone.utc).date().isoformat()
            except (ValueError, OSError):
                continue
            if dt not in seen:
                seen.add(dt)
                dates.append(dt)
    dates.sort()
    return {
        "tool": "aggregate_count_distinct_dates",
        "query": query,
        "facts_matched": len(matched),
        "distinct_count": len(dates),
        "dates": dates[:30],
    }


def tool_timeline(retrievals: list, query: str) -> dict:
    matched = _filter_for_query(retrievals, query)
    events = []
    for hit in matched:
        for v in hit.get("structured", []):
            if v["kind"] != "event_date":
                continue
            try:
                dt = datetime.fromtimestamp(int(v["value"]) / 1000, tz=timezone.utc).isoformat()
            except (ValueError, OSError):
                continue
            events.append({"date": dt, "fact_id": hit.get("fact_id"), "raw": v["raw"]})
    events.sort(key=lambda e: e["date"])
    return {
        "tool": "aggregate_timeline",
        "query": query,
        "facts_matched": len(matched),
        "events": events[:30],
    }


def tool_count_facts(retrievals: list, query: str, fact_type: str = "") -> dict:
    matched = _filter_for_query(retrievals, query)
    by_type: dict = defaultdict(int)
    for hit in matched:
        ft = hit.get("fact_type", "")
        by_type[ft] += 1
    if fact_type:
        return {
            "tool": "aggregate_count_facts",
            "query": query,
            "fact_type": fact_type,
            "matched": by_type.get(fact_type, 0),
        }
    return {
        "tool": "aggregate_count_facts",
        "query": query,
        "matched": len(matched),
        "by_type": dict(by_type),
    }


def tool_dump_more_context(retrievals: list, query: str, limit: int = 5) -> dict:
    matched = _filter_for_query(retrievals, query)
    matched.sort(key=lambda h: h.get("score", 0), reverse=True)
    return {
        "tool": "dump_more_context",
        "query": query,
        "snippets": [
            {
                "fact_id": h.get("fact_id"),
                "session_id": h.get("session_id"),
                "content_preview": h.get("content", "")[:400],
            }
            for h in matched[:limit]
        ],
    }


TOOLS = {
    "aggregate_sum_currency": tool_sum_currency,
    "aggregate_sum_duration": tool_sum_duration,
    "aggregate_count_distinct_dates": tool_count_distinct_dates,
    "aggregate_timeline": tool_timeline,
    "aggregate_count_facts": tool_count_facts,
    "dump_more_context": tool_dump_more_context,
}


def execute_tool(name: str, args: dict, retrievals: list) -> dict:
    fn = TOOLS.get(name)
    if not fn:
        return {"error": f"unknown tool '{name}'", "available": list(TOOLS.keys())}
    try:
        return fn(retrievals, **args)
    except TypeError as e:
        return {"error": f"bad args for '{name}': {e}", "expected_args": fn.__doc__ or ""}
    except Exception as e:
        return {"error": f"{name} failed: {e}"}


# ------------------------ Loop / prompt ---------------------------------

AGENTIC_SYSTEM_PROMPT = """\
You are answering a user's question about themselves using snippets from \
your past conversations.

**Most questions are answered directly from the snippets.** Read them \
carefully. If the answer is visible, COMMIT it immediately on step 1.

You may optionally call tools when the question is one of:
  - cross-session arithmetic (sum money / sum durations across sessions)
  - cross-session counting (how many distinct items / events)
  - the initial snippets clearly don't contain the answer

You have a budget of {max_iters} steps. Output STRICTLY one JSON \
object per step:

```json
{{"answer": "<final answer text>"}}
```

OR

```json
{{"tool": "<name>", "args": {{"query": "<sub-query>", ...}}}}
```

Tools (deterministic, no LLM):
* aggregate_sum_currency(query) — sums $/€/£/¥/₩ across matching facts.
* aggregate_sum_duration(query) — sums durations (returns total in seconds/minutes/hours/days/weeks).
* aggregate_count_distinct_dates(query) — count of unique dates.
* aggregate_timeline(query) — chronological dated events.
* aggregate_count_facts(query) — fact count matching the query.
* dump_more_context(query, limit=5) — surface more matching snippets. \
Use ONLY when the answer is clearly missing from initial context (do \
not call this just to "double check"; it returns the same retrieval \
pool).

Decision flow:
1. Question asks for total $ / total time / count distinct dates? → call the matching aggregate tool.
2. Answer obviously visible in initial snippets? → COMMIT.
3. Initial snippets seem off-topic AND no aggregation needed? → call dump_more_context once with refined query, then commit on the next step.

DO NOT mix narration with the JSON. Output JSON only. After \
{max_iters} steps without an answer, you forfeit. Prefer committing \
a partial answer over forfeiting.
"""


USER_INITIAL_PROMPT = """\
# Question
{question}

# Initial context (top-{n_initial} session blocks from retrieval)

{sessions}

You have {budget} step(s) left. Output JSON: tool call OR final answer.
"""


USER_TOOL_RESULT_PROMPT = """\
# Tool result for step {step}/{max_iters}

```json
{result}
```

You have {budget} step(s) left. Output JSON: tool call OR final answer.
"""


def parse_agent_output(raw: str) -> dict:
    """Pull a JSON object out of the model's output. Tolerant of code
    fences, trailing prose, and leading think blocks."""
    text = raw
    if "</think>" in text:
        text = text.split("</think>", 1)[1]
    # Find a fenced JSON block first.
    m = re.search(r"```(?:json)?\s*\n?(\{.*?\})\s*\n?```", text, re.DOTALL)
    if m:
        candidate = m.group(1)
    else:
        # Fallback: first {...} block.
        start = text.find("{")
        end = text.rfind("}")
        if start < 0 or end <= start:
            return {"_parse_error": "no JSON object found", "raw": raw[-300:]}
        candidate = text[start : end + 1]
    try:
        return json.loads(candidate)
    except json.JSONDecodeError as e:
        return {"_parse_error": str(e), "raw": raw[-300:]}


# ------------------------ Pipeline --------------------------------------

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
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--max-iterations", type=int, default=3)
    ap.add_argument(
        "--n-initial-snippets",
        type=int,
        default=0,
        help="0 = use omega category-specific max_res (recommended). "
        "Positive int = fixed cap.",
    )
    ap.add_argument(
        "--only-category",
        default="",
        help="Restrict to one category (e.g., multi-session). Empty = all.",
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
    if args.only_category:
        rows = [r for r in rows if r["question_type"] == args.only_category]
    print(f"Agentic harness: {len(rows)} questions, max_iter={args.max_iterations}, "
          f"reader={args.reader}, workers={args.workers}")

    done = set()
    if hyp_path.exists():
        for line in open(hyp_path):
            done.add(json.loads(line)["question_id"])
    todo = [r for r in rows if r["question_id"] not in done]
    print(f"  reader: {len(todo)} new")

    def _agentic_one(row):
        qdata = {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "question": row["question"],
            "question_date": row.get("question_date"),
        }
        # Build initial context (small — let agent ask for more if needed).
        qtype = qdata["question_type"]
        cfg = dict(_CATEGORY_CONFIG.get(qtype, _DEFAULT_CONFIG))
        if qtype == "multi-session" and _is_counting_question(qdata["question"]):
            cfg["max_res"] = min(30, int(cfg["max_res"] * 1.5))
        retr_copy = [dict(r) for r in row.get("retrievals", [])]
        filtered = _filter_and_sort(retr_copy, cfg)
        n_initial = args.n_initial_snippets if args.n_initial_snippets > 0 else int(cfg["max_res"])
        initial_blocks = []
        for i, r in enumerate(filtered[:n_initial], 1):
            date_iso = _epoch_ms_to_iso(r.get("referenced_date"))
            initial_blocks.append(_format_session_block(r["content"], date_iso, i))
        sessions_text = "\n\n".join(initial_blocks)

        system = AGENTIC_SYSTEM_PROMPT.format(max_iters=args.max_iterations)
        user_msg = USER_INITIAL_PROMPT.format(
            question=qdata["question"],
            n_initial=len(initial_blocks),
            sessions=sessions_text,
            budget=args.max_iterations,
        )
        history = [
            {"role": "system", "content": system},
            {"role": "user", "content": user_msg},
        ]

        hypothesis = ""
        tool_calls: list = []
        forfeit = False
        for step in range(1, args.max_iterations + 1):
            try:
                resp = _call_openai(
                    reader_key, args.reader, history,
                    4096, args.reader_base_url, temperature=0.0,
                )
                raw = _extract_text(resp)
            except Exception as e:
                print(f"  ! reader fail {row['question_id']} step {step}: {e}", file=sys.stderr)
                hypothesis = ""
                break
            parsed = parse_agent_output(raw)
            if "_parse_error" in parsed:
                # Malformed — give one chance with explicit reminder.
                history.append({"role": "assistant", "content": raw[:1000]})
                history.append({
                    "role": "user",
                    "content": "Output was not parseable JSON. Output ONLY a single JSON object: {\"tool\": ...} or {\"answer\": ...}. Nothing else.",
                })
                continue
            if "answer" in parsed:
                hypothesis = str(parsed["answer"])
                break
            if "tool" in parsed:
                tool_name = parsed["tool"]
                tool_args = parsed.get("args", {})
                if not isinstance(tool_args, dict):
                    tool_args = {"query": str(tool_args)}
                result = execute_tool(tool_name, tool_args, row.get("retrievals", []))
                tool_calls.append({"step": step, "tool": tool_name, "args": tool_args, "result_summary": str(result)[:300]})
                history.append({"role": "assistant", "content": raw[:1500]})
                budget_left = args.max_iterations - step
                history.append({
                    "role": "user",
                    "content": USER_TOOL_RESULT_PROMPT.format(
                        step=step,
                        max_iters=args.max_iterations,
                        result=json.dumps(result, indent=2)[:3000],
                        budget=budget_left,
                    ),
                })
            else:
                # Unknown JSON shape.
                history.append({"role": "assistant", "content": raw[:500]})
                history.append({
                    "role": "user",
                    "content": "JSON had no 'tool' or 'answer' key. Output {\"answer\": ...} now.",
                })
        else:
            # Loop exhausted without an answer.
            forfeit = True
            try:
                history.append({
                    "role": "user",
                    "content": "You hit the step budget. Commit your best answer now as JSON: {\"answer\": \"...\"}",
                })
                resp = _call_openai(
                    reader_key, args.reader, history,
                    4096, args.reader_base_url, temperature=0.0,
                )
                raw = _extract_text(resp)
                parsed = parse_agent_output(raw)
                if "answer" in parsed:
                    hypothesis = str(parsed["answer"])
                elif raw.strip():
                    # Last-ditch: take whatever text after </think> as the
                    # answer rather than forfeiting silently.
                    text = raw.split("</think>",1)[-1].strip()
                    hypothesis = text[:1500] if text else ""
            except Exception:
                pass

        return {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "question": row["question"],
            "hypothesis": hypothesis,
            "tool_calls": tool_calls,
            "n_tool_calls": len(tool_calls),
            "forfeit": forfeit,
        }

    write_lock = threading.Lock()
    with open(hyp_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_agentic_one, r): r for r in todo}
        i = 0
        for fut in as_completed(futures):
            i += 1
            r = fut.result()
            with write_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 5 == 0 or i == len(todo):
                print(
                    f"    [{i:>4}/{len(todo)}] {r['question_id']} "
                    f"({r['n_tool_calls']} tools, "
                    f"{'forfeit' if r['forfeit'] else 'committed'})",
                    file=sys.stderr,
                )

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
            "n_tool_calls": hyp.get("n_tool_calls", 0),
            "forfeit": hyp.get("forfeit", False),
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
                print(f"    [{i:>4}/{len(todo_j)}] {r['question_id']}", file=sys.stderr)

    js = [json.loads(l) for l in open(judg_path)]
    ok = sum(1 for j in js if j["correct"] is True)
    bad = sum(1 for j in js if j["correct"] is False)
    unk = sum(1 for j in js if j["correct"] is None)
    n = ok + bad + unk
    forfeit = sum(1 for j in js if j.get("forfeit"))
    avg_tools = sum(j.get("n_tool_calls", 0) for j in js) / max(1, n)
    print()
    print(f"Result (agentic): reader={args.reader}, judge={args.judge}, max_iter={args.max_iterations}")
    print(f"  total: {n} | forfeits: {forfeit} | avg tool calls/q: {avg_tools:.2f}")
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
