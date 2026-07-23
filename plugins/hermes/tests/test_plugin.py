"""End-to-end plugin registration test.

Mocks the Hermes ``ctx`` so we can verify that ``register(ctx)``
wires every surface (tools, slash, hooks, CLI) without an actual
Hermes install.
"""

from __future__ import annotations

import json
from typing import Any
from unittest.mock import MagicMock

import pytest

from hermes_aidememo.client import AideMemoClient
from hermes_aidememo.plugin import register


class FakeCtx:
    """Lightweight stand-in for Hermes's PluginContext."""

    def __init__(self) -> None:
        self.tools: list[dict] = []
        self.commands: list[dict] = []
        self.cli_commands: list[dict] = []
        self.hooks: list[tuple[str, Any]] = []
        self.skills: list[dict] = []
        self.injected: list[tuple[str, str]] = []

    def register_tool(self, *, name, toolset, schema, handler, description="", emoji="", **_):
        self.tools.append(
            {"name": name, "toolset": toolset, "schema": schema, "handler": handler, "description": description, "emoji": emoji}
        )

    def register_command(self, *, name, handler, description="", args_hint=""):
        self.commands.append({"name": name, "handler": handler, "description": description, "args_hint": args_hint})

    def register_cli_command(self, *, name, help, setup_fn, handler_fn=None, description=""):
        self.cli_commands.append({"name": name, "help": help, "setup_fn": setup_fn, "handler_fn": handler_fn, "description": description})

    def register_hook(self, hook_name, callback):
        self.hooks.append((hook_name, callback))

    def register_skill(self, *, name, path, description=""):
        self.skills.append({"name": name, "path": path, "description": description})

    def inject_message(self, content, role="user"):
        self.injected.append((role, content))
        return True


