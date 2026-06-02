#!/usr/bin/env python3
"""Scenario #1 — bulk write throughput, aidememo vs beads.

Inputs: corpus_aidememo.jsonl, corpus_beads.jsonl (run gen.py first).

For each tool we measure: wall time, throughput (records/s), and
final on-disk size of the data directory.

  - aidememo side    : invokes the `aidememo_fact_add_many` MCP tool ONE TIME
                 with all N records as a JSON array. This is aidememo's
                 first-class bulk-insert path (single redb write
                 transaction).
  - beads side : `bd import < jsonl` — beads's first-class bulk
                 import path (single Dolt transaction by default).

Both backends run with their default durability so the comparison
reflects what an out-of-the-box install actually delivers.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

WG = os.environ.get("AIDEMEMO_BIN", "/Users/mixlink/.local/bin/aidememo")
BD = os.environ.get("BD_BIN", "/opt/homebrew/bin/bd")
DATA = Path(os.environ.get("BENCH_DATA", "bench/beads-vs-aidememo/data"))
RESULTS = Path(os.environ.get("BENCH_RESULTS", "bench/beads-vs-aidememo/results"))


def du_bytes(path: Path) -> int:
    """Recursive size of a directory in bytes."""
    if path.is_file():
        return path.stat().st_size
    total = 0
    for p in path.rglob("*"):
        if p.is_file():
            try:
                total += p.stat().st_size
            except OSError:
                pass
    return total


# ---------- aidememo side ----------

def aidememo_bulk_via_mcp(corpus: Path, store: Path) -> dict:
    """Issue ONE aidememo_fact_add_many MCP call with all N records."""
    items = []
    for line in corpus.read_text().splitlines():
        if not line.strip():
            continue
        rec = json.loads(line)
        items.append({"content": rec["content"], "entities": [rec["entity"]]})

    init = {"jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}}}
    call = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "aidememo_fact_add_many", "arguments": {"items": items}}}
    payload = json.dumps(init) + "\n" + json.dumps(call) + "\n"

    t = time.perf_counter_ns()
    proc = subprocess.run(
        [WG, "mcp", str(store)],
        input=payload, capture_output=True, text=True, timeout=300,
    )
    wall_ms = (time.perf_counter_ns() - t) / 1e6

    if proc.returncode != 0:
        raise RuntimeError(f"aidememo mcp failed: {proc.stderr.strip()[:300]}")

    # The mcp_tools tests show fact_add_many emits "Added N facts:" plain
    # text. We don't need the IDs here — just confirm the response landed.
    success = "Added" in proc.stdout or "added" in proc.stdout

    # Verify what's actually in the store.
    out = subprocess.run(
        [WG, "--store", str(store), "stats", "--json"],
        capture_output=True, text=True, timeout=30,
    )
    stats = json.loads(out.stdout) if out.stdout.strip() else {}

    return {
        "wall_ms": wall_ms,
        "facts_in_store": stats.get("fact_count", 0),
        "entities_in_store": stats.get("entity_count", 0),
        "throughput_per_s": (len(items) / wall_ms) * 1000.0,
        "tool_response_ok": success,
    }


# ---------- beads side ----------

def bd_bulk_via_import(corpus: Path, beads_dir: Path) -> dict:
    """`bd import` reads JSONL from stdin into a fresh embedded Dolt store.

    bd resolves the store from cwd (looking for `.beads/`), not from
    BEADS_DIR. Run every subprocess with cwd=beads_dir so each invocation
    is isolated to a fresh `.beads/` inside that directory.
    """
    payload = corpus.read_text()
    n_records = sum(1 for line in payload.splitlines() if line.strip())

    cwd = str(beads_dir)

    # Initialise the dolt store (idempotent; --stealth keeps it git-free).
    init = subprocess.run([BD, "init", "--quiet", "--stealth"],
                          cwd=cwd, capture_output=True, text=True, timeout=30)
    if init.returncode != 0:
        raise RuntimeError(f"bd init failed: {init.stderr.strip()}")

    t = time.perf_counter_ns()
    proc = subprocess.run([BD, "import", "-"],
                          input=payload, cwd=cwd,
                          capture_output=True, text=True, timeout=600)
    wall_ms = (time.perf_counter_ns() - t) / 1e6

    if proc.returncode != 0:
        raise RuntimeError(f"bd import failed: {proc.stderr.strip()[:300]}")

    # bd count: total issues now in the store.
    count = subprocess.run([BD, "count", "--json"],
                           cwd=cwd, capture_output=True, text=True, timeout=30)
    count_n = 0
    if count.returncode == 0 and count.stdout.strip():
        try:
            count_n = json.loads(count.stdout).get("count", 0)
        except json.JSONDecodeError:
            count_n = 0

    return {
        "wall_ms": wall_ms,
        "issues_in_store": count_n,
        "throughput_per_s": (n_records / wall_ms) * 1000.0,
        "import_stdout_tail": proc.stdout.strip()[-200:],
    }


# ---------- runner ----------

def reset_dir(p: Path) -> None:
    if p.exists():
        shutil.rmtree(p)
    p.mkdir(parents=True, exist_ok=True)


def main() -> int:
    aidememo_corpus = DATA / "corpus_aidememo.jsonl"
    bd_corpus = DATA / "corpus_beads.jsonl"
    if not aidememo_corpus.exists() or not bd_corpus.exists():
        print(f"missing corpus — run gen.py first", file=sys.stderr)
        return 2

    n_records = sum(1 for line in aidememo_corpus.read_text().splitlines() if line.strip())

    aidememo_dir = Path("/tmp/aidememo-vs-beads/aidememo")
    bd_dir = Path("/tmp/aidememo-vs-beads/beads")
    reset_dir(aidememo_dir)
    reset_dir(bd_dir)
    aidememo_store = aidememo_dir / "wiki.redb"

    print(f"# aidememo bulk insert (N={n_records}) via aidememo_fact_add_many…",
          file=sys.stderr)
    aidememo_res = aidememo_bulk_via_mcp(aidememo_corpus, aidememo_store)
    aidememo_res["disk_bytes"] = du_bytes(aidememo_dir)

    print(f"# bd bulk import (N={n_records}) via bd import…",
          file=sys.stderr)
    bd_res = bd_bulk_via_import(bd_corpus, bd_dir)
    bd_res["disk_bytes"] = du_bytes(bd_dir)

    out = {
        "scenario": "#1 — bulk write throughput",
        "n_records": n_records,
        "aidememo": aidememo_res,
        "beads": bd_res,
        "comparison": {
            "wall_speedup_aidememo_over_beads": bd_res["wall_ms"] / aidememo_res["wall_ms"]
                if aidememo_res["wall_ms"] > 0 else None,
            "disk_ratio_aidememo_over_beads": aidememo_res["disk_bytes"] / bd_res["disk_bytes"]
                if bd_res["disk_bytes"] > 0 else None,
        },
    }
    RESULTS.mkdir(parents=True, exist_ok=True)
    (RESULTS / "scenario_1.json").write_text(
        json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
