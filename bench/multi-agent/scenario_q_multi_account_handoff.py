#!/usr/bin/env python3
"""Scenario Q - multi-account handoff without broker semantics.

Three independent MCP processes share one store as codex-one, codex-two, and
claude-main. The scenario proves that handoff records are pull-based session
pointers: no topic, offset, consumer group, retry state, or copied payload is
persisted. It does not measure whether a downstream model completes the task.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
AM = os.environ.get("AIDEMEMO_BIN", str(REPO / "target" / "debug" / "aidememo"))
STORE = os.environ.get(
    "AIDEMEMO_E2E_STORE",
    str(Path(tempfile.gettempdir()) / "aidememo-e2e-q" / "multi-account.sqlite"),
)
SOURCE_ID = "account-handoff-project"
OTHER_SOURCE_ID = "unrelated-project"
ACTORS = ("codex-one", "codex-two", "claude-main")
RECEIVER_FACT = "Review result: the account handoff keeps one tracked session."
FORBIDDEN_RECORD_KEYS = {
    "content",
    "payload",
    "topic",
    "offset",
    "consumer_group",
    "retry",
    "retry_count",
    "delivery_attempt",
}


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
            f"stdout={proc.stdout[:1200]}\nstderr={proc.stderr[:1800]}"
        )
    return proc


def reset_store() -> None:
    path = Path(STORE)
    if path.parent.exists():
        for sibling in path.parent.iterdir():
            if sibling.name.startswith(path.name):
                if sibling.is_dir():
                    shutil.rmtree(sibling)
                else:
                    sibling.unlink()
    path.parent.mkdir(parents=True, exist_ok=True)


def actor_env(actor: str, session_id: str | None = None) -> dict[str, str]:
    env = {
        "AIDEMEMO_ACTOR_ID": actor,
        "AIDEMEMO_SOURCE_ID": SOURCE_ID,
    }
    if session_id:
        env["AIDEMEMO_SESSION_ID"] = session_id
    return env


def mcp_tool(
    actor: str,
    name: str,
    arguments: dict[str, Any],
    *,
    session_id: str | None = None,
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
        [AM, "--store", STORE, "mcp"],
        env=actor_env(actor, session_id),
        input_text="".join(json.dumps(request) + "\n" for request in requests),
    )
    responses = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
    response = next((row for row in responses if row.get("id") == 1), None)
    if response is None or response.get("error"):
        raise RuntimeError(f"MCP {actor}/{name} failed: {response or proc.stdout[:1200]}")
    result = response.get("result") or {}
    if result.get("isError"):
        raise RuntimeError(f"MCP {actor}/{name} returned isError: {result}")
    blocks = result.get("content") or []
    text = "\n".join(
        str(block.get("text") or "")
        for block in blocks
        if isinstance(block, dict) and block.get("type") == "text"
    ).strip()
    if not text:
        raise RuntimeError(f"MCP {actor}/{name} returned no text")
    return json.loads(text)


def stats() -> dict[str, Any]:
    return json.loads(run([AM, "--store", STORE, "--json", "stats"]).stdout)


def assignment_record(handoff_id: str) -> dict[str, Any]:
    entity = json.loads(
        run([AM, "--store", STORE, "--json", "entity", "get", handoff_id]).stdout
    )
    return json.loads(entity["source_page"])


def record_keys(value: Any) -> set[str]:
    if isinstance(value, dict):
        keys = {str(key) for key in value}
        for nested in value.values():
            keys.update(record_keys(nested))
        return keys
    if isinstance(value, list):
        keys: set[str] = set()
        for nested in value:
            keys.update(record_keys(nested))
        return keys
    return set()


def main() -> int:
    reset_store()
    if not Path(AM).exists():
        raise RuntimeError(f"AIDEMEMO_BIN does not exist: {AM}")
    started = time.perf_counter_ns()

    workflow = mcp_tool(
        "codex-one",
        "aidememo_workflow_start",
        {
            "title": "Review a multi-account handoff",
            "body": "Codex one implements, Codex two reviews, Claude verifies.",
            "source": "orchestrator:scenario-q/run-01",
            "bm25_only": True,
        },
    )
    session_id = str(workflow["session_id"])
    mcp_tool(
        "codex-one",
        "aidememo_fact_add",
        {
            "content": "Decision: use account aliases only as routing metadata.",
            "fact_type": "decision",
            "entities": ["AccountHandoff"],
            "session_id": session_id,
        },
    )
    before_dispatch = stats()

    first = mcp_tool(
        "codex-one",
        "aidememo_handoff",
        {
            "session_id": session_id,
            "to_actor": "codex-two",
            "from": "codex/coding",
            "to": "codex/reviewer",
            "focus": "Review the account-handoff contract.",
            "done_when": "Focused tests pass.",
            "dispatch": True,
        },
    )
    first_id = str(first["handoff_id"])
    after_first_dispatch = stats()
    stored_first = assignment_record(first_id)

    codex_two_inbox = mcp_tool(
        "codex-two", "aidememo_handoff_inbox", {"action": "list"}
    )
    codex_one_inbox = mcp_tool(
        "codex-one", "aidememo_handoff_inbox", {"action": "list"}
    )
    claude_before = mcp_tool(
        "claude-main", "aidememo_handoff_inbox", {"action": "list"}
    )
    wrong_source = mcp_tool(
        "codex-two",
        "aidememo_handoff_inbox",
        {"action": "list", "source_id": OTHER_SOURCE_ID},
    )
    accepted_first = mcp_tool(
        "codex-two",
        "aidememo_handoff_inbox",
        {"action": "accept", "handoff_id": first_id},
    )

    accepted_session_id = str(
        accepted_first["resume"]["env"]["AIDEMEMO_SESSION_ID"]
    )
    mcp_tool(
        "codex-two",
        "aidememo_fact_add",
        {
            "content": RECEIVER_FACT,
            "fact_type": "lesson",
            "entities": ["AccountHandoff"],
            # The receiver uses the structured accept result directly. No user
            # has to find or copy a session id between account terminals.
            "session_id": accepted_session_id,
        },
    )
    after_receiver_write = stats()

    second = mcp_tool(
        "codex-two",
        "aidememo_handoff",
        {
            "session_id": session_id,
            "to_actor": "claude-main",
            "from": "codex/reviewer",
            "to": "claude-code/verifier",
            "focus": "Verify the reviewed handoff without copying context.",
            "done_when": "The receiver fact is visible in the same session.",
            "dispatch": True,
        },
    )
    second_id = str(second["handoff_id"])
    after_second_dispatch = stats()
    claude_inbox = mcp_tool(
        "claude-main", "aidememo_handoff_inbox", {"action": "list"}
    )
    accepted_second = mcp_tool(
        "claude-main",
        "aidememo_handoff_inbox",
        {"action": "accept", "handoff_id": second_id},
    )
    first_complete = mcp_tool(
        "codex-two",
        "aidememo_handoff_inbox",
        {"action": "complete", "handoff_id": first_id},
    )
    second_complete = mcp_tool(
        "claude-main",
        "aidememo_handoff_inbox",
        {"action": "complete", "handoff_id": second_id},
    )
    codex_two_after = mcp_tool(
        "codex-two", "aidememo_handoff_inbox", {"action": "list"}
    )
    claude_after = mcp_tool(
        "claude-main", "aidememo_handoff_inbox", {"action": "list"}
    )

    first_assignments = codex_two_inbox["assignments"]
    claude_assignments = claude_inbox["assignments"]
    forbidden_keys = sorted(record_keys(stored_first) & FORBIDDEN_RECORD_KEYS)
    gates = {
        "sender_actor_env_fallback": first["from_actor"] == "codex-one",
        "codex_two_only_sees_first_assignment": (
            [item["handoff_id"] for item in first_assignments] == [first_id]
            and codex_one_inbox["assignments"] == []
            and claude_before["assignments"] == []
        ),
        "source_scope_isolated": wrong_source["assignments"] == [],
        "accept_preserves_session": (
            accepted_first["assignment"]["status"] == "accepted"
            and accepted_first["resume"]["env"]["AIDEMEMO_SESSION_ID"] == session_id
            and accepted_first["resume"]["env"]["AIDEMEMO_ACTOR_ID"] == "codex-two"
        ),
        "receiver_writes_to_same_session": RECEIVER_FACT in accepted_second["content"],
        "second_route_reaches_claude_only": (
            [item["handoff_id"] for item in claude_assignments] == [second_id]
            and claude_assignments[0]["session_id"] == session_id
        ),
        "dispatch_adds_pointer_entity_not_fact": (
            after_first_dispatch["entity_count"] == before_dispatch["entity_count"] + 1
            and after_first_dispatch["fact_count"] == before_dispatch["fact_count"]
            and after_second_dispatch["entity_count"] == after_receiver_write["entity_count"] + 1
            and after_second_dispatch["fact_count"] == after_receiver_write["fact_count"]
        ),
        "assignment_has_no_broker_or_payload_keys": forbidden_keys == [],
        "receiver_write_is_one_new_fact": (
            after_receiver_write["fact_count"] == after_first_dispatch["fact_count"] + 1
        ),
        "completion_is_explicit_and_inbox_hides_completed": (
            first_complete["assignment"]["status"] == "completed"
            and second_complete["assignment"]["status"] == "completed"
            and codex_two_after["assignments"] == []
            and claude_after["assignments"] == []
        ),
    }
    elapsed_ms = (time.perf_counter_ns() - started) / 1e6
    out = {
        "scenario": "Q - multi-account handoff without broker semantics",
        "claim_boundary": (
            "Zero-token routing evidence only: account aliases can pull, accept, and complete "
            "session-pointer assignments. This is not authenticated identity, exclusive locking, "
            "exactly-once delivery, or downstream model task-success evidence."
        ),
        "store": STORE,
        "actors": list(ACTORS),
        "source_id": SOURCE_ID,
        "session_id": session_id,
        "handoff_ids": [first_id, second_id],
        "persisted_assignment_keys": sorted(record_keys(stored_first)),
        "forbidden_persisted_keys": forbidden_keys,
        "counts": {
            "before_dispatch": before_dispatch,
            "after_first_dispatch": after_first_dispatch,
            "after_receiver_write": after_receiver_write,
            "after_second_dispatch": after_second_dispatch,
        },
        "timing_ms": round(elapsed_ms, 2),
        "gates": gates,
        "summary": {
            "passed": sum(gates.values()),
            "total": len(gates),
            "ok": all(gates.values()),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_q.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
