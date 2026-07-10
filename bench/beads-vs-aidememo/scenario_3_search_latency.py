#!/usr/bin/env python3
"""Scenario #3 — single-query search latency, aidememo vs beads.

Both tools have a `search` subcommand and a `--json` output mode. We
issue 100 random queries (sampled from the seeded corpus's title
tokens so every query is guaranteed to hit) against the stores
populated by scenario #1 and report p50/p95/max latency.

Caveats:
  - aidememo's hybrid search is BM25 + (optional) embeddings. We turn off
    semantic by querying the BM25-only path (default config).
  - bd's search is plain SQL LIKE on title. No relevance ranking.
  - Cold-start cost is dominated by Dolt vs redb open. We measure
    end-to-end CLI latency: that IS the agent-perceived latency.
"""

from __future__ import annotations

import json
import os
import random
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

WG = os.environ.get("AIDEMEMO_BIN", "aidememo")
BD = os.environ.get("BD_BIN", "bd")
AIDEMEMO_STORE = "/tmp/aidememo-vs-beads/aidememo/wiki.sqlite"
BD_DIR = "/tmp/aidememo-vs-beads/beads"
DATA = Path("bench/beads-vs-aidememo/data")
RESULTS = Path("bench/beads-vs-aidememo/results")
N_QUERIES = 100


def sample_queries(corpus_path: Path, n: int, seed: int = 42) -> list[str]:
    """Pull `n` mid-frequency tokens from titles so every query hits at
    least one record but isn't trivial single-record."""
    rng = random.Random(seed)
    tokens: dict[str, int] = {}
    for line in corpus_path.read_text().splitlines():
        if not line.strip():
            continue
        rec = json.loads(line)
        title = rec.get("title") or rec.get("content", "").splitlines()[0]
        for tok in re.findall(r"[a-z]{4,12}", title.lower()):
            tokens[tok] = tokens.get(tok, 0) + 1
    # Mid-frequency: tokens that appear in 5–80 records (avoid 1-hit and
    # truly common). Falls back to anything if the band is empty.
    pool = [t for t, c in tokens.items() if 5 <= c <= 80]
    if len(pool) < n:
        pool = list(tokens.keys())
    rng.shuffle(pool)
    return pool[:n]


def time_cmd(cmd: list[str], cwd: str | None = None) -> tuple[float, int, str]:
    t = time.perf_counter_ns()
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=30)
    elapsed = (time.perf_counter_ns() - t) / 1e6
    return elapsed, proc.returncode, proc.stdout


def percentile(xs: list[float], p: float) -> float:
    xs = sorted(xs)
    return xs[int(round((p / 100.0) * (len(xs) - 1)))]


def measure(name: str, cmds: list[list[str]], cwd: str | None = None) -> dict:
    lats = []
    failures = 0
    output_chars = []
    for cmd in cmds:
        ms, rc, out = time_cmd(cmd, cwd=cwd)
        if rc != 0:
            failures += 1
        lats.append(ms)
        output_chars.append(len(out))
    return {
        "name": name,
        "n": len(cmds),
        "failures": failures,
        "p50_ms": percentile(lats, 50),
        "p95_ms": percentile(lats, 95),
        "max_ms": max(lats),
        "wall_total_ms": sum(lats),
        "avg_output_chars": int(statistics.mean(output_chars)) if output_chars else 0,
    }


def main() -> int:
    queries = sample_queries(DATA / "corpus_aidememo.jsonl", N_QUERIES)
    print(f"# {len(queries)} queries (mid-freq tokens, seed=42)", file=sys.stderr)

    # Warmup so first-call disk-cache doesn't bias us.
    subprocess.run([WG, "--store", AIDEMEMO_STORE, "search", queries[0], "--json", "-l", "10"],
                   capture_output=True)
    subprocess.run([BD, "search", queries[0], "--json", "--limit", "10"],
                   cwd=BD_DIR, capture_output=True)

    aidememo_cmds = [[WG, "--store", AIDEMEMO_STORE, "search", q, "--json", "-l", "10"] for q in queries]
    bd_cmds = [[BD, "search", q, "--json", "--limit", "10"] for q in queries]

    print(f"# measuring aidememo…", file=sys.stderr)
    aidememo_res = measure("aidememo search", aidememo_cmds)
    print(f"#   p50={aidememo_res['p50_ms']:.1f}ms p95={aidememo_res['p95_ms']:.1f}ms",
          file=sys.stderr)

    print(f"# measuring beads…", file=sys.stderr)
    bd_res = measure("bd search", bd_cmds, cwd=BD_DIR)
    print(f"#   p50={bd_res['p50_ms']:.1f}ms p95={bd_res['p95_ms']:.1f}ms",
          file=sys.stderr)

    out = {
        "scenario": "#3 — single-query search latency",
        "n_queries": N_QUERIES,
        "store_records": 1000,
        "aidememo": aidememo_res,
        "beads": bd_res,
        "comparison": {
            "p50_ratio_aidememo_over_beads": aidememo_res["p50_ms"] / bd_res["p50_ms"]
                if bd_res["p50_ms"] > 0 else None,
            "p95_ratio_aidememo_over_beads": aidememo_res["p95_ms"] / bd_res["p95_ms"]
                if bd_res["p95_ms"] > 0 else None,
        },
    }
    RESULTS.mkdir(parents=True, exist_ok=True)
    (RESULTS / "scenario_3.json").write_text(
        json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
