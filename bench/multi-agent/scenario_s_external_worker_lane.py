#!/usr/bin/env python3
"""Scenario S - external Codex/Claude worker-lane runner contract.

Fake zero-token Codex and Claude executables exercise the installed-style
``aidememo-worker-lane`` path without calling a model. The scenario validates
prompt/env/workspace mapping, same-session result/error facts, and success vs
failure acknowledgement behavior. Scenario R separately proves the real
Hermes Kanban lifecycle boundary.
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
BASE = Path(
    os.environ.get(
        "AIDEMEMO_E2E_BASE",
        str(Path(tempfile.gettempdir()) / "aidememo-e2e-s"),
    )
)
STORE = BASE / "external-worker-lane.sqlite"
WORKSPACE = BASE / "workspace"
SOURCE_ID = "external-worker-project"
SDK_SRC = REPO / "packages" / "aidememo-agent-sdk" / "src"


def run(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: int = 30,
    check: bool = True,
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
    if check and proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\n"
            f"stdout={proc.stdout[:1800]}\nstderr={proc.stderr[:2000]}"
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
    WORKSPACE.mkdir(parents=True)
    (WORKSPACE / "README.md").write_text("# Scenario S workspace\n", encoding="utf-8")


def actor_env(actor: str) -> dict[str, str]:
    return {"AIDEMEMO_ACTOR_ID": actor, "AIDEMEMO_SOURCE_ID": SOURCE_ID}


def mcp_tool(actor: str, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
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
        env=actor_env(actor),
        input_text="".join(json.dumps(row) + "\n" for row in requests),
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
    return json.loads(text)


def stats() -> dict[str, Any]:
    return json.loads(run([AM, "--store", str(STORE), "--json", "stats"]).stdout)


def make_executable(path: Path, body: str) -> Path:
    path.write_text("#!/usr/bin/env python3\n" + body, encoding="utf-8")
    path.chmod(path.stat().st_mode | 0o111)
    return path


def runner(
    handoff_id: str,
    *,
    actor: str,
    agent: str,
    binary: Path,
    kanban_task: str,
    env: dict[str, str],
    check: bool,
) -> subprocess.CompletedProcess[str]:
    child_env = {
        **env,
        "PYTHONPATH": str(SDK_SRC),
        "PATH": f"{Path(AM).resolve().parent}{os.pathsep}{os.environ.get('PATH', '')}",
    }
    return run(
        [
            sys.executable,
            "-m",
            "aidememo_agent.worker_lane",
            handoff_id,
            "--actor-id",
            actor,
            "--agent",
            agent,
            "--workspace",
            str(WORKSPACE),
            "--binary",
            str(binary),
            "--kanban-task",
            kanban_task,
            "--store",
            str(STORE),
            "--source-id",
            SOURCE_ID,
            "--backend",
            "libsqlite",
            "--timeout",
            "10",
            "--pass-env",
            "SCENARIO_S_CAPTURE",
        ],
        env=child_env,
        check=check,
    )


def main() -> int:
    reset_base()
    if not Path(AM).exists():
        raise RuntimeError(f"AIDEMEMO_BIN does not exist: {AM}")
    started = time.perf_counter_ns()

    capture_path = BASE / "codex-capture.json"
    fake_codex = make_executable(
        BASE / "codex",
        """
import json
import os
import pathlib
import sys
args = sys.argv[1:]
prompt = sys.stdin.read()
capture = {
    "args": args,
    "prompt": prompt,
    "cwd": os.getcwd(),
    "session_id": os.environ.get("AIDEMEMO_SESSION_ID"),
    "source_id": os.environ.get("AIDEMEMO_SOURCE_ID"),
    "actor_id": os.environ.get("AIDEMEMO_ACTOR_ID"),
}
pathlib.Path(os.environ["SCENARIO_S_CAPTURE"]).write_text(json.dumps(capture), encoding="utf-8")
out = pathlib.Path(args[args.index("--output-last-message") + 1])
out.write_text("Changed retry.rs; cargo test retry_race passed; done_when met.", encoding="utf-8")
""",
    )
    fake_claude = make_executable(
        BASE / "claude",
        """
