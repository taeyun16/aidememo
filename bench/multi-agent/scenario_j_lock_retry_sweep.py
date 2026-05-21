#!/usr/bin/env python3
"""Scenario J — serverless shared-store lock-retry sweep.

Scenario D verifies the fail-fast redb lock contract and Scenario E
verifies the long-lived `wg mcp-serve` pattern. Scenario J answers a
more product-facing question: how far can the serverless CLI path go
with `store.lock_retry_ms` before users should switch to a shared
daemon/server?

It runs fresh `wg fact add` subprocesses from 1/2/4/8 concurrent
processes against the same store with two config profiles:

  - retry 0 ms: old fail-fast behaviour.
  - retry 5000 ms: recommended short local contention smoothing.

No LLM is involved. Results are written to
`bench/multi-agent/results/scenario_j.json`.
"""

from __future__ import annotations

import json
import multiprocessing as mp
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
WG = os.environ.get("WG_BIN", str(REPO / "target" / "debug" / "wg"))
BASE = Path(os.environ.get("WG_E2E_BASE", str(Path(tempfile.gettempdir()) / "wg-e2e-j")))
PROCESSES = [
    int(x)
    for x in os.environ.get("WG_E2E_SWEEP_PROCESSES", "1,2,4,8").split(",")
    if x.strip()
]
RETRIES = [
    int(x)
    for x in os.environ.get("WG_E2E_SWEEP_RETRY_MS", "0,5000").split(",")
    if x.strip()
]
N_PER_PROC = int(os.environ.get("WG_E2E_N_PER_PROC", "10"))


@dataclass
class SweepCase:
    processes: int
    retry_ms: int
    store: Path
    home: Path


def reset_dir(path: Path) -> None:
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)


def write_config(home: Path, retry_ms: int) -> None:
    env = os.environ.copy()
    env["HOME"] = str(home)
    env.pop("WG_STORE", None)
    proc = subprocess.run(
        [WG, "config", "set", "store.lock_retry_ms", str(retry_ms)],
        capture_output=True,
        text=True,
        timeout=15,
        env=env,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"failed to write config for retry={retry_ms}: {proc.stderr}")


