#!/usr/bin/env python3
"""Scenario K - workflow_start parity across CLI, Python SDK, and Node SDK.

Scenario F validates the user-facing workflow contract across CLI/MCP/Hermes.
Scenario G validates Hermes' Python binding path against the CLI fallback.
Scenario K is the SDK-specific counterpart: it runs the same sparse tickets
through:

  - CLI:        wg --json workflow start ...
  - Python SDK: wg_python.WikiGraph.workflow_start(...)
  - Node SDK:   new WgStore(...).workflowStart(...)

The comparison is shape parity, not byte-for-byte equality. Session and ticket
IDs are expected to differ per store. The pass condition is that all drivers
create sessions/ticket facts, surface the same prior counts for each ticket,
include expected context, and avoid forbidden cross-source leakage.
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
from typing import Any, Callable

REPO = Path(__file__).resolve().parents[2]
WG = os.environ.get("WG_BIN", str(REPO / "target" / "debug" / "wg"))
BASE = Path(os.environ.get("WG_E2E_BASE", str(Path(tempfile.gettempdir()) / "wg-e2e-k")))
STORES = {
    "cli": str(BASE / "workflow-cli.redb"),
    "python": str(BASE / "workflow-python.redb"),
    "node": str(BASE / "workflow-node.redb"),
}

NODE_SCRIPT = r"""
const payload = JSON.parse(process.env.WG_NODE_WORKFLOW_PAYLOAD);
const { WgStore } = require(process.env.WG_NAPI_DIR);
const g = new WgStore(payload.store);
const rows = payload.tickets.map((ticket) => {
  const start = process.hrtime.bigint();
  const out = g.workflowStart(ticket.title, {
    body: ticket.body,
    source: ticket.source,
    sourceId: ticket.source_id,
    limit: 10,
    depth: 2,
    recentLimit: 5,
  });
  const elapsedMs = Number(process.hrtime.bigint() - start) / 1e6;
  return { ticket: ticket.name, payload: JSON.parse(out), latency_ms: elapsedMs };
});
process.stdout.write(JSON.stringify(rows));
"""


@dataclass
class Ticket:
    name: str
    title: str
    body: str
    source: str
    source_id: str
    must_contain: list[str] = field(default_factory=list)
    must_not_contain: list[str] = field(default_factory=list)
    expect_priors: bool = True


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
        expect_priors=False,
    ),
]


def run(cmd: list[str], *, input_text: str | None = None, timeout: int = 30) -> subprocess.CompletedProcess:
    proc = subprocess.run(
        cmd,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\nstdout={proc.stdout[:800]}\nstderr={proc.stderr[:1200]}"
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


def workflow_python(client: Any, ticket: Ticket) -> dict[str, Any]:
    return client.workflow_start(
        ticket.title,
        body=ticket.body,
        source=ticket.source,
        source_id=ticket.source_id,
        limit=10,
        depth=2,
        recent_limit=5,
    )


def workflow_node_all(store: str, tickets: list[Ticket]) -> list[tuple[dict[str, Any], float]]:
    env = os.environ.copy()
    env["WG_NAPI_DIR"] = str(REPO / "crates" / "wg-napi")
    env["WG_NODE_WORKFLOW_PAYLOAD"] = json.dumps(
        {"store": store, "tickets": [ticket.__dict__ for ticket in tickets]},
        ensure_ascii=False,
    )
    proc = subprocess.run(
        ["node", "-e", NODE_SCRIPT],
        capture_output=True,
        text=True,
        timeout=30,
        env=env,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"node workflowStart failed\nstdout={proc.stdout[:800]}\nstderr={proc.stderr[:1200]}"
        )
    rows = json.loads(proc.stdout)
    return [(row["payload"], float(row["latency_ms"])) for row in rows]


def timed(fn: Callable[[], dict[str, Any]]) -> tuple[dict[str, Any], float]:
    start = time.perf_counter_ns()
    payload = fn()
    return payload, (time.perf_counter_ns() - start) / 1e6


def normalize(ticket: Ticket, payload: dict[str, Any], latency_ms: float) -> dict[str, Any]:
    text = json.dumps(payload, ensure_ascii=False)
    leaks = {needle: needle in text for needle in ticket.must_not_contain}
    contains = {needle: needle in text for needle in ticket.must_contain}
    context = payload.get("context") or {}
    return {
        "ticket": ticket.name,
        "session_id_present": str(payload.get("session_id", "")).startswith("session-"),
        "ticket_fact_id_present": isinstance(payload.get("ticket_fact_id"), str),
        "source_id": payload.get("source_id"),
        "latency_ms": round(latency_ms, 2),
        "context_chars": len(text),
        "search_hits": len(context.get("search") or []),
        "prior_lessons": len(payload.get("prior_lessons") or []),
        "prior_errors": len(payload.get("prior_errors") or []),
        "relevant_decisions": len(payload.get("relevant_decisions") or []),
        "prior_total": sum(
            len(payload.get(key) or [])
            for key in ("prior_lessons", "prior_errors", "relevant_decisions")
        ),
        "must_contain": contains,
        "forbidden_leakage_count": sum(1 for v in leaks.values() if v),
    }


def percentile(xs: list[float], p: float) -> float:
    if not xs:
        return 0.0
    ordered = sorted(xs)
    return ordered[int(round((p / 100.0) * (len(ordered) - 1)))]


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    lats = [float(r["latency_ms"]) for r in rows]
    return {
        "p50_ms": round(percentile(lats, 50), 2),
        "p95_ms": round(percentile(lats, 95), 2),
        "max_ms": round(max(lats) if lats else 0.0, 2),
        "context_max_chars": max(int(r["context_chars"]) for r in rows),
        "leakage_total": sum(int(r["forbidden_leakage_count"]) for r in rows),
    }


def parity_against_cli(cli_rows: list[dict[str, Any]], other_rows: list[dict[str, Any]]) -> dict[str, dict[str, bool]]:
    parity = {}
    for cli, other in zip(cli_rows, other_rows):
        parity[cli["ticket"]] = {
            "search_hits_equal": cli["search_hits"] == other["search_hits"],
            "prior_counts_equal": (
                cli["prior_lessons"],
                cli["prior_errors"],
                cli["relevant_decisions"],
            )
            == (
                other["prior_lessons"],
                other["prior_errors"],
                other["relevant_decisions"],
            ),
            "contains_expected": all(other["must_contain"].values()),
            "no_leakage": other["forbidden_leakage_count"] == 0,
            "source_id_equal": cli["source_id"] == other["source_id"],
        }
    return parity


def main() -> int:
    try:
        import wg_python
    except Exception as exc:
        print(
            "wg_python is not importable; run `scripts/wg-python-pack-smoke.sh` or maturin develop first.",
            file=sys.stderr,
        )
        print(f"import error: {exc}", file=sys.stderr)
        return 2
    if not hasattr(wg_python.WikiGraph, "workflow_start"):
        print(
            "wg_python is importable but lacks workflow_start; run "
            "`cd crates/wg-python && maturin develop` for this checkout.",
            file=sys.stderr,
        )
        return 2

    for store in STORES.values():
        reset_store(store)
        seed(store)

    rows: dict[str, list[dict[str, Any]]] = {}
    cli_rows = []
    for ticket in TICKETS:
        payload, elapsed = timed(lambda ticket=ticket: workflow_cli(STORES["cli"], ticket))
        cli_rows.append(normalize(ticket, payload, elapsed))
    rows["cli"] = cli_rows

    python_client = wg_python.WikiGraph(STORES["python"])
    python_rows = []
    for ticket in TICKETS:
        payload, elapsed = timed(lambda ticket=ticket: workflow_python(python_client, ticket))
        python_rows.append(normalize(ticket, payload, elapsed))
    rows["python"] = python_rows

    rows["node"] = [
        normalize(ticket, payload, elapsed)
        for ticket, (payload, elapsed) in zip(TICKETS, workflow_node_all(STORES["node"], TICKETS))
    ]

    summaries = {driver: summarize(driver_rows) for driver, driver_rows in rows.items()}
    python_parity = parity_against_cli(rows["cli"], rows["python"])
    node_parity = parity_against_cli(rows["cli"], rows["node"])

    invariants = {
        "cli_all_sessions": all(r["session_id_present"] and r["ticket_fact_id_present"] for r in rows["cli"]),
        "python_all_sessions": all(r["session_id_present"] and r["ticket_fact_id_present"] for r in rows["python"]),
        "node_all_sessions": all(r["session_id_present"] and r["ticket_fact_id_present"] for r in rows["node"]),
        "python_shape_parity": all(all(v.values()) for v in python_parity.values()),
        "node_shape_parity": all(all(v.values()) for v in node_parity.values()),
        "all_leakage_zero": all(summary["leakage_total"] == 0 for summary in summaries.values()),
        "known_tickets_have_priors": all(
            row["prior_total"] > 0
            for driver_rows in rows.values()
            for row, ticket in zip(driver_rows, TICKETS)
            if ticket.expect_priors
        ),
        "unknown_ticket_has_no_priors": all(
            row["prior_total"] == 0
            for driver_rows in rows.values()
            for row, ticket in zip(driver_rows, TICKETS)
            if not ticket.expect_priors
        ),
    }

    out = {
        "scenario": "K - workflow_start parity across CLI, Python SDK, and Node SDK",
        "stores": STORES,
        "runs": rows,
        "summaries": summaries,
        "parity": {
            "python_vs_cli": python_parity,
            "node_vs_cli": node_parity,
        },
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_k.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
