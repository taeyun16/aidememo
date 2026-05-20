#!/usr/bin/env python3
"""Scenario E — `wg mcp-serve` HTTP mode + concurrent clients.

Companion to scenario D. D showed that multi-process stdio `wg mcp`
clients fight over redb's per-process file lock. The recommended
shared-store pattern is one long-lived `wg mcp-serve` HTTP server
with every agent talking to the same endpoint. This scenario
verifies that pattern: a single server, M concurrent HTTP clients,
each sending N JSON-RPC tools/call wg_fact_add — every insert must
land, with no IDs collisions and no lock errors (the server's redb
handle is single-process, so contention is internal serialization).

Compared to D:
  - D (stdio, default lock_retry_ms=0):  only the process that wins
    the redb lock writes successfully; others fail explicitly.
  - D (stdio, lock_retry_ms=5000):       short collisions can smooth
    out for ordinary local sharing.
  - E (mcp-serve, no retry needed):      M*N/M*N, lower wall.

E is what the docs (AGENTS.md "Multi-agent shared store") recommend
for actual multi-agent setups.
"""

from __future__ import annotations

import json
import multiprocessing as mp
import os
import socket
import subprocess
import sys
import time
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

WG = os.environ.get("WG_BIN", "/Users/mixlink/.local/bin/wg")
STORE = os.environ.get("WG_E2E_STORE", "/tmp/wg-e2e-e/wiki.redb")
PORT = int(os.environ.get("WG_E2E_PORT", "3939"))
M_CLIENTS = int(os.environ.get("WG_E2E_CLIENTS", "4"))
N_PER_CLIENT = int(os.environ.get("WG_E2E_N_PER_CLIENT", "25"))


def reset_store() -> None:
    p = Path(STORE)
    p.parent.mkdir(parents=True, exist_ok=True)
    for sib in p.parent.iterdir():
        if sib.name.startswith(p.name):
            sib.unlink()


def free_port_or(port: int) -> int:
    """Return `port` if free; otherwise pick any free port."""
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


# ----- HTTP client (multiprocessing.Pool friendly) -----

def http_post(port: int, payload: dict, timeout_s: float = 10.0) -> tuple[dict | None, str]:
    body = json.dumps(payload).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/mcp",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as r:
            data = r.read().decode()
            return json.loads(data), ""
    except Exception as exc:
        return None, str(exc)[:200]


def write_one(args: tuple[int, int, int]) -> dict:
    port, client_idx, fact_idx = args
    payload = {
        "jsonrpc": "2.0", "id": fact_idx + 1,
        "method": "tools/call",
        "params": {
            "name": "wg_fact_add",
            "arguments": {
                "content": f"http/c{client_idx}/f{fact_idx}/{os.urandom(4).hex()}",
                "entities": [f"E{client_idx}"],
            },
        },
    }
    t = time.perf_counter_ns()
    resp, err = http_post(port, payload)
    elapsed = (time.perf_counter_ns() - t) / 1e6
    if err:
        return {"id": "", "latency_ms": elapsed, "error": err}
    if resp is None or "error" in resp:
        return {"id": "", "latency_ms": elapsed,
                "error": json.dumps(resp.get("error") if resp else "no resp")[:200]}
    content = resp.get("result", {}).get("content") or []
    if content and content[0].get("type") == "text":
        try:
            return {"id": json.loads(content[0]["text"]).get("id", ""),
                    "latency_ms": elapsed, "error": ""}
        except json.JSONDecodeError:
            return {"id": "", "latency_ms": elapsed,
                    "error": f"non-json: {content[0]['text'][:100]}"}
    return {"id": "", "latency_ms": elapsed, "error": "no content"}


def client_worker(args: tuple[int, int]) -> list[dict]:
    port, client_idx = args
    return [write_one((port, client_idx, i)) for i in range(N_PER_CLIENT)]


# ----- runner -----

@dataclass
class Result:
    expected: int
    got: int
    unique_ids: int
    errors: list[str] = field(default_factory=list)
    p50_ms: float = 0.0
    p95_ms: float = 0.0
    max_ms: float = 0.0
    wall_ms: float = 0.0


def percentile(xs: list[float], p: float) -> float:
    if not xs:
        return 0.0
    xs = sorted(xs)
    return xs[int(round((p / 100.0) * (len(xs) - 1)))]


def main() -> int:
    reset_store()
    port = free_port_or(PORT)

    # Start `wg mcp-serve` in the background.
    log_path = Path(STORE).parent / "server.log"
    log_fp = open(log_path, "w")
    server = subprocess.Popen(
        [WG, "mcp-serve", "--port", str(port), STORE],
        stdout=log_fp, stderr=subprocess.STDOUT,
    )

    try:
        if not wait_for_health(port):
            print(f"server failed to come up. log:\n{log_path.read_text()}",
                  file=sys.stderr)
            return 2

        t = time.perf_counter_ns()
        with mp.Pool(M_CLIENTS) as pool:
            per_client = pool.map(client_worker, [(port, i) for i in range(M_CLIENTS)])
        wall_ms = (time.perf_counter_ns() - t) / 1e6
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
        log_fp.close()

    flat = [r for sub in per_client for r in sub]
    ids = [r["id"] for r in flat if r["id"]]
    errs = [r["error"] for r in flat if r["error"]][:5]
    lats = [r["latency_ms"] for r in flat if r["latency_ms"] > 0]

    # Cross-check from disk via a one-shot CLI list.
    out = subprocess.run(
        [WG, "--store", STORE, "fact", "list", "-l", "1000", "--json"],
        capture_output=True, text=True, timeout=15,
    )
    persisted = []
    if out.returncode == 0 and out.stdout.strip():
        try:
            persisted = json.loads(out.stdout)
        except json.JSONDecodeError:
            persisted = []

    res = Result(
        expected=M_CLIENTS * N_PER_CLIENT,
        got=len(persisted),
        unique_ids=len({f["id"] for f in persisted}),
        errors=errs,
        p50_ms=percentile(lats, 50),
        p95_ms=percentile(lats, 95),
        max_ms=max(lats) if lats else 0.0,
        wall_ms=wall_ms,
    )

    invariants = {
        "every_write_persisted": res.got == res.expected,
        "every_write_returned_an_id": len(ids) == res.expected,
        "no_id_duplication": res.unique_ids == res.got,
        "no_lock_or_http_errors": not res.errors,
        "wall_under_30s": res.wall_ms < 30_000,
    }
    out_obj = {
        "scenario": "E — mcp-serve HTTP shared writes",
        "store": STORE, "port": port,
        "concurrency": {"clients": M_CLIENTS, "per_client": N_PER_CLIENT,
                        "expected_total": res.expected},
        "result": res.__dict__,
        "invariants": invariants,
        "summary": {"passed": sum(1 for v in invariants.values() if v),
                    "total": len(invariants)},
    }
    out_path = Path("bench/multi-agent/results/scenario_e.json")
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out_obj, indent=2, ensure_ascii=False))
    print(json.dumps(out_obj, indent=2, ensure_ascii=False))
    return 0 if out_obj["summary"]["passed"] == out_obj["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
