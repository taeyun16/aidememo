#!/usr/bin/env python3
"""Scenario F — workflow-trigger simulation across distinct tickets.

This is the deterministic counterpart to a real coding-agent prompt
like "Issue #123 just arrived; start work."  It exercises the entry
point agents should call at task start:

  - CLI:          wg --json workflow start ...
  - MCP stdio:    tools/call wg_workflow_start
  - Hermes path:  WgClient.workflow_start(...)

The scenario seeds overlapping project memory, including intentionally
conflicting Redis facts under different ``source_id`` values, then
starts several unrelated tickets.  It verifies:

  1. Every ticket gets its own session and ticket fact.
  2. Prior decisions / lessons / errors match the ticket topic.
  3. ``source_id`` prevents same-keyword cross-tenant leakage.
  4. Unknown tickets still produce a tracked session + ticket fact.

No LLM is involved.  This is a cheap regression for the workflow
contract; model-driven Hermes/Claude/Codex tests can sit on top of it.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
WG = os.environ.get("WG_BIN", str(REPO / "target" / "debug" / "wg"))
STORE = os.environ.get(
    "WG_E2E_STORE",
    str(Path(tempfile.gettempdir()) / "wg-e2e-f" / "workflow.redb"),
)


@dataclass
class Ticket:
    name: str
    driver: str
    title: str
    body: str
    source: str
    source_id: str | None
    must_contain: list[str] = field(default_factory=list)
    must_not_contain: list[str] = field(default_factory=list)
    expect_priors: bool = True


def run(cmd: list[str], *, input_text: str | None = None, timeout: int = 20) -> subprocess.CompletedProcess:
    proc = subprocess.run(
        cmd,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\nstdout={proc.stdout[:500]}\nstderr={proc.stderr[:1000]}"
        )
    return proc


def reset_store() -> None:
    path = Path(STORE)
    path.parent.mkdir(parents=True, exist_ok=True)
    for sibling in path.parent.iterdir():
        if sibling.name.startswith(path.name):
            sibling.unlink()


def fact_add(content: str, fact_type: str, entities: list[str], source_id: str) -> str:
    cmd = [
        WG,
        "--store",
        STORE,
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


def seed_memory() -> list[str]:
    seeds = [
        (
            "Decision: Redis timeout fixes must go through the Worker job wrapper.",
            "decision",
            ["Redis", "Worker"],
            "team-a",
        ),
        (
            "Lesson: The last Worker Redis timeout was DNS resolution, not pool size.",
            "lesson",
            ["Redis", "Worker"],
            "team-a",
        ),
        (
            "Error: Avoid increasing Redis pool size before checking DNS metrics.",
            "error",
            ["Redis", "Worker"],
            "team-a",
        ),
        (
            "Decision: Billing webhook retries use idempotency keys at the event processor.",
            "decision",
            ["Billing", "Webhook"],
            "team-a",
        ),
        (
            "Lesson: Duplicate Stripe events came from retry races, not queue ordering.",
            "lesson",
            ["Billing", "Webhook"],
            "team-a",
        ),
        (
            "Error: Do not disable Stripe signature checks while debugging webhook duplicates.",
            "error",
            ["Billing", "Webhook"],
            "team-a",
        ),
        (
            "Decision: Redis timeout fixes for edge traffic belong in edge cache config.",
            "decision",
            ["Redis", "EdgeCache"],
            "team-b",
        ),
    ]
    return [fact_add(*seed) for seed in seeds]


def workflow_cli(ticket: Ticket) -> dict[str, Any]:
    cmd = [
        WG,
        "--store",
        STORE,
        "--json",
        "workflow",
        "start",
        ticket.title,
        "--body",
        ticket.body,
        "--source",
        ticket.source,
        "--limit",
        "10",
        "--depth",
        "2",
        "--recent-limit",
        "5",
    ]
    if ticket.source_id:
        cmd += ["--source-id", ticket.source_id]
    return json.loads(run(cmd).stdout)


def mcp_tool_call(name: str, args: dict[str, Any]) -> dict[str, Any]:
    calls = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}},
        },
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        },
    ]
    proc = run([WG, "--store", STORE, "mcp"], input_text="".join(json.dumps(c) + "\n" for c in calls))
    responses = [json.loads(line) for line in proc.stdout.splitlines() if line.strip().startswith("{")]
    by_id = {r.get("id"): r for r in responses}
    response = by_id.get(2) or {}
    if "error" in response:
        raise RuntimeError(f"MCP {name} failed: {response['error']}")
    content = response.get("result", {}).get("content") or []
    if not content:
        raise RuntimeError(f"MCP {name} returned no content: {response}")
    return json.loads(content[0]["text"])


def workflow_mcp(ticket: Ticket) -> dict[str, Any]:
    args: dict[str, Any] = {
        "title": ticket.title,
        "body": ticket.body,
        "source": ticket.source,
        "limit": 10,
        "depth": 2,
        "recent_limit": 5,
    }
    if ticket.source_id:
        args["source_id"] = ticket.source_id
    return mcp_tool_call("wg_workflow_start", args)


def workflow_hermes(ticket: Ticket) -> dict[str, Any]:
    sys.path.insert(0, str(REPO / "plugins" / "hermes" / "src"))
    os.environ["PATH"] = f"{Path(WG).parent}:{os.environ.get('PATH', '')}"
    from hermes_wg.client import WgClient

    client = WgClient(store_path=STORE, lock_retry_ms=5000)
    return client.workflow_start(
        ticket.title,
        body=ticket.body,
        source=ticket.source,
        source_id=ticket.source_id,
        limit=10,
        depth=2,
        recent_limit=5,
    )


def workflow_start(ticket: Ticket) -> dict[str, Any]:
    if ticket.driver == "cli":
        return workflow_cli(ticket)
    if ticket.driver == "mcp":
        return workflow_mcp(ticket)
    if ticket.driver == "hermes":
        return workflow_hermes(ticket)
    raise ValueError(f"unknown driver: {ticket.driver}")


def json_text(payload: Any) -> str:
    return json.dumps(payload, ensure_ascii=False)


def count_prior_items(payload: dict[str, Any]) -> int:
    return sum(
        len(payload.get(key) or [])
        for key in ("prior_lessons", "prior_errors", "relevant_decisions")
    )


def list_workflow_questions() -> list[dict[str, Any]]:
    out = run([WG, "--store", STORE, "--json", "fact", "list", "--type", "question", "-l", "50"])
    payload = json.loads(out.stdout or "[]")
    if isinstance(payload, dict):
        return payload.get("facts") or payload.get("items") or []
    return payload


def main() -> int:
    reset_store()
    seed_ids = seed_memory()

    tickets = [
        Ticket(
            name="redis-worker-team-a",
            driver="cli",
            title="Fix Redis timeout in worker",
            body="Worker jobs intermittently time out against Redis. The issue body has no more detail.",
            source="github:org/app#123",
            source_id="team-a",
            must_contain=["Worker job wrapper", "DNS resolution", "DNS metrics"],
            must_not_contain=["edge cache config", "Stripe signature checks"],
        ),
        Ticket(
            name="billing-webhook-team-a",
            driver="mcp",
            title="Stop duplicate billing webhook processing",
            body="Stripe webhooks sometimes process the same invoice twice.",
            source="linear:ENG-456",
            source_id="team-a",
            must_contain=["idempotency keys", "Duplicate Stripe events", "signature checks"],
            must_not_contain=["Worker job wrapper", "edge cache config"],
        ),
        Ticket(
            name="redis-edge-team-b",
            driver="hermes",
            title="Investigate Redis timeout on edge traffic",
            body="Edge traffic sees Redis timeout spikes after cache routing changes.",
            source="github:org/edge#77",
            source_id="team-b",
            must_contain=["edge cache config"],
            must_not_contain=["Worker job wrapper", "DNS resolution", "idempotency keys"],
        ),
        Ticket(
            name="unknown-mobile-team-c",
            driver="mcp",
            title="Fix mobile dark mode flicker",
            body="The app flashes a light background before dark mode applies.",
            source="linear:MOB-9",
            source_id="team-c",
            must_contain=["Fix mobile dark mode flicker"],
            must_not_contain=["Redis timeout", "Stripe", "edge cache config"],
            expect_priors=False,
        ),
    ]

    runs: list[dict[str, Any]] = []
    for ticket in tickets:
        payload = workflow_start(ticket)
        text = json_text(payload)
        contains = {needle: needle in text for needle in ticket.must_contain}
        leaks = {needle: needle in text for needle in ticket.must_not_contain}
        runs.append(
            {
                "ticket": ticket.name,
                "driver": ticket.driver,
                "source_id": ticket.source_id,
                "session_id": payload.get("session_id"),
                "ticket_fact_id": payload.get("ticket_fact_id"),
                "search_hits": len((payload.get("context") or {}).get("search") or []),
                "prior_lessons": len(payload.get("prior_lessons") or []),
                "prior_errors": len(payload.get("prior_errors") or []),
                "relevant_decisions": len(payload.get("relevant_decisions") or []),
                "prior_total": count_prior_items(payload),
                "must_contain": contains,
                "must_not_contain": leaks,
                "expect_priors": ticket.expect_priors,
            }
        )

    question_facts = list_workflow_questions()
    sessions = [r["session_id"] for r in runs]
    ticket_fact_ids = [r["ticket_fact_id"] for r in runs]

    invariants = {
        "every_run_has_session": all(isinstance(s, str) and s.startswith("session-") for s in sessions),
        "every_run_has_ticket_fact": all(isinstance(fid, str) and len(fid) >= 20 for fid in ticket_fact_ids),
        "sessions_are_unique": len(set(sessions)) == len(sessions),
        "ticket_facts_are_unique": len(set(ticket_fact_ids)) == len(ticket_fact_ids),
        "workflow_question_fact_per_ticket": len(
            [f for f in question_facts if "workflow-start" in (f.get("tags") or [])]
        )
        == len(tickets),
        "all_expected_context_present": all(all(r["must_contain"].values()) for r in runs),
        "no_forbidden_context_leaked": all(not any(r["must_not_contain"].values()) for r in runs),
        "known_tickets_have_priors": all(
            (not r["expect_priors"]) or r["prior_total"] > 0 for r in runs
        ),
        "unknown_ticket_has_no_priors": all(
            r["prior_total"] == 0 for r in runs if not r["expect_priors"]
        ),
        "every_run_has_search_hit": all(r["search_hits"] >= 1 for r in runs),
    }

    out = {
        "scenario": "F — workflow-trigger simulation across distinct tickets",
        "store": STORE,
        "seed_count": len(seed_ids),
        "runs": runs,
        "question_facts": [
            {
                "id": f.get("id"),
                "source": f.get("source"),
                "source_id": f.get("source_id"),
                "tags": f.get("tags"),
                "content": f.get("content"),
            }
            for f in question_facts
            if "workflow-start" in (f.get("tags") or [])
        ],
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_f.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
