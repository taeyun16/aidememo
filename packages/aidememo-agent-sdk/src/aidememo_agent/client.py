"""Thin wrapper for agent code that talks to aidememo through one API.

Callers do not need to care whether the in-process binding
(``aidememo-python``) or the subprocess CLI is available.

Order of preference:
1. ``aidememo-python`` PyO3 binding — ~100× faster (no JSON encode, no
   process spawn). Used when ``import aidememo_python`` succeeds.
2. ``aidememo`` CLI — universal fallback, requires the binary on PATH.

Both backends return the same shapes (lists of dicts / dicts), so
upstream code never branches on which is in use.
"""

from __future__ import annotations

import json
import os
import re
import secrets
import shutil
import subprocess
import time
from typing import Any


class AideMemoUnavailable(RuntimeError):
    """Neither ``aidememo-python`` nor the ``aidememo`` CLI is reachable."""


# Exception tuples used across SDK integrations. Centralising them here
# keeps catch sites narrow without making readers chase imports.
#
# CLIENT_ERRORS — anything the AideMemoClient calls can raise: our own
# AideMemoUnavailable for subprocess / binding failures, OSError for file-
# system or signal issues, JSONDecodeError when an old aidememo binary
# returns prose to a `--json` request, and RuntimeError as the umbrella
# the PyO3 binding raises for backend-side problems.
CLIENT_ERRORS: tuple[type[BaseException], ...] = (
    AideMemoUnavailable,
    OSError,
    json.JSONDecodeError,
    RuntimeError,
)

