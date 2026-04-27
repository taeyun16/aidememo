"""Pending-log module tests.

The dry-run recorder writes to a JSONL file; ``/wg-pending`` reads,
commits, and clears entries from the same file. These tests lock
the round-trip down: append → read → commit / clear → re-read.
"""

from __future__ import annotations

from pathlib import Path
from unittest.mock import patch

import pytest

from hermes_wg import pending
from hermes_wg.client import WgClient, WgUnavailable
from hermes_wg.decisions import DetectedFact


def _entry(idx: int, content: str, fact_type: str = "decision", confidence: float = 0.9) -> pending.PendingEntry:
    return pending.PendingEntry(
        idx=idx,
        ts_ms=1234,
        content=content,
        fact_type=fact_type,
        confidence=confidence,
        source_line=content,
    )


def test_append_creates_directory_and_writes_jsonl(tmp_path: Path) -> None:
    log = tmp_path / "nested" / "wg-pending.jsonl"
    pending.append(
        [
            DetectedFact(content="Use HNSW", fact_type="decision", confidence=0.95, source_line="Decision: use HNSW"),
            DetectedFact(content="Always lint", fact_type="convention", confidence=0.85, source_line="Always lint"),
        ],
        path=log,
    )
    text = log.read_text(encoding="utf-8").splitlines()
    assert len(text) == 2
    assert "Use HNSW" in text[0]


def test_read_skips_corrupt_lines(tmp_path: Path) -> None:
    log = tmp_path / "wg-pending.jsonl"
    log.write_text(
        '{"content": "ok", "fact_type": "decision", "confidence": 0.9, "ts_ms": 1, "source_line": "ok"}\n'
        '{not valid json\n'
        '\n'
        '{"content": "second", "fact_type": "note", "confidence": 0.7, "ts_ms": 2, "source_line": "second"}\n',
        encoding="utf-8",
    )
    entries = pending.read(log)
    assert [e.content for e in entries] == ["ok", "second"]
    assert [e.idx for e in entries] == [1, 2]


def test_clear_one_renumbers_remaining(tmp_path: Path) -> None:
    log = tmp_path / "wg-pending.jsonl"
    pending.write([_entry(1, "a"), _entry(2, "b"), _entry(3, "c")], log)

    dropped = pending.clear_one(2, log)
    assert dropped.content == "b"

    rest = pending.read(log)
    assert [e.content for e in rest] == ["a", "c"]
    assert [e.idx for e in rest] == [1, 2], "remaining entries should be renumbered 1..N"


def test_clear_all_truncates(tmp_path: Path) -> None:
    log = tmp_path / "wg-pending.jsonl"
    pending.write([_entry(1, "a"), _entry(2, "b")], log)
    n = pending.clear_all(log)
    assert n == 2
    assert pending.read(log) == []


def test_clear_one_out_of_range_raises(tmp_path: Path) -> None:
    log = tmp_path / "wg-pending.jsonl"
    pending.write([_entry(1, "a")], log)
    with pytest.raises(IndexError):
        pending.clear_one(5, log)


def test_commit_one_writes_to_wg_and_drops_entry(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    log = tmp_path / "wg-pending.jsonl"
    pending.write([_entry(1, "a"), _entry(2, "b")], log)

    captured: list[tuple] = []

    def stub_fact_add(self, content, entities=None, fact_type="note", tags=None, **_kw):
        captured.append((content, fact_type, tuple(tags or [])))
        return "STUB-ID"

    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(WgClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    entry = pending.commit_one(WgClient(), 1, log)
    assert entry.content == "a"
    assert captured == [("a", "decision", ("auto-recorded", "hermes-session"))]

    rest = pending.read(log)
    assert [e.content for e in rest] == ["b"]
    assert rest[0].idx == 1


def test_commit_all_keeps_failed_entries(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    log = tmp_path / "wg-pending.jsonl"
    pending.write([_entry(1, "ok"), _entry(2, "boom"), _entry(3, "fine")], log)

    def stub_fact_add(self, content, entities=None, fact_type="note", tags=None, **_kw):
        if content == "boom":
            raise WgUnavailable("simulated failure")
        return "STUB"

    monkeypatch.setattr(WgClient, "__init__", lambda self, *_a, **_kw: None)
    monkeypatch.setattr(WgClient, "fact_add", stub_fact_add)
    monkeypatch.setattr("hermes_wg.client.WgClient._has_cli", staticmethod(lambda: True))

    committed, leftover = pending.commit_all(WgClient(), log)
    assert committed == 2
    assert [e.content for e in leftover] == ["boom"]

    on_disk = pending.read(log)
    assert [e.content for e in on_disk] == ["boom"]
    assert on_disk[0].idx == 1, "leftover should be renumbered"


def test_pending_log_path_honors_state_dir(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("HERMES_STATE_DIR", str(tmp_path))
    assert pending.pending_log_path() == tmp_path / "wg-pending.jsonl"
    monkeypatch.delenv("HERMES_STATE_DIR")