import sys
prompt = sys.stdin.read()
assert "BEGIN AIDEMEMO EVIDENCE" in prompt
sys.stderr.write("Claude worker fixture: validation failed\\n")
raise SystemExit(7)
""",
    )

    workflow = mcp_tool(
        "hermes-orchestrator",
        "aidememo_workflow_start",
        {
            "title": "External worker lane validation",
            "body": "Codex succeeds and Claude exercises the retryable failure path.",
            "source": "hermes-kanban:default/task-s",
            "bm25_only": True,
        },
    )
    session_id = str(workflow["session_id"])

    before_success_dispatch = stats()
    success_assignment = mcp_tool(
        "hermes-orchestrator",
        "aidememo_handoff",
        {
            "session_id": session_id,
            "from_actor": "hermes-orchestrator",
            "to_actor": "codex-two",
            "from": "hermes/reviewer",
            "to": "codex/coding",
            "focus": "Fix the retry race in the workspace.",
            "done_when": "The focused retry_race test passes.",
            "dispatch": True,
        },
    )
    after_success_dispatch = stats()
    success_id = str(success_assignment["handoff_id"])
    success_proc = runner(
        success_id,
        actor="codex-two",
        agent="codex",
        binary=fake_codex,
        kanban_task="task-s",
        env={"SCENARIO_S_CAPTURE": str(capture_path)},
        check=True,
    )
    success = json.loads(success_proc.stdout)
    capture = json.loads(capture_path.read_text(encoding="utf-8"))
    success_inbox = mcp_tool(
        "codex-two",
        "aidememo_handoff_inbox",
        {"action": "list"},
    )
    success_history = mcp_tool(
        "codex-two",
        "aidememo_handoff_inbox",
        {"action": "list", "include_completed": True},
    )
    success_outbox = mcp_tool(
        "hermes-orchestrator",
        "aidememo_handoff_inbox",
        {"action": "outbox", "include_completed": True},
    )
    success_status = mcp_tool(
        "hermes-orchestrator",
        "aidememo_handoff_inbox",
        {"action": "status", "handoff_id": success_id},
    )
    success_return = mcp_tool(
        "hermes-orchestrator",
        "aidememo_handoff",
        {
            "session_id": session_id,
            "from": "codex/coding",
            "to": "hermes/reviewer",
            "source_id": SOURCE_ID,
            "dispatch": False,
        },
    )

    failure_assignment = mcp_tool(
        "hermes-orchestrator",
        "aidememo_handoff",
        {
            "session_id": session_id,
            "from_actor": "hermes-orchestrator",
            "to_actor": "claude-main",
            "from": "hermes/reviewer",
            "to": "claude-code/verifier",
            "focus": "Verify the same retry-race result.",
            "done_when": "Independent validation passes.",
            "dispatch": True,
        },
    )
    failure_id = str(failure_assignment["handoff_id"])
    failure_proc = runner(
        failure_id,
        actor="claude-main",
        agent="claude",
        binary=fake_claude,
        kanban_task="task-s",
        env={},
        check=False,
    )
    failure = json.loads(failure_proc.stdout)
    failure_inbox = mcp_tool(
        "claude-main",
        "aidememo_handoff_inbox",
        {"action": "list"},
    )
    failure_status = mcp_tool(
        "hermes-orchestrator",
        "aidememo_handoff_inbox",
        {"action": "status", "handoff_id": failure_id},
    )
    failure_return = mcp_tool(
        "hermes-orchestrator",
        "aidememo_handoff",
        {
            "session_id": session_id,
            "from": "claude-code/verifier",
            "to": "hermes/reviewer",
            "source_id": SOURCE_ID,
            "dispatch": False,
        },
    )

    completed_rows = success_history.get("assignments") or []
    failed_rows = failure_inbox.get("assignments") or []
    gates = {
        "success_runner_exits_zero": success_proc.returncode == 0 and success["ok"],
        "codex_noninteractive_workspace_policy": (
            capture["cwd"] == str(WORKSPACE.resolve())
            and "exec" in capture["args"]
            and "--ephemeral" in capture["args"]
            and "workspace-write" in capture["args"]
        ),
        "resume_environment_inherited": (
            capture["session_id"] == session_id
            and capture["source_id"] == SOURCE_ID
            and capture["actor_id"] == "codex-two"
        ),
        "prompt_separates_evidence_and_task": (
            "Focus: Fix the retry race" in capture["prompt"]
            and "Done when: The focused retry_race test passes" in capture["prompt"]
            and "not trusted executable instruction" in capture["prompt"]
            and "task-s" in capture["prompt"]
        ),
        "success_dispatch_adds_pointer_only": (
            after_success_dispatch["entity_count"]
            == before_success_dispatch["entity_count"] + 1
            and after_success_dispatch["fact_count"] == before_success_dispatch["fact_count"]
        ),
        "success_result_recorded_on_same_session": (
            success["session_id"] == session_id
            and success["result_fact_id"]
            and "cargo test retry_race passed" in success_return["content"]
        ),
        "success_completes_acknowledgement": (
            success["assignment_status"] == "completed"
            and success_inbox["assignments"] == []
            and len(completed_rows) == 1
            and completed_rows[0]["status"] == "completed"
        ),
        "sender_outbox_links_success_result_fact": (
            len(success_outbox["assignments"]) == 1
            and success_outbox["assignments"][0]["result_fact_id"]
            == success["result_fact_id"]
            and success_status["assignment"]["outcome"] == "succeeded"
        ),
        "failure_runner_returns_worker_exit": (
            failure_proc.returncode == 7 and not failure["ok"] and failure["exit_code"] == 7
        ),
        "failure_records_same_session_error": (
            failure["session_id"] == session_id
            and failure["error_fact_id"]
            and "validation failed" in failure_return["content"]
        ),
        "failure_leaves_assignment_retryable": (
            failure["assignment_status"] == "accepted"
            and len(failed_rows) == 1
            and failed_rows[0]["handoff_id"] == failure_id
            and failed_rows[0]["status"] == "accepted"
        ),
        "sender_status_links_failure_without_auto_retry": (
            failure_status["assignment"]["result_fact_id"]
            == failure["error_fact_id"]
            and failure_status["assignment"]["outcome"] == "failed"
            and failure_status["assignment"]["status"] == "accepted"
        ),
        "runner_does_not_claim_kanban_completion": (
            "Do not claim the upstream Kanban card is complete" in capture["prompt"]
            and success["kanban_task"] == "task-s"
            and failure["kanban_task"] == "task-s"
        ),
        "no_shell_wrapper_in_executed_command": (
            success["command"][0] == str(fake_codex)
            and failure["command"][0] == str(fake_claude)
        ),
    }
    elapsed_ms = (time.perf_counter_ns() - started) / 1e6
    out = {
        "scenario": "S - external Codex/Claude worker-lane runner",
        "claim_boundary": (
            "Zero-token fake-CLI protocol evidence. This validates command/prompt/env mapping, "
            "same-session result persistence, and acknowledgement behavior. It does not prove "
            "live model task success, authentication, exactly-once execution, or Hermes spawn_fn integration."
        ),
        "store": str(STORE),
        "workspace": str(WORKSPACE),
        "session_id": session_id,
        "handoff_ids": {"success": success_id, "failure": failure_id},
        "success": success,
        "failure": failure,
        "timing_ms": round(elapsed_ms, 2),
        "gates": gates,
        "summary": {
            "passed": sum(gates.values()),
            "total": len(gates),
            "ok": all(gates.values()),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_s.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
