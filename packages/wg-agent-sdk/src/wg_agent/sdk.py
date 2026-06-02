"""Composable wg primitives for code-first agent workflows.

The tool layer exposes convenient endpoints for the model. This module is
the code-oriented surface: an agent can run a small Python program that
fans out retrieval, keeps intermediate state in ordinary Python objects,
dedupes / groups / checks coverage deterministically, and only renders the
final compact evidence back to the model.
"""

from __future__ import annotations

from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from typing import Any, Iterable

from .client import WgClient


class WgMemorySDK:
    """Small, dependency-free SDK for programmable local memory retrieval."""

    def __init__(self, client: WgClient) -> None:
        self.client = client

    @classmethod
    def open(
        cls,
        *,
        store_path: str | None = None,
        source_id: str | None = None,
        lock_retry_ms: int | None = None,
    ) -> "WgMemorySDK":
        """Create an SDK from normal wg environment defaults."""

        return cls(WgClient(store_path=store_path, source_id=source_id, lock_retry_ms=lock_retry_ms))

    def search_many(
        self,
        queries: Iterable[str | dict[str, Any]],
        *,
        limit_per_query: int = 10,
        source_id: str | None = None,
        concurrency: int = 8,
    ) -> list[dict[str, Any]]:
        jobs = [_normalise_query(q) for q in queries]
        return _map_ordered(
            jobs,
            lambda job: {
                **job,
                "hits": self.client.search(
                    str(job["query"]),
                    limit=int(job.get("limit") or limit_per_query),
                    source_id=job.get("source_id") or source_id,
                ),
            },
            concurrency=concurrency,
        )

    def search_rows(
        self,
        queries: Iterable[str | dict[str, Any]],
        *,
        limit_per_query: int = 10,
        source_id: str | None = None,
        concurrency: int = 8,
        dedupe: bool = True,
    ) -> list[dict[str, Any]]:
        """Run fanout search and return a flat row list ready for grouping.

        This is the ergonomic path for most code-first agent tasks. Use
        ``search_many`` only when the caller needs the per-query envelopes.
        """

        rows = self.flatten_hits(
            self.search_many(
                queries,
                limit_per_query=limit_per_query,
                source_id=source_id,
                concurrency=concurrency,
            )
        )
        return self.dedupe_by_fact(rows) if dedupe else rows

    def query_many(
        self,
        topics: Iterable[str | dict[str, Any]],
        *,
        limit: int = 5,
        depth: int = 2,
        recent_limit: int = 5,
        source_id: str | None = None,
        concurrency: int = 8,
    ) -> list[dict[str, Any]]:
        jobs = [_normalise_query(t, key="topic") for t in topics]
        return _map_ordered(
            jobs,
            lambda job: {
                **job,
                "context": self.client.query(
                    str(job["topic"]),
                    limit=int(job.get("limit") or limit),
                    depth=int(job.get("depth") or depth),
                    recent_limit=int(job.get("recent_limit") or recent_limit),
                    source_id=job.get("source_id") or source_id,
                ),
            },
            concurrency=concurrency,
        )

    def aggregate_many(
        self,
        requests: Iterable[str | dict[str, Any]],
        *,
        op: str = "count",
        limit: int = 50,
        source_id: str | None = None,
        concurrency: int = 8,
    ) -> list[dict[str, Any]]:
        jobs = [_normalise_query(r) for r in requests]
        return _map_ordered(
            jobs,
            lambda job: {
                **job,
                "result": self.client.aggregate(
                    str(job["query"]),
                    op=str(job.get("op") or op),
                    limit=int(job.get("limit") or limit),
                    fact_type=job.get("fact_type"),
                    entity=job.get("entity"),
                    since=job.get("since"),
                    source_id=job.get("source_id") or source_id,
                    relevance_threshold=job.get("relevance_threshold"),
                ),
            },
            concurrency=concurrency,
        )

    def dedupe_by_fact(self, rows: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
        seen: set[str] = set()
        out: list[dict[str, Any]] = []
        for row in rows:
            key = _row_identity(row)
            if key in seen:
                continue
            seen.add(key)
            out.append(row)
        return out

    def filter_by_source(self, rows: Iterable[dict[str, Any]], source_id: str) -> list[dict[str, Any]]:
        return [row for row in rows if row.get("source_id") == source_id]

    def group_by_entity(self, rows: Iterable[dict[str, Any]]) -> dict[str, list[dict[str, Any]]]:
        groups: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for row in rows:
            entities = row.get("entity_names") or row.get("entities") or ["(no entity)"]
            for entity in entities:
                groups[str(entity)].append(row)
        return dict(groups)

    def coverage_by(
        self,
        rows: Iterable[dict[str, Any]],
        keys: str | Iterable[str],
    ) -> dict[str, Any]:
        key_list = [keys] if isinstance(keys, str) else list(keys)
        counter: Counter[tuple[str, ...]] = Counter()
        missing = 0
        total = 0
        for row in rows:
            total += 1
            values = tuple(str(row.get(key) or "") for key in key_list)
            if any(value == "" for value in values):
                missing += 1
            counter[values] += 1
        groups = [
            {
                **{key: value for key, value in zip(key_list, values)},
                "count": count,
            }
            for values, count in sorted(counter.items(), key=lambda item: item[0])
        ]
        return {"keys": key_list, "total": total, "missing": missing, "groups": groups}

    def flatten_hits(
        self,
        batches: Iterable[dict[str, Any]],
        *,
        hit_key: str = "hits",
    ) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        for batch in batches:
            meta = {k: v for k, v in batch.items() if k != hit_key}
            for hit in batch.get(hit_key) or []:
                rows.append({**meta, **hit})
        return rows

    def to_fact_batch(
        self,
        observations: Iterable[dict[str, Any]],
        *,
        default_fact_type: str = "note",
        default_entities: list[str] | None = None,
        source_id: str | None = None,
        session_id: str | None = None,
        tags: list[str] | None = None,
    ) -> list[dict[str, Any]]:
        items: list[dict[str, Any]] = []
        for obs in observations:
            content = str(obs.get("content") or obs.get("observation") or "").strip()
            if not content:
                continue
            item = {
                "content": content,
                "fact_type": obs.get("fact_type") or default_fact_type,
                "entities": obs.get("entities") or default_entities or [],
                "tags": obs.get("tags") or tags or [],
            }
            if obs.get("confidence") is not None:
                item["confidence"] = obs["confidence"]
            if obs.get("source_id") or source_id:
                item["source_id"] = obs.get("source_id") or source_id
            if obs.get("session_id") or session_id:
                item["session_id"] = obs.get("session_id") or session_id
            items.append(item)
        return items

    def commit_fact_batch(self, items: list[dict[str, Any]]) -> list[str]:
        return self.client.fact_add_many(items)

    def remember(
        self,
        observations: Iterable[dict[str, Any]],
        *,
        default_fact_type: str = "note",
        default_entities: list[str] | None = None,
        source_id: str | None = None,
        session_id: str | None = None,
        tags: list[str] | None = None,
    ) -> list[str]:
        """Convert observations to facts and persist them in one batch."""

        return self.commit_fact_batch(
            self.to_fact_batch(
                observations,
                default_fact_type=default_fact_type,
                default_entities=default_entities,
                source_id=source_id,
                session_id=session_id,
                tags=tags,
            )
        )


def _normalise_query(value: str | dict[str, Any], *, key: str = "query") -> dict[str, Any]:
    if isinstance(value, dict):
        return dict(value)
    return {key: value}


def _map_ordered(
    jobs: list[dict[str, Any]],
    fn,
    *,
    concurrency: int,
) -> list[dict[str, Any]]:
    if not jobs:
        return []
    workers = max(1, min(int(concurrency), len(jobs)))
    if workers == 1:
        return [fn(job) for job in jobs]
    out: list[dict[str, Any] | None] = [None] * len(jobs)
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {pool.submit(fn, job): idx for idx, job in enumerate(jobs)}
        for future in as_completed(futures):
            out[futures[future]] = future.result()
    return [row for row in out if row is not None]


def _row_identity(row: dict[str, Any]) -> str:
    for key in ("fact_id", "id", "url"):
        value = row.get(key)
        if value:
            return f"{key}:{value}"
    return f"content:{row.get('content') or row.get('preview') or row}"
