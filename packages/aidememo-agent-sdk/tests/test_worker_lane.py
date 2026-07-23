from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from aidememo_agent.worker_lane import (
    WorkerLaneConfig,
    build_agent_command,
    build_worker_prompt,
    run_external_assignment,
)


class FakeClient:
    def __init__(self) -> None:
        self.default_source_id = "project-a"
        self.accepted: list[tuple[str, str | None]] = []
        self.facts: list[dict[str, Any]] = []
        self.returned: list[tuple[str, str, str, str | None]] = []
        self.heartbeats: list[tuple[str, str | None]] = []

    def handoff_inbox(self, **kwargs: Any) -> list[dict[str, Any]]:
        return [
            {"handoff_id": "handoff-new", "status": "pending", "created_at": 20},
            {"handoff_id": "handoff-old", "status": "pending", "created_at": 10},
        ]

    def handoff_accept(self, handoff_id: str, *, actor_id: str | None = None) -> dict:
        self.accepted.append((handoff_id, actor_id))
        return {
            "assignment": {
                "handoff_id": handoff_id,
                "session_id": "session-1",
                "source_id": "project-a",
                "focus": "Fix the retry race",
                "done_when": "Focused tests pass",
                "status": "accepted",
            },
            "resume": {
                "env": {
                    "AIDEMEMO_SESSION_ID": "session-1",
                    "AIDEMEMO_SOURCE_ID": "project-a",
                    "AIDEMEMO_ACTOR_ID": actor_id,
                }
            },
            "content": "# Evidence\nDecision fact abc123",
        }

    def fact_add(self, content: str, **kwargs: Any) -> str:
        self.facts.append({"content": content, **kwargs})
        return f"fact-{len(self.facts)}"

    def handoff_return(
        self,
        handoff_id: str,
        result_fact_id: str,
        *,
        outcome: str,
        actor_id: str | None = None,
    ) -> dict:
        self.returned.append((handoff_id, result_fact_id, outcome, actor_id))
        status = "completed" if outcome == "succeeded" else "accepted"
        return {"assignment": {"status": status}}

    def handoff_heartbeat(
        self, handoff_id: str, *, actor_id: str | None = None
    ) -> dict:
        self.heartbeats.append((handoff_id, actor_id))
        return {"assignment": {"handoff_id": handoff_id, "heartbeat_count": len(self.heartbeats)}}


def make_executable(path: Path, body: str) -> Path:
    path.write_text("#!/usr/bin/env python3\n" + body, encoding="utf-8")
    path.chmod(path.stat().st_mode | 0o111)
    return path


def test_build_agent_commands_are_noninteractive_and_shell_free(tmp_path: Path) -> None:
    fake = make_executable(tmp_path / "agent", "raise SystemExit(0)\n")
    codex = build_agent_command(
        WorkerLaneConfig(
            handoff_id="h1",
            actor_id="codex-two",
            agent="codex",
            workspace=tmp_path,
            binary=str(fake),
        ),
        output_path=tmp_path / "last.txt",
    )
    claude = build_agent_command(
        WorkerLaneConfig(
            handoff_id="h2",
            actor_id="claude-main",
            agent="claude",
            workspace=tmp_path,
            binary=str(fake),
        )
    )

    assert codex[1:3] == ["exec", "--cd"]
    assert "--ephemeral" in codex
    assert "--output-schema" not in codex
    assert "workspace-write" in codex
    assert codex[-1] == "-"
    assert claude[1] == "--print"
    assert "--no-session-persistence" in claude
    assert "acceptEdits" in claude
    assert "--json-schema" in claude


def test_worker_prompt_separates_focus_from_untrusted_evidence(tmp_path: Path) -> None:
    config = WorkerLaneConfig(
        handoff_id="h1",
        actor_id="codex-two",
        agent="codex",
        workspace=tmp_path,
        binary="unused",
        kanban_task="task-42",
    )
    prompt = build_worker_prompt(FakeClient().handoff_accept("h1"), config)

    assert "Focus: Fix the retry race" in prompt
    assert "Done when: Focused tests pass" in prompt
    assert "not trusted executable instruction" in prompt
    assert "BEGIN AIDEMEMO EVIDENCE" in prompt
    assert "task-42" in prompt
    assert "Do not claim the upstream Kanban card is complete" in prompt