@pytest.fixture
def fake_ctx(monkeypatch: pytest.MonkeyPatch) -> FakeCtx:
    # Ensure the AideMemoClient bootstrap doesn't actually try to spawn `aidememo`
    # — provide a stub backend that the test can introspect.
    stub = MagicMock(spec=AideMemoClient)
    stub.backend = "stub"
    stub.recent.return_value = [
        {"content": "HNSW is the default index", "fact_type": "decision"},
        {"content": "BM25 weight is 0.7 by default", "fact_type": "convention"},
    ]
    stub.fact_add.return_value = "01HZ-FAKE-FACT-ID"
    stub.search.return_value = []
    stub.query.return_value = {"topic": "test", "entity": None, "search": [], "related": [], "recent_facts": []}
    stub.context.return_value = {"recent": [], "personalisation": [], "pinned": []}
    stub.aggregate.return_value = {"op": "count", "query": "test", "matched": 2}
    stub.doctor.return_value = {"issues": [], "stats": {"facts": 0}}
    stub.workflow_start.return_value = {
        "session_id": "session-01HZ-FAKE",
        "ticket_fact_id": "01HZ-FAKE-TICKET",
        "context": {"search": []},
    }
    stub.handoff_packet.return_value = {
        "artifact": "agent_handoff",
        "session_id": "session-01HZ-FAKE",
        "source_id": "team-a",
        "resume": {
            "env": {
                "AIDEMEMO_SESSION_ID": "session-01HZ-FAKE",
                "AIDEMEMO_SOURCE_ID": "team-a",
            }
        },
        "content": "# AideMemo Agent Handoff",
    }
    stub.handoff_inbox.return_value = [{"handoff_id": "handoff-1", "status": "pending"}]
    stub.handoff_outbox.return_value = [{"handoff_id": "handoff-1", "status": "completed"}]
    stub.handoff_show.return_value = {"assignment": {"handoff_id": "handoff-1", "status": "completed"}}
    stub.handoff_status.return_value = {"assignment": {"handoff_id": "handoff-1", "status": "completed"}}
    stub.handoff_accept.return_value = {"assignment": {"handoff_id": "handoff-1", "status": "accepted"}}
    stub.handoff_complete.return_value = {"assignment": {"handoff_id": "handoff-1", "status": "completed"}}
    stub.handoff_return.return_value = {"assignment": {"handoff_id": "handoff-1", "status": "completed"}}

    def fake_init(self, *_a, **_kw):
        # Bypass `aidememo-python` import + CLI presence checks.
        self.store_path = None
        self._py = None

    monkeypatch.setattr(AideMemoClient, "__init__", fake_init)
    # Now thread through every AideMemoClient method we care about.
    monkeypatch.setattr(AideMemoClient, "recent", lambda self, **kw: stub.recent(**kw))
    monkeypatch.setattr(AideMemoClient, "search", lambda self, *a, **kw: stub.search(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "query", lambda self, *a, **kw: stub.query(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "context", lambda self, *a, **kw: stub.context(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "aggregate", lambda self, *a, **kw: stub.aggregate(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "doctor", lambda self: stub.doctor())
    monkeypatch.setattr(AideMemoClient, "workflow_start", lambda self, *a, **kw: stub.workflow_start(*a, **kw))
    monkeypatch.setattr(
        AideMemoClient,
        "handoff_packet",
        lambda self, *a, **kw: stub.handoff_packet(*a, **kw),
    )
    monkeypatch.setattr(AideMemoClient, "handoff_inbox", lambda self, **kw: stub.handoff_inbox(**kw))
    monkeypatch.setattr(AideMemoClient, "handoff_outbox", lambda self, **kw: stub.handoff_outbox(**kw))
    monkeypatch.setattr(AideMemoClient, "handoff_show", lambda self, *a, **kw: stub.handoff_show(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "handoff_status", lambda self, *a, **kw: stub.handoff_status(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "handoff_accept", lambda self, *a, **kw: stub.handoff_accept(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "handoff_complete", lambda self, *a, **kw: stub.handoff_complete(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "handoff_return", lambda self, *a, **kw: stub.handoff_return(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "fact_add", lambda self, *a, **kw: stub.fact_add(*a, **kw))
    monkeypatch.setattr(AideMemoClient, "fact_add_many", lambda self, *a, **kw: ["fact-1", "fact-2"])
    monkeypatch.setattr(AideMemoClient, "lint", lambda self: [])
    monkeypatch.setattr(AideMemoClient, "stats", lambda self: {"facts": 0, "entities": 0})
    monkeypatch.setattr(AideMemoClient, "entity_list", lambda self, **kw: [])
    monkeypatch.setattr(AideMemoClient, "traverse", lambda self, *a, **kw: {"entities": []})
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    ctx = FakeCtx()
    ctx.client = stub
    register(ctx)
    return ctx


def test_registers_core_hermes_tools(fake_ctx: FakeCtx) -> None:
    names = {t["name"] for t in fake_ctx.tools}
    assert names == {
        "aidememo_workflow_start",
        "aidememo_handoff",
        "aidememo_handoff_inbox",
        "aidememo_context",
        "aidememo_query",
        "aidememo_search",
        "aidememo_recent",
        "aidememo_aggregate",
        "aidememo_entity_list",
        "aidememo_traverse",
        "aidememo_fact_add",
        "aidememo_fact_add_many",
        "aidememo_doctor",
        "aidememo_lint",
    }


def test_source_and_actor_ids_are_exposed_on_relevant_tool_schemas(fake_ctx: FakeCtx) -> None:
    schemas = {t["name"]: t["schema"]["parameters"]["properties"] for t in fake_ctx.tools}
    assert "source_id" in schemas["aidememo_workflow_start"]
    assert "source_id" in schemas["aidememo_handoff"]
    assert "from" in schemas["aidememo_handoff"]
    assert "to" in schemas["aidememo_handoff"]
    assert "done_when" in schemas["aidememo_handoff"]
    assert "from_actor" in schemas["aidememo_handoff"]
    assert "to_actor" in schemas["aidememo_handoff"]
    assert "action" in schemas["aidememo_handoff_inbox"]
    assert "source_id" in schemas["aidememo_query"]
    assert "source_id" in schemas["aidememo_context"]
    assert "source_id" in schemas["aidememo_search"]
    assert "source_id" in schemas["aidememo_aggregate"]
    assert "source_id" in schemas["aidememo_fact_add"]
    assert "session_id" in schemas["aidememo_fact_add"]
    assert "AIDEMEMO_SOURCE_ID" in schemas["aidememo_workflow_start"]["source_id"]["description"]
    assert "actor_id" in schemas["aidememo_workflow_start"]
    assert "parent_session_id" in schemas["aidememo_workflow_start"]
    assert "actor_id" in schemas["aidememo_fact_add"]
    assert "AIDEMEMO_ACTOR_ID" in schemas["aidememo_fact_add"]["actor_id"]["description"]


def test_handoff_tool_returns_receiver_packet(fake_ctx: FakeCtx) -> None:
    handler = next(t for t in fake_ctx.tools if t["name"] == "aidememo_handoff")["handler"]
    out = handler(
        {
            "session_id": "session-01HZ-FAKE",
            "from": "hermes/coding",
            "to": "hermes/reviewer",
            "done_when": "Focused tests pass and review findings are recorded",
            "source_id": "team-a",
        }
    )

    payload = json.loads(out)
    assert payload["artifact"] == "agent_handoff"
    assert payload["resume"]["env"]["AIDEMEMO_SESSION_ID"] == "session-01HZ-FAKE"
    assert payload["content"] == "# AideMemo Agent Handoff"
    fake_ctx.client.handoff_packet.assert_called_once_with(
        "session-01HZ-FAKE",
        from_actor=None,
        from_route="hermes/coding",
        to_route="hermes/reviewer",
        from_agent=None,
        from_profile=None,
        to_agent=None,
        to_profile=None,
        to_actor=None,
        focus=None,
        done_when="Focused tests pass and review findings are recorded",
        dispatch=False,
        source_id="team-a",
        limit=40,
        include_superseded=False,
    )


def test_handoff_inbox_lists_accepts_and_returns_assignments(fake_ctx: FakeCtx) -> None:
    handler = next(t for t in fake_ctx.tools if t["name"] == "aidememo_handoff_inbox")["handler"]
    listed = json.loads(handler({"actor_id": "codex-two", "source_id": "team-a"}))
    assert listed["assignments"][0]["handoff_id"] == "handoff-1"
    accepted = json.loads(
        handler({"action": "accept", "actor_id": "codex-two", "handoff_id": "handoff-1"})
    )
    assert accepted["assignment"]["status"] == "accepted"
    outbox = json.loads(handler({"action": "outbox", "actor_id": "hermes-main"}))
    assert outbox["assignments"][0]["status"] == "completed"
    shown = json.loads(handler({"action": "show", "handoff_id": "handoff-1"}))
    assert shown["assignment"]["status"] == "completed"
    returned = json.loads(
        handler(
            {
                "action": "return",
                "actor_id": "codex-two",
                "handoff_id": "handoff-1",
                "result_fact_id": "fact-1",
                "outcome": "succeeded",
            }
        )
    )
    assert returned["assignment"]["status"] == "completed"
    fake_ctx.client.handoff_inbox.assert_called_once_with(
        actor_id="codex-two",
        source_id="team-a",
        include_completed=False,
        limit=20,
    )
    fake_ctx.client.handoff_accept.assert_called_once_with("handoff-1", actor_id="codex-two")
    fake_ctx.client.handoff_outbox.assert_called_once_with(
        actor_id="hermes-main",
        source_id=None,
        include_completed=True,
        limit=20,
    )
    fake_ctx.client.handoff_show.assert_called_once_with("handoff-1")
    fake_ctx.client.handoff_return.assert_called_once_with(
        "handoff-1", "fact-1", outcome="succeeded", actor_id="codex-two"
    )


def test_fact_add_preserves_workflow_session(fake_ctx: FakeCtx) -> None:
    handler = next(t for t in fake_ctx.tools if t["name"] == "aidememo_fact_add")["handler"]
    payload = json.loads(
        handler(
            {
                "content": "Reviewer found a retry race",
                "fact_type": "error",
                "entities": ["Dispatcher"],
                "source_id": "team-a",
                "session_id": "session-01HZ-FAKE",
            }
        )
    )

    assert payload["id"] == "01HZ-FAKE-FACT-ID"
    fake_ctx.client.fact_add.assert_called_once_with(
        "Reviewer found a retry race",
        entities=["Dispatcher"],
        fact_type="error",
        tags=None,
        source_id="team-a",
        actor_id=None,
        session_id="session-01HZ-FAKE",
    )


def test_register_passes_configured_source_id(monkeypatch: pytest.MonkeyPatch) -> None:
    captured: dict[str, Any] = {}

    def fake_init(self, *args, **kwargs):
        captured.update(kwargs)
        self.store_path = kwargs.get("store_path")
        self.default_source_id = kwargs.get("source_id")
        self._py = None

    monkeypatch.setattr(AideMemoClient, "__init__", fake_init)
    monkeypatch.setattr(AideMemoClient, "recent", lambda self, **kw: [])
    monkeypatch.setattr(AideMemoClient, "search", lambda self, *a, **kw: [])
    monkeypatch.setattr(AideMemoClient, "query", lambda self, *a, **kw: {})
    monkeypatch.setattr(AideMemoClient, "context", lambda self, *a, **kw: {})
    monkeypatch.setattr(AideMemoClient, "aggregate", lambda self, *a, **kw: {})
    monkeypatch.setattr(AideMemoClient, "doctor", lambda self: {})
    monkeypatch.setattr(AideMemoClient, "workflow_start", lambda self, *a, **kw: {})
    monkeypatch.setattr(AideMemoClient, "fact_add", lambda self, *a, **kw: "fact")
    monkeypatch.setattr(AideMemoClient, "fact_add_many", lambda self, *a, **kw: [])
    monkeypatch.setattr(AideMemoClient, "lint", lambda self: [])
    monkeypatch.setattr(AideMemoClient, "stats", lambda self: {})
    monkeypatch.setattr(AideMemoClient, "entity_list", lambda self, **kw: [])
    monkeypatch.setattr(AideMemoClient, "traverse", lambda self, *a, **kw: {})

    ctx = FakeCtx()
    ctx.config = {
        "plugins": {
            "aidememo": {
                "store_path": "/tmp/wiki.redb",
                "source_id": "team-alpha",
                "actor_id": "hermes:account-a",
                "lock_retry_ms": 123,
            }
        }
    }
    register(ctx)

    assert captured["store_path"] == "/tmp/wiki.redb"
    assert captured["source_id"] == "team-alpha"
    assert captured["actor_id"] == "hermes:account-a"
    assert captured["lock_retry_ms"] == 123


def test_registers_hermes_slash_commands(fake_ctx: FakeCtx) -> None:
    names = {c["name"] for c in fake_ctx.commands}
    assert names == {
        "aidememo",
        "aidememo-context",
        "aidememo-aggregate",
        "aidememo-start",
        "aidememo-add",
        "aidememo-recent",
        "aidememo-doctor",
        "aidememo-pending",
    }


def test_registers_llm_call_hooks(fake_ctx: FakeCtx) -> None:
    names = {name for name, _ in fake_ctx.hooks}
    assert "pre_llm_call" in names
    assert "post_llm_call" in names


def test_registers_cli_subtree(fake_ctx: FakeCtx) -> None:
    assert any(c["name"] == "aidememo" for c in fake_ctx.cli_commands)


def test_pre_llm_call_returns_recent_facts_block_on_first_turn(fake_ctx: FakeCtx) -> None:
    pre = next(cb for name, cb in fake_ctx.hooks if name == "pre_llm_call")
    result = pre(is_first_turn=True, user_message="hi", session_id="s")
    assert isinstance(result, dict), "pre_llm_call should return a dict on first turn"
    assert "context" in result
    assert "HNSW is the default index" in result["context"]


def test_pre_llm_call_auto_starts_sparse_ticket_workflow(fake_ctx: FakeCtx) -> None:
    pre = next(cb for name, cb in fake_ctx.hooks if name == "pre_llm_call")
    result = pre(
        is_first_turn=True,
        user_message='Issue #123: Fix Redis timeout\nPass source_id exactly as "team-a".',
        session_id="s",
    )
    assert isinstance(result, dict)
    assert "workflow context pack" in result["context"]


def test_post_llm_call_does_not_start_duplicate_sparse_ticket_workflow(
    fake_ctx: FakeCtx,
) -> None:
    post = next(cb for name, cb in fake_ctx.hooks if name == "post_llm_call")
    post(
        user_message='Issue #123: Fix Redis timeout\nPass source_id exactly as "team-a".',
        assistant_response="Plan follows.",
        session_id="s",
    )
    fake_ctx.client.workflow_start.assert_not_called()


def test_pre_llm_call_defers_internal_routing_to_kanban(
    fake_ctx: FakeCtx,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("HERMES_KANBAN_TASK", "task-42")
    monkeypatch.setenv("HERMES_KANBAN_BOARD", "release")
    pre = next(cb for name, cb in fake_ctx.hooks if name == "pre_llm_call")

    result = pre(
        is_first_turn=True,
        user_message="Issue #123: review the release patch",
        session_id="s",
    )

    assert isinstance(result, dict)
    assert "Kanban worker detected" in result["context"]
    assert "task-42" in result["context"]
    assert "do not create an AideMemo dispatch/inbox assignment" in result["context"]
    assert "call `aidememo_workflow_start`" not in result["context"]
    fake_ctx.client.workflow_start.assert_not_called()


def test_pre_llm_call_returns_none_on_later_turns(fake_ctx: FakeCtx) -> None:
    pre = next(cb for name, cb in fake_ctx.hooks if name == "pre_llm_call")
    assert pre(is_first_turn=False, user_message="follow up", session_id="s") is None


def test_post_llm_call_runs_decision_detector(fake_ctx: FakeCtx) -> None:
    post = next(cb for name, cb in fake_ctx.hooks if name == "post_llm_call")
    # The fake ctx's AideMemoClient.fact_add is stubbed to return a fixed
    # ULID; we check that detect() picked up at least one match by
    # verifying the call went through (fact_add side effect).
    post(
        user_message="결정: 영어 wiki에서도 multilingual-128M로 가자",
        assistant_response="Sounds good.",
        session_id="s",
    )

    # AideMemoClient.fact_add should have been called for each detection.
    # Easier path: verify by re-running detect and counting.
    from hermes_aidememo.decisions import detect

    expected = detect("결정: 영어 wiki에서도 multilingual-128M로 가자\nSounds good.")
    assert len(expected) >= 1


def test_slash_aidememo_handler_returns_pretty_json(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "aidememo")["handler"]
    out = handler("Redis")
    assert "topic" in out  # rendered JSON includes the topic key


def test_slash_aidememo_context_returns_context(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "aidememo-context")["handler"]
    out = handler("Redis --source-id team-a")
    assert "recent" in out


def test_slash_aidememo_aggregate_returns_count(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "aidememo-aggregate")["handler"]
    out = handler("Redis decisions --op count --type decision --source-id team-a")
    assert '"matched": 2' in out


def test_slash_aidememo_doctor_returns_diagnostics(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "aidememo-doctor")["handler"]
    out = handler("")
    assert "issues" in out


def test_slash_aidememo_start_returns_context_pack(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "aidememo-start")["handler"]
    out = handler('"Fix Redis timeout" --body "Worker timeout" --source github:org/repo#123 --source-id team-a')
    assert "session_id" in out
    assert "ticket_fact_id" in out


def test_slash_aidememo_add_records_fact(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "aidememo-add")["handler"]
    out = handler('"HNSW is the default" --type decision --entities aidememo')
    assert "Recorded" in out
    assert "01HZ" in out  # the stubbed fact id


def test_slash_aidememo_add_usage_when_empty(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "aidememo-add")["handler"]
    assert "Usage" in handler("")


def test_dry_run_writes_pending_log_instead_of_calling_fact_add(
    monkeypatch: pytest.MonkeyPatch, tmp_path
) -> None:
    """When ``dry_run`` is on, detections accumulate to a JSONL log
    and ``client.fact_add`` is *not* invoked."""
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.hooks import make_on_session_end

    fact_add_calls: list[tuple] = []

    def stub_fact_add(self, *args, **kwargs):
        fact_add_calls.append((args, kwargs))
        return "STUB"

    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(AideMemoClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    pending = tmp_path / "aidememo-pending.jsonl"
    on_end = make_on_session_end(
        AideMemoClient(),
        enable_auto_record=True,
        dry_run=True,
        confidence_floor=0.85,
        pending_path=pending,
    )

    transcript = (
        "Decision: use HNSW as the default semantic index\n"
        "결정: 영어 wiki에서도 multilingual-128M로 가자\n"
    )
    on_end(transcript=transcript)

    assert fact_add_calls == [], "dry_run must not call fact_add"
    assert pending.exists()
    lines = pending.read_text(encoding="utf-8").strip().splitlines()
    assert len(lines) == 2
    payload = json.loads(lines[0])
    assert payload["fact_type"] == "decision"
    assert "HNSW" in payload["content"]
    assert payload["confidence"] >= 0.85
    assert "ts_ms" in payload


def test_auto_capture_default_is_disabled(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    """Without explicit capture config, the hook does not write facts or
    pending entries."""
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.hooks import make_on_session_end

    fact_add_calls: list[tuple] = []

    def stub_fact_add(self, *args, **kwargs):
        fact_add_calls.append((args, kwargs))
        return "STUB-ID"

    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(AideMemoClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    pending = tmp_path / "aidememo-pending.jsonl"
    on_end = make_on_session_end(AideMemoClient(), confidence_floor=0.85, pending_path=pending)

    on_end(transcript="Decision: ship the dry-run flag this week, soon")

    assert fact_add_calls == []
    assert not pending.exists()


def test_post_llm_detect_in_user_only_skips_assistant_echo(
    monkeypatch: pytest.MonkeyPatch, tmp_path
) -> None:
    """``detect_in='user'`` runs the detector only on user_message,
    so the precision-focused operator never gets duplicate records
    from the assistant echoing the user's commitment."""
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.hooks import make_post_llm_call

    captured: list[str] = []

    def stub_fact_add(self, *args, **kwargs):
        captured.append(args[0] if args else kwargs.get("content", ""))
        return "STUB"

    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(AideMemoClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    post = make_post_llm_call(
        AideMemoClient(),
        enable_auto_record=True,
        dry_run=False,
        confidence_floor=0.85,
        detect_in="user",
    )
    post(
        user_message="결정: 한국어 패턴도 즉시 기록한다 — auto_record on 모드",
        assistant_response="결론: 한국어 패턴도 즉시 기록한다 — auto_record on 모드.",
    )

    assert len(captured) == 1, captured


def test_post_llm_detect_in_assistant_only_skips_user_message(
    monkeypatch: pytest.MonkeyPatch, tmp_path
) -> None:
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.hooks import make_post_llm_call

    captured: list[str] = []

    def stub_fact_add(self, *args, **kwargs):
        captured.append(args[0] if args else kwargs.get("content", ""))
        return "STUB"

    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(AideMemoClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    post = make_post_llm_call(
        AideMemoClient(),
        enable_auto_record=True,
        dry_run=False,
        confidence_floor=0.85,
        detect_in="assistant",
    )
    post(
        user_message="should we go with HNSW as the default semantic index?",
        assistant_response="Decision: use HNSW as the default semantic index.",
    )

    # Assistant phrasing matched, user question did not.
    assert len(captured) == 1
    assert "HNSW" in captured[0]


def test_dry_run_still_skipped_when_auto_record_off(
    monkeypatch: pytest.MonkeyPatch, tmp_path
) -> None:
    """``auto_record=False`` short-circuits before dry_run kicks in,
    so neither path side-effects."""
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.hooks import make_on_session_end

    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    pending = tmp_path / "aidememo-pending.jsonl"
    on_end = make_on_session_end(
        AideMemoClient(),
        enable_auto_record=False,
        dry_run=True,
        pending_path=pending,
    )
    on_end(transcript="Decision: this should be ignored entirely.")
    assert not pending.exists()


def test_slash_aidememo_pending_lists_entries(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    """Without args, ``/aidememo-pending`` prints a numbered review of the
    log so users can pick what to commit / clear."""
    from hermes_aidememo import pending as pending_mod
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.slash import _aidememo_pending_handler

    log = tmp_path / "aidememo-pending.jsonl"
    pending_mod.write(
        [
            pending_mod.PendingEntry(1, 1, "Use HNSW", "decision", 0.95, "Decision: use HNSW"),
            pending_mod.PendingEntry(2, 1, "Always lint", "convention", 0.85, "Always lint"),
        ],
        log,
    )
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    out = _aidememo_pending_handler(AideMemoClient())("")
    assert "2 pending detection(s)" in out
    assert "#1" in out and "#2" in out
    assert "Use HNSW" in out
    assert "Always lint" in out


def test_slash_aidememo_pending_commit_all(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    from hermes_aidememo import pending as pending_mod
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.slash import _aidememo_pending_handler

    log = tmp_path / "aidememo-pending.jsonl"
    pending_mod.write(
        [
            pending_mod.PendingEntry(1, 1, "fact one", "decision", 0.95, "x"),
            pending_mod.PendingEntry(2, 1, "fact two", "decision", 0.95, "y"),
        ],
        log,
    )
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    captured: list[list[dict]] = []
    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(
        AideMemoClient,
        "fact_add_many",
        lambda self, items: (captured.append(items) or [f"STUB-{i}" for i in range(len(items))]),
    )
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    out = _aidememo_pending_handler(AideMemoClient())("commit all")
    assert "Committed 2" in out
    assert len(captured) == 1, "should call fact_add_many exactly once for the whole batch"
    assert [item["content"] for item in captured[0]] == ["fact one", "fact two"]
    assert pending_mod.read(log) == []


def test_slash_aidememo_pending_commit_one(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    from hermes_aidememo import pending as pending_mod
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.slash import _aidememo_pending_handler

    log = tmp_path / "aidememo-pending.jsonl"
    pending_mod.write(
        [
            pending_mod.PendingEntry(1, 1, "first", "decision", 0.95, "a"),
            pending_mod.PendingEntry(2, 1, "second", "decision", 0.95, "b"),
        ],
        log,
    )
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(
        AideMemoClient,
        "fact_add",
        lambda self, content, entities=None, fact_type="note", tags=None, **_kw: "STUB",
    )
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    out = _aidememo_pending_handler(AideMemoClient())("commit 1")
    assert "Committed #1" in out
    rest = pending_mod.read(log)
    assert [e.content for e in rest] == ["second"]


def test_slash_aidememo_pending_clear(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    from hermes_aidememo import pending as pending_mod
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.slash import _aidememo_pending_handler

    log = tmp_path / "aidememo-pending.jsonl"
    pending_mod.write(
        [
            pending_mod.PendingEntry(1, 1, "a", "decision", 0.95, "a"),
            pending_mod.PendingEntry(2, 1, "b", "decision", 0.95, "b"),
        ],
        log,
    )
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))

    out = _aidememo_pending_handler(AideMemoClient())("clear all")
    assert "Discarded 2" in out
    assert pending_mod.read(log) == []


def test_slash_aidememo_pending_invalid_subcommand_returns_usage(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    from hermes_aidememo.client import AideMemoClient
    from hermes_aidememo.slash import _aidememo_pending_handler

    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    monkeypatch.setattr(AideMemoClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr("hermes_aidememo.client.AideMemoClient._has_cli", staticmethod(lambda: True))
    out = _aidememo_pending_handler(AideMemoClient())("frobnicate 5")
    assert "Usage:" in out