class AideMemoClient:
    """Bidirectional adapter for aidememo.

    Pass an explicit ``store_path`` to pin the local store; otherwise
    we trust the default resolution aidememo performs (``~/.aidememo/wiki.sqlite``
    or whatever ``aidememo config`` says).
    """

    def __init__(
        self,
        store_path: str | os.PathLike | None = None,
        lock_retry_ms: int | None = None,
        source_id: str | None = None,
        actor_id: str | None = None,
        storage_backend: str | None = None,
    ) -> None:
        self.store_path = str(store_path) if store_path else None
        self.storage_backend = _normalise_storage_backend(storage_backend)
        self.lock_retry_ms = 5000 if lock_retry_ms is None else max(0, int(lock_retry_ms))
        self.default_source_id = _normalise_source_id(source_id or os.environ.get("AIDEMEMO_SOURCE_ID"))
        self.default_actor_id = _normalise_source_id(actor_id or os.environ.get("AIDEMEMO_ACTOR_ID"))
        self._py = self._try_load_pyo3()
        if self._py is None and not self._has_cli():
            raise AideMemoUnavailable(
                "aidememo-python is not installed and the `aidememo` CLI is not on PATH; "
                "install one of them from the AideMemo checkout, or install the public packages "
                "after the registry releases land."
            )

    # ------------------------------------------------------------------
    # Backend selection
    # ------------------------------------------------------------------

    def _try_load_pyo3(self) -> Any | None:
        try:
            import aidememo_python  # type: ignore[import-untyped]
        except ImportError:
            return None
        if self.store_path is None:
            # The PyO3 binding requires an explicit store path. Without
            # one, fall through to the CLI — which honors `aidememo config`.
            return None
        if self.storage_backend is None:
            return aidememo_python.AideMemo(self.store_path)
        return aidememo_python.AideMemo(self.store_path, backend=self.storage_backend)

    @staticmethod
    def _has_cli() -> bool:
        return shutil.which("aidememo") is not None

    @property
    def backend(self) -> str:
        return "aidememo-python" if self._py is not None else "cli"

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
        source_id = self._source_id(source_id)
        if self._py is not None:
            kwargs = {"limit": limit, "depth": depth, "recent_limit": recent_limit}
            if source_id is not None:
                kwargs["source_id"] = source_id
            return self._py.query(topic, **kwargs)
        args = ["query", topic, "--limit", str(limit), "-d", str(depth), "--recent-limit", str(recent_limit)]
        if source_id:
            args += ["--source-id", source_id]
        return self._cli_json(args)

    def context(
        self,
        topic: str | None = None,
        limit: int = 10,
        pinned_limit: int = 10,
        recent_limit: int = 10,
        recent_days: int = 7,
        depth: int = 2,
        source_id: str | None = None,
        format: str = "full",
        preview_chars: int = 160,
        max_chars: int | None = None,
    ) -> dict | str:
        source_id = self._source_id(source_id)
        args: dict[str, Any] = {
            "limit": limit,
            "pinned_limit": pinned_limit,
            "recent_limit": recent_limit,
            "recent_days": recent_days,
            "depth": depth,
            "format": format,
            "preview_chars": preview_chars,
        }
        if topic:
            args["topic"] = topic
        if source_id:
            args["source_id"] = source_id
        if max_chars is not None:
            args["max_chars"] = max_chars
        if self._py is None:
            return self._mcp_tool("aidememo_context", args)

        recent = self.recent(
            last=f"{recent_days}d",
            limit=recent_limit,
            source_id=source_id,
        )
        pinned_fn = getattr(self._py, "pinned_facts", None)
        pinned_kwargs: dict[str, Any] = {"limit": max(pinned_limit, 0)}
        if source_id is not None:
            pinned_kwargs["source_id"] = source_id
        pinned = list(pinned_fn(**pinned_kwargs)) if pinned_fn is not None else []
        payload: dict[str, Any] = {
            "pinned": pinned[:pinned_limit],
            "personalisation": [
                f
                for f in recent
                if str(f.get("fact_type") or "").lower() in {"preference", "lesson", "error"}
            ][:limit],
            "recent": recent,
        }
        if topic:
            query_result = self.query(
                topic,
                limit=limit,
                depth=depth,
                recent_limit=min(5, recent_limit),
                source_id=source_id,
            )
            typed_hits = self.search(topic, limit=max(limit, 30), source_id=source_id)
            payload["topic"] = {
                "topic": topic,
                "query_result": query_result,
                "topic_lessons": _take_fact_type(typed_hits, "lesson", 5),
                "topic_errors": _take_fact_type(typed_hits, "error", 5),
            }
        if format == "text":
            return _context_to_text(payload, preview_chars=preview_chars, max_chars=max_chars)
        return payload

    def search(self, query: str, limit: int = 10, source_id: str | None = None) -> list[dict]:
        source_id = self._source_id(source_id)
        if self._py is not None:
            kwargs = {"limit": limit}
            if source_id is not None:
                kwargs["source_id"] = source_id
            return self._py.search(query, **kwargs)
        args = ["search", query, "--limit", str(limit)]
        if source_id:
            args += ["--source-id", source_id]
        return self._cli_json(args)

    def recent(
        self,
        last: str = "7d",
        limit: int = 10,
        source_id: str | None = None,
    ) -> list[dict]:
        # CLI: `aidememo recent --last 7d --limit N`. The PyO3 binding takes
        # an explicit `since_epoch_ms`, which we derive from the same
        # ``Nd / Nh / Nw / Ny`` mini-grammar aidememo's CLI uses so the two
        # paths agree to the second.
        source_id = self._source_id(source_id)
        if self._py is not None:
            since_ms = _now_ms() - parse_window_ms(last)
            kwargs: dict[str, Any] = {"limit": limit, "since_epoch_ms": since_ms}
            if source_id is not None:
                kwargs["source_id"] = source_id
            return self._py.fact_list(**kwargs)
        if source_id is not None:
            payload = self._mcp_tool(
                "aidememo_recent",
                {"last": last, "limit": limit, "source_id": source_id},
            )
            return list(payload.get("facts") or [])
        return self._cli_json(["recent", "--last", last, "-n", str(limit)])

    def entity_list(self, limit: int = 50, source_id: str | None = None) -> list[dict]:
        source_id = self._source_id(source_id)
        if self._py is not None:
            kwargs: dict[str, Any] = {"limit": limit}
            if source_id is not None:
                kwargs["source_id"] = source_id
            return self._py.entity_list(**kwargs)
        if source_id is not None:
            payload = self._mcp_tool(
                "aidememo_entity_list",
                {"limit": limit, "source_id": source_id},
            )
            return list(payload.get("entities") or [])
        return self._cli_json(["entity", "list", "--limit", str(limit)])

    def traverse(
        self,
        entity: str,
        depth: int = 2,
        source_id: str | None = None,
    ) -> dict:
        source_id = self._source_id(source_id)
        if self._py is not None:
            kwargs: dict[str, Any] = {"depth": depth, "direction": "both"}
            if source_id is not None:
                kwargs["source_id"] = source_id
            return self._py.traverse(entity, **kwargs)
        if source_id is not None:
            return self._mcp_tool(
                "aidememo_traverse",
                {"entity": entity, "depth": depth, "direction": "forward", "source_id": source_id},
            )
        return self._cli_json(["traverse", entity, "-d", str(depth)])

    def aggregate(
        self,
        query: str,
        op: str = "count",
        limit: int = 50,
        fact_type: str | None = None,
        entity: str | None = None,
        since: str | None = None,
        source_id: str | None = None,
        current_only: bool = True,
        preview_chars: int = 120,
        relevance_threshold: float | None = None,
    ) -> dict:
        source_id = self._source_id(source_id)
        args: dict[str, Any] = {
            "query": query,
            "op": op,
            "limit": limit,
            "current_only": current_only,
            "preview_chars": preview_chars,
        }
        if fact_type:
            args["fact_type"] = fact_type
        if entity:
            args["entity"] = entity
        if since:
            args["since"] = since
        if source_id:
            args["source_id"] = source_id
        if relevance_threshold is not None:
            args["relevance_threshold"] = relevance_threshold
        if self._py is None:
            return self._mcp_tool("aidememo_aggregate", args)

        if op not in {"count", "enumerate", "by_entity"}:
            raise AideMemoUnavailable(
                f"aidememo_aggregate op={op!r} requires the MCP/CLI backend; "
                "aidememo-python does not expose structured aggregate slots yet."
            )
        hits = self.search(query, limit=limit, source_id=source_id)
        if fact_type:
            wanted = fact_type.lower()
            hits = [h for h in hits if str(h.get("fact_type") or "").lower() == wanted]
        if entity:
            hits = [h for h in hits if entity in (h.get("entity_names") or h.get("entities") or [])]
        if op == "count":
            return {"op": "count", "query": query, "matched": len(hits), "facts_considered": len(hits)}
        if op == "enumerate":
            return {
                "op": "enumerate",
                "query": query,
                "matched": len(hits),
                "items": [_aggregate_item(h, preview_chars) for h in hits],
            }

        groups: dict[str, dict[str, Any]] = {}
        for hit in hits:
            entities = hit.get("entity_names") or hit.get("entities") or ["(no entity)"]
            key = str(entities[0])
            group = groups.setdefault(key, {"entity": key, "count": 0, "fact_types": set(), "max_score": 0.0})
            group["count"] += 1
            group["fact_types"].add(str(hit.get("fact_type") or "note").lower())
            group["max_score"] = max(float(group["max_score"]), float(hit.get("score") or 0.0))
        return {
            "op": "by_entity",
            "query": query,
            "matched": len(hits),
            "groups": [
                {**g, "fact_types": sorted(g["fact_types"])}
                for g in sorted(groups.values(), key=lambda row: row["max_score"], reverse=True)
            ],
        }

    def doctor(self) -> dict:
        self._ensure_global_diagnostics_allowed("doctor")
        if self._py is None:
            return self._mcp_tool("aidememo_doctor", {})
        return {
            "backend": self.backend,
            "stats": self.stats(),
            "issues": self.lint(),
            "sharing": {
                "mode": "in_process_binding",
                "lock_retry_ms": self.lock_retry_ms,
                "source_id": self.default_source_id,
                "hint": "aidememo-python owns the redb handle in this process; use daemon/MCP for high-concurrency shared writes.",
            },
        }

    def lint(self) -> list[dict]:
        self._ensure_global_diagnostics_allowed("lint")
        if self._py is not None:
            return self._py.lint()
        return self._cli_json(["lint"])

    def stats(self) -> dict:
        self._ensure_global_diagnostics_allowed("stats")
        if self._py is not None:
            return self._py.stats()
        return self._cli_json(["stats"])

    def workflow_start(
        self,
        title: str,
        body: str | None = None,
        source: str | None = None,
        source_id: str | None = None,
        actor_id: str | None = None,
        parent_session_id: str | None = None,
        limit: int = 8,
        depth: int = 2,
        recent_limit: int = 5,
        bm25_only: bool = False,
    ) -> dict:
        """Start a workflow-triggered coding task.

        Prefer the PyO3 path when available so a process that already holds
        the redb handle does not shell out to a second `aidememo` process and fight
        its own file lock. The CLI path remains the universal fallback when
        the binding is not installed.
        """
        source_id = self._source_id(source_id)
        actor_id = self._actor_id(actor_id)
        if self._py is not None:
            return self._workflow_start_pyo3(
                title,
                body=body,
                source=source,
                source_id=source_id,
                actor_id=actor_id,
                parent_session_id=parent_session_id,
                limit=limit,
                depth=depth,
                recent_limit=recent_limit,
                bm25_only=bm25_only,
            )
        args = [
            "workflow",
            "start",
            title,
            "--limit",
            str(limit),
            "--depth",
            str(depth),
            "--recent-limit",
            str(recent_limit),
        ]
        if body:
            args += ["--body", body]
        if source:
            args += ["--source", source]
        if source_id:
            args += ["--source-id", source_id]
        if actor_id:
            args += ["--actor-id", actor_id]
        if parent_session_id:
            args += ["--parent-session-id", parent_session_id]
        if bm25_only:
            args.append("--bm25-only")
        return self._cli_json(args)

    def _workflow_start_pyo3(
        self,
        title: str,
        body: str | None = None,
        source: str | None = None,
        source_id: str | None = None,
        actor_id: str | None = None,
        parent_session_id: str | None = None,
        limit: int = 8,
        depth: int = 2,
        recent_limit: int = 5,
        bm25_only: bool = False,
    ) -> dict:
        if self._py is None:
            raise AideMemoUnavailable("aidememo-python backend is not available")

        if parent_session_id:
            try:
                parent_kwargs: dict[str, Any] = {}
                if source_id is not None:
                    parent_kwargs["source_id"] = source_id
                self._py.entity_get(parent_session_id, **parent_kwargs)
            except TypeError as exc:
                raise AideMemoUnavailable(
                    "installed aidememo-python does not support source-scoped parent session validation; "
                    "rebuild/install the current aidememo-python package"
                ) from exc

        session_id = f"session-{_new_ulid()}"
        session_entity_id = self._py.entity_add(
            session_id,
            entity_type="session",
            source_page=source or title,
        )
        if parent_session_id:
            relation_kwargs: dict[str, Any] = {}
            if source_id is not None:
                relation_kwargs["source_id"] = source_id
            self._py.relation_add(
                session_id,
                parent_session_id,
                "continued_from",
                **relation_kwargs,
            )

        trimmed_body = body.strip() if isinstance(body, str) and body.strip() else None
        ticket_content = f"Workflow ticket: {title}"
        if trimmed_body:
            ticket_content = f"{ticket_content}\n\n{trimmed_body}"

        fact_kwargs: dict[str, Any] = {
            "entity_ids": [session_entity_id],
            "fact_type": "question",
            "tags": ["workflow-start", "ticket"],
            "confidence": 1.0,
        }
        if source is not None:
            fact_kwargs["source"] = source
        if source_id is not None:
            fact_kwargs["source_id"] = source_id
        if actor_id is not None:
            fact_kwargs["actor_id"] = actor_id
        try:
            ticket_fact_id = self._py.fact_add(ticket_content, **fact_kwargs)
        except TypeError as exc:
            raise AideMemoUnavailable(
                "installed aidememo-python does not support workflow source_id fields; "
                "rebuild/install the current aidememo-python package"
            ) from exc

        query_text = f"{title}\n\n{trimmed_body}" if trimmed_body else title
        query_kwargs: dict[str, Any] = {
            "limit": limit,
            "depth": depth,
            "recent_limit": recent_limit,
            "current_only": True,
            "mode": "local" if bm25_only else "hybrid",
        }
        search_kwargs: dict[str, Any] = {"limit": 30, "current_only": True}
        if source_id is not None:
            query_kwargs["source_id"] = source_id
            search_kwargs["source_id"] = source_id
        try:
            context = self._py.query(query_text, **query_kwargs)
            typed_hits = self._py.search(query_text, **search_kwargs)
        except TypeError as exc:
            raise AideMemoUnavailable(
                "installed aidememo-python does not support source-scoped workflow retrieval; "
                "rebuild/install the current aidememo-python package"
            ) from exc

        prior_lessons = _take_fact_type(typed_hits, "lesson", 5)
        prior_errors = _take_fact_type(typed_hits, "error", 5)
        relevant_decisions = _take_fact_type(typed_hits, "decision", 5)
        return {
            "session_id": session_id,
            "export": f"export AIDEMEMO_SESSION_ID={session_id}",
            "title": title,
            "source": source,
            "source_id": source_id,
            "actor_id": actor_id,
            "parent_session_id": parent_session_id,
            "ticket_fact_id": ticket_fact_id,
            "context": context,
            "prior_lessons": prior_lessons,
            "prior_errors": prior_errors,
            "relevant_decisions": relevant_decisions,
        }

    def session_canvas(
        self,
        session_id: str | None = None,
        *,
        limit: int = 80,
        include_superseded: bool = False,
        source_id: str | None = None,
    ) -> str:
        """Return the read-only Markdown + Mermaid canvas for a workflow session."""

        args: dict[str, Any] = {
            "limit": limit,
            "include_superseded": include_superseded,
        }
        source_id = self._source_id(source_id)
        if source_id:
            args["source_id"] = source_id
        if session_id:
            args["session"] = session_id
        payload = self._mcp_tool("aidememo_session_canvas", args)
        if isinstance(payload, dict):
            content = payload.get("content")
            return content if isinstance(content, str) else ""
        return str(payload)

    def project_profile(
        self,
        *,
        limit: int = 80,
        source_id: str | None = None,
        include_sessions: bool = False,
    ) -> str:
        """Return the read-only project_profile.md text artifact."""

        source_id = self._source_id(source_id)
        args: dict[str, Any] = {
            "limit": limit,
            "include_sessions": include_sessions,
        }
        if source_id:
            args["source_id"] = source_id
        payload = self._mcp_tool("aidememo_profile_export", args)
        if isinstance(payload, dict):
            content = payload.get("content")
            return content if isinstance(content, str) else ""
        return str(payload)

    # ------------------------------------------------------------------
    # Write API
    # ------------------------------------------------------------------

    def fact_add(
        self,
        content: str,
        entities: list[str] | None = None,
        fact_type: str | None = None,
        tags: list[str] | None = None,
        confidence: float | None = None,
        source_id: str | None = None,
        actor_id: str | None = None,
        session_id: str | None = None,
    ) -> str:
        source_id = self._source_id(source_id)
        actor_id = self._actor_id(actor_id)
        if self._py is not None:
            entity_ids = [self._py.resolve_entity(e) for e in (entities or [])]
            kwargs: dict = {
                "entity_ids": entity_ids,
                "tags": tags or [],
            }
            if fact_type is not None:
                kwargs["fact_type"] = fact_type
            if confidence is not None:
                kwargs["confidence"] = confidence
            if source_id is not None:
                kwargs["source_id"] = source_id
            if actor_id is not None:
                kwargs["actor_id"] = actor_id
            if session_id is not None:
                kwargs["session_id"] = session_id
            return self._py.fact_add(content, **kwargs)
        if session_id:
            mcp_args: dict[str, Any] = {
                "content": content,
                "session_id": session_id,
            }
            if fact_type is not None:
                mcp_args["fact_type"] = fact_type
            if entities:
                mcp_args["entities"] = entities
            if tags:
                mcp_args["tags"] = tags
            if confidence is not None:
                mcp_args["confidence"] = confidence
            if source_id:
                mcp_args["source_id"] = source_id
            if actor_id:
                mcp_args["actor_id"] = actor_id
            payload = self._mcp_tool("aidememo_fact_add", mcp_args)
            if isinstance(payload, dict) and isinstance(payload.get("id"), str):
                return payload["id"]
            raise AideMemoUnavailable(f"aidememo_fact_add returned no id: {payload!r}")
        args = ["fact", "add", content]
        if fact_type is not None:
            args += ["--type", fact_type]
        if entities:
            args += ["--entities", ",".join(entities)]
        if tags:
            # `aidememo fact add` takes a single `--tags A,B,C` flag, not
            # repeated `--tag` entries — the latter raises
            # `Error: no such flag: --tag`. Comma-join the list to
            # match the CLI's actual surface.
            args += ["--tags", ",".join(tags)]
        if confidence is not None:
            args += ["--confidence", f"{confidence:.3f}"]
        if source_id:
            args += ["--source-id", source_id]
        if actor_id:
            args += ["--actor-id", actor_id]
        # Prefer the structured `--json` output (`{"id": "<ULID>",
        # "auto_created_entities": [...]}`) over scraping the human
        # message; falls back to ULID-grep on older aidememo binaries that
        # haven't shipped the JSON path yet.
        try:
            payload = self._cli_json(args)
        except AideMemoUnavailable:
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
        ``fact_add`` at typical batch sizes). On the CLI fallback we call
        the MCP ``aidememo_fact_add_many`` tool so source/session semantics match
        agent tool calls instead of degrading to sequential CLI inserts.
        """
        default_source_id = self._source_id(None)
        default_actor_id = self._actor_id(None)
        if self._py is None:
            args: dict[str, Any] = {"items": items}
            if default_source_id:
                args["source_id"] = default_source_id
            if default_actor_id:
                args["actor_id"] = default_actor_id
            payload = self._mcp_tool("aidememo_fact_add_many", args)
            facts = payload.get("facts") if isinstance(payload, dict) else None
            if isinstance(facts, list):
                return [str(f.get("id")) for f in facts if isinstance(f, dict) and f.get("id")]
            return []

        if self._py is not None:
            py_items = []
            for item in items:
                names = item.get("entities") or []
                entity_ids = [self._py.resolve_entity(n) for n in names]
                source_id = _normalise_source_id(item.get("source_id")) or default_source_id
                actor_id = _normalise_source_id(item.get("actor_id")) or default_actor_id
                py_items.append({
                    "content": item["content"],
                    "entity_ids": entity_ids,
                    "fact_type": item.get("fact_type"),
                    "tags": item.get("tags") or [],
                    "confidence": item.get("confidence"),
                    "source_id": source_id,
                    "actor_id": actor_id,
                    "session_id": item.get("session_id"),
                })
            return list(self._py.fact_add_many(py_items))
        return []

    # ------------------------------------------------------------------
    # Branch log API
    # ------------------------------------------------------------------

    def branch_push(
        self,
        branch: str,
        destination: str | os.PathLike,
        *,
        base: str | os.PathLike | None = None,
    ) -> dict:
        """Push this store's branch segment to a local directory or S3 prefix.

        Local paths use the in-process binding when available. S3 targets fall
        back to the CLI so the user's `aidememo --features s3` build controls
        credentials, compression, and AWS SDK behavior.
        """

        branch = str(branch).strip()
        destination_s = str(destination)
        base_s = str(base) if base is not None else None
        if self._py is not None and not _is_s3_uri(destination_s) and not _is_s3_uri(base_s):
            try:
                return self._py.branch_push(branch, destination_s, base=base_s)
            except AttributeError as exc:
                raise AideMemoUnavailable(
                    "installed aidememo-python does not expose branch_push; "
                    "rebuild/install the current aidememo-python package"
                ) from exc
        args = ["branch", "push", "--branch", branch]
        if base_s:
            args += ["--base", base_s]
        args.append(destination_s)
        return self._cli_json(args)

    def branch_merge(
        self,
        source: str | os.PathLike,
        *,
        branch: str | None = None,
    ) -> dict:
        """Merge branch segments from a local directory or S3 prefix."""

        source_s = str(source)
        branch_s = str(branch).strip() if branch is not None else None
        if branch_s == "":
            branch_s = None
        if self._py is not None and not _is_s3_uri(source_s):
            try:
                return self._py.branch_merge(source_s, branch=branch_s)
            except AttributeError as exc:
                raise AideMemoUnavailable(
                    "installed aidememo-python does not expose branch_merge; "
                    "rebuild/install the current aidememo-python package"
                ) from exc
        args = ["branch", "merge"]
        if branch_s:
            args += ["--branch", branch_s]
        args.append(source_s)
        return self._cli_json(args)

    def _source_id(self, source_id: str | None) -> str | None:
        return _normalise_source_id(source_id) or self.default_source_id

    def _actor_id(self, actor_id: str | None) -> str | None:
        return _normalise_source_id(actor_id) or getattr(self, "default_actor_id", None)

    def _ensure_global_diagnostics_allowed(self, operation: str) -> None:
        if getattr(self, "default_source_id", None) is not None:
            raise AideMemoUnavailable(
                f"{operation} is global store metadata and is unavailable when default_source_id is set"
            )

    def _fact_add_legacy(self, args: list[str]) -> str:
        """Legacy fallback for `aidememo` binaries that pre-date the
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

    def _cli(self, args: list[str], input_text: str | None = None) -> str:
        cmd = ["aidememo"]
        if self.storage_backend:
            cmd += ["--backend", self.storage_backend]
        if self.store_path:
            cmd += ["--store", self.store_path]
        cmd += args
        deadline = time.monotonic() + (self.lock_retry_ms / 1000.0)
        attempts = 0
        last = None
        while True:
            attempts += 1
            completed = subprocess.run(
                cmd,
                input=input_text,
                capture_output=True,
                text=True,
                check=False,
            )
            if completed.returncode == 0:
                return completed.stdout
            last = completed
            stderr = completed.stderr.strip()
            if not _is_lock_error(stderr) or self.lock_retry_ms == 0 or time.monotonic() >= deadline:
                break
            time.sleep(0.1)

        assert last is not None
        retry_note = f" after {attempts} attempt(s)" if attempts > 1 else ""
        raise AideMemoUnavailable(
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
            raise AideMemoUnavailable(
                f"non-JSON output from `aidememo {' '.join(args)}`: {out[:200]!r}"
            ) from exc

    def _mcp_tool(self, name: str, arguments: dict[str, Any]) -> Any:
        arguments = dict(arguments)
        default_source_id = getattr(self, "default_source_id", None)
        default_actor_id = getattr(self, "default_actor_id", None)
        if default_source_id is not None:
            arguments.setdefault("source_id", default_source_id)
        if default_actor_id is not None:
            arguments.setdefault("actor_id", default_actor_id)
        init = {
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {"clientInfo": {"name": "aidememo-agent-sdk", "version": "0.1.0"}},
        }
        call = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }
        out = self._cli(["mcp"], input_text=f"{json.dumps(init)}\n{json.dumps(call)}\n")
        responses = [json.loads(line) for line in out.splitlines() if line.strip()]
        response = next((r for r in responses if r.get("id") == 1), None)
        if response is None:
            raise AideMemoUnavailable(f"aidememo mcp returned no response for {name}")
        if response.get("error"):
            raise AideMemoUnavailable(f"aidememo mcp {name} failed: {response['error']}")
        result = response.get("result") or {}
        if result.get("isError"):
            text = _mcp_text(result)
            raise AideMemoUnavailable(text or f"aidememo mcp {name} returned isError")
        text = _mcp_text(result)
        if not text:
            return result
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text

