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
