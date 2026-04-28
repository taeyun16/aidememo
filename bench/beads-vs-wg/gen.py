#!/usr/bin/env python3
"""Synthetic dataset generator for the wg vs beads comparison.

Produces N records with stable text shape:
  - title       64 chars
  - description 256 chars

Two outputs (same N, same content order, fixed seed):
  - corpus_wg.jsonl     {"entity": str, "content": str}
  - corpus_beads.jsonl  {"title": str, "description": str,
                         "issue_type": "task", "priority": 2}

Run:
  python3 bench/beads-vs-wg/gen.py --n 1000 --seed 42 --out bench/beads-vs-wg/data
"""

from __future__ import annotations

import argparse
import json
import random
import string
import sys
from pathlib import Path

# 5 entity buckets so that wg's entity-based search has something to
# group on (mimics a real wiki where one entity has multiple facts).
ENTITIES = ["Redis", "Postgres", "Kafka", "Envoy", "Otel"]


def lorem(rng: random.Random, n: int) -> str:
    """N-char text from a stable alphabet — easier to compare across tools
    than any natural-language source (no encoding surprises, fixed length)."""
    return "".join(rng.choice(string.ascii_lowercase + " ") for _ in range(n)).strip()


def gen(n: int, seed: int) -> list[dict]:
    rng = random.Random(seed)
    records = []
    for i in range(n):
        ent = ENTITIES[i % len(ENTITIES)]
        title = f"{ent}-{i:05d}: " + lorem(rng, 64 - len(f"{ent}-{i:05d}: "))
        desc = lorem(rng, 256)
        records.append({"entity": ent, "title": title, "description": desc})
    return records


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=1000)
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--out", default="bench/beads-vs-wg/data")
    args = ap.parse_args()

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    records = gen(args.n, args.seed)

    wg_path = out_dir / "corpus_wg.jsonl"
    bd_path = out_dir / "corpus_beads.jsonl"

    with wg_path.open("w") as f:
        for r in records:
            # wg fact body = title + "\n" + description (a representative
            # natural-language fact). Entity gets attached separately.
            f.write(json.dumps({
                "entity": r["entity"],
                "content": r["title"] + "\n" + r["description"],
            }) + "\n")

    with bd_path.open("w") as f:
        for r in records:
            f.write(json.dumps({
                "title": r["title"],
                "description": r["description"],
                "issue_type": "task",
                "priority": 2,
            }) + "\n")

    print(f"wrote {len(records)} records:")
    print(f"  {wg_path} ({wg_path.stat().st_size / 1024:.1f} KiB)")
    print(f"  {bd_path} ({bd_path.stat().st_size / 1024:.1f} KiB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