def _normalise_source_id(source_id: Any) -> str | None:
    if source_id is None:
        return None
    source_id = str(source_id).strip()
    return source_id or None


def _normalise_storage_backend(backend: Any) -> str | None:
    if backend is None:
        return None
    backend = str(backend).strip()
    return backend or None


def _is_s3_uri(value: Any) -> bool:
    return isinstance(value, str) and value.startswith("s3://")


# Crockford's ULID alphabet: 26 chars, [0-9A-HJKMNP-TV-Z] (no I, L, O,
# U). We don't enforce the full alphabet — `[0-9A-Z]{26}` is a tighter
# match than the previous "isalnum + isupper" walk and good enough to
# tell ULIDs apart from ordinary words in `aidememo fact add` prose output.
_ULID_RE = re.compile(r"^[0-9A-Z]{26}$")
_CROCKFORD32 = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


# Window grammar (`30d`, `12h`, `4w`, `1y`, `90m`, `60s`). Mirrors
# `aidememo_core::time::parse_duration_to_ms` so the PyO3 backend agrees
# with what `aidememo recent --last <window>` would compute.
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


def _take_fact_type(hits: list[dict], fact_type: str, limit: int) -> list[dict]:
    out: list[dict] = []
    wanted = fact_type.lower()
    for hit in hits:
        if str(hit.get("fact_type") or "").lower() != wanted:
            continue
        out.append(hit)
        if len(out) >= limit:
            break
    return out


