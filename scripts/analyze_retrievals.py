#!/usr/bin/env python3
"""Tolerance-band cross-tab between retrieval rank and reader verdict.

Imports gbrain-evals's per-K-tolerance reporting style: instead of
just R@5, we want to know "how often does the reader get it right
*conditional on* the gold evidence landing at rank ≤ K?". That gap
between retrieval R@K and reader-correct@K is the quantitative
reader-bound signal we've been describing qualitatively all session.

Inputs:
  --retrievals  bench --emit-retrievals JSONL (carries
                first_evidence_rank per question)
  --judgements  omega_style.py / multihop_rag_reader.py JSONL
                (carries `correct: bool` per question_id)

Outputs:
  R@1/R@3/R@5/R@10/R@30 (recall — does the gold land in top-K?)
  Reader-correct@K (of questions whose gold landed in top-K, what
                     fraction did the reader get right?)
  Reader-correct conditional on rank bucket (rank=1, 2-3, 4-5,
                                              6-10, 11-30, miss)

Usage:
  python3 scripts/analyze_retrievals.py \
      --retrievals /tmp/wg_retrievals_240bal_dates_full.jsonl \
      --judgements /tmp/wg_omega_240dates_run1/judgements_*.jsonl
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", required=True, type=Path,
                    help="JSONL with question_id + first_evidence_rank")
    ap.add_argument("--judgements", required=True, type=Path,
                    help="JSONL with question_id + correct bool")
    args = ap.parse_args()

    retr = {r["question_id"]: r for r in (json.loads(l) for l in open(args.retrievals))}
    judg = {j["question_id"]: j for j in (json.loads(l) for l in open(args.judgements))}

    common = sorted(set(retr) & set(judg))
    if not common:
        print("error: no overlap between retrievals + judgements", file=sys.stderr)
        return 2
    print(f"matched {len(common)} questions ({len(retr)} retrievals, {len(judg)} judgements)")

    rows = []
    for qid in common:
        r = retr[qid]
        j = judg[qid]
        rank = r.get("first_evidence_rank")
        rows.append({
            "qid": qid,
            "qtype": j.get("question_type", r.get("question_type", "?")),
            "rank": rank,
            "correct": bool(j.get("correct")),
        })

    n = len(rows)
    n_correct = sum(1 for r in rows if r["correct"])
    print(f"\nOverall: {n_correct}/{n} = {n_correct/n:.1%} correct")

    # --- Tolerance bands ---
    print(f"\n{'K':<6} {'R@K (recall)':<16} {'reader@K':<22} {'gap':<8}")
    print("-" * 56)
    for k in [1, 3, 5, 10, 30]:
        in_topk = [r for r in rows if r["rank"] is not None and r["rank"] <= k]
        n_topk = len(in_topk)
        ok_topk = sum(1 for r in in_topk if r["correct"])
        recall = n_topk / n
        reader_at_k = ok_topk / n_topk if n_topk else 0.0
        # Reader correctness restricted to questions retrieval surfaced — measures
        # how well reader USES retrievals (independent of retrieval miss rate).
        gap = recall - (n_correct / n)
        print(f"R@{k:<3} {n_topk}/{n}={recall:.1%}  {ok_topk}/{n_topk}={reader_at_k:.1%}     {gap:+.2f}")

    # --- Per-rank-bucket reader correctness ---
    buckets = [(1, 1, "rank=1"), (2, 3, "rank=2-3"), (4, 5, "rank=4-5"),
               (6, 10, "rank=6-10"), (11, 30, "rank=11-30")]
    print(f"\n{'Rank bucket':<14} {'n':<6} {'reader correct':<20}")
    print("-" * 44)
    for lo, hi, label in buckets:
        b = [r for r in rows if r["rank"] is not None and lo <= r["rank"] <= hi]
        if not b:
            continue
        ok = sum(1 for r in b if r["correct"])
        print(f"{label:<14} {len(b):<6} {ok}/{len(b)} = {ok/len(b):.1%}")
    miss = [r for r in rows if r["rank"] is None or r["rank"] > 30]
    if miss:
        ok = sum(1 for r in miss if r["correct"])
        print(f"{'miss/>30':<14} {len(miss):<6} {ok}/{len(miss)} = {ok/len(miss):.1%}")

    # --- Per-question-type ---
    print(f"\n{'qtype':<28} {'n':<5} {'R@5':<10} {'reader@5':<14} {'overall':<10}")
    print("-" * 70)
    by_type: dict = defaultdict(list)
    for r in rows:
        by_type[r["qtype"]].append(r)
    for qt in sorted(by_type):
        b = by_type[qt]
        n_q = len(b)
        in_top5 = [r for r in b if r["rank"] is not None and r["rank"] <= 5]
        ok_top5 = sum(1 for r in in_top5 if r["correct"])
        ok_overall = sum(1 for r in b if r["correct"])
        r5 = len(in_top5) / n_q
        reader5 = ok_top5 / len(in_top5) if in_top5 else 0.0
        print(f"{qt:<28} {n_q:<5} {len(in_top5)}/{n_q}={r5:.1%}  "
              f"{ok_top5}/{len(in_top5)}={reader5:.1%}    "
              f"{ok_overall}/{n_q}={ok_overall/n_q:.1%}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
