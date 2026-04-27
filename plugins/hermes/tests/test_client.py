"""WgClient tests — focus on the subprocess fallback path since the
PyO3 binding is exercised by the wg-python smoke tests upstream."""

from __future__ import annotations

import json
from unittest.mock import patch

import pytest

from hermes_wg.client import WgClient, WgUnavailable


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
