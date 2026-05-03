#!/usr/bin/env python3
"""Create N modified LongMemEval JSON files where each question's text is
replaced by its k-th sub-query (from longmemeval_decompose_queries output).

The bench then runs once per modified file, producing N retrieval JSONLs
that can be merged into a single multi-hop retrieval set.

Output: /tmp/longmemeval_subq_<i>.json for i in [1, n_subqueries]
        with only the questions present in the subqueries map (so we
        don't waste bench time on questions we won't use).

Usage:
  python3 scripts/longmemeval_apply_subqueries.py \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --subqueries /tmp/longmemeval_subqueries_120q.json \
      --out-prefix /tmp/longmemeval_subq
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--subqueries", required=True, type=Path)
    ap.add_argument("--out-prefix", required=True)
    args = ap.parse_args()

    gold = json.load(open(args.gold))
    subs = json.loads(args.subqueries.read_text())
    print(f"gold: {len(gold)} questions / sub-queries: {len(subs)} questions")

    # Determine N (max sub-query count per question).
    n_max = max(len(v) for v in subs.values()) if subs else 0
    print(f"n_subqueries (max per question): {n_max}")

    for i in range(n_max):
        out_questions = []
        for q in gold:
            qid = q["question_id"]
            if qid not in subs:
                continue
            sublist = subs[qid]
            if i >= len(sublist):
                continue
            new_q = dict(q)
            new_q["question"] = sublist[i]
            new_q["_original_question"] = q["question"]
            new_q["_sub_query_idx"] = i
            out_questions.append(new_q)
        out_path = Path(f"{args.out_prefix}_{i+1}.json")
        out_path.write_text(json.dumps(out_questions))
        print(f"  wrote {out_path} — {len(out_questions)} questions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
