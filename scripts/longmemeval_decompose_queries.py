#!/usr/bin/env python3
"""Generate per-question sub-query decompositions for multi-hop retrieval.

For each LongMemEval question, ask an LLM to decompose it into 2-4 atomic
sub-queries that target different facets / sub-topics. Each sub-query is
later issued as its own hybrid_search; the union of hits feeds the reader.

This is the DSPy RAG-Fusion pattern: instead of one search per question,
N focused sub-searches that together expand retrieval coverage. Especially
helpful when a question has implicit decomposition (e.g., "bike-related
expenses" → bike lights / helmet / maintenance / rack).

Output JSON shape:
  { "<question_id>": ["<sub-query 1>", "<sub-query 2>", ...] }

Usage:
  python3 scripts/longmemeval_decompose_queries.py \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --qids /tmp/qids120.txt \
      --out /tmp/longmemeval_subqueries_120q.json \
      --model MiniMax-M2.7-highspeed \
      --base-url https://api.minimax.io/v1 \
      --api-key-env MINIMAX_API_KEY \
      --workers 12 --n-subqueries 3
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import _call_openai, _extract_text  # noqa: E402


DECOMPOSE_PROMPT = """\
A user is asking a question about themselves; the answer comes from \
their past conversation history. Decompose this question into {n} \
SHORTER, MORE FOCUSED sub-queries that each target a SPECIFIC item, \
event, value, or fact that might appear in the conversation logs.

The goal is to RETRIEVE evidence — each sub-query will run a keyword + \
semantic search separately, and the union of their hits feeds back to \
answer the original question. So the sub-queries should be:
- Specific (target one item/event, not a category)
- Diverse (cover different sub-topics so retrieval doesn't duplicate)
- In the user's likely vocabulary (terms they would use in conversation)

Examples:

Q: "How many bike-related expenses since start of year?"
Sub-queries:
- bike lights cost
- bike helmet purchase
- bike maintenance fee
- bike rack price

Q: "How many doctors did I visit?"
Sub-queries:
- primary care doctor appointment
- dermatologist visit
- ENT specialist consultation

Q: "How many weeks did MCU + Star Wars take?"
Sub-queries:
- Marvel movies finished
- Star Wars films watched

Now decompose:

Question: {question}

Output ONLY a JSON list of {n} strings (no markdown, no explanation):
"""


def parse_subqueries(text: str, n: int) -> list:
    """Pull a JSON list out of the LLM response. Tolerant of think
    blocks, surrounding markdown, and trailing prose."""
    # Find the first '[' and last ']' — JSON list.
    start = text.find("[")
    end = text.rfind("]")
    if start < 0 or end <= start:
        return []
    chunk = text[start : end + 1]
    try:
        arr = json.loads(chunk)
        if isinstance(arr, list):
            return [str(x).strip() for x in arr if x][:n]
    except json.JSONDecodeError:
        pass
    return []


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--qids", type=Path, help="Optional file with space-separated question_ids to filter to")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--model", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=12)
    ap.add_argument("--n-subqueries", type=int, default=3)
    args = ap.parse_args()

    api_key = os.environ.get(args.api_key_env, "")
    if not api_key:
        print(f"error: {args.api_key_env} not set", file=sys.stderr)
        return 2

    questions = json.load(open(args.gold))
    if args.qids:
        with open(args.qids) as f:
            wanted = set(f.read().split())
        questions = [q for q in questions if q["question_id"] in wanted]
    print(f"Decomposing {len(questions)} questions into {args.n_subqueries} sub-queries each")

    # Resume: load existing decompositions from out file if any.
    existing = {}
    if args.out.exists():
        try:
            existing = json.loads(args.out.read_text())
            print(f"  resuming with {len(existing)} cached decompositions")
        except Exception:
            existing = {}

    todo = [q for q in questions if q["question_id"] not in existing]
    if not todo:
        print(f"  all {len(questions)} already decomposed; writing existing")
        args.out.write_text(json.dumps(existing, indent=2))
        return 0

    def _one(q):
        prompt = DECOMPOSE_PROMPT.format(question=q["question"], n=args.n_subqueries)
        try:
            resp = _call_openai(
                api_key, args.model,
                [{"role": "user", "content": prompt}],
                2048, args.base_url, temperature=0.0,
            )
            text = _extract_text(resp)
            subs = parse_subqueries(text, args.n_subqueries)
            if not subs:
                # Fall back to using the original question as a single
                # sub-query — better than dropping the question.
                subs = [q["question"]]
            return q["question_id"], subs
        except Exception as e:
            print(f"  ! decompose fail {q['question_id']}: {e}", file=sys.stderr)
            return q["question_id"], [q["question"]]

    out = dict(existing)
    write_lock = threading.Lock()
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_one, q): q for q in todo}
        i = 0
        for fut in as_completed(futures):
            qid, subs = fut.result()
            with write_lock:
                out[qid] = subs
                # Periodic checkpoint so restarting works.
                if i % 20 == 0:
                    args.out.write_text(json.dumps(out, indent=2))
            i += 1
            if i % 10 == 0 or i == len(todo):
                print(f"    [{i:>4}/{len(todo)}] {qid}: {subs}", file=sys.stderr)

    args.out.write_text(json.dumps(out, indent=2))
    print(f"wrote: {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
