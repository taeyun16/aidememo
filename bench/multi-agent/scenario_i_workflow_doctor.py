#!/usr/bin/env python3
"""Scenario I — workflow ticket traces surface in `aidememo doctor --json`.

P3.5 added a doctor workflow-readiness block. This scenario verifies the
end-to-end user path behind it:

  1. Start sparse tickets through three integration paths:
     CLI, MCP stdio, and Hermes AideMemoClient.
  2. Run `aidememo doctor --json` in an isolated HOME with a known Codex MCP
     config so `workflow.ready` is deterministic.
  3. Assert doctor reports the created workflow tickets and does not emit
     setup hints that contradict the fixture.

No LLM is involved; this is a cheap regression for the workflow diagnostic
surface rather than a natural-language adoption test.
"""

from __future__ import annotations

import json
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
WG = os.environ.get("AIDEMEMO_BIN", str(REPO / "target" / "debug" / "aidememo"))
BASE = Path(os.environ.get("AIDEMEMO_E2E_BASE", str(Path(tempfile.gettempdir()) / "aidememo-e2e-i")))
AGENT_SDK_SRC = REPO / "packages" / "aidememo-agent-sdk" / "src"
STORE = str(BASE / "workflow-doctor.sqlite")


@dataclass
class Ticket:
    name: str
    driver: str
    title: str
    body: str
    source: str
    source_id: str


TICKETS = [
    Ticket(
        name="cli-redis",
        driver="cli",
        title="Fix Redis timeout in worker",
        body="Worker jobs intermittently time out against Redis.",
        source="github:org/app#123",
        source_id="team-a",
    ),
    Ticket(
        name="mcp-billing",
        driver="mcp",
        title="Stop duplicate billing webhook processing",
        body="Stripe webhooks sometimes process the same invoice twice.",
        source="linear:ENG-456",
        source_id="team-a",
    ),
    Ticket(
        name="hermes-mobile",
        driver="hermes",
        title="Fix mobile dark mode flicker",
        body="The app flashes a light background before dark mode applies.",
        source="linear:MOB-9",
        source_id="team-c",
    ),
]


def run(
    cmd: list[str],
    *,
    input_text: str | None = None,
    env: dict[str, str] | None = None,
    timeout: int = 20,
) -> subprocess.CompletedProcess:
    proc = subprocess.run(
        cmd,
        input=input_text,
        capture_output=True,
        text=True,
        env=env,
        timeout=timeout,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\nstdout={proc.stdout[:500]}\nstderr={proc.stderr[:1000]}"
        )
    return proc


def reset() -> None:
    BASE.mkdir(parents=True, exist_ok=True)
    if BASE.exists():
        for child in BASE.iterdir():
            if child.is_dir():
                shutil.rmtree(child)
            else:
                child.unlink()


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


def seed_memory() -> None:
    seeds = [
        (
            "Decision: Redis timeout fixes must go through the Worker job wrapper.",
            "decision",
            ["Redis", "Worker"],
            "team-a",
        ),
        (
            "Lesson: Duplicate Stripe events came from retry races, not queue ordering.",
            "lesson",
            ["Billing", "Webhook"],
            "team-a",
        ),
        (
            "Decision: Mobile theme bootstrap reads the saved dark-mode preference before paint.",
            "decision",
            ["Mobile", "Theme"],
            "team-c",
        ),
    ]
    for item in seeds:
        fact_add(*item)


def workflow_cli(ticket: Ticket) -> dict[str, Any]:
    return json.loads(
        run(
            [
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
                "--source-id",
                ticket.source_id,
                "--limit",
                "6",
                "--depth",
                "1",
                "--recent-limit",
                "3",
                "--bm25-only",
            ]
        ).stdout
    )


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
    response = {r.get("id"): r for r in responses}.get(2) or {}
    if "error" in response:
        raise RuntimeError(f"MCP {name} failed: {response['error']}")
    content = response.get("result", {}).get("content") or []
    if not content:
        raise RuntimeError(f"MCP {name} returned no content: {response}")
    return json.loads(content[0]["text"])


def workflow_mcp(ticket: Ticket) -> dict[str, Any]:
    return mcp_tool_call(
        "aidememo_workflow_start",
        {
            "title": ticket.title,
            "body": ticket.body,
            "source": ticket.source,
            "source_id": ticket.source_id,
            "limit": 6,
            "depth": 1,
            "recent_limit": 3,
            "bm25_only": True,
        },
    )


