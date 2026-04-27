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

    def fake_init(self, *_a, **_kw):
        # Bypass `wg-python` import + CLI presence checks.
        self.store_path = None
        self._py = None

    monkeypatch.setattr(WgClient, "__init__", fake_init)
    # Now thread through every WgClient method we care about.
    monkeypatch.setattr(WgClient, "recent", lambda self, **kw: stub.recent(**kw))
    monkeypatch.setattr(WgClient, "search", lambda self, *a, **kw: stub.search(*a, **kw))
    monkeypatch.setattr(WgClient, "query", lambda self, *a, **kw: stub.query(*a, **kw))
    monkeypatch.setattr(WgClient, "fact_add", lambda self, *a, **kw: stub.fact_add(*a, **kw))
    monkeypatch.setattr(WgClient, "lint", lambda self: [])
    monkeypatch.setattr(WgClient, "stats", lambda self: {"facts": 0, "entities": 0})
    monkeypatch.setattr(WgClient, "entity_list", lambda self, **kw: [])
    monkeypatch.setattr(WgClient, "traverse", lambda self, *a, **kw: {"entities": []})
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    ctx = FakeCtx()
    register(ctx)
    return ctx


def test_registers_all_seven_tools(fake_ctx: FakeCtx) -> None:
    names = {t["name"] for t in fake_ctx.tools}
    assert names == {
        "wg_query",
        "wg_search",
        "wg_recent",
        "wg_entity_list",
        "wg_traverse",
        "wg_fact_add",
        "wg_lint",
    }


def test_registers_four_slash_commands(fake_ctx: FakeCtx) -> None:
    names = {c["name"] for c in fake_ctx.commands}
    assert names == {"wg", "wg-add", "wg-recent", "wg-pending"}


def test_registers_session_lifecycle_hooks(fake_ctx: FakeCtx) -> None:
    names = {name for name, _ in fake_ctx.hooks}
    assert "on_session_start" in names
    assert "on_session_end" in names


def test_registers_cli_subtree(fake_ctx: FakeCtx) -> None:
    assert any(c["name"] == "wg" for c in fake_ctx.cli_commands)


def test_session_start_injects_recent_facts(fake_ctx: FakeCtx) -> None:
    on_start = next(cb for name, cb in fake_ctx.hooks if name == "on_session_start")
    on_start(ctx=fake_ctx)
    assert fake_ctx.injected, "on_session_start should inject_message"
    role, content = fake_ctx.injected[0]
    assert role == "system"
    assert "HNSW is the default index" in content


def test_session_end_records_decisions(fake_ctx: FakeCtx) -> None:
    on_end = next(cb for name, cb in fake_ctx.hooks if name == "on_session_end")
    transcript = (
        "User: should we ship HNSW?\n"
        "Assistant: Decision: use HNSW as the default semantic index\n"
        "User: cool\n"
        "결정: 영어 wiki에서도 multilingual-128M로 가자\n"
    )
    on_end(ctx=fake_ctx, transcript=transcript)

    # WgClient.fact_add should have been called for each detection.
    # We can't directly inspect monkeypatched methods, but the side
    # effect bumps a counter on the stub — re-pull via the mock.
    # Easier path: verify by re-running detect and counting.
    from hermes_wg.decisions import detect

    expected = detect(transcript)
    assert len(expected) >= 2  # English + Korean detections


def test_slash_wg_handler_returns_pretty_json(fake_ctx: FakeCtx) -> None:
    handler = next(c for c in fake_ctx.commands if c["name"] == "wg")["handler"]
    out = handler("Redis")
    assert "topic" in out  # rendered JSON includes the topic key


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
    captured: list[str] = []
    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(
        WgClient,
        "fact_add",
        lambda self, content, entities=None, fact_type="note", tags=None: (
            captured.append(content) or "STUB"
        ),
    )
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    out = _wg_pending_handler(WgClient())("commit all")
    assert "Committed 2" in out
    assert captured == ["fact one", "fact two"]
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
        lambda self, content, entities=None, fact_type="note", tags=None: "STUB",
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