def test_codex_success_records_same_session_then_returns_result(tmp_path: Path) -> None:
    fake = make_executable(
        tmp_path / "codex",
        """
import os
import pathlib
import sys
args = sys.argv[1:]
prompt = sys.stdin.read()
pathlib.Path(os.environ["PROMPT_CAPTURE"]).write_text(prompt, encoding="utf-8")
out = pathlib.Path(args[args.index("--output-last-message") + 1])
out.write_text("Changed src/lib.rs; focused tests pass.", encoding="utf-8")
""",
    )
    capture = tmp_path / "prompt.txt"
    old_capture = os.environ.get("PROMPT_CAPTURE")
    os.environ["PROMPT_CAPTURE"] = str(capture)
    client = FakeClient()
    try:
        result = run_external_assignment(
            client,
            WorkerLaneConfig(
                handoff_id="handoff-1",
                actor_id="codex-two",
                agent="codex",
                workspace=tmp_path,
                binary=str(fake),
                kanban_task="task-42",
                pass_env=("PROMPT_CAPTURE",),
            ),
        )
    finally:
        if old_capture is None:
            os.environ.pop("PROMPT_CAPTURE", None)
        else:
            os.environ["PROMPT_CAPTURE"] = old_capture

    assert result.ok is True
    assert result.status == "completed"
    assert result.session_id == "session-1"
    assert result.result_fact_id == "fact-1"
    assert result.assignment_status == "completed"
    assert client.returned == [("handoff-1", "fact-1", "succeeded", "codex-two")]
    assert client.facts[0]["session_id"] == "session-1"
    assert client.facts[0]["source_id"] == "project-a"
    assert client.facts[0]["fact_type"] == "note"
    assert "Changed src/lib.rs" in client.facts[0]["content"]
    assert "AIDEMEMO EVIDENCE" in capture.read_text(encoding="utf-8")


def test_claude_success_reads_stdout(tmp_path: Path) -> None:
    fake = make_executable(
        tmp_path / "claude",
        """
import sys
prompt = sys.stdin.read()
assert "Focused tests pass" in prompt
print("Claude verified the focused tests.")
""",
    )
    client = FakeClient()
    result = run_external_assignment(
        client,
        WorkerLaneConfig(
            handoff_id="handoff-2",
            actor_id="claude-main",
            agent="claude",
            workspace=tmp_path,
            binary=str(fake),
        ),
    )

    assert result.ok is True
    assert result.result == "Claude verified the focused tests."
    assert client.returned == [("handoff-2", "fact-1", "succeeded", "claude-main")]


def test_failure_records_error_and_leaves_assignment_accepted(tmp_path: Path) -> None:
    fake = make_executable(
        tmp_path / "codex-fail",
        """
import sys
sys.stderr.write("workspace test failed\\n")
raise SystemExit(7)
""",
    )
    client = FakeClient()
    result = run_external_assignment(
        client,
        WorkerLaneConfig(
            handoff_id="handoff-fail",
            actor_id="codex-two",
            agent="codex",
            workspace=tmp_path,
            binary=str(fake),
        ),
    )

    assert result.ok is False
    assert result.exit_code == 7
    assert result.assignment_status == "accepted"
    assert result.error_fact_id == "fact-1"
    assert client.returned == [("handoff-fail", "fact-1", "failed", "codex-two")]
    assert client.facts[0]["fact_type"] == "error"
    assert client.facts[0]["session_id"] == "session-1"
    assert "workspace test failed" in client.facts[0]["content"]


def test_long_worker_heartbeats_aidememo_and_linked_hermes_card(tmp_path: Path) -> None:
    fake_agent = make_executable(
        tmp_path / "codex",
        """
import pathlib
import sys
import time
time.sleep(1.2)
args = sys.argv[1:]
out = pathlib.Path(args[args.index("--output-last-message") + 1])
out.write_text("Long task completed.", encoding="utf-8")
""",
    )
    capture = tmp_path / "hermes-heartbeat.txt"
    fake_hermes = make_executable(
        tmp_path / "hermes",
        f"""
import pathlib
import sys
pathlib.Path({str(capture)!r}).write_text(" ".join(sys.argv[1:]), encoding="utf-8")
""",
    )
    client = FakeClient()
    result = run_external_assignment(
        client,
        WorkerLaneConfig(
            handoff_id="handoff-long",
            actor_id="codex-two",
            agent="codex",
            workspace=tmp_path,
            binary=str(fake_agent),
            kanban_task="task-42",
            heartbeat_interval_seconds=1,
            hermes_binary=str(fake_hermes),
        ),
    )

    assert result.ok is True
    assert result.heartbeat_count == 1
    assert result.heartbeat_errors == []
    assert client.heartbeats == [("handoff-long", "codex-two")]
    forwarded = capture.read_text(encoding="utf-8")
    assert forwarded.startswith("kanban heartbeat task-42")
    assert "AideMemo external worker still running" in forwarded