def workflow_hermes(ticket: Ticket) -> dict[str, Any]:
    sys.path.insert(0, str(AGENT_SDK_SRC))
    sys.path.insert(0, str(REPO / "plugins" / "hermes" / "src"))
    os.environ["PATH"] = f"{Path(WG).parent}:{os.environ.get('PATH', '')}"
    from hermes_aidememo.client import AideMemoClient

    client = AideMemoClient(store_path=STORE, lock_retry_ms=5000)
    return client.workflow_start(
        ticket.title,
        body=ticket.body,
        source=ticket.source,
        source_id=ticket.source_id,
        limit=6,
        depth=1,
        recent_limit=3,
        bm25_only=True,
    )


def workflow_start(ticket: Ticket) -> tuple[dict[str, Any], float]:
    start = time.perf_counter_ns()
    if ticket.driver == "cli":
        payload = workflow_cli(ticket)
    elif ticket.driver == "mcp":
        payload = workflow_mcp(ticket)
    elif ticket.driver == "hermes":
        payload = workflow_hermes(ticket)
    else:
        raise ValueError(f"unknown driver: {ticket.driver}")
    return payload, (time.perf_counter_ns() - start) / 1e6


def isolated_home() -> Path:
    home = BASE / "home"
    codex = home / ".codex"
    codex.mkdir(parents=True, exist_ok=True)
    (codex / "config.toml").write_text(
        "\n".join(
            [
                "[mcp_servers.aidememo]",
                f'command = "{WG}"',
                f'args = ["--store", "{STORE}", "mcp"]',
                "",
            ]
        )
    )
    return home


def doctor_json(home: Path) -> dict[str, Any]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["PATH"] = "/nonexistent"
    env.pop("AIDEMEMO_STORE", None)
    return json.loads(run([WG, "--json", "--store", STORE, "doctor"], env=env).stdout)


def percentile(values: list[float], p: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[int(round((p / 100.0) * (len(ordered) - 1)))]


def main() -> int:
    reset()
    seed_memory()

    runs: list[dict[str, Any]] = []
    for ticket in TICKETS:
        payload, latency_ms = workflow_start(ticket)
        runs.append(
            {
                "ticket": ticket.name,
                "driver": ticket.driver,
                "source": ticket.source,
                "source_id": ticket.source_id,
                "session_id": payload.get("session_id"),
                "ticket_fact_id": payload.get("ticket_fact_id"),
                "latency_ms": round(latency_ms, 2),
                "context_search_hits": len((payload.get("context") or {}).get("search") or []),
            }
        )

    doctor = doctor_json(isolated_home())
    workflow = doctor.get("workflow") or {}
    recent_tickets = workflow.get("recent_tickets") or []
    hint_codes = [h.get("code") for h in workflow.get("hints") or []]
    source_ids = sorted({t.get("source_id") for t in recent_tickets if t.get("source_id")})
    previews = " ".join(t.get("preview") or "" for t in recent_tickets)
    latencies = [float(r["latency_ms"]) for r in runs]

    invariants = {
        "every_run_has_session": all(str(r.get("session_id", "")).startswith("session-") for r in runs),
        "every_run_has_ticket_fact": all(isinstance(r.get("ticket_fact_id"), str) for r in runs),
        "doctor_workflow_ready": workflow.get("ready") is True,
        "doctor_mcp_ready": workflow.get("mcp_ready") is True,
        "doctor_counts_all_tickets": workflow.get("recent_ticket_count") == len(TICKETS),
        "doctor_recent_summaries_cover_sources": source_ids == ["team-a", "team-c"],
        "doctor_recent_previews_cover_titles": all(ticket.title in previews for ticket in TICKETS),
        "doctor_no_mcp_gap_hint": "workflow_no_mcp_agent" not in hint_codes,
        "doctor_no_recent_ticket_hint": "workflow_no_recent_tickets" not in hint_codes,
        "p95_latency_under_5s": percentile(latencies, 95) < 5_000,
    }

    out = {
        "scenario": "I — workflow traces surface in doctor",
        "store": STORE,
        "runs": runs,
        "doctor_workflow": workflow,
        "measurements": {
            "tickets": len(TICKETS),
            "drivers": sorted({r["driver"] for r in runs}),
            "latency_ms": {
                "p50": round(percentile(latencies, 50), 2),
                "p95": round(percentile(latencies, 95), 2),
                "max": round(max(latencies) if latencies else 0.0, 2),
            },
            "doctor_recent_ticket_count": workflow.get("recent_ticket_count"),
            "doctor_hint_codes": hint_codes,
        },
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
        },
    }

    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_i.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