def _mcp_text(result: dict) -> str:
    parts: list[str] = []
    for block in result.get("content") or []:
        if isinstance(block, dict) and block.get("type") == "text":
            parts.append(str(block.get("text") or ""))
    return "\n".join(p for p in parts if p).strip()


def _aggregate_item(hit: dict, preview_chars: int) -> dict:
    content = str(hit.get("content") or "")
    if len(content) > preview_chars:
        content = content[: max(0, preview_chars - 1)].rstrip() + "..."
    return {
        "id": hit.get("fact_id") or hit.get("id"),
        "content": content,
        "fact_type": hit.get("fact_type") or "note",
        "score": hit.get("score"),
        "entities": hit.get("entity_names") or hit.get("entities") or [],
    }


def _context_to_text(payload: dict, preview_chars: int = 160, max_chars: int | None = None) -> str:
    lines: list[str] = ["# AideMemo context"]
    for title, key in [
        ("Pinned", "pinned"),
        ("Personalisation", "personalisation"),
        ("Recent", "recent"),
    ]:
        rows = payload.get(key) or []
        if not rows:
            continue
        lines.extend(["", f"## {title}"])
        for row in rows:
            lines.append(f"- {_fact_preview(row, preview_chars)}")
    topic = payload.get("topic")
    if isinstance(topic, dict):
        lines.extend(["", f"## Topic: {topic.get('topic') or ''}".rstrip()])
        query_result = topic.get("query_result") or {}
        for hit in query_result.get("search") or []:
            lines.append(f"- {_fact_preview(hit, preview_chars)}")
        for label, key in [("Lessons", "topic_lessons"), ("Errors", "topic_errors")]:
            rows = topic.get(key) or []
            if rows:
                lines.extend(["", f"### {label}"])
                for row in rows:
                    lines.append(f"- {_fact_preview(row, preview_chars)}")
    text = "\n".join(lines)
    if max_chars is not None and len(text) > max_chars:
        return text[: max(0, max_chars - 3)].rstrip() + "..."
    return text


def _fact_preview(row: dict, preview_chars: int) -> str:
    content = str(row.get("content") or row.get("preview") or "").strip()
    if len(content) > preview_chars:
        content = content[: max(0, preview_chars - 1)].rstrip() + "..."
    ftype = row.get("fact_type") or "note"
    return f"({ftype}) {content}" if content else f"({ftype})"


def _new_ulid() -> str:
    value = (int(time.time() * 1000) << 80) | secrets.randbits(80)
    chars = []
    for shift in range(125, -1, -5):
        chars.append(_CROCKFORD32[(value >> shift) & 0x1F])
    return "".join(chars)


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
