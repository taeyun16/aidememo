#!/usr/bin/env python3
"""Scenario #5 — daemon (mcp-serve) warm path vs local fresh-spawn.

Adds two daemon-mode rows to the scenario #4 grid:
  4. aidememo --via --bm25 warm           (BM25 over HTTP, model not loaded)
  5. aidememo --via (hybrid HNSW) warm    (HNSW over HTTP, model warm)

Daemon spawns a single `aidememo mcp-serve` background process before the
measurement loop and tears it down after. Warm-up: 5 throwaway calls
to the daemon to load the model + JIT caches. Then measure N queries.

Comparison anchors:
  - aidememo --bm25 (local, fresh CLI)         from scenario #4 = ~71ms
  - bd search                             from scenario #4 = ~387ms
  - aidememo (HNSW hybrid, local fresh CLI)     from scenario #4 = ~850ms
"""

from __future__ import annotations

import json
import os
import random
import re
import socket
import statistics
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

WG = os.environ.get("AIDEMEMO_BIN", "/Users/mixlink/.local/bin/aidememo")
AIDEMEMO_STORE = "/tmp/aidememo-vs-beads/aidememo/wiki.sqlite"
DATA = Path("bench/beads-vs-aidememo/data")
RESULTS = Path("bench/beads-vs-aidememo/results")
N_QUERIES = 50


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


def free_port_or(port: int) -> int:
    with socket.socket() as s:
        try:
            s.bind(("127.0.0.1", port))
            return port
        except OSError:
            with socket.socket() as s2:
                s2.bind(("127.0.0.1", 0))
                return s2.getsockname()[1]


def wait_for_health(port: int, timeout_s: float = 10.0) -> bool:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1) as r:
                if r.status == 200:
                    return True
        except Exception:
            time.sleep(0.1)
    return False


def time_cmd(cmd: list[str]) -> float:
    t = time.perf_counter_ns()
    subprocess.run(cmd, capture_output=True, timeout=20)
    return (time.perf_counter_ns() - t) / 1e6


def percentile(xs: list[float], p: float) -> float:
    xs = sorted(xs)
    return xs[int(round((p / 100.0) * (len(xs) - 1)))]


def measure(name: str, queries: list[str], cmd_for) -> dict:
    lats = [time_cmd(cmd_for(q)) for q in queries]
    return {
        "name": name,
        "n": len(queries),
        "p50_ms": percentile(lats, 50),
        "p95_ms": percentile(lats, 95),
        "max_ms": max(lats),
        "wall_total_ms": sum(lats),
    }


def main() -> int:
    queries = sample_queries(DATA / "corpus_aidememo.jsonl", N_QUERIES)
    print(f"# {len(queries)} queries", file=sys.stderr)

    port = free_port_or(3939)
    log_path = Path("/tmp/aidememo-vs-beads/scenario5-daemon.log")
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_fp = open(log_path, "w")
    server = subprocess.Popen(
        [WG, "mcp-serve", "--port", str(port), AIDEMEMO_STORE],
        stdout=log_fp, stderr=subprocess.STDOUT,
    )
    try:
        if not wait_for_health(port):
            print(f"daemon failed: {log_path.read_text()}", file=sys.stderr)
            return 2

        # Warmup: 5 hybrid + 5 bm25 calls to load the model and any
        # caches before measurement starts.
        warm_url = f"http://127.0.0.1:{port}"
        for _ in range(5):
            subprocess.run([WG, "search", queries[0], "-l", "5", "--via", warm_url],
                           capture_output=True, timeout=20)
            subprocess.run([WG, "search", queries[0], "-l", "5", "--via", warm_url, "--bm25"],
                           capture_output=True, timeout=20)
        print("# daemon warm", file=sys.stderr)

        m_via_bm25 = measure(
            "aidememo --via --bm25 (warm daemon)",
            queries,
            lambda q: [WG, "search", q, "-l", "5", "--via", warm_url, "--bm25"],
        )
        print(f"# --via --bm25 p50={m_via_bm25['p50_ms']:.1f}ms p95={m_via_bm25['p95_ms']:.1f}ms",
              file=sys.stderr)

        m_via_hybrid = measure(
            "aidememo --via hybrid (HNSW, warm daemon)",
            queries,
            lambda q: [WG, "search", q, "-l", "5", "--via", warm_url],
        )
        print(f"# --via hybrid p50={m_via_hybrid['p50_ms']:.1f}ms p95={m_via_hybrid['p95_ms']:.1f}ms",
              file=sys.stderr)
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
        log_fp.close()

    out = {
        "scenario": "#5 — daemon warm-path vs local fresh-spawn",
        "n_queries": N_QUERIES,
        "store_records": 1000,
        "modes": [m_via_bm25, m_via_hybrid],
        "anchors_from_scenario_4": {
            "aidememo --bm25 (local fresh)": "p50 ~71 ms",
            "bd search":                "p50 ~387 ms",
            "aidememo HNSW hybrid (local fresh)": "p50 ~850 ms",
        },
    }
    RESULTS.mkdir(parents=True, exist_ok=True)
    (RESULTS / "scenario_5.json").write_text(
        json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
