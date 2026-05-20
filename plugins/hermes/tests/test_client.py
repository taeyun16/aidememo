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


def test_fact_add_many_with_source_id_falls_back_to_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakePy:
        def fact_add_many(self, items):
            raise AssertionError("source-scoped batches require the CLI until wg-python supports source_id")

    monkeypatch.setattr("hermes_wg.client.WgClient._try_load_pyo3", lambda self: FakePy())
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))
    client = WgClient(store_path="/tmp/wiki.redb")

    calls: list[list[str]] = []

    def fake_cli_json(args: list[str]):
        calls.append(args)
        return {"id": f"fact-{len(calls)}"}

    monkeypatch.setattr(client, "_cli_json", fake_cli_json)

    ids = client.fact_add_many([
        {"content": "alpha", "entities": ["Redis"], "source_id": "agent-a"},
        {"content": "beta", "entities": ["Redis"], "source_id": "agent-b"},
    ])

    assert ids == ["fact-1", "fact-2"]
    assert calls[0][-2:] == ["--source-id", "agent-a"]
    assert calls[1][-2:] == ["--source-id", "agent-b"]


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
