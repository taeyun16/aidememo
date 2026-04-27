"""Shared accessors for the dry-run pending log.

The on_session_end hook (when ``dry_run: true``) appends each
detected fact to a JSONL file. The ``/wg-pending`` slash command
reads that file back so users can audit, commit, or discard the
captures interactively.

The file lives at ``$HERMES_STATE_DIR/wg-pending.jsonl`` (default
``~/.hermes/state/wg-pending.jsonl``) and uses one JSON object per
line so partial writes during a crash leave the rest of the log
parseable.
"""

from __future__ import annotations

import json
import os
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

from .client import CLIENT_ERRORS, WgClient
from .decisions import DetectedFact


def pending_log_path() -> Path:
    """Resolve the pending log path. Precedence:

    1. ``HERMES_STATE_DIR`` — explicit override, most specific.
    2. ``HERMES_HOME/state`` — follow Hermes's own state-dir
       convention so our log lives next to the rest of the agent's
       state and isolated test profiles never bleed into the
       operator's real ``~/.hermes/state``.
    3. ``~/.hermes/state`` — last-resort default for a stock setup.
    """
    env_state = os.environ.get("HERMES_STATE_DIR")
    if env_state:
        return Path(env_state) / "wg-pending.jsonl"
    env_home = os.environ.get("HERMES_HOME")
    if env_home:
        return Path(env_home) / "state" / "wg-pending.jsonl"
    return Path.home() / ".hermes" / "state" / "wg-pending.jsonl"


@dataclass(frozen=True)
class PendingEntry:
    """One row from the pending log. ``idx`` is 1-based to match
    what we surface to the user in slash command output."""

    idx: int
    ts_ms: int
    content: str
    fact_type: str
    confidence: float
    source_line: str

    @classmethod
    def from_dict(cls, idx: int, raw: dict) -> "PendingEntry":
        return cls(
            idx=idx,
            ts_ms=int(raw.get("ts_ms") or 0),
            content=str(raw.get("content") or ""),
            fact_type=str(raw.get("fact_type") or "note"),
            confidence=float(raw.get("confidence") or 0.0),
            source_line=str(raw.get("source_line") or ""),
        )


def append(detections: Iterable[DetectedFact], path: Path | None = None) -> Path:
    """Write each detection as one JSONL line. Creates the parent
    directory on demand. Returns the resolved path."""
    target = path or pending_log_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    now_ms = int(time.time() * 1000)
    with target.open("a", encoding="utf-8") as fh:
        for d in detections:
            fh.write(
                json.dumps(
                    {
                        "ts_ms": now_ms,
                        "content": d.content,
                        "fact_type": d.fact_type,
                        "confidence": d.confidence,
                        "source_line": d.source_line,
                    },
                    ensure_ascii=False,
                )
                + "\n"
            )
    return target


def read(path: Path | None = None) -> list[PendingEntry]:
    """Parse the pending log. Bad lines are skipped (not raised) so
    one corrupt row from a partial write doesn't sink the rest."""
    target = path or pending_log_path()
    if not target.exists():
        return []
    out: list[PendingEntry] = []
    for line_no, raw_line in enumerate(target.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw_line.strip()
        if not line:
            continue
        try:
            doc = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(doc, dict):
            continue
        out.append(PendingEntry.from_dict(len(out) + 1, doc))
        del line_no  # unused but keeps the enumerate hint clear
    return out


def write(entries: Iterable[PendingEntry], path: Path | None = None) -> Path:
    """Replace the log with ``entries``. If the iterable is empty,
    the file is truncated rather than removed — keeps semantic
    parity with ``open(path, "w")`` and lets ``read()`` continue
    returning ``[]`` afterward."""
    target = path or pending_log_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open("w", encoding="utf-8") as fh:
        for e in entries:
            fh.write(
                json.dumps(
                    {
                        "ts_ms": e.ts_ms,
                        "content": e.content,
                        "fact_type": e.fact_type,
                        "confidence": e.confidence,
                        "source_line": e.source_line,
                    },
                    ensure_ascii=False,
                )
                + "\n"
            )
    return target


def commit_one(client: WgClient, idx: int, path: Path | None = None) -> PendingEntry:
    """Commit a single entry to wg and remove it from the log.
    Raises ``IndexError`` if ``idx`` is out of range, and any of
    ``CLIENT_ERRORS`` if ``fact_add`` fails — in which case the log
    is left untouched so the user can retry."""
    entries = read(path)
    target_idx = idx - 1
    if target_idx < 0 or target_idx >= len(entries):
        raise IndexError(f"no pending entry at index {idx}")
    entry = entries[target_idx]
    client.fact_add(
        entry.content,
        entities=None,
        fact_type=entry.fact_type,
        tags=["auto-recorded", "hermes-session"],
        confidence=entry.confidence,
    )
    remaining = [e for i, e in enumerate(entries) if i != target_idx]
    write(_renumber(remaining), path)
    return entry


def commit_all(client: WgClient, path: Path | None = None) -> tuple[int, list[PendingEntry]]:
    """Commit every entry to wg and clear the log on success.

    Uses ``fact_add_many`` so the entire batch lands in a single redb
    transaction — one fsync, much faster than the per-entry loop the
    older implementation paid for. Semantically all-or-nothing: any
    failure (wg unreachable, validation error from one entry) leaves
    *all* entries in the log so the operator can fix and retry. The
    caller's tuple return shape is preserved so existing UI code
    keeps working — committed = N on success, 0 on failure.
    """
    entries = read(path)
    if not entries:
        return 0, []
    items = [
        {
            "content": entry.content,
            "fact_type": entry.fact_type,
            "tags": ["auto-recorded", "hermes-session"],
            "confidence": entry.confidence,
        }
        for entry in entries
    ]
    try:
        client.fact_add_many(items)
    except CLIENT_ERRORS:
        # Whole batch failed — keep every entry intact for retry.
        return 0, list(entries)
    write([], path)
    return len(entries), []


def clear_one(idx: int, path: Path | None = None) -> PendingEntry:
    """Drop a single entry without writing to wg.
    Raises ``IndexError`` if ``idx`` is out of range."""
    entries = read(path)
    target_idx = idx - 1
    if target_idx < 0 or target_idx >= len(entries):
        raise IndexError(f"no pending entry at index {idx}")
    entry = entries[target_idx]
    remaining = [e for i, e in enumerate(entries) if i != target_idx]
    write(_renumber(remaining), path)
    return entry


def clear_all(path: Path | None = None) -> int:
    """Truncate the log. Returns the number of entries that were
    discarded so callers can include the count in user-facing
    messages."""
    entries = read(path)
    write([], path)
    return len(entries)


def _renumber(entries: Iterable[PendingEntry]) -> list[PendingEntry]:
    """Re-stamp ``idx`` so the on-disk view stays 1..N — matches
    what the user sees on the next ``/wg-pending``."""
    return [
        PendingEntry(
            idx=i,
            ts_ms=e.ts_ms,
            content=e.content,
            fact_type=e.fact_type,
            confidence=e.confidence,
            source_line=e.source_line,
        )
        for i, e in enumerate(entries, start=1)
    ]
