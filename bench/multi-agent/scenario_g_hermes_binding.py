#!/usr/bin/env python3
"""Scenario G — Hermes workflow_start CLI fallback vs aidememo-python path.

P3.3 asks whether Hermes should keep shelling out for workflow
trigger context packs or use the in-process ``aidememo-python`` binding
when it is installed.  This script seeds two identical stores and
runs the same four sparse tickets through:

  - CLI baseline: ``aidememo --json workflow start ...``
  - Hermes binding path: ``AideMemoClient.workflow_start(...)`` with
    ``aidememo_python.AideMemo`` loaded

The pass condition is shape parity, not byte-for-byte equality:
session/ticket IDs are expected to differ.  We compare prior counts,
search-hit counts, required context strings, and forbidden leakage.

Run after installing the local binding, for example:

  (cd crates/aidememo-python && maturin develop)
  python3 bench/multi-agent/scenario_g_hermes_binding.py
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
WG = os.environ.get("AIDEMEMO_BIN", str(REPO / "target" / "debug" / "aidememo"))
BASE = Path(os.environ.get("AIDEMEMO_E2E_BASE", str(Path(tempfile.gettempdir()) / "aidememo-e2e-g")))
AGENT_SDK_SRC = REPO / "packages" / "aidememo-agent-sdk" / "src"
CLI_STORE = str(BASE / "workflow-cli.sqlite")
PY_STORE = str(BASE / "workflow-py.sqlite")


@dataclass
class Ticket:
    name: str
    title: str
    body: str
    source: str
    source_id: str
    must_contain: list[str] = field(default_factory=list)
    must_not_contain: list[str] = field(default_factory=list)


TICKETS = [
    Ticket(
        name="redis-worker-team-a",
        title="Fix Redis timeout in worker",
        body="Worker jobs intermittently time out against Redis. The issue body has no more detail.",
        source="github:org/app#123",
        source_id="team-a",
        must_contain=["Worker job wrapper", "DNS resolution", "DNS metrics"],
        must_not_contain=["edge cache config", "Stripe signature checks"],
    ),
    Ticket(
        name="billing-webhook-team-a",
        title="Stop duplicate billing webhook processing",
        body="Stripe webhooks sometimes process the same invoice twice.",
        source="linear:ENG-456",
        source_id="team-a",
        must_contain=["idempotency keys", "Duplicate Stripe events", "signature checks"],
        must_not_contain=["Worker job wrapper", "edge cache config"],
    ),
    Ticket(
        name="redis-edge-team-b",
        title="Investigate Redis timeout on edge traffic",
        body="Edge traffic sees Redis timeout spikes after cache routing changes.",
        source="github:org/edge#77",
        source_id="team-b",
        must_contain=["edge cache config"],
        must_not_contain=["Worker job wrapper", "DNS resolution", "idempotency keys"],
    ),
    Ticket(
        name="unknown-mobile-team-c",
        title="Fix mobile dark mode flicker",
        body="The app flashes a light background before dark mode applies.",
        source="linear:MOB-9",
        source_id="team-c",
        must_contain=["Fix mobile dark mode flicker"],
        must_not_contain=["Redis timeout", "Stripe", "edge cache config"],
    ),
]


def run(cmd: list[str], *, timeout: int = 20) -> subprocess.CompletedProcess:
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\nstdout={proc.stdout[:500]}\nstderr={proc.stderr[:1000]}"
        )
    return proc


def reset_store(path: str) -> None:
    p = Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    for sibling in p.parent.iterdir():
        if sibling.name.startswith(p.name):
            if sibling.is_dir():
                shutil.rmtree(sibling)
            else:
                sibling.unlink()


def fact_add(store: str, content: str, fact_type: str, entities: list[str], source_id: str) -> str:
    cmd = [
        WG,
        "--store",
        store,
        "--json",
        "fact",
        "add",
        content,
        "--type",
        fact_type,
        "--entities",
        ",".join(entities),
        "--source-id",
        source_id,
    ]
    return json.loads(run(cmd).stdout)["id"]


def seed(store: str) -> None:
    seeds = [
        ("Decision: Redis timeout fixes must go through the Worker job wrapper.", "decision", ["Redis", "Worker"], "team-a"),
        ("Lesson: The last Worker Redis timeout was DNS resolution, not pool size.", "lesson", ["Redis", "Worker"], "team-a"),
        ("Error: Avoid increasing Redis pool size before checking DNS metrics.", "error", ["Redis", "Worker"], "team-a"),
        ("Decision: Billing webhook retries use idempotency keys at the event processor.", "decision", ["Billing", "Webhook"], "team-a"),
        ("Lesson: Duplicate Stripe events came from retry races, not queue ordering.", "lesson", ["Billing", "Webhook"], "team-a"),
        ("Error: Do not disable Stripe signature checks while debugging webhook duplicates.", "error", ["Billing", "Webhook"], "team-a"),
        ("Decision: Redis timeout fixes for edge traffic belong in edge cache config.", "decision", ["Redis", "EdgeCache"], "team-b"),
    ]
    for item in seeds:
        fact_add(store, *item)


def workflow_cli(store: str, ticket: Ticket) -> dict[str, Any]:
    cmd = [
        WG,
        "--store",
        store,
        "--json",
        "workflow",
        "start",
        ticket.title,
        "--body",
        ticket.body,
        "--source",
        ticket.source,
        "--source-id",
        ticket.source_id,
        "--limit",
        "10",
        "--depth",
        "2",
        "--recent-limit",
        "5",
    ]
    return json.loads(run(cmd).stdout)


def workflow_py(client: Any, ticket: Ticket) -> dict[str, Any]:
    return client.workflow_start(
        ticket.title,
        body=ticket.body,
        source=ticket.source,
        source_id=ticket.source_id,
        limit=10,
        depth=2,
        recent_limit=5,
    )


def timed(fn, *args) -> tuple[dict[str, Any], float]:
    start = time.perf_counter_ns()
    payload = fn(*args)
    return payload, (time.perf_counter_ns() - start) / 1e6


def percentile(xs: list[float], p: float) -> float:
    if not xs:
        return 0.0
    ordered = sorted(xs)
    return ordered[int(round((p / 100.0) * (len(ordered) - 1)))]


def normalize(ticket: Ticket, payload: dict[str, Any], latency_ms: float) -> dict[str, Any]:
    text = json.dumps(payload, ensure_ascii=False)
    leaks = {needle: needle in text for needle in ticket.must_not_contain}
    contains = {needle: needle in text for needle in ticket.must_contain}
    return {
        "ticket": ticket.name,
        "session_id_present": str(payload.get("session_id", "")).startswith("session-"),
        "ticket_fact_id_present": isinstance(payload.get("ticket_fact_id"), str),
        "latency_ms": round(latency_ms, 2),
        "context_chars": len(text),
        "search_hits": len((payload.get("context") or {}).get("search") or []),
        "prior_lessons": len(payload.get("prior_lessons") or []),
        "prior_errors": len(payload.get("prior_errors") or []),
        "relevant_decisions": len(payload.get("relevant_decisions") or []),
        "must_contain": contains,
        "forbidden_leakage_count": sum(1 for v in leaks.values() if v),
    }


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    lats = [float(r["latency_ms"]) for r in rows]
    return {
        "p50_ms": round(percentile(lats, 50), 2),
        "p95_ms": round(percentile(lats, 95), 2),
        "max_ms": round(max(lats) if lats else 0.0, 2),
        "context_max_chars": max(int(r["context_chars"]) for r in rows),
        "leakage_total": sum(int(r["forbidden_leakage_count"]) for r in rows),
    }


def main() -> int:
    try:
        import aidememo_python  # noqa: F401
    except Exception as exc:
        print(
            "aidememo_python is not importable; run `(cd crates/aidememo-python && maturin develop)` first.",
            file=sys.stderr,
        )
        print(f"import error: {exc}", file=sys.stderr)
        return 2

    sys.path.insert(0, str(AGENT_SDK_SRC))
    sys.path.insert(0, str(REPO / "plugins" / "hermes" / "src"))
    os.environ["PATH"] = f"{Path(WG).parent}:{os.environ.get('PATH', '')}"
    from hermes_aidememo.client import AideMemoClient

    reset_store(CLI_STORE)
    reset_store(PY_STORE)
    seed(CLI_STORE)
    seed(PY_STORE)

    cli_rows: list[dict[str, Any]] = []
    for ticket in TICKETS:
        payload, elapsed = timed(workflow_cli, CLI_STORE, ticket)
        cli_rows.append(normalize(ticket, payload, elapsed))

    client = AideMemoClient(store_path=PY_STORE, lock_retry_ms=5000)
    if client.backend != "aidememo-python":
        raise RuntimeError(f"expected aidememo-python backend, got {client.backend}")

    py_rows: list[dict[str, Any]] = []
    for ticket in TICKETS:
        payload, elapsed = timed(workflow_py, client, ticket)
        py_rows.append(normalize(ticket, payload, elapsed))

    parity = {}
    for cli, py in zip(cli_rows, py_rows):
        parity[cli["ticket"]] = {
            "search_hits_equal": cli["search_hits"] == py["search_hits"],
            "prior_counts_equal": (
                cli["prior_lessons"],
                cli["prior_errors"],
                cli["relevant_decisions"],
            )
            == (
                py["prior_lessons"],
                py["prior_errors"],
                py["relevant_decisions"],
            ),
            "contains_expected": all(py["must_contain"].values()),
            "no_py_leakage": py["forbidden_leakage_count"] == 0,
        }

    cli_summary = summarize(cli_rows)
    py_summary = summarize(py_rows)
    speedup = cli_summary["p50_ms"] / py_summary["p50_ms"] if py_summary["p50_ms"] else 0.0
    invariants = {
        "cli_all_sessions": all(r["session_id_present"] and r["ticket_fact_id_present"] for r in cli_rows),
        "py_all_sessions": all(r["session_id_present"] and r["ticket_fact_id_present"] for r in py_rows),
        "shape_parity": all(all(v.values()) for v in parity.values()),
        "py_leakage_zero": py_summary["leakage_total"] == 0,
        "py_p95_under_cli_p95": py_summary["p95_ms"] < cli_summary["p95_ms"],
    }

    out = {
        "scenario": "G — Hermes workflow_start CLI fallback vs aidememo-python path",
        "stores": {"cli": CLI_STORE, "aidememo_python": PY_STORE},
        "cli": {"runs": cli_rows, "summary": cli_summary},
        "aidememo_python": {"runs": py_rows, "summary": py_summary},
        "speedup_p50": round(speedup, 2),
        "parity": parity,
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_g.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
