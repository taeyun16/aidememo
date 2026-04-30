#!/usr/bin/env python3
"""Aggregate every E2E judgement file under a directory tree into a
single comparison table — overall accuracy + per-category — for the
README / .notes write-up.

Walks /tmp/wg_e2e_500_*/judgements_*.jsonl by default; each judgement
filename encodes the reader + judge model so the output table can label
rows correctly.

Usage:
  python3 scripts/longmemeval_compile.py
  python3 scripts/longmemeval_compile.py --root /tmp
"""
from __future__ import annotations

import argparse
import json
import re
from collections import Counter
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=Path("/tmp"), type=Path)
    args = ap.parse_args()

    rows: list[tuple[str, str, dict]] = []  # (reader, judge, category-counts)
    for path in sorted(args.root.glob("wg_e2e_*/judgements_*.jsonl")):
        m = re.match(r"judgements_(.+)_judge_(.+)\.jsonl$", path.name)
        if not m:
            continue
        reader, judge = m.group(1), m.group(2)
        verdicts = [json.loads(line) for line in open(path)]
        cats: dict[str, list] = {}
        for v in verdicts:
            cats.setdefault(v["question_type"], []).append(v["correct"])
        rows.append((reader, judge, cats))

    if not rows:
        print(f"no judgement files under {args.root}")
        return

    # Determine column order: union of all categories, sorted by overall N.
    all_cats: Counter = Counter()
    for _, _, cats in rows:
        for c, lst in cats.items():
            all_cats[c] += len(lst)
    categories = sorted(all_cats.keys())

    # Header.
    print(f"{'reader':>16}  {'judge':>16}  {'N':>4}  {'OVERALL':>8}", end="")
    for c in categories:
        print(f"  {c[:18]:>18}", end="")
    print()
    print(f"{'-'*16}  {'-'*16}  {'-'*4}  {'-'*8}", end="")
    for _ in categories:
        print(f"  {'-'*18}", end="")
    print()

    for reader, judge, cats in rows:
        total = sum(len(v) for v in cats.values())
        ok = sum(1 for v in cats.values() for r in v if r is True)
        print(f"{reader:>16}  {judge:>16}  {total:>4}  {ok/total:>8.3f}", end="")
        for c in categories:
            sub = cats.get(c, [])
            if sub:
                acc = sum(1 for r in sub if r is True) / len(sub)
                print(f"  {acc:>18.3f}", end="")
            else:
                print(f"  {'—':>18}", end="")
        print()


if __name__ == "__main__":
    main()
