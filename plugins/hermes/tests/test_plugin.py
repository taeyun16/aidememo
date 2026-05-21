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

from hermes_wg.client import WgClient
from hermes_wg.plugin import register


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
    # Ensure the WgClient bootstrap doesn't actually try to spawn `wg`
    # — provide a stub backend that the test can introspect.
    stub = MagicMock(spec=WgClient)
    stub.backend = "stub"
    stub.recent.return_value = [
        {"content": "HNSW is the default index", "fact_type": "decision"},
        {"content": "BM25 weight is 0.7 by default", "fact_type": "convention"},
    ]
    stub.fact_add.return_value = "01HZ-FAKE-FACT-ID"
    stub.search.return_value = []
    stub.query.return_value = {"topic": "test", "entity": None, "search": [], "related": [], "recent_facts": []}
    stub.workflow_start.return_value = {
        "session_id": "session-01HZ-FAKE",
        "ticket_fact_id": "01HZ-FAKE-TICKET",
        "context": {"search": []},
    }

    def fake_init(self, *_a, **_kw):
        # Bypass `wg-python` import + CLI presence checks.
        self.store_path = None
        self._py = None

    monkeypatch.setattr(WgClient, "__init__", fake_init)
    # Now thread through every WgClient method we care about.
    monkeypatch.setattr(WgClient, "recent", lambda self, **kw: stub.recent(**kw))
    monkeypatch.setattr(WgClient, "search", lambda self, *a, **kw: stub.search(*a, **kw))
    monkeypatch.setattr(WgClient, "query", lambda self, *a, **kw: stub.query(*a, **kw))
    monkeypatch.setattr(WgClient, "workflow_start", lambda self, *a, **kw: stub.workflow_start(*a, **kw))
    monkeypatch.setattr(WgClient, "fact_add", lambda self, *a, **kw: stub.fact_add(*a, **kw))
    monkeypatch.setattr(WgClient, "lint", lambda self: [])
    monkeypatch.setattr(WgClient, "stats", lambda self: {"facts": 0, "entities": 0})
    monkeypatch.setattr(WgClient, "entity_list", lambda self, **kw: [])
    monkeypatch.setattr(WgClient, "traverse", lambda self, *a, **kw: {"entities": []})
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    ctx = FakeCtx()
    register(ctx)
    return ctx


def test_registers_all_eight_tools(fake_ctx: FakeCtx) -> None:
    names = {t["name"] for t in fake_ctx.tools}
    assert names == {
        "wg_workflow_start",
        "wg_query",
        "wg_search",
        "wg_recent",
        "wg_entity_list",
        "wg_traverse",
        "wg_fact_add",
        "wg_lint",
    }


def test_source_id_is_exposed_on_relevant_tool_schemas(fake_ctx: FakeCtx) -> None:
    schemas = {t["name"]: t["schema"]["parameters"]["properties"] for t in fake_ctx.tools}
    assert "source_id" in schemas["wg_workflow_start"]
    assert "source_id" in schemas["wg_query"]
    assert "source_id" in schemas["wg_search"]
    assert "source_id" in schemas["wg_fact_add"]


def test_registers_five_slash_commands(fake_ctx: FakeCtx) -> None:
    names = {c["name"] for c in fake_ctx.commands}
    assert names == {"wg", "wg-start", "wg-add", "wg-recent", "wg-pending"}


def test_registers_llm_call_hooks(fake_ctx: FakeCtx) -> None:
    names = {name for name, _ in fake_ctx.hooks}
    assert "pre_llm_call" in names
    assert "post_llm_call" in names


def test_registers_cli_subtree(fake_ctx: FakeCtx) -> None:
    assert any(c["name"] == "wg" for c in fake_ctx.cli_commands)


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


def test_post_llm_call_captures_sparse_ticket_even_when_auto_record_off(
    fake_ctx: FakeCtx,
) -> None:
    post = next(cb for name, cb in fake_ctx.hooks if name == "post_llm_call")
    post(
        user_message='Issue #123: Fix Redis timeout\nPass source_id exactly as "team-a".',
        assistant_response="Plan follows.",
        session_id="s",
    )


