#!/usr/bin/env python3
"""HyDE preprocessor — generate a hypothetical answer for each
LongMemEval question. Bench then uses that answer's text as the
search query (its embedding is what hybrid_search compares against
the corpus), rather than the literal question.

Why: questions like "How many days passed between X and Y?" are bad
embedding queries — they're abstract, share little surface form with
the relevant turns. A hypothetical answer ("The user attended X on
2023-05-12 and Y on 2023-05-19, six days apart") looks more like the
underlying turns and pulls them in. Standard RAG trick (Gao et al.
2022, "Precise Zero-Shot Dense Retrieval without Relevance Labels").

Usage:
  set -a; source ~/.hermes/.env; set +a
  python3 scripts/longmemeval_hyde_questions.py \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --balanced-sample 10 \
      --reader MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 \
      --reader-api-key-env MINIMAX_API_KEY \
      --workers 6 \
      --out /tmp/longmemeval_60bal_hyde.json
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

sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import _call_openai, _extract_text  # noqa: E402


PROMPT = """\
You will be given a question that a user is asking about their own \
past conversations with an assistant. Write a single sentence that \
PLAUSIBLY ANSWERS the question — invent specific entities, dates, \
quantities; the goal is text that LOOKS LIKE the relevant past turn, \
not text that paraphrases the question.

Question: {question}

Output ONE line: the hypothetical answer sentence. No prefix, no \
explanation, no quotes."""


def hyde_one(api_key, model, base_url, question):
    try:
        resp = _call_openai(
            api_key, model,
            [{"role": "user", "content": PROMPT.format(question=question)}],
            512, base_url, temperature=0.0,
        )
        raw = _extract_text(resp)
    except Exception as e:
        print(f"  ! hyde fail: {e}", file=sys.stderr)
        return question  # fallback: just return the question itself
    text = raw.split("</think>", 1)[-1].strip() if "</think>" in raw else raw.strip()
    # Strip any leading "Answer:" / "A:" / quotes the model still puts in.
    for p in ("Answer:", "answer:", "A:", "Hypothetical:", "Hypothesis:"):
        if text.startswith(p):
            text = text[len(p):].strip()
    text = text.strip('"').strip("'").strip()
    return text or question


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--balanced-sample", type=int, default=10)
    ap.add_argument("--reader", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--reader-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--reader-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    api_key = os.environ.get(args.reader_api_key_env, "")
    if not api_key:
        print("error: API key not set", file=sys.stderr)
        return 2

    gold = json.load(open(args.gold))
    by_type: dict = OrderedDict()
    for q in gold:
        by_type.setdefault(q["question_type"], []).append(q)
    sample = []
    for _t, bucket in by_type.items():
        sample.extend(bucket[: args.balanced_sample])
    print(f"sample: {len(sample)} questions")

    existing: dict = {}
    if args.out.exists():
        existing = json.loads(args.out.read_text())
        print(f"  resuming: {len(existing)} on disk")
    todo = [q for q in sample if q["question_id"] not in existing]
    print(f"  new: {len(todo)}")

    results = dict(existing)
    write_lock = threading.Lock()

    def _one(q):
        h = hyde_one(api_key, args.reader, args.reader_base_url, q["question"])
        return q["question_id"], h

    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(_one, q): q for q in todo}
        i = 0
        for fut in as_completed(futs):
            try:
                qid, h = fut.result()
            except Exception as e:
                print(f"  ! job fail: {e}", file=sys.stderr)
                continue
            with write_lock:
                results[qid] = h
                i += 1
                if i % 10 == 0:
                    args.out.write_text(json.dumps(results))
                    print(f"    [{i:>3}/{len(todo)}] checkpointed", file=sys.stderr)

    args.out.write_text(json.dumps(results))
    print(f"wrote: {args.out}")
    print(f"total: {len(results)} questions")
    sample_keys = list(results.keys())[:3]
    print("\nsample HyDE outputs:")
    for k in sample_keys:
        q_text = next((q["question"] for q in sample if q["question_id"] == k), "?")
        print(f"  Q: {q_text[:120]}")
        print(f"  H: {results[k][:160]}")
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
