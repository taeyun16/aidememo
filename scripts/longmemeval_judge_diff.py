#!/usr/bin/env python3
"""Compare two judgement files for the same hypothesis set.

Reports:
  - Agreement rate (Cohen's kappa-friendly counts)
  - Where the two judges disagree, by category
  - Sample disagreements with hypothesis + gold so the operator
    can hand-arbitrate which judge is more accurate

Usage:
  python3 scripts/longmemeval_judge_diff.py \\
      --reader gpt-4o \\
      --hypotheses /tmp/aidememo_e2e_500_4o_v2/hypotheses_gpt-4o.jsonl \\
      --judge-a /tmp/aidememo_e2e_500_4o_v2/judgements_gpt-4o_judge_gpt-4o-mini.jsonl \\
      --judge-b /tmp/aidememo_e2e_500_4o_v2_4ojudge/judgements_gpt-4o_judge_gpt-4o.jsonl \\
      --gold /tmp/longmemeval/longmemeval_s_cleaned.json \\
      --samples 5
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--reader", required=True)
    ap.add_argument("--hypotheses", required=True, type=Path)
    ap.add_argument("--judge-a", required=True, type=Path)
    ap.add_argument("--judge-b", required=True, type=Path)
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--samples", type=int, default=5)
    args = ap.parse_args()

    hyp = {h["question_id"]: h for h in (json.loads(l) for l in open(args.hypotheses))}
    a = {j["question_id"]: j for j in (json.loads(l) for l in open(args.judge_a))}
    b = {j["question_id"]: j for j in (json.loads(l) for l in open(args.judge_b))}
    gold = {q["question_id"]: q["answer"] for q in json.load(open(args.gold))}

    common = sorted(set(a) & set(b))
    print(f"Reader: {args.reader}")
    print(f"  judge A: {args.judge_a.parent.name}")
    print(f"  judge B: {args.judge_b.parent.name}")
    print(f"  shared questions: {len(common)}")

    a_correct = sum(1 for q in common if a[q]["correct"])
    b_correct = sum(1 for q in common if b[q]["correct"])
    print(f"  A says CORRECT: {a_correct} ({a_correct/len(common):.3f})")
    print(f"  B says CORRECT: {b_correct} ({b_correct/len(common):.3f})")

    agree = sum(1 for q in common if a[q]["correct"] == b[q]["correct"])
    a_only = sum(1 for q in common if a[q]["correct"] and not b[q]["correct"])
    b_only = sum(1 for q in common if not a[q]["correct"] and b[q]["correct"])
    print(f"  Agreement:    {agree} / {len(common)} ({agree/len(common):.3f})")
    print(f"    A correct, B wrong:  {a_only}")
    print(f"    B correct, A wrong:  {b_only}")
    print()

    # Per-category disagreement.
    by_type: dict[str, list[bool]] = {}
    for q in common:
        qt = a[q]["question_type"]
        by_type.setdefault(qt, []).append(a[q]["correct"] == b[q]["correct"])
    print("  Agreement by question_type:")
    for qt in sorted(by_type):
        arr = by_type[qt]
        print(f"    {qt:30}  {sum(arr)/len(arr):.3f}  ({sum(arr)}/{len(arr)})")
    print()

    # Sample disagreements — those rows where the operator should
    # eyeball the hypothesis vs gold to decide which judge is right.
    print(f"  Sample disagreements (up to {args.samples} of each direction):")
    for direction, sym in (("A says CORRECT, B says wrong", "a_only"), ("B says CORRECT, A says wrong", "b_only")):
        print(f"    --- {direction} ---")
        n = 0
        for q in common:
            if sym == "a_only" and a[q]["correct"] and not b[q]["correct"]:
                pass
            elif sym == "b_only" and not a[q]["correct"] and b[q]["correct"]:
                pass
            else:
                continue
            print(f"    qid={q}  type={a[q]['question_type']}")
            print(f"      gold:       {str(gold.get(q, ''))[:100]!r}")
            print(f"      hypothesis: {hyp[q]['hypothesis'][:120]!r}")
            print(f"      A verdict:  {a[q]['verdict_raw']!r}")
            print(f"      B verdict:  {b[q]['verdict_raw']!r}")
            n += 1
            if n >= args.samples:
                break


if __name__ == "__main__":
    main()