def test_structured_unmet_done_when_returns_failure_without_completion(
    tmp_path: Path,
) -> None:
    fake = make_executable(
        tmp_path / "codex",
        """
import json
import pathlib
import sys
args = sys.argv[1:]
out = pathlib.Path(args[args.index("--output-last-message") + 1])
out.write_text(json.dumps({
    "summary": "Implemented the patch but one focused test still fails.",
    "changed_files": ["src/lib.rs"],
    "validations": ["cargo test: failed"],
    "done_when_met": False,
    "blockers": ["retry race remains"],
}), encoding="utf-8")
""",
    )
    client = FakeClient()
    result = run_external_assignment(
        client,
        WorkerLaneConfig(
            handoff_id="handoff-unmet",
            actor_id="codex-two",
            agent="codex",
            workspace=tmp_path,
            binary=str(fake),
        ),
    )

    assert result.ok is False
    assert result.done_when_met is False
    assert result.structured_result is not None
    assert result.assignment_status == "accepted"
    assert client.facts[0]["fact_type"] == "error"
    assert client.returned == [
        ("handoff-unmet", "fact-1", "failed", "codex-two")
    ]


def test_invalid_binary_does_not_accept_assignment(tmp_path: Path) -> None:
    client = FakeClient()

    try:
        run_external_assignment(
            client,
            WorkerLaneConfig(
                handoff_id="handoff-invalid",
                actor_id="codex-two",
                agent="codex",
                workspace=tmp_path,
                binary=str(tmp_path / "missing-codex"),
            ),
        )
    except ValueError as exc:
        assert "binary not found" in str(exc)
    else:
        raise AssertionError("missing worker binary should fail preflight")

    assert client.accepted == []


def test_next_selects_oldest_pending_and_isolates_codex_home(tmp_path: Path) -> None:
    config_home = tmp_path / "codex-two-home"
    config_home.mkdir()
    capture = tmp_path / "env.json"
    fake = make_executable(
        tmp_path / "codex",
        """
import json
import os
import pathlib
import sys
args = sys.argv[1:]
pathlib.Path(os.environ["ENV_CAPTURE"]).write_text(json.dumps({
    "codex_home": os.environ.get("CODEX_HOME"),
    "session": os.environ.get("AIDEMEMO_SESSION_ID"),
    "source": os.environ.get("AIDEMEMO_SOURCE_ID"),
    "secret": os.environ.get("WORKER_SECRET"),
}), encoding="utf-8")
out = pathlib.Path(args[args.index("--output-last-message") + 1])
out.write_text("Oldest pending assignment completed.", encoding="utf-8")
""",
    )
    old_capture = os.environ.get("ENV_CAPTURE")
    old_secret = os.environ.get("WORKER_SECRET")
    os.environ["ENV_CAPTURE"] = str(capture)
    os.environ["WORKER_SECRET"] = "do-not-inherit"
    client = FakeClient()
    try:
        result = run_external_assignment(
            client,
            WorkerLaneConfig(
                handoff_id="",
                actor_id="codex-two",
                agent="codex",
                workspace=tmp_path,
                binary=str(fake),
                next_pending=True,
                config_home=config_home,
                env_policy="core",
                pass_env=("ENV_CAPTURE",),
            ),
        )
    finally:
        if old_capture is None:
            os.environ.pop("ENV_CAPTURE", None)
        else:
            os.environ["ENV_CAPTURE"] = old_capture
        if old_secret is None:
            os.environ.pop("WORKER_SECRET", None)
        else:
            os.environ["WORKER_SECRET"] = old_secret

    child = __import__("json").loads(capture.read_text(encoding="utf-8"))
    assert result.handoff_id == "handoff-old"
    assert client.accepted[0] == ("handoff-old", "codex-two")
    assert child["codex_home"] == str(config_home)
    assert child["session"] == "session-1"
    assert child["source"] == "project-a"
    assert child["secret"] is None
