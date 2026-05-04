#!/usr/bin/env python3
"""End-to-end reader+judge harness for MultiHop-RAG retrievals.

Takes the JSONL output of `target/release/multihop_rag --emit-retrievals`,
runs each query through a reader LLM with the top-K retrieval snippets,
then judges the answer against gold via a second LLM call.

The MultiHop-RAG paper grades exact-match against numeric / span answers
plus LLM-judge for free-form. We use a single LLM judge prompt that
handles both numeric and free-form (same as our LongMemEval pipeline).

Usage:
  python3 scripts/multihop_rag_reader.py \
      --retrievals /tmp/wg_multihop_full.jsonl \
      --reader MiniMax-M2.7-highspeed --judge MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 --reader-api-key-env MINIMAX_API_KEY \
      --judge-base-url https://api.minimax.io/v1 --judge-api-key-env MINIMAX_API_KEY \
      --workers 6 --out /tmp/wg_multihop_eval
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import _call_openai, _extract_text  # noqa: E402


READER_PROMPT = """\
You are answering a question using snippets retrieved from a news \
corpus. Each snippet is from one news article (title + excerpt). The \
question often requires COMBINING information from multiple articles \
— compare what each article says, look for the bridging detail, then \
answer.

Question: {query}

Retrieved snippets (top {n}):
{snippets}

Answer concisely:
* Yes/no questions → "Yes." or "No." with a one-line justification \
quoting both articles when comparison is required.
* "Who/what/when/where" → just the entity or value.
* Comparison questions → answer the comparison directly using \
specifics from BOTH articles. Don't say "Insufficient evidence" if \
the snippets contain BOTH sides of the comparison — combine them.
* Only use "Insufficient evidence." when the snippets clearly do \
not mention the topic at all. Default toward attempting the answer \
from what's given; partial information is better than abstention.
"""


JUDGE_PROMPT = """\
Question: {query}
Gold answer: {gold}
Model answer: {hypothesis}

Is the model answer factually correct? It does NOT have to match the \
gold answer word-for-word — semantic equivalence counts as correct. \
For "Insufficient evidence" responses, mark CORRECT only if the gold \
answer is also "Insufficient evidence" / null / "no answer".

Reply with exactly one word: CORRECT or INCORRECT.
"""


def format_snippets(retrievals: list, max_chars_each: int = 600) -> str:
    out = []
    for h in retrievals:
        title = h.get("doc_title", "")
        date = h.get("published_at", "") or ""
        if date:
            date = f" ({date[:10]})"
        snippet = h["content"][:max_chars_each].strip()
        out.append(f"[Article: {title}{date}]\n{snippet}")
    return "\n\n".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", required=True, type=Path)
    ap.add_argument("--reader", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--judge", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--reader-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--reader-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--judge-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--judge-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--limit", type=int, default=0, help="0 = all")
    ap.add_argument("--only-type", default="")
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
    if args.only_type:
        rows = [r for r in rows if r["question_type"] == args.only_type]
    if args.limit:
        rows = rows[:args.limit]
    print(f"Reader: {len(rows)} queries, model={args.reader}, workers={args.workers}")

    done = set()
    if hyp_path.exists():
        for line in open(hyp_path):
            done.add(json.loads(line)["question_id"])
        print(f"  resuming reader: {len(done)} done")
    todo = [r for r in rows if r["question_id"] not in done]

    def _read_one(row):
        snippets = format_snippets(row.get("retrievals", []))
        prompt = READER_PROMPT.format(
            query=row["query"],
            n=len(row.get("retrievals", [])),
            snippets=snippets,
        )
        try:
            resp = _call_openai(
                reader_key, args.reader,
                [{"role": "user", "content": prompt}],
                4096, args.reader_base_url, temperature=0.0,
            )
            hypothesis = _extract_text(resp)
        except Exception as e:
            print(f"  ! reader fail q={row['question_id']}: {e}", file=sys.stderr)
            hypothesis = ""
        return {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "query": row["query"],
            "gold_answer": row["gold_answer"],
            "hypothesis": hypothesis,
        }

    write_lock = threading.Lock()
    with open(hyp_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(_read_one, r): r for r in todo}
        i = 0
        for fut in as_completed(futs):
            i += 1
            r = fut.result()
            with write_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 100 == 0 or i == len(todo):
                print(f"    [{i:>4}/{len(todo)}]", file=sys.stderr)

    judged = set()
    if judg_path.exists():
        for line in open(judg_path):
            judged.add(json.loads(line)["question_id"])
    hyps = [json.loads(l) for l in open(hyp_path)]
    todo_j = [h for h in hyps if h["question_id"] not in judged]
    print(f"  judge: {len(todo_j)} new")

    def _judge_one(h):
        gold = h["gold_answer"]
        gold_str = json.dumps(gold) if not isinstance(gold, str) else gold
        prompt = JUDGE_PROMPT.format(
            query=h["query"],
            gold=gold_str,
            hypothesis=h["hypothesis"],
        )
        try:
            resp = _call_openai(
                judge_key, args.judge,
                [{"role": "user", "content": prompt}],
                4096, args.judge_base_url, temperature=0.0,
            )
            raw = _extract_text(resp)
        except Exception as e:
            print(f"  ! judge fail q={h['question_id']}: {e}", file=sys.stderr)
            raw = ""
        # MiniMax reasoning models emit <think>…</think>verdict — the
        # max_tokens budget can swallow the closing </think> if think
        # is long. Pull the verdict from whatever survives.
        text = raw.split("</think>", 1)[-1].strip().upper()
        if not text:
            # Fallback: scan whole raw text (think block may carry the
            # verdict reasoning even without the close tag).
            text = raw.upper()
        # CORRECT must NOT be preceded by "IN" (matches "INCORRECT").
        # Use word-boundary-ish check.
        import re as _re
        has_correct = bool(_re.search(r'(?<!IN)CORRECT', text))
        has_incorrect = "INCORRECT" in text
        # When both appear (judge reasoning mentions both), trust the
        # last one as the final verdict.
        if has_correct and has_incorrect:
            last_c = text.rfind("CORRECT")
            last_ic = text.rfind("INCORRECT")
            correct = last_c > last_ic + 2
        else:
            correct = has_correct and not has_incorrect
        return {
            "question_id": h["question_id"],
            "question_type": h["question_type"],
            "verdict_raw": raw,
            "correct": correct,
        }

    judge_lock = threading.Lock()
    with open(judg_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(_judge_one, h): h for h in todo_j}
        i = 0
        for fut in as_completed(futs):
            i += 1
            r = fut.result()
            with judge_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 100 == 0 or i == len(todo_j):
                print(f"    [{i:>4}/{len(todo_j)}]", file=sys.stderr)

    js = [json.loads(l) for l in open(judg_path)]
    ok = sum(1 for j in js if j["correct"])
    n = len(js)
    print()
    print(f"Result: reader={args.reader}, judge={args.judge}, total={n}")
    print(f"  CORRECT:    {ok}  ({ok/n:.3f})")
    print(f"  INCORRECT:  {n-ok}  ({(n-ok)/n:.3f})")
    by_t = defaultdict(lambda: [0, 0])
    for j in js:
        by_t[j["question_type"]][1] += 1
        if j["correct"]: by_t[j["question_type"]][0] += 1
    print("\n  By question_type:")
    for qt in sorted(by_t):
        o, t = by_t[qt]
        print(f"    {qt:<24} {o/t:.3f} ({o}/{t})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
