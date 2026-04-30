#!/usr/bin/env python3
"""Cross-tab a LongMemEval E2E judgement file with the wg retrieval
metadata so we can answer questions like:

  - Of the questions where retrieval R@10 was a hit, what fraction
    did the LLM still get wrong? (= reader/grading failure, not
    retrieval failure)
  - For the questions where retrieval missed, did the LLM hallucinate
    or correctly say "I don't know"?

Usage:
  python3 scripts/longmemeval_analyze.py \\
      --judgements /tmp/wg_e2e_500_mini/judgements_gpt-4o-mini_judge_gpt-4o-mini.jsonl
"""
from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--judgements", required=True, type=Path)
    args = ap.parse_args()

    rows = [json.loads(line) for line in open(args.judgements)]
    by_type = Counter(r["question_type"] for r in rows)
    print(f"Loaded {len(rows)} judgements across {len(by_type)} categories")
    print()

    # Cross: retrieval-hit (rank ≤ 10) × correctness verdict.
    cells = Counter()
    for r in rows:
        retrieval_hit = r.get("first_evidence_rank") is not None
        verdict = r["correct"]  # True/False/None
        cells[(retrieval_hit, verdict)] += 1

    print("Retrieval × Verdict matrix:")
    print(f"  {'':18}  {'CORRECT':>8} {'INCORRECT':>10} {'unparseable':>12}")
    for retrieval_hit in (True, False):
        label = "retrieval HIT" if retrieval_hit else "retrieval MISS"
        ok = cells[(retrieval_hit, True)]
        bad = cells[(retrieval_hit, False)]
        unk = cells[(retrieval_hit, None)]
        print(f"  {label:18}  {ok:>8} {bad:>10} {unk:>12}")
    print()

    # Bottlenecks: where does each category lose accuracy?
    print("Per-category breakdown:")
    print(
        f"  {'category':30}  {'total':>5}  {'CORRECT':>8} "
        f"{'r-MISS':>7}  {'r-HIT-but-WRONG':>16}"
    )
    for qt in sorted(by_type):
        sub = [r for r in rows if r["question_type"] == qt]
        n = len(sub)
        ok = sum(1 for r in sub if r["correct"])
        retrieval_misses = sum(1 for r in sub if r.get("first_evidence_rank") is None)
        r_hit_wrong = sum(
            1
            for r in sub
            if r.get("first_evidence_rank") is not None and r["correct"] is False
        )
        print(
            f"  {qt:30}  {n:>5}  {ok:>8} ({ok/n:.3f})  "
            f"{retrieval_misses:>7}  {r_hit_wrong:>16}"
        )


if __name__ == "__main__":
    main()
