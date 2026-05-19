"""Thin wrapper that lets the rest of the plugin talk to wg without
caring whether the in-process binding (``wg-python``) or the
subprocess CLI is available.

Order of preference:
1. ``wg-python`` PyO3 binding — ~100× faster (no JSON encode, no
   process spawn). Used when ``import wg_python`` succeeds.
2. ``wg`` CLI — universal fallback, requires the binary on PATH.

Both backends return the same shapes (lists of dicts / dicts), so
upstream code never branches on which is in use.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any


class WgUnavailable(RuntimeError):
    """Neither ``wg-python`` nor the ``wg`` CLI is reachable."""


# Exception tuples used across the plugin. Centralising them here
# keeps catch sites narrow without making readers chase imports.
#
# CLIENT_ERRORS — anything the WgClient calls can raise: our own
# WgUnavailable for subprocess / binding failures, OSError for file-
# system or signal issues, JSONDecodeError when an old wg binary
# returns prose to a `--json` request, and RuntimeError as the umbrella
# the PyO3 binding raises for backend-side problems.
CLIENT_ERRORS: tuple[type[BaseException], ...] = (
    WgUnavailable,
    OSError,
    json.JSONDecodeError,
    RuntimeError,
)

# HERMES_API_ERRORS — what we expect the Hermes plugin host to throw
# when our calls don't fit. AttributeError covers a method moving or
# disappearing across Hermes versions; TypeError covers signature
# drift; ValueError / FileNotFoundError are the documented failure
# modes of `ctx.register_skill`.
HERMES_API_ERRORS: tuple[type[BaseException], ...] = (
    AttributeError,
    TypeError,
    ValueError,
    FileNotFoundError,
)


class WgClient:
    """Bidirectional adapter for wg.

    Pass an explicit ``store_path`` to pin the redb store; otherwise
    we trust the default resolution wg performs (``~/.wg/wiki.redb``
    or whatever ``wg config`` says).
    """

    def __init__(
        self,
        store_path: str | os.PathLike | None = None,
        lock_retry_ms: int | None = None,
    ) -> None:
        self.store_path = str(store_path) if store_path else None
        self.lock_retry_ms = 5000 if lock_retry_ms is None else max(0, int(lock_retry_ms))
        self._py = self._try_load_pyo3()
        if self._py is None and not self._has_cli():
            raise WgUnavailable(
                "wg-python is not installed and the `wg` CLI is not on PATH; "
                "install one of them: `pip install wg-python` or `cargo install wg-cli`."
            )

    # ------------------------------------------------------------------
    # Backend selection
    # ------------------------------------------------------------------

    def _try_load_pyo3(self) -> Any | None:
        try:
            import wg_python  # type: ignore[import-untyped]
        except ImportError:
            return None
        if self.store_path is None:
            # The PyO3 binding requires an explicit store path. Without
            # one, fall through to the CLI — which honors `wg config`.
            return None
        return wg_python.WikiGraph(self.store_path)

    @staticmethod
    def _has_cli() -> bool:
        return shutil.which("wg") is not None

    @property
    def backend(self) -> str:
        return "wg-python" if self._py is not None else "cli"

    # ------------------------------------------------------------------
    # Read API — used by tools, slash commands, hooks
    # ------------------------------------------------------------------

    def query(
        self,
        topic: str,
        limit: int = 5,
        depth: int = 2,
        recent_limit: int = 5,
        source_id: str | None = None,
    ) -> dict:
        if self._py is not None and source_id is None:
            return self._py.query(topic, limit=limit, depth=depth, recent_limit=recent_limit)
        args = ["query", topic, "--limit", str(limit), "-d", str(depth), "--recent-limit", str(recent_limit)]
        if source_id:
            args += ["--source-id", source_id]
        return self._cli_json(args)

    def search(self, query: str, limit: int = 10, source_id: str | None = None) -> list[dict]:
        if self._py is not None and source_id is None:
            return self._py.search(query, limit=limit)
        args = ["search", query, "--limit", str(limit)]
        if source_id:
            args += ["--source-id", source_id]
        return self._cli_json(args)

    def recent(self, last: str = "7d", limit: int = 10) -> list[dict]:
        # CLI: `wg recent --last 7d --limit N`. The PyO3 binding takes
        # an explicit `since_epoch_ms`, which we derive from the same
        # ``Nd / Nh / Nw / Ny`` mini-grammar wg's CLI uses so the two
        # paths agree to the second.
        if self._py is not None:
            since_ms = _now_ms() - parse_window_ms(last)
            return self._py.fact_list(limit=limit, since_epoch_ms=since_ms)
        return self._cli_json(["recent", "--last", last, "-n", str(limit)])

    def entity_list(self, limit: int = 50) -> list[dict]:
        if self._py is not None:
            return self._py.entity_list(limit=limit)
        return self._cli_json(["entity", "list", "--limit", str(limit)])

    def traverse(self, entity: str, depth: int = 2) -> dict:
        if self._py is not None:
            return self._py.traverse(entity, depth=depth, direction="both")
        return self._cli_json(["traverse", entity, "-d", str(depth)])

    def lint(self) -> list[dict]:
        if self._py is not None:
            return self._py.lint()
        return self._cli_json(["lint"])

    def stats(self) -> dict:
        if self._py is not None:
            return self._py.stats()
        return self._cli_json(["stats"])

    # ------------------------------------------------------------------
    # Write API
    # ------------------------------------------------------------------

    def fact_add(
        self,
        content: str,
        entities: list[str] | None = None,
        fact_type: str = "note",
        tags: list[str] | None = None,
        confidence: float | None = None,
        source_id: str | None = None,
    ) -> str:
        if self._py is not None and source_id is None:
            entity_ids = [self._py.resolve_entity(e) for e in (entities or [])]
            kwargs: dict = {
                "entity_ids": entity_ids,
                "fact_type": fact_type,
                "tags": tags or [],
            }
            if confidence is not None:
                kwargs["confidence"] = confidence
            return self._py.fact_add(content, **kwargs)
        args = ["fact", "add", content, "--type", fact_type]
        if entities:
            args += ["--entities", ",".join(entities)]
        if tags:
            # `wg fact add` takes a single `--tags A,B,C` flag, not
            # repeated `--tag` entries — the latter raises
            # `Error: no such flag: --tag`. Comma-join the list to
            # match the CLI's actual surface.
            args += ["--tags", ",".join(tags)]
        if confidence is not None:
            args += ["--confidence", f"{confidence:.3f}"]
        if source_id:
            args += ["--source-id", source_id]
        # Prefer the structured `--json` output (`{"id": "<ULID>",
        # "auto_created_entities": [...]}`) over scraping the human
        # message; falls back to ULID-grep on older wg binaries that
        # haven't shipped the JSON path yet.
        try:
            payload = self._cli_json(args)
        except WgUnavailable:
            return self._fact_add_legacy(args)
        if isinstance(payload, dict) and isinstance(payload.get("id"), str):
            return payload["id"]
        return self._fact_add_legacy(args)

    def fact_add_many(self, items: list[dict]) -> list[str]:
        """Insert N facts in one transaction.

        Each item is a dict with the same shape ``fact_add`` accepts:
        ``content`` (required), ``entities``, ``fact_type``, ``tags``,
        ``confidence``. Entity *names* are resolved to IDs before the
        call so callers don't need to know about ULIDs.

        On the PyO3 path the batch lands in a single redb write
        transaction (one fsync, ~70× faster per fact than sequential
        ``fact_add`` at typical batch sizes). On the CLI fallback there's
        no equivalent subcommand, so we loop ``fact_add`` to preserve
        correctness — operators who care about the speedup should
        install the ``wg_python`` wheel.
        """
        if self._py is not None and all(not item.get("source_id") for item in items):
            py_items = []
            for item in items:
                names = item.get("entities") or []
                entity_ids = [self._py.resolve_entity(n) for n in names]
                py_items.append({
                    "content": item["content"],
                    "entity_ids": entity_ids,
                    "fact_type": item.get("fact_type", "note"),
                    "tags": item.get("tags") or [],
                    "confidence": item.get("confidence"),
                })
            return list(self._py.fact_add_many(py_items))
        # CLI fallback — no `wg fact add-many` command yet, so loop.
        return [
            self.fact_add(
                content=item["content"],
                entities=item.get("entities"),
                fact_type=item.get("fact_type", "note"),
                tags=item.get("tags"),
                confidence=item.get("confidence"),
                source_id=item.get("source_id"),
            )
            for item in items
        ]

    def _fact_add_legacy(self, args: list[str]) -> str:
        """Legacy fallback for `wg` binaries that pre-date the
        structured JSON output on `fact add`. Walks every line of
        the human message looking for a 26-char ULID."""
        out = self._cli(args)
        for line in out.splitlines():
            for token in line.split():
                token = token.strip(".,:;")
                if _ULID_RE.match(token):
                    return token
        return out.strip()

    # ------------------------------------------------------------------
    # CLI plumbing
    # ------------------------------------------------------------------

    def _cli(self, args: list[str]) -> str:
        cmd = ["wg"]
        if self.store_path:
            cmd += ["--store", self.store_path]
        cmd += args
        deadline = time.monotonic() + (self.lock_retry_ms / 1000.0)
        attempts = 0
        last = None
        while True:
            attempts += 1
            completed = subprocess.run(cmd, capture_output=True, text=True, check=False)
            if completed.returncode == 0:
                return completed.stdout
            last = completed
            stderr = completed.stderr.strip()
            if not _is_lock_error(stderr) or self.lock_retry_ms == 0 or time.monotonic() >= deadline:
                break
            time.sleep(0.1)

        assert last is not None
        retry_note = f" after {attempts} attempt(s)" if attempts > 1 else ""
        raise WgUnavailable(
            f"`{' '.join(cmd)}` exited {last.returncode}{retry_note}: "
            f"{last.stderr.strip() or '<no stderr>'}"
        )

    def _cli_json(self, args: list[str]) -> Any:
        out = self._cli(["--json", *args])
        out = out.strip()
        if not out:
            return [] if args[0] in {"search", "recent", "lint", "entity"} else {}
        try:
            return json.loads(out)
        except json.JSONDecodeError as exc:
            raise WgUnavailable(
                f"non-JSON output from `wg {' '.join(args)}`: {out[:200]!r}"
            ) from exc


def default_skills_path() -> Path:
    """Where the bundled SKILL.md lives inside the installed wheel."""
    return Path(__file__).parent / "skills" / "wg"


# Crockford's ULID alphabet: 26 chars, [0-9A-HJKMNP-TV-Z] (no I, L, O,
# U). We don't enforce the full alphabet — `[0-9A-Z]{26}` is a tighter
# match than the previous "isalnum + isupper" walk and good enough to
# tell ULIDs apart from ordinary words in `wg fact add` prose output.
_ULID_RE = re.compile(r"^[0-9A-Z]{26}$")


# Window grammar (`30d`, `12h`, `4w`, `1y`, `90m`, `60s`). Mirrors
# `wg_core::time::parse_duration_to_ms` so the PyO3 backend agrees
# with what `wg recent --last <window>` would compute.
_WINDOW_RE = re.compile(r"^\s*(\d+)\s*([smhdwy])\s*$", re.IGNORECASE)
_WINDOW_UNITS = {
    "s": 1_000,
    "m": 60 * 1_000,
    "h": 60 * 60 * 1_000,
    "d": 24 * 60 * 60 * 1_000,
    "w": 7 * 24 * 60 * 60 * 1_000,
    "y": 365 * 24 * 60 * 60 * 1_000,
}


def _is_lock_error(stderr: str) -> bool:
    lowered = stderr.lower()
    return "cannot acquire lock" in lowered or "database already open" in lowered


def parse_window_ms(window: str) -> int:
    """Convert a window string like ``"7d"`` or ``"12h"`` to
    milliseconds. Raises :class:`ValueError` on unparseable input
    so callers can decide whether to surface or default."""
    m = _WINDOW_RE.match(window or "")
    if not m:
        raise ValueError(
            f"unparseable window {window!r}; expected forms like 30d, 12h, 4w, 1y, 90m, 60s"
        )
    qty, unit = int(m.group(1)), m.group(2).lower()
    return qty * _WINDOW_UNITS[unit]


def _now_ms() -> int:
    return int(time.time() * 1000)
