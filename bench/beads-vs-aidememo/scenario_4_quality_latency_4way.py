#!/usr/bin/env python3
"""Scenario #4 — 4-way latency + result-set quality.

Modes:
  1. aidememo --bm25      : new lazy fast path. No model load.
  2. aidememo (default)   : hybrid_search; with HNSW sidecar present uses
                      hybrid_search_with_hnsw, otherwise BM25 fallback
                      that still embeds the query.
  3. aidememo --no-hnsw   : forces the BM25-fallback hybrid path. Lets us
                      isolate "model load + BM25 fallback" from "model
                      load + HNSW lookup". Implemented here by
                      temporarily renaming the sidecar.
  4. bd search      : SQL LIKE on title.

Quality metric:
  - top-5 hit ID set per query, per mode.
  - For each mode, overlap@5 against the BM25-only baseline.
    HNSW that returns DIFFERENT hits than BM25 means embeddings
    surfaced a semantic neighbour BM25 missed (recall diversity).
    Same hits = HNSW is paying the model-load tax for nothing.

Notes:
  - Data is synthetic lorem; "relevance ground truth" doesn't exist.
    overlap is the only honest quality metric we can compute.
  - Each measurement spawns a fresh CLI — agent-perceived latency.
"""

from __future__ import annotations

import json
import os
import random
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

WG = os.environ.get("AIDEMEMO_BIN", "/Users/mixlink/.local/bin/aidememo")
BD = os.environ.get("BD_BIN", "/opt/homebrew/bin/bd")
AIDEMEMO_STORE = "/tmp/aidememo-vs-beads/aidememo/wiki.sqlite"
AIDEMEMO_HNSW = Path("/tmp/aidememo-vs-beads/aidememo/wiki.hnsw.bin")
BD_DIR = "/tmp/aidememo-vs-beads/beads"
DATA = Path("bench/beads-vs-aidememo/data")
RESULTS = Path("bench/beads-vs-aidememo/results")
N_QUERIES = 50  # 4 modes × 50 ≈ 200 calls


def sample_queries(corpus_path: Path, n: int, seed: int = 42) -> list[str]:
    rng = random.Random(seed)
    tokens: dict[str, int] = {}
    for line in corpus_path.read_text().splitlines():
        if not line.strip():
            continue
        rec = json.loads(line)
        title = rec.get("title") or rec.get("content", "").splitlines()[0]
        for tok in re.findall(r"[a-z]{4,12}", title.lower()):
            tokens[tok] = tokens.get(tok, 0) + 1
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


def parse_aidememo_ids(stdout: str) -> list[str]:
    try:
        hits = json.loads(stdout)
        return [h.get("fact_id", "") for h in hits if isinstance(h, dict)]
    except json.JSONDecodeError:
        return []


def parse_bd_ids(stdout: str) -> list[str]:
    try:
        hits = json.loads(stdout)
        if isinstance(hits, list):
            return [h.get("id", "") for h in hits if isinstance(h, dict)]
    except json.JSONDecodeError:
        pass
    return []


def measure_mode(name: str, queries: list[str], cmd_for: callable,
                 parse: callable, cwd: str | None = None) -> dict:
    lats, hits_per_query, failures = [], [], 0
    for q in queries:
        ms, rc, out = time_cmd(cmd_for(q), cwd=cwd)
        if rc != 0:
            failures += 1
        lats.append(ms)
        hits_per_query.append(parse(out))
    return {
        "name": name,
        "n": len(queries),
        "failures": failures,
        "p50_ms": percentile(lats, 50),
        "p95_ms": percentile(lats, 95),
        "max_ms": max(lats),
        "wall_total_ms": sum(lats),
        "hits": hits_per_query,
    }


def overlap_at_k(a: list[list[str]], b: list[list[str]], k: int = 5) -> float:
    """Mean Jaccard overlap between top-k id sets across queries."""
    overlaps = []
    for ah, bh in zip(a, b):
        sa = set(ah[:k])
        sb = set(bh[:k])
        if not sa and not sb:
            overlaps.append(1.0)
            continue
        union = sa | sb
        if not union:
            continue
        overlaps.append(len(sa & sb) / len(union))
    return sum(overlaps) / len(overlaps) if overlaps else 0.0