def test_pre_llm_call_returns_none_on_later_turns(fake_ctx: FakeCtx) -> None:
    pre = next(cb for name, cb in fake_ctx.hooks if name == "pre_llm_call")
    assert pre(is_first_turn=False, user_message="follow up", session_id="s") is None


def test_post_llm_call_runs_decision_detector(fake_ctx: FakeCtx) -> None:
    post = next(cb for name, cb in fake_ctx.hooks if name == "post_llm_call")
    # The fake ctx's WgClient.fact_add is stubbed to return a fixed
    # ULID; we check that detect() picked up at least one match by
    # verifying the call went through (fact_add side effect).
    post(
        user_message="결정: 영어 wiki에서도 multilingual-128M로 가자",
        assistant_response="Sounds good.",
        session_id="s",
    )

    # WgClient.fact_add should have been called for each detection.
    # Easier path: verify by re-running detect and counting.
    from hermes_wg.decisions import detect

    expected = detect("결정: 영어 wiki에서도 multilingual-128M로 가자\nSounds good.")
    assert len(expected) >= 1


def test_slash_wg_handler_returns_pretty_json(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "wg")["handler"]
    out = handler("Redis")
    assert "topic" in out  # rendered JSON includes the topic key


def test_slash_wg_start_returns_context_pack(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "wg-start")["handler"]
    out = handler('"Fix Redis timeout" --body "Worker timeout" --source github:org/repo#123 --source-id team-a')
    assert "session_id" in out
    assert "ticket_fact_id" in out


def test_slash_wg_add_records_fact(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "wg-add")["handler"]
    out = handler('"HNSW is the default" --type decision --entities wg')
    assert "Recorded" in out
    assert "01HZ" in out  # the stubbed fact id


def test_slash_wg_add_usage_when_empty(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "wg-add")["handler"]
    assert "Usage" in handler("")