def percentile(values: list[float], p: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[int(round((p / 100.0) * (len(ordered) - 1)))]


def is_expected_lock_error(error: str) -> bool:
    lower = error.lower()
    return "lock" in lower or "database already open" in lower or "cannot acquire" in lower


def write_one(args: tuple[str, str, int, int]) -> dict[str, Any]:
    store, home, proc_idx, fact_idx = args
    content = f"sweep/p{proc_idx}/f{fact_idx}/{os.urandom(4).hex()}"
    env = os.environ.copy()
    env["HOME"] = home
    env.pop("WG_STORE", None)
    start = time.perf_counter_ns()
    try:
        proc = subprocess.run(
            [
                WG,
                "--store",
                store,
                "--json",
                "fact",
                "add",
                content,
                "--type",
                "note",
                "--entities",
                f"E{proc_idx}",
            ],
            capture_output=True,
            text=True,
            timeout=30,
            env=env,
        )
    except subprocess.TimeoutExpired:
        return {"id": "", "latency_ms": -1.0, "error": "timeout"}
    elapsed = (time.perf_counter_ns() - start) / 1e6
    if proc.returncode != 0:
        return {"id": "", "latency_ms": elapsed, "error": proc.stderr.strip()[:240]}
    try:
        payload = json.loads(proc.stdout or "{}")
    except json.JSONDecodeError:
        return {"id": "", "latency_ms": elapsed, "error": f"non-json: {proc.stdout[:200]}"}
    return {"id": payload.get("id", ""), "latency_ms": elapsed, "error": ""}


def worker(args: tuple[str, str, int]) -> list[dict[str, Any]]:
    store, home, proc_idx = args
    return [write_one((store, home, proc_idx, i)) for i in range(N_PER_PROC)]


def persisted_count(store: Path, home: Path) -> tuple[int, int]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    env.pop("WG_STORE", None)
    proc = subprocess.run(
        [WG, "--store", str(store), "--json", "fact", "list", "-l", "10000"],
        capture_output=True,
        text=True,
        timeout=30,
        env=env,
    )
    if proc.returncode != 0 or not proc.stdout.strip():
        return 0, 0
    facts = json.loads(proc.stdout)
    return len(facts), len({f["id"] for f in facts})


def run_case(case: SweepCase) -> dict[str, Any]:
    reset_dir(case.store.parent)
    reset_dir(case.home)
    write_config(case.home, case.retry_ms)

    start = time.perf_counter_ns()
    with mp.Pool(case.processes) as pool:
        per_proc = pool.map(
            worker,
            [(str(case.store), str(case.home), proc_idx) for proc_idx in range(case.processes)],
        )
    wall_ms = (time.perf_counter_ns() - start) / 1e6

    flat = [item for sub in per_proc for item in sub]
    ids = [r["id"] for r in flat if r["id"]]
    errors = [r["error"] for r in flat if r["error"]]
    latencies = [float(r["latency_ms"]) for r in flat if float(r["latency_ms"]) > 0]
    persisted, unique_persisted = persisted_count(case.store, case.home)
    expected = case.processes * N_PER_PROC

    return {
        "processes": case.processes,
        "retry_ms": case.retry_ms,
        "attempted": expected,
        "returned_ids": len(ids),
        "persisted": persisted,
        "unique_persisted": unique_persisted,
        "success_rate": round(persisted / expected, 3) if expected else 0.0,
        "error_count": len(errors),
        "sample_errors": errors[:5],
        "all_errors_are_lock_errors": all(is_expected_lock_error(e) for e in errors),
        "latency_ms": {
            "p50": round(percentile(latencies, 50), 2),
            "p95": round(percentile(latencies, 95), 2),
            "max": round(max(latencies) if latencies else 0.0, 2),
        },
        "wall_ms": round(wall_ms, 2),
    }


def smooth_until(rows: list[dict[str, Any]], retry_ms: int) -> int:
    smooth = 0
    for row in sorted((r for r in rows if r["retry_ms"] == retry_ms), key=lambda r: r["processes"]):
        if row["persisted"] == row["attempted"] and row["latency_ms"]["p95"] < 1_500:
            smooth = int(row["processes"])
    return smooth


def row_at(rows: list[dict[str, Any]], retry_ms: int, processes: int) -> dict[str, Any]:
    for row in rows:
        if row["retry_ms"] == retry_ms and row["processes"] == processes:
            return row
    return {}


def main() -> int:
    reset_dir(BASE)
    rows: list[dict[str, Any]] = []
    for retry_ms in RETRIES:
        for processes in PROCESSES:
            case = SweepCase(
                processes=processes,
                retry_ms=retry_ms,
                store=BASE / f"retry-{retry_ms}" / f"p-{processes}" / "wiki.redb",
                home=BASE / f"home-retry-{retry_ms}-p-{processes}",
            )
            rows.append(run_case(case))

    by_key = {(r["retry_ms"], r["processes"]): r for r in rows}
    max_processes = max(PROCESSES)
    retry_max = max(RETRIES)
    fail_fast_max = min(RETRIES)
    smooth_at_retry_max = smooth_until(rows, retry_max)
    recommended_floor = min(4, max_processes) if 4 in PROCESSES else min(PROCESSES)
    retry_max_row = row_at(rows, retry_max, max_processes)
    fail_fast_max_row = row_at(rows, fail_fast_max, max_processes)

    invariants = {
        "single_process_always_succeeds": all(
            by_key[(retry, 1)]["persisted"] == by_key[(retry, 1)]["attempted"]
            for retry in RETRIES
            if (retry, 1) in by_key
        ),
        "retry_improves_max_concurrency_success": (
            retry_max_row["success_rate"] >= fail_fast_max_row["success_rate"]
        ),
        "retry_smooth_until_recommended_floor": smooth_at_retry_max >= recommended_floor,
        "retry_max_concurrency_mostly_persisted": retry_max_row["success_rate"] >= 0.95,
        "no_id_duplication": all(r["persisted"] == r["unique_persisted"] for r in rows),
        "fail_fast_errors_are_explicit_locks": all(
            r["all_errors_are_lock_errors"] for r in rows if r["retry_ms"] == fail_fast_max
        ),
        "no_deadlocks": all(r["wall_ms"] < 60_000 for r in rows),
    }

    out = {
        "scenario": "J — serverless lock-retry concurrency sweep",
        "base": str(BASE),
        "config": {
            "processes": PROCESSES,
            "retry_ms": RETRIES,
            "facts_per_process": N_PER_PROC,
        },
        "rows": rows,
        "measurements": {
            "smooth_until_processes_at_retry_5000": smooth_until(rows, 5000),
            "serverless_recommended_until_processes": smooth_at_retry_max,
            "max_processes": max_processes,
            "retry_5000_max_success_rate": by_key.get((5000, max_processes), {}).get("success_rate"),
            "retry_0_max_success_rate": by_key.get((0, max_processes), {}).get("success_rate"),
        },
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_j.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