def main() -> int:
    queries = sample_queries(DATA / "corpus_aidememo.jsonl", N_QUERIES)
    print(f"# {len(queries)} queries", file=sys.stderr)

    has_sidecar = AIDEMEMO_HNSW.exists()
    print(f"# HNSW sidecar present: {has_sidecar}", file=sys.stderr)
    if not has_sidecar:
        print("# build sidecar first: aidememo --store … vector-rebuild", file=sys.stderr)
        return 2

    # warmup
    subprocess.run([WG, "--store", AIDEMEMO_STORE, "search", queries[0],
                    "--bm25", "--json", "-l", "5"], capture_output=True)
    subprocess.run([BD, "search", queries[0], "--json", "--limit", "5"],
                   cwd=BD_DIR, capture_output=True)

    # Mode 1: BM25 (lazy fast path)
    print("# aidememo --bm25…", file=sys.stderr)
    m1 = measure_mode(
        "aidememo --bm25",
        queries,
        lambda q: [WG, "--store", AIDEMEMO_STORE, "search", q, "--bm25", "--json", "-l", "5"],
        parse_aidememo_ids,
    )
    print(f"#   p50={m1['p50_ms']:.1f}ms p95={m1['p95_ms']:.1f}ms",
          file=sys.stderr)

    # Mode 2: HNSW path (sidecar in place — current default)
    print("# aidememo (HNSW hybrid)…", file=sys.stderr)
    m2 = measure_mode(
        "aidememo (HNSW hybrid)",
        queries,
        lambda q: [WG, "--store", AIDEMEMO_STORE, "search", q, "--json", "-l", "5"],
        parse_aidememo_ids,
    )
    print(f"#   p50={m2['p50_ms']:.1f}ms p95={m2['p95_ms']:.1f}ms",
          file=sys.stderr)

    # Mode 3: BM25-fallback hybrid path. Hide the sidecar so
    # hybrid_search picks the BM25-fallback branch (still loads model
    # to embed query, just doesn't do HNSW lookup).
    backup = AIDEMEMO_HNSW.with_suffix(".bin.hidden")
    shutil.move(str(AIDEMEMO_HNSW), str(backup))
    try:
        print("# aidememo (BM25-fallback hybrid, sidecar hidden)…", file=sys.stderr)
        m3 = measure_mode(
            "aidememo (hybrid, no HNSW)",
            queries,
            lambda q: [WG, "--store", AIDEMEMO_STORE, "search", q, "--json", "-l", "5"],
            parse_aidememo_ids,
        )
        print(f"#   p50={m3['p50_ms']:.1f}ms p95={m3['p95_ms']:.1f}ms",
              file=sys.stderr)
    finally:
        shutil.move(str(backup), str(AIDEMEMO_HNSW))

    # Mode 4: bd search
    print("# bd search…", file=sys.stderr)
    m4 = measure_mode(
        "bd search",
        queries,
        lambda q: [BD, "search", q, "--json", "--limit", "5"],
        parse_bd_ids,
        cwd=BD_DIR,
    )
    print(f"#   p50={m4['p50_ms']:.1f}ms p95={m4['p95_ms']:.1f}ms",
          file=sys.stderr)

    quality = {
        # Baseline = BM25-only. Other modes' overlap@5 against it.
        "overlap_hnsw_vs_bm25": overlap_at_k(m1["hits"], m2["hits"], k=5),
        "overlap_fallback_vs_bm25": overlap_at_k(m1["hits"], m3["hits"], k=5),
        # Sanity: BM25 vs itself == 1.0 (hits should be deterministic).
        "overlap_self_bm25": overlap_at_k(m1["hits"], m1["hits"], k=5),
    }

    out = {
        "scenario": "#4 — 4-way latency + quality (overlap@5)",
        "n_queries": N_QUERIES,
        "store_records": 1000,
        "modes": [
            {**m1, "hits": None},  # drop big payload from JSON
            {**m2, "hits": None},
            {**m3, "hits": None},
            {**m4, "hits": None},
        ],
        "quality": quality,
        "ranking": {
            "p50_fastest": min(
                [(m1["p50_ms"], m1["name"]), (m2["p50_ms"], m2["name"]),
                 (m3["p50_ms"], m3["name"]), (m4["p50_ms"], m4["name"])])[1],
        },
    }
    RESULTS.mkdir(parents=True, exist_ok=True)
    (RESULTS / "scenario_4.json").write_text(
        json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
