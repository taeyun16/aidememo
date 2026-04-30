#!/usr/bin/env python3
"""Per-category retrieval diagnostics on a wg `--emit-retrievals`
JSONL file. Shows R@K + MRR + the rank distribution of the first
evidence-session hit so we can see WHERE the weak categories lose
recall (rank-2-out-of-10 vs rank-9-out-of-10 vs total miss).

Usage:
  python3 scripts/longmemeval_retrieval_diag.py \\
      --retrievals /tmp/wg_retrievals_500_bge.jsonl \\
      [--retrievals /tmp/wg_retrievals_500_decay90.jsonl  # compare]
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def aggregate(rows: list[dict]) -> dict[str, dict]:
    """Group rows by question_type and compute R@1/3/5/10 + MRR +
    rank histogram of the first evidence hit."""
    by_type: dict[str, list[int | None]] = {}
    for r in rows:
        by_type.setdefault(r["question_type"], []).append(r.get("first_evidence_rank"))
    out = {}
    for qt, ranks in by_type.items():
        n = len(ranks)
        r1 = sum(1 for x in ranks if x is not None and x <= 1) / n
        r3 = sum(1 for x in ranks if x is not None and x <= 3) / n
        r5 = sum(1 for x in ranks if x is not None and x <= 5) / n
        r10 = sum(1 for x in ranks if x is not None and x <= 10) / n
        rsum = sum(1.0 / x for x in ranks if x is not None)
        mrr = rsum / n
        # Rank distribution buckets within top-10.
        hist = [0] * 12  # 1..10, then 'miss'
        for x in ranks:
            if x is None or x > 10:
                hist[11] += 1
            else:
                hist[x] += 1
        out[qt] = {
            "n": n,
            "r1": r1,
            "r3": r3,
            "r5": r5,
            "r10": r10,
            "mrr": mrr,
            "hist": hist,  # 1-indexed; index 11 = miss
        }
    return out


def print_table(label: str, agg: dict[str, dict]) -> None:
    print(f"=== {label} ===")
    print(f"{'category':30}  {'N':>4}  {'R@1':>5}  {'R@3':>5}  {'R@5':>5}  {'R@10':>5}  {'MRR':>5}")
    overall_n = 0
    for qt, m in sorted(agg.items()):
        print(
            f"{qt:30}  {m['n']:>4}  {m['r1']:>.3f}  {m['r3']:>.3f}  "
            f"{m['r5']:>.3f}  {m['r10']:>.3f}  {m['mrr']:>.3f}"
        )
        overall_n += m["n"]
    # Overall row.
    if agg:
        all_ranks: list[int | None] = []
        for v in agg.values():
            for i in range(1, 11):
                all_ranks.extend([i] * v["hist"][i])
            all_ranks.extend([None] * v["hist"][11])
        n = len(all_ranks)
        r1 = sum(1 for x in all_ranks if x is not None and x <= 1) / n
        r3 = sum(1 for x in all_ranks if x is not None and x <= 3) / n
        r5 = sum(1 for x in all_ranks if x is not None and x <= 5) / n
        r10 = sum(1 for x in all_ranks if x is not None and x <= 10) / n
        mrr = sum(1.0 / x for x in all_ranks if x is not None) / n
        print(
            f"{'OVERALL':30}  {n:>4}  {r1:>.3f}  {r3:>.3f}  "
            f"{r5:>.3f}  {r10:>.3f}  {mrr:>.3f}"
        )
    print()


def print_rank_dist(label: str, agg: dict[str, dict], categories: list[str]) -> None:
    print(f"=== {label}  rank distribution (where the first evidence lands) ===")
    print(f"{'category':30}  " + "  ".join(f"r{i:>2}" for i in range(1, 11)) + "  miss")
    for qt in categories:
        if qt not in agg:
            continue
        m = agg[qt]
        cells = [f"{m['hist'][i]:>3}" for i in range(1, 11)]
        cells.append(f"{m['hist'][11]:>4}")
        print(f"{qt:30}  " + "  ".join(cells))
    print()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", action="append", required=True, type=Path)
    args = ap.parse_args()

    aggs = []
    for path in args.retrievals:
        rows = [json.loads(line) for line in open(path)]
        aggs.append((path.name, aggregate(rows)))

    weak = ["multi-session", "temporal-reasoning", "single-session-preference"]
    for label, agg in aggs:
        print_table(label, agg)
        print_rank_dist(label, agg, weak)


if __name__ == "__main__":
    main()
