#!/usr/bin/env python3
"""Scenario R - Hermes Kanban lifecycle with an external AideMemo handoff.

This zero-token scenario runs a real temporary Hermes Kanban board and the
AideMemo Hermes plugin surface. It proves the ownership boundary:

1. Hermes Kanban owns an internal coding -> reviewer profile transition.
2. AideMemo does not auto-start or dispatch a second task for that transition.
3. A Hermes card can dispatch one session pointer to an external Codex account.
4. The external result returns on the same AideMemo session.
5. Hermes, not the AideMemo acknowledgement ledger, marks the card done.

It does not spawn a Codex model process or measure downstream task quality.
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
AM = os.environ.get("AIDEMEMO_BIN", str(REPO / "target" / "debug" / "aidememo"))
HERMES = os.environ.get("HERMES_BIN", shutil.which("hermes") or "hermes")
BASE = Path(
    os.environ.get(
        "AIDEMEMO_E2E_BASE",
        str(Path(tempfile.gettempdir()) / "aidememo-e2e-r"),
    )
)
STORE = BASE / "hermes-kanban-boundary.sqlite"
HERMES_HOME = BASE / "hermes-home"
SOURCE_ID = "hermes-kanban-release"
EXTERNAL_ACTOR = "codex-two"
EXTERNAL_RESULT = "External review result: the release retry race is covered."


def run(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: int = 30,
) -> subprocess.CompletedProcess[str]:
    child_env = os.environ.copy()
    if env:
        child_env.update(env)
    proc = subprocess.run(
        cmd,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
        env=child_env,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\n"
            f"stdout={proc.stdout[:1600]}\nstderr={proc.stderr[:2000]}"
        )
    return proc


def reset_base() -> None:
    resolved = BASE.resolve()
    temp_roots = {
        Path(tempfile.gettempdir()).resolve(),
        Path("/tmp").resolve(),
        Path("/private/tmp").resolve(),
    }
    if any(resolved == root for root in temp_roots) or not any(
        root in resolved.parents for root in temp_roots
    ):
        raise RuntimeError(f"refusing to reset non-temporary scenario base: {resolved}")
    if resolved.exists():
        shutil.rmtree(resolved)
    resolved.mkdir(parents=True)


def hermes_env(task_id: str | None = None) -> dict[str, str]:
    env = {
        "HERMES_HOME": str(HERMES_HOME),
        "HERMES_KANBAN_BOARD": "default",
    }
    if task_id:
        env["HERMES_KANBAN_TASK"] = task_id
    return env


def hermes_json(args: list[str]) -> dict[str, Any] | list[Any]:
    return json.loads(run([HERMES, *args], env=hermes_env()).stdout)


def hermes_task(task_id: str) -> dict[str, Any]:
    payload = hermes_json(["kanban", "show", task_id, "--json"])
    if not isinstance(payload, dict):
        raise RuntimeError(f"unexpected Hermes show payload: {payload!r}")
    task = payload.get("task", payload)
    if not isinstance(task, dict):
        raise RuntimeError(f"unexpected Hermes task payload: {task!r}")
    return task


def stats() -> dict[str, Any]:
    return json.loads(run([AM, "--store", str(STORE), "--json", "stats"]).stdout)


def mcp_tool(
    actor: str,
    name: str,
    arguments: dict[str, Any],
) -> dict[str, Any]:
    requests = [
        {
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": actor, "version": "0"},
                "capabilities": {},
            },
        },
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        },
    ]
    proc = run(
        [AM, "--store", str(STORE), "mcp"],
        env={
            "AIDEMEMO_ACTOR_ID": actor,
            "AIDEMEMO_SOURCE_ID": SOURCE_ID,
        },
        input_text="".join(json.dumps(request) + "\n" for request in requests),
    )
    responses = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
    response = next((row for row in responses if row.get("id") == 1), None)
    if response is None or response.get("error"):
        raise RuntimeError(f"MCP {actor}/{name} failed: {response or proc.stdout[:1600]}")
    result = response.get("result") or {}
    if result.get("isError"):
        raise RuntimeError(f"MCP {actor}/{name} returned isError: {result}")
    text = "\n".join(
        str(block.get("text") or "")
        for block in result.get("content") or []
        if isinstance(block, dict) and block.get("type") == "text"
    ).strip()
    if not text:
        raise RuntimeError(f"MCP {actor}/{name} returned no text")
    return json.loads(text)


class ToolCtx:
    def __init__(self) -> None:
        self.tools: dict[str, Any] = {}

    def register_tool(self, *, name: str, handler: Any, **_: Any) -> None:
        self.tools[name] = handler


def prepare_hermes_plugin() -> tuple[Any, ToolCtx, Any]:
    sys.path.insert(0, str(REPO / "packages" / "aidememo-agent-sdk" / "src"))
    sys.path.insert(0, str(REPO / "plugins" / "hermes" / "src"))
    binary_dir = str(Path(AM).resolve().parent)
    os.environ["PATH"] = f"{binary_dir}{os.pathsep}{os.environ.get('PATH', '')}"

    from aidememo_agent import AideMemoClient  # noqa: PLC0415
    from hermes_aidememo.hooks import make_pre_llm_call  # noqa: PLC0415
    from hermes_aidememo.tools import register_all  # noqa: PLC0415

    client = AideMemoClient(
        store_path=str(STORE),
        source_id=SOURCE_ID,
        storage_backend="libsqlite",
    )
    # Exercise the universal CLI/MCP fallback used by a wheel without the
    # optional PyO3 binding, independent of what is installed on this host.
    client._py = None
    ctx = ToolCtx()
    register_all(ctx, client)
    return client, ctx, make_pre_llm_call


def call_plugin(ctx: ToolCtx, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    return json.loads(ctx.tools[name](arguments))


def main() -> int:
    reset_base()
    if not Path(AM).exists():
        raise RuntimeError(f"AIDEMEMO_BIN does not exist: {AM}")
    if not shutil.which(HERMES) and not Path(HERMES).exists():
        raise RuntimeError(f"HERMES_BIN does not exist: {HERMES}")
    started = time.perf_counter_ns()

    run([HERMES, "kanban", "init"], env=hermes_env())
    card = hermes_json(
        [
            "kanban",
            "create",
            "Review the release retry boundary",
            "--body",
            "Hermes owns the card; durable evidence may cross to Codex.",
            "--assignee",
            "coding",
            "--idempotency-key",
            "scenario-r-card",
            "--json",
        ]
    )
    if not isinstance(card, dict):
        raise RuntimeError(f"unexpected Hermes card payload: {card!r}")
    task_id = str(card["id"])

    workflow = mcp_tool(
        "hermes-orchestrator",
        "aidememo_workflow_start",
        {
            "title": "Review the release retry boundary",
            "body": "Reuse this memory session across the Hermes card and external reviewer.",
            "source": f"hermes-kanban:default/{task_id}",
            "bm25_only": True,
        },
    )
    session_id = str(workflow["session_id"])
    client, plugin, make_pre_llm_call = prepare_hermes_plugin()

    previous_env = {
        key: os.environ.get(key)
        for key in ("HERMES_HOME", "HERMES_KANBAN_BOARD", "HERMES_KANBAN_TASK")
    }
    os.environ.update(hermes_env(task_id))
    before_hook = stats()
    hook_result = make_pre_llm_call(client)(
        is_first_turn=True,
        user_message="Issue: review the release retry boundary",
        session_id="hermes-chat-session",
    )
    after_hook = stats()
    for key, value in previous_env.items():
        if value is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = value

    call_plugin(
        plugin,
        "aidememo_fact_add",
        {
            "content": "Decision: Hermes Kanban remains the canonical task lifecycle.",
            "fact_type": "decision",
            "entities": ["HermesKanban", "AideMemoHandoff"],
            "source_id": SOURCE_ID,
            "session_id": session_id,
        },
    )

    before_internal_preview = stats()
    internal_preview = call_plugin(
        plugin,
        "aidememo_handoff",
        {
            "session_id": session_id,
            "from": "hermes/coding",
            "to": "hermes/reviewer",
            "focus": "Review the same-board card.",
            "done_when": "Reviewer validates the focused checks.",
            "source_id": SOURCE_ID,
            "dispatch": False,
        },
    )
    after_internal_preview = stats()
    internal_inbox = call_plugin(
        plugin,
        "aidememo_handoff_inbox",
        {"action": "list", "actor_id": "hermes-reviewer", "source_id": SOURCE_ID},
    )

    run(
        [
            HERMES,
            "kanban",
            "reassign",
            task_id,
            "reviewer",
            "--reason",
            "Internal profile routing stays in Kanban",
        ],
        env=hermes_env(),
    )
    after_reassign = hermes_task(task_id)

    before_external_dispatch = stats()
    external = call_plugin(
        plugin,
        "aidememo_handoff",
        {
            "session_id": session_id,
            "from_actor": "hermes-orchestrator",
            "to_actor": EXTERNAL_ACTOR,
            "from": "hermes/reviewer",
            "to": "codex/coding",
            "focus": "Verify the retry-race regression externally.",
            "done_when": "The regression result is attached to the shared session.",
            "source_id": SOURCE_ID,
            "dispatch": True,
        },
    )
    handoff_id = str(external["handoff_id"])
    after_external_dispatch = stats()
    external_inbox = mcp_tool(
        EXTERNAL_ACTOR,
        "aidememo_handoff_inbox",
        {"action": "list"},
    )
    accepted = mcp_tool(
        EXTERNAL_ACTOR,
        "aidememo_handoff_inbox",
        {"action": "accept", "handoff_id": handoff_id},
    )
    accepted_session = str(accepted["resume"]["env"]["AIDEMEMO_SESSION_ID"])
    mcp_tool(
        EXTERNAL_ACTOR,
        "aidememo_fact_add",
        {
            "content": EXTERNAL_RESULT,
            "fact_type": "lesson",
            "entities": ["HermesKanban", "RetryRace"],
            "session_id": accepted_session,
        },
    )
    return_preview = call_plugin(
        plugin,
        "aidememo_handoff",
        {
            "session_id": session_id,
            "from": "codex/coding",
            "to": "hermes/reviewer",
            "focus": "Validate the returned external evidence.",
            "done_when": "Hermes marks its card complete after validation.",
            "source_id": SOURCE_ID,
            "dispatch": False,
        },
    )
    completed_assignment = mcp_tool(
        EXTERNAL_ACTOR,
        "aidememo_handoff_inbox",
        {"action": "complete", "handoff_id": handoff_id},
    )
    external_after = mcp_tool(
        EXTERNAL_ACTOR,
        "aidememo_handoff_inbox",
        {"action": "list"},
    )
    before_kanban_complete = hermes_task(task_id)

    completion_metadata = json.dumps(
        {
            "aidememo_session_id": session_id,
            "external_actor": EXTERNAL_ACTOR,
            "handoff_id": handoff_id,
        },
        separators=(",", ":"),
    )
    run(
        [
            HERMES,
            "kanban",
            "complete",
            task_id,
            "--result",
            "External evidence validated on the shared AideMemo session.",
            "--summary",
            "Hermes retained lifecycle ownership; Codex returned fact-linked evidence.",
            "--metadata",
            completion_metadata,
        ],
        env=hermes_env(),
    )
    final_card = hermes_task(task_id)

    hook_context = str((hook_result or {}).get("context") or "")
    external_assignments = external_inbox.get("assignments") or []
    gates = {
        "kanban_worker_guidance_injected": (
            task_id in hook_context
            and "Kanban is the canonical owner" in hook_context
            and "call `aidememo_workflow_start`" not in hook_context
        ),
        "kanban_hook_creates_no_second_workflow": before_hook == after_hook,
        "single_fact_write_preserves_session": (
            "Hermes Kanban remains the canonical task lifecycle" in internal_preview["content"]
        ),
        "internal_preview_is_read_only": before_internal_preview == after_internal_preview,
        "internal_profile_has_no_aidememo_assignment": internal_inbox["assignments"] == [],
        "kanban_owns_internal_reassignment": (
            after_reassign["assignee"] == "reviewer" and after_reassign["status"] == "ready"
        ),
        "external_dispatch_adds_pointer_only": (
            after_external_dispatch["entity_count"]
            == before_external_dispatch["entity_count"] + 1
            and after_external_dispatch["fact_count"]
            == before_external_dispatch["fact_count"]
        ),
        "external_actor_receives_only_addressed_session": (
            [row["handoff_id"] for row in external_assignments] == [handoff_id]
            and external_assignments[0]["session_id"] == session_id
        ),
        "external_accept_preserves_session": accepted_session == session_id,
        "external_result_returns_as_session_evidence": EXTERNAL_RESULT in return_preview["content"],
        "aidememo_completion_is_explicit_ack_only": (
            completed_assignment["assignment"]["status"] == "completed"
            and external_after["assignments"] == []
            and before_kanban_complete["status"] == "ready"
        ),
        "hermes_explicitly_completes_canonical_card": (
            final_card["status"] == "done"
            and final_card["assignee"] == "reviewer"
            and final_card["result"]
            == "External evidence validated on the shared AideMemo session."
        ),
    }
    elapsed_ms = (time.perf_counter_ns() - started) / 1e6
    hermes_version = run([HERMES, "--version"]).stdout.strip()
    out = {
        "scenario": "R - Hermes Kanban lifecycle and external handoff boundary",
        "claim_boundary": (
            "Zero-token protocol evidence using a real temporary Hermes Kanban DB. "
            "This proves lifecycle/pointer ownership and same-session evidence return; "
            "it does not prove an external CLI worker adapter, model task success, or authentication."
        ),
        "hermes_version": hermes_version,
        "store": str(STORE),
        "hermes_home": str(HERMES_HOME),
        "task_id": task_id,
        "session_id": session_id,
        "handoff_id": handoff_id,
        "source_id": SOURCE_ID,
        "external_actor": EXTERNAL_ACTOR,
        "kanban_states": {
            "created": {"status": card["status"], "assignee": card["assignee"]},
            "reassigned": {
                "status": after_reassign["status"],
                "assignee": after_reassign["assignee"],
            },
            "before_explicit_complete": {
                "status": before_kanban_complete["status"],
                "assignee": before_kanban_complete["assignee"],
            },
            "final": {"status": final_card["status"], "assignee": final_card["assignee"]},
        },
        "counts": {
            "before_hook": before_hook,
            "after_hook": after_hook,
            "before_internal_preview": before_internal_preview,
            "after_internal_preview": after_internal_preview,
            "before_external_dispatch": before_external_dispatch,
            "after_external_dispatch": after_external_dispatch,
        },
        "timing_ms": round(elapsed_ms, 2),
        "gates": gates,
        "summary": {
            "passed": sum(gates.values()),
            "total": len(gates),
            "ok": all(gates.values()),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_r.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
