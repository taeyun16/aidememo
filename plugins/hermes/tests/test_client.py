"""WgClient tests — focus on the subprocess fallback path since the
PyO3 binding is exercised by the wg-python smoke tests upstream."""

from __future__ import annotations

import json
from unittest.mock import patch

import pytest

from hermes_wg.client import WgClient, WgUnavailable, parse_window_ms


def test_raises_when_neither_backend_available(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: False))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    with pytest.raises(WgUnavailable):
        WgClient()


def test_cli_fallback_dispatch(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient()
    assert client.backend == "cli"

    fake_payload = json.dumps([{"content": "x", "fact_type": "note"}])
    completed = type(
        "P", (), {"returncode": 0, "stdout": fake_payload, "stderr": ""}
    )()
    with patch("subprocess.run", return_value=completed) as run:
        out = client.recent(last="14d", limit=3)
        assert out and out[0]["content"] == "x"
        cmd = run.call_args.args[0]
        assert "--json" in cmd
        assert "recent" in cmd
        assert "14d" in cmd


def test_mcp_only_context_uses_stdio_tool_call(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient(source_id="team-a")

    stdout = "\n".join([
        json.dumps({"jsonrpc": "2.0", "id": 0, "result": {}}),
        json.dumps({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [{"type": "text", "text": json.dumps({"recent": [], "source_id": "team-a"})}]
            },
        }),
    ])
    completed = type("P", (), {"returncode": 0, "stdout": stdout, "stderr": ""})()
    with patch("subprocess.run", return_value=completed) as run:
        out = client.context(topic="Redis", source_id=None)

    assert out["source_id"] == "team-a"
    cmd = run.call_args.args[0]
    assert cmd == ["wg", "mcp"]
    payload = run.call_args.kwargs["input"]
    assert '"name": "wg_context"' in payload
    assert '"source_id": "team-a"' in payload


def test_aggregate_forwards_source_id_to_mcp(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient()

    stdout = "\n".join([
        json.dumps({"jsonrpc": "2.0", "id": 0, "result": {}}),
        json.dumps({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [{"type": "text", "text": json.dumps({"op": "count", "matched": 1})}]
            },
        }),
    ])
    completed = type("P", (), {"returncode": 0, "stdout": stdout, "stderr": ""})()
    with patch("subprocess.run", return_value=completed) as run:
        out = client.aggregate("Redis", source_id="team-a")

    assert out["matched"] == 1
    assert '"name": "wg_aggregate"' in run.call_args.kwargs["input"]
    assert '"source_id": "team-a"' in run.call_args.kwargs["input"]


def test_cli_fallback_propagates_failure(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient()

    completed = type(
        "P", (), {"returncode": 1, "stdout": "", "stderr": "boom"}
    )()
    with patch("subprocess.run", return_value=completed):
        with pytest.raises(WgUnavailable, match="exited 1"):
            client.recent()


def test_cli_fallback_retries_short_lock_contention(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient(lock_retry_ms=500)

    locked = type(
        "P",
        (),
        {
            "returncode": 1,
            "stdout": "",
            "stderr": "Database already open. Cannot acquire lock.",
        },
    )()
    ok = type("P", (), {"returncode": 0, "stdout": "[]", "stderr": ""})()
    with patch("time.sleep") as sleep, patch("subprocess.run", side_effect=[locked, ok]) as run:
        assert client.recent() == []
        assert run.call_count == 2
        sleep.assert_called_once()


def test_cli_fallback_lock_retry_zero_fails_fast(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient(lock_retry_ms=0)

    locked = type(
        "P",
        (),
        {
            "returncode": 1,
            "stdout": "",
            "stderr": "Database already open. Cannot acquire lock.",
        },
    )()
    with patch("subprocess.run", return_value=locked) as run:
        with pytest.raises(WgUnavailable, match="Cannot acquire lock"):
            client.recent()
        assert run.call_count == 1


def test_window_grammar_units():
    assert parse_window_ms("60s") == 60 * 1000
    assert parse_window_ms("90m") == 90 * 60 * 1000
    assert parse_window_ms("12h") == 12 * 3600 * 1000
    assert parse_window_ms("7d") == 7 * 86_400 * 1000
    assert parse_window_ms("4w") == 4 * 7 * 86_400 * 1000
    assert parse_window_ms("1y") == 365 * 86_400 * 1000


def test_window_grammar_rejects_garbage():
    with pytest.raises(ValueError):
        parse_window_ms("forever")
    with pytest.raises(ValueError):
        parse_window_ms("7days")
    with pytest.raises(ValueError):
        parse_window_ms("")


def test_recent_uses_pyo3_without_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakePy:
        def fact_list(self, **kwargs):
            assert kwargs["limit"] == 3
            assert kwargs["since_epoch_ms"] > 0
            return [{"content": "recent"}]

    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: FakePy())
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: False))
    client = WgClient(store_path="/tmp/wiki.redb")

    assert client.recent(last="14d", limit=3) == [{"content": "recent"}]


def test_fact_add_many_with_source_id_uses_pyo3(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakePy:
        def resolve_entity(self, name):
            return f"id-{name}"

        def fact_add_many(self, items):
            assert items[0]["source_id"] == "agent-a"
            assert items[1]["source_id"] == "agent-b"
            assert items[0]["entity_ids"] == ["id-Redis"]
            return ["fact-1", "fact-2"]

    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: FakePy())
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    client = WgClient(store_path="/tmp/wiki.redb")

    ids = client.fact_add_many([
        {"content": "alpha", "entities": ["Redis"], "source_id": "agent-a"},
        {"content": "beta", "entities": ["Redis"], "source_id": "agent-b"},
    ])

    assert ids == ["fact-1", "fact-2"]


def test_fact_add_many_cli_uses_mcp_batch_with_session_id(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient()

    stdout = "\n".join([
        json.dumps({"jsonrpc": "2.0", "id": 0, "result": {}}),
        json.dumps({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [{
                    "type": "text",
                    "text": json.dumps({"count": 1, "facts": [{"id": "fact-1"}]}),
                }]
            },
        }),
    ])
    completed = type("P", (), {"returncode": 0, "stdout": stdout, "stderr": ""})()
    with patch("subprocess.run", return_value=completed) as run:
        ids = client.fact_add_many([
            {
                "content": "Redis decision",
                "entities": ["Redis"],
                "source_id": "team-a",
                "session_id": "session-1",
            }
        ])

    assert ids == ["fact-1"]
    payload = run.call_args.kwargs["input"]
    assert '"name": "wg_fact_add_many"' in payload
    assert '"source_id": "team-a"' in payload
    assert '"session_id": "session-1"' in payload


def test_fact_add_prefers_json(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient()
    payload = json.dumps({"id": "01HZ-TEST-FACT-ID-12345678", "auto_created_entities": ["foo"]})
    completed = type("P", (), {"returncode": 0, "stdout": payload, "stderr": ""})()
    with patch("subprocess.run", return_value=completed) as run:
        fid = client.fact_add("hello", entities=["foo"], fact_type="note")
        assert fid == "01HZ-TEST-FACT-ID-12345678"
        # First call goes through `--json` so we trust the structured path.
        cmd = run.call_args.args[0]
        assert "--json" in cmd
        assert "fact" in cmd and "add" in cmd


def test_source_id_is_forwarded_to_cli_filters(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient()

    payloads = [
        json.dumps({"topic": "Redis", "search": []}),
        json.dumps([]),
        json.dumps({"id": "01KQ6RT4RXQFF14MBYTB40M4N3"}),
    ]
    completed = [
        type("P", (), {"returncode": 0, "stdout": p, "stderr": ""})()
        for p in payloads
    ]
    with patch("subprocess.run", side_effect=completed) as run:
        client.query("Redis", source_id="team-a")
        client.search("Redis", source_id="team-a")
        client.fact_add("Redis policy", entities=["Redis"], source_id="team-a")

    calls = [call.args[0] for call in run.call_args_list]
    assert calls[0][-2:] == ["--source-id", "team-a"]
    assert calls[1][-2:] == ["--source-id", "team-a"]
    assert calls[2][-2:] == ["--source-id", "team-a"]


def test_default_source_id_is_used_for_cli_calls(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient(source_id=" team-default ")

    payloads = [
        json.dumps({"topic": "Redis", "search": []}),
        json.dumps([]),
        json.dumps({"id": "01KQ6RT4RXQFF14MBYTB40M4N3"}),
        json.dumps({
            "session_id": "session-01KQ6RT4RXQFF14MBYTB40M4N4",
            "ticket_fact_id": "01KQ6RT4RXQFF14MBYTB40M4N5",
        }),
    ]
    completed = [
        type("P", (), {"returncode": 0, "stdout": p, "stderr": ""})()
        for p in payloads
    ]
    with patch("subprocess.run", side_effect=completed) as run:
        client.query("Redis")
        client.search("Redis")
        client.fact_add("Redis policy", entities=["Redis"])
        client.workflow_start("Fix Redis timeout")

    calls = [call.args[0] for call in run.call_args_list]
    assert calls[0][-2:] == ["--source-id", "team-default"]
    assert calls[1][-2:] == ["--source-id", "team-default"]
    assert calls[2][-2:] == ["--source-id", "team-default"]
    assert calls[3][-2:] == ["--source-id", "team-default"]


def test_workflow_start_dispatches_to_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient()

    payload = json.dumps({
        "session_id": "session-01KQ6RT4RXQFF14MBYTB40M4N3",
        "ticket_fact_id": "01KQ6RT4RXQFF14MBYTB40M4N4",
    })
    completed = type("P", (), {"returncode": 0, "stdout": payload, "stderr": ""})()
    with patch("subprocess.run", return_value=completed) as run:
        out = client.workflow_start(
            "Fix Redis timeout",
            body="Worker jobs timeout",
            source="github:org/repo#123",
            source_id="team-a",
        )

    assert out["session_id"].startswith("session-")
    cmd = run.call_args.args[0]
    assert cmd[:3] == ["wg", "--json", "workflow"]
    assert "start" in cmd
    assert "Fix Redis timeout" in cmd
    assert cmd[-2:] == ["--source-id", "team-a"]


def test_workflow_start_prefers_pyo3(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakePy:
        def entity_add(self, name, **kwargs):
            assert name.startswith("session-")
            assert kwargs["entity_type"] == "session"
            assert kwargs["source_page"] == "github:org/repo#123"
            return "entity-session"

        def fact_add(self, content, **kwargs):
            assert content == "Workflow ticket: Fix Redis timeout\n\nWorker jobs timeout"
            assert kwargs["entity_ids"] == ["entity-session"]
            assert kwargs["fact_type"] == "question"
            assert kwargs["tags"] == ["workflow-start", "ticket"]
            assert kwargs["source"] == "github:org/repo#123"
            assert kwargs["source_id"] == "team-a"
            return "01KQ6RT4RXQFF14MBYTB40M4N4"

        def query(self, topic, **kwargs):
            assert "Worker jobs timeout" in topic
            assert kwargs["source_id"] == "team-a"
            assert kwargs["current_only"] is True
            return {"topic": topic, "search": []}

        def search(self, query, **kwargs):
            assert kwargs["source_id"] == "team-a"
            return [
                {"fact_type": "decision", "content": "Decision: wrapper"},
                {"fact_type": "lesson", "content": "Lesson: DNS"},
                {"fact_type": "error", "content": "Error: pool"},
            ]

    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: FakePy())
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: False))
    client = WgClient(store_path="/tmp/wiki.redb")

    out = client.workflow_start(
        "Fix Redis timeout",
        body="Worker jobs timeout",
        source="github:org/repo#123",
        source_id="team-a",
    )

    assert out["session_id"].startswith("session-")
    assert out["ticket_fact_id"] == "01KQ6RT4RXQFF14MBYTB40M4N4"
    assert len(out["relevant_decisions"]) == 1
    assert len(out["prior_lessons"]) == 1
    assert len(out["prior_errors"]) == 1


def test_default_source_id_is_used_for_pyo3_workflow(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakePy:
        def entity_add(self, name, **kwargs):
            return "entity-session"

        def fact_add(self, content, **kwargs):
            assert kwargs["source_id"] == "team-default"
            return "01KQ6RT4RXQFF14MBYTB40M4N4"

        def query(self, topic, **kwargs):
            assert kwargs["source_id"] == "team-default"
            return {"topic": topic, "search": []}

        def search(self, query, **kwargs):
            assert kwargs["source_id"] == "team-default"
            return []

    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: FakePy())
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: False))
    client = WgClient(store_path="/tmp/wiki.redb", source_id="team-default")

    out = client.workflow_start("Fix Redis timeout")

    assert out["source_id"] == "team-default"


def test_fact_add_falls_back_to_human_output(monkeypatch: pytest.MonkeyPatch) -> None:
    """Older wg binaries silently dropped the global `--json` flag for
    `fact add`, returning the human prose. The legacy path should
    still pluck the ULID from that text."""
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: None)
    client = WgClient()
    legacy = "Added fact with ID 01KQ6RT4RXQFF14MBYTB40M4N3\n  auto-created entity: foo"
    # First subprocess.run is the JSON attempt and returns prose (no
    # JSON parse). Second is the legacy fallback — we feed both with
    # the same bytes so either branch works the same way.
    completed = type("P", (), {"returncode": 0, "stdout": legacy, "stderr": ""})()
    with patch("subprocess.run", return_value=completed):
        fid = client.fact_add("hello", entities=["foo"], fact_type="note")
        assert fid == "01KQ6RT4RXQFF14MBYTB40M4N3"
