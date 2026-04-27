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
import shutil
import subprocess
from pathlib import Path
from typing import Any


class WgUnavailable(RuntimeError):
    """Neither ``wg-python`` nor the ``wg`` CLI is reachable."""


class WgClient:
    """Bidirectional adapter for wg.

    Pass an explicit ``store_path`` to pin the redb store; otherwise
    we trust the default resolution wg performs (``~/.wg/wiki.redb``
    or whatever ``wg config`` says).
    """

    def __init__(self, store_path: str | os.PathLike | None = None) -> None:
        self.store_path = str(store_path) if store_path else None
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

    def query(self, topic: str, limit: int = 5, depth: int = 2, recent_limit: int = 5) -> dict:
        if self._py is not None:
            return self._py.query(topic, limit=limit, depth=depth, recent_limit=recent_limit)
        return self._cli_json(
            ["query", topic, "--limit", str(limit), "-d", str(depth), "--recent-limit", str(recent_limit)]
        )

    def search(self, query: str, limit: int = 10) -> list[dict]:
        if self._py is not None:
            return self._py.search(query, limit=limit)
        return self._cli_json(["search", query, "--limit", str(limit)])

    def recent(self, last: str = "7d", limit: int = 10) -> list[dict]:
        # CLI: `wg recent --last 7d --limit N`. wg-python doesn't expose
        # `recent` directly — fall back to fact_list filtered by epoch.
        if self._py is not None:
            return self._py.fact_list(limit=limit)  # filter not exposed; CLI is more accurate
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
    ) -> str:
        if self._py is not None:
            entity_ids = [self._py.resolve_entity(e) for e in (entities or [])]
            return self._py.fact_add(
                content,
                entity_ids=entity_ids,
                fact_type=fact_type,
                tags=tags or [],
            )
        args = ["fact", "add", content, "--type", fact_type]
        if entities:
            args += ["--entities", ",".join(entities)]
        for tag in tags or []:
            args += ["--tag", tag]
        out = self._cli(args)
        # `wg fact add` prints "Added fact with ID <ULID>" on the first
        # line and may follow with secondary notices like
        # "auto-created entities: foo, bar". Walk every line to find
        # the ULID rather than guessing position.
        for line in out.splitlines():
            for token in line.split():
                token = token.strip(".,:;")
                if len(token) == 26 and token.isalnum() and token.isupper():
                    return token
        # Fallback: return whatever we got — at least the caller can log it.
        return out.strip()

    # ------------------------------------------------------------------
    # CLI plumbing
    # ------------------------------------------------------------------

    def _cli(self, args: list[str]) -> str:
        cmd = ["wg"]
        if self.store_path:
            cmd += ["--store", self.store_path]
        cmd += args
        completed = subprocess.run(cmd, capture_output=True, text=True, check=False)
        if completed.returncode != 0:
            raise WgUnavailable(
                f"`{' '.join(cmd)}` exited {completed.returncode}: "
                f"{completed.stderr.strip() or '<no stderr>'}"
            )
        return completed.stdout

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
