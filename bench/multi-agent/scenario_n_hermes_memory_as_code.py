#!/usr/bin/env python3
"""Scenario N - Hermes Memory-as-Code research profile.

This zero-token scenario validates the Perplexity-style lesson we want for
Hermes: expose local memory as programmable primitives so the agent can fan out,
dedupe, check coverage, aggregate, and persist findings in one code path instead
of pushing intermediate candidate sets through token space.

The script uses the Hermes plugin SDK directly:

  1. Seed source-scoped research facts for two neighbouring projects.
  2. Fan out search requests across tool/dataset hypotheses.
  3. Flatten + dedupe hits and compute coverage by tool/fact_type.
  4. Persist derived research observations with one fact_add_many batch.
  5. Use aggregate_many to count scoped decisions/lessons without beta leakage.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
WG = os.environ.get("WG_BIN", str(REPO / "target" / "debug" / "wg"))
STORE = os.environ.get(
    "WG_E2E_STORE",
    str(Path(tempfile.gettempdir()) / "wg-e2e-n" / "hermes-memory-as-code.redb"),
)
PLUGIN_SRC = REPO / "plugins" / "hermes" / "src"
SOURCE_ID = "hermes-research-alpha"
NEIGHBOUR_SOURCE_ID = "hermes-research-beta"

sys.path.insert(0, str(PLUGIN_SRC))
os.environ["PATH"] = f"{Path(WG).parent}{os.pathsep}{os.environ.get('PATH', '')}"

from hermes_wg import WgClient, WgMemorySDK  # noqa: E402


SEED_FACTS: list[dict[str, Any]] = [
    {
        "content": "Decision: Hermes retrieval final method uses a top1_mass support gate before applying prior residual updates.",
        "fact_type": "decision",
        "entities": ["Hermes", "SupportGate"],
        "source_id": SOURCE_ID,
    },
    {
        "content": "Lesson: top1_mass leave-one-tool-out calibration reached MRR 0.4781 and stayed close to the tool oracle.",
        "fact_type": "lesson",
        "entities": ["Hermes", "SupportGate"],
        "source_id": SOURCE_ID,
    },
    {
        "content": "Error: patch and browser_vision are negative prior cases; do not apply fixed prior residual updates there.",
        "fact_type": "error",
        "entities": ["Hermes", "Patch", "BrowserVision"],
        "source_id": SOURCE_ID,
    },
    {
        "content": "Claim: search_files and browser_snapshot were positive support tools for Hermes prior residual updates.",
        "fact_type": "claim",
        "entities": ["Hermes", "SearchFiles", "BrowserSnapshot"],
        "source_id": SOURCE_ID,
    },
    {
        "content": "Decision: Beta Hermes profile uses URL query-shape gating for browsing tasks.",
        "fact_type": "decision",
        "entities": ["Hermes", "URL"],
        "source_id": NEIGHBOUR_SOURCE_ID,
    },
]


def run(cmd: list[str], *, timeout: int = 30) -> subprocess.CompletedProcess:
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\nstdout={proc.stdout[:1000]}\nstderr={proc.stderr[:1600]}"
        )
    return proc


def reset_store() -> None:
    path = Path(STORE)
    path.parent.mkdir(parents=True, exist_ok=True)
    for sibling in path.parent.iterdir():
        if sibling.name.startswith(path.name):
            if sibling.is_dir():
                shutil.rmtree(sibling)
            else:
                sibling.unlink()


def main() -> int:
    reset_store()
    if not Path(WG).exists():
        raise RuntimeError(f"WG_BIN does not exist: {WG}")

    client = WgClient(store_path=STORE, source_id=SOURCE_ID, lock_retry_ms=5000)
    # Scenario N validates the Hermes plugin's MCP-shaped programmable path.
    # Force the CLI backend even when wg-python is installed so batch writes
    # auto-create entities and aggregate calls exercise source-scoped MCP tools.
    client._py = None
    sdk = WgMemorySDK(client)

    seed_start = time.perf_counter_ns()
    seed_ids = client.fact_add_many(SEED_FACTS)
    seed_ms = (time.perf_counter_ns() - seed_start) / 1e6

    fanout_queries = [
        {"query": "Hermes top1_mass support gate", "tool": "search_query"},
        {"query": "Hermes patch browser_vision negative prior", "tool": "patch"},
        {"query": "Hermes search_files browser_snapshot positive support", "tool": "search_files"},
        {"query": "Hermes URL query-shape gating", "tool": "url", "source_id": NEIGHBOUR_SOURCE_ID},
    ]
    fanout_start = time.perf_counter_ns()
    fanout = sdk.search_many(fanout_queries, limit_per_query=6, concurrency=4)
    rows = sdk.dedupe_by_fact(sdk.flatten_hits(fanout))
    scoped_rows = sdk.filter_by_source(rows, SOURCE_ID)
    coverage = sdk.coverage_by(scoped_rows, ["tool", "fact_type"])
    groups = sdk.group_by_entity(scoped_rows)
    fanout_ms = (time.perf_counter_ns() - fanout_start) / 1e6

    derived_items = sdk.to_fact_batch(
        [
            {
                "content": "Lesson: Hermes Memory-as-Code profile should fan out retrieval, dedupe hits, then check coverage before writing conclusions.",
                "fact_type": "lesson",
                "entities": ["Hermes", "MemoryAsCode"],
            },
            {
                "content": "Decision: Hermes research loops store derived observations with fact_add_many after coverage checks.",
                "fact_type": "decision",
                "entities": ["Hermes", "MemoryAsCode"],
            },
        ],
        source_id=SOURCE_ID,
        tags=["scenario-n", "memory-as-code"],
    )
    derived_start = time.perf_counter_ns()
    derived_ids = sdk.commit_fact_batch(derived_items)
    derived_ms = (time.perf_counter_ns() - derived_start) / 1e6

    aggregates = sdk.aggregate_many(
        [
            {"query": "Hermes Memory-as-Code", "op": "count", "fact_type": "lesson"},
            {"query": "Hermes support gate decision", "op": "count", "fact_type": "decision"},
        ],
        source_id=SOURCE_ID,
        concurrency=2,
    )
    aggregate_text = json.dumps(aggregates, ensure_ascii=False)

    scoped_text = json.dumps(scoped_rows, ensure_ascii=False)
    all_text = json.dumps(rows, ensure_ascii=False)
    invariants = {
        "seed_batch_inserted": len(seed_ids) == len(SEED_FACTS),
        "fanout_query_count": len(fanout) == len(fanout_queries),
        "dedupe_reduced_or_equal": len(scoped_rows) <= sum(len(batch.get("hits") or []) for batch in fanout),
        "coverage_has_support_gate": "SupportGate" in json.dumps(groups, ensure_ascii=False),
        "derived_batch_inserted": len(derived_ids) == len(derived_items),
        "aggregate_counted_scoped_rows": all((row.get("result") or {}).get("matched", 0) >= 1 for row in aggregates),
        "scoped_rows_no_beta_url": "URL query-shape gating" not in scoped_text,
        "all_rows_include_beta_probe": "URL query-shape gating" in all_text,
        "context_payload_below_8k": len(json.dumps({"coverage": coverage, "aggregates": aggregates})) < 8000,
    }

    out = {
        "scenario": "N - Hermes Memory-as-Code research profile",
        "store": STORE,
        "backend": client.backend,
        "counts": {
            "seed_ids": len(seed_ids),
            "fanout_queries": len(fanout),
            "raw_hits": sum(len(batch.get("hits") or []) for batch in fanout),
            "deduped_rows": len(rows),
            "scoped_rows": len(scoped_rows),
            "derived_ids": len(derived_ids),
            "coverage_groups": len(coverage["groups"]),
        },
        "timing_ms": {
            "seed_batch": round(seed_ms, 2),
            "fanout_dedupe_coverage": round(fanout_ms, 2),
            "derived_batch": round(derived_ms, 2),
        },
        "coverage": coverage,
        "entity_groups": sorted(groups),
        "aggregates": aggregates,
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_n.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