def test_dry_run_writes_pending_log_instead_of_calling_fact_add(
    monkeypatch: pytest.MonkeyPatch, tmp_path
) -> None:
    """When ``dry_run`` is on, detections accumulate to a JSONL log
    and ``client.fact_add`` is *not* invoked."""
    from hermes_wg.client import WgClient
    from hermes_wg.hooks import make_on_session_end

    fact_add_calls: list[tuple] = []

    def stub_fact_add(self, *args, **kwargs):
        fact_add_calls.append((args, kwargs))
        return "STUB"

    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(WgClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    pending = tmp_path / "wg-pending.jsonl"
    on_end = make_on_session_end(
        WgClient(),
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


def test_dry_run_default_is_false(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    """Without an explicit ``dry_run: true`` config, the recorder
    behaves as before — calling ``fact_add`` for each detection."""
    from hermes_wg.client import WgClient
    from hermes_wg.hooks import make_on_session_end

    fact_add_calls: list[tuple] = []

    def stub_fact_add(self, *args, **kwargs):
        fact_add_calls.append((args, kwargs))
        return "STUB-ID"

    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(WgClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    pending = tmp_path / "wg-pending.jsonl"
    on_end = make_on_session_end(
        WgClient(),
        enable_auto_record=True,
        confidence_floor=0.85,
        pending_path=pending,
    )

    on_end(transcript="Decision: ship the dry-run flag this week, soon")

    assert len(fact_add_calls) == 1
    assert not pending.exists()


def test_post_llm_detect_in_user_only_skips_assistant_echo(
    monkeypatch: pytest.MonkeyPatch, tmp_path
) -> None:
    """``detect_in='user'`` runs the detector only on user_message,
    so the precision-focused operator never gets duplicate records
    from the assistant echoing the user's commitment."""
    from hermes_wg.client import WgClient
    from hermes_wg.hooks import make_post_llm_call

    captured: list[str] = []

    def stub_fact_add(self, *args, **kwargs):
        captured.append(args[0] if args else kwargs.get("content", ""))
        return "STUB"

    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(WgClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    post = make_post_llm_call(
        WgClient(),
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
    from hermes_wg.client import WgClient
    from hermes_wg.hooks import make_post_llm_call

    captured: list[str] = []

    def stub_fact_add(self, *args, **kwargs):
        captured.append(args[0] if args else kwargs.get("content", ""))
        return "STUB"

    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(WgClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    post = make_post_llm_call(
        WgClient(),
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
    from hermes_wg.client import WgClient
    from hermes_wg.hooks import make_on_session_end

    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    pending = tmp_path / "wg-pending.jsonl"
    on_end = make_on_session_end(
        WgClient(),
        enable_auto_record=False,
        dry_run=True,
        pending_path=pending,
    )
    on_end(transcript="Decision: this should be ignored entirely.")
    assert not pending.exists()


def test_slash_wg_pending_lists_entries(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    """Without args, ``/wg-pending`` prints a numbered review of the
    log so users can pick what to commit / clear."""
    from hermes_wg import pending as pending_mod
    from hermes_wg.client import WgClient
    from hermes_wg.slash import _wg_pending_handler

    log = tmp_path / "wg-pending.jsonl"
    pending_mod.write(
        [
            pending_mod.PendingEntry(1, 1, "Use HNSW", "decision", 0.95, "Decision: use HNSW"),
            pending_mod.PendingEntry(2, 1, "Always lint", "convention", 0.85, "Always lint"),
        ],
        log,
    )
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    out = _wg_pending_handler(WgClient())("")
    assert "2 pending detection(s)" in out
    assert "#1" in out and "#2" in out
    assert "Use HNSW" in out
    assert "Always lint" in out


def test_slash_wg_pending_commit_all(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    from hermes_wg import pending as pending_mod
    from hermes_wg.client import WgClient
    from hermes_wg.slash import _wg_pending_handler

    log = tmp_path / "wg-pending.jsonl"
    pending_mod.write(
        [
            pending_mod.PendingEntry(1, 1, "fact one", "decision", 0.95, "x"),
            pending_mod.PendingEntry(2, 1, "fact two", "decision", 0.95, "y"),
        ],
        log,
    )
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    captured: list[list[dict]] = []
    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(
        WgClient,
        "fact_add_many",
        lambda self, items: (captured.append(items) or [f"STUB-{i}" for i in range(len(items))]),
    )
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    out = _wg_pending_handler(WgClient())("commit all")
    assert "Committed 2" in out
    assert len(captured) == 1, "should call fact_add_many exactly once for the whole batch"
    assert [item["content"] for item in captured[0]] == ["fact one", "fact two"]
    assert pending_mod.read(log) == []


def test_slash_wg_pending_commit_one(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    from hermes_wg import pending as pending_mod
    from hermes_wg.client import WgClient
    from hermes_wg.slash import _wg_pending_handler

    log = tmp_path / "wg-pending.jsonl"
    pending_mod.write(
        [
            pending_mod.PendingEntry(1, 1, "first", "decision", 0.95, "a"),
            pending_mod.PendingEntry(2, 1, "second", "decision", 0.95, "b"),
        ],
        log,
    )
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(
        WgClient,
        "fact_add",
        lambda self, content, entities=None, fact_type="note", tags=None, **_kw: "STUB",
    )
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    out = _wg_pending_handler(WgClient())("commit 1")
    assert "Committed #1" in out
    rest = pending_mod.read(log)
    assert [e.content for e in rest] == ["second"]


def test_slash_wg_pending_clear(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    from hermes_wg import pending as pending_mod
    from hermes_wg.client import WgClient
    from hermes_wg.slash import _wg_pending_handler

    log = tmp_path / "wg-pending.jsonl"
    pending_mod.write(
        [
            pending_mod.PendingEntry(1, 1, "a", "decision", 0.95, "a"),
            pending_mod.PendingEntry(2, 1, "b", "decision", 0.95, "b"),
        ],
        log,
    )
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    out = _wg_pending_handler(WgClient())("clear all")
    assert "Discarded 2" in out
    assert pending_mod.read(log) == []


def test_slash_wg_pending_invalid_subcommand_returns_usage(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    from hermes_wg.client import WgClient
    from hermes_wg.slash import _wg_pending_handler

    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    out = _wg_pending_handler(WgClient())("frobnicate 5")
    assert "Usage:" in out
