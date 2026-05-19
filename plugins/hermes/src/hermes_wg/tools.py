"""Tool definitions for the wg plugin.

Each tool returns plain JSON-serialisable Python (Hermes serialises
to the model). Schemas are minimal - the description is the most
load-bearing field for the model's tool selection, so we keep them
crisp and grounded in actual user phrasings.
"""

from __future__ import annotations

import json
from typing import Any, Callable

from .client import WgClient

TOOLSET = "wg"


def _make_handlers(client: WgClient) -> list[tuple[str, dict, Callable[..., Any], str, str]]:
    """Return ``(name, schema, handler, description, emoji)`` tuples
    for every tool we expose. Kept as a single function so call sites
    don't drift out of sync with the schemas.

    Hermes calls each handler as ``handler(args: dict, **framework_kw)``
    - the JSON-decoded tool input arrives as one positional dict, not
    as exploded keyword arguments, and Hermes adds runtime kwargs of
    its own (``task_id`` and friends). The matching signature on every
    handler is therefore ``(args, **_)``, with each helper pulling its
    own keys out of ``args``.

    The ``schema`` dict has to have the full Anthropic-style function
    shape - ``{"name": ..., "description": ..., "parameters": {...}}``
    - because ``agent.anthropic_adapter._coerce_tools`` reads
    ``fn["parameters"]`` directly when serialising tools for the
    upstream API. Passing the bare parameters object loses every
    property and ships an empty schema to the model (the symptom we
    saw with MiniMax: "unhashable type: 'slice'" because the streamed
    response had no fields to bind back to).
    """

    def _serialize(value: Any) -> str:
        # Hermes's `agent.display._detect_tool_failure` slices the
        # result with `result[:500]` to scan it for error markers.
        # That blows up with `unhashable type: 'slice'` if a tool
        # returns a dict or list (slicing dicts hashes the slice
        # object). Spotify's bundled handlers all return JSON
        # strings; we follow the same pattern. The model still
        # gets a structured payload because every wg call shape
        # is JSON-serialisable.
        return json.dumps(value, ensure_ascii=False, default=str)

    def _query(args: dict, **_: Any) -> str:
        return _serialize(
            client.query(
                str(args.get("topic") or ""),
                limit=int(args.get("limit") or 5),
                depth=int(args.get("depth") or 2),
                recent_limit=int(args.get("recent_limit") or 5),
                source_id=args.get("source_id"),
            )
        )

    def _search(args: dict, **_: Any) -> str:
        return _serialize(
            client.search(
                str(args.get("query") or ""),
                limit=int(args.get("limit") or 10),
                source_id=args.get("source_id"),
            )
        )

    def _recent(args: dict, **_: Any) -> str:
        return _serialize(
            client.recent(
                last=str(args.get("last") or "7d"),
                limit=int(args.get("limit") or 10),
            )
        )

    def _entity_list(args: dict, **_: Any) -> str:
        return _serialize(client.entity_list(limit=int(args.get("limit") or 50)))

    def _traverse(args: dict, **_: Any) -> str:
        return _serialize(
            client.traverse(
                str(args.get("entity") or ""),
                depth=int(args.get("depth") or 2),
            )
        )

    def _fact_add(args: dict, **_: Any) -> str:
        entities = args.get("entities")
        if entities is not None and not isinstance(entities, list):
            entities = [str(entities)]
        tags = args.get("tags")
        if tags is not None and not isinstance(tags, list):
            tags = [str(tags)]
        return _serialize(
            {
                "id": client.fact_add(
                    str(args.get("content") or ""),
                    entities=entities,
                    fact_type=str(args.get("fact_type") or "note"),
                    tags=tags,
                    source_id=args.get("source_id"),
                )
            }
        )

    def _lint(args: dict, **_: Any) -> str:
        return _serialize(client.lint())

    def _schema(name: str, description: str, parameters: dict) -> dict:
        return {"name": name, "description": description, "parameters": parameters}

    return [
        (
            "wg_query",
            _schema(
                "wg_query",
                "One-shot context fetch: search + traverse + recent for a topic. Prefer over chaining wg_search/wg_traverse/wg_recent.",
                {
                    "type": "object",
                    "properties": {
                        "topic": {"type": "string", "description": "The entity name or topic to gather context on."},
                        "limit": {"type": "integer", "description": "Top-N search hits (default 5)."},
                        "depth": {"type": "integer", "description": "Graph traversal depth (default 2)."},
                        "recent_limit": {"type": "integer", "description": "How many recent facts to include (default 5)."},
                        "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id filter."},
                    },
                    "required": ["topic"],
                },
            ),
            _query,
            "One-shot context fetch: search + traverse + recent for a topic. Prefer over chaining wg_search/wg_traverse/wg_recent.",
            "🧠",
        ),
        (
            "wg_search",
            _schema(
                "wg_search",
                "Hybrid BM25 + semantic search across all facts. Use when the user asks about a concept rather than a specific entity.",
                {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Free-text query (BM25 + semantic vectors)."},
                        "limit": {"type": "integer", "description": "Default 10."},
                        "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id filter."},
                    },
                    "required": ["query"],
                },
            ),
            _search,
            "Hybrid BM25 + semantic search across all facts. Use when the user asks about a concept rather than a specific entity.",
            "🔍",
        ),
        (
            "wg_recent",
            _schema(
                "wg_recent",
                "Most recent facts within a time window. Useful for 'what's changed lately' questions.",
                {
                    "type": "object",
                    "properties": {
                        "last": {"type": "string", "description": "Window like 7d / 24h / 30d. Default 7d."},
                        "limit": {"type": "integer", "description": "Default 10."},
                    },
                },
            ),
            _recent,
            "Most recent facts within a time window. Useful for 'what's changed lately' questions.",
            "🕒",
        ),
        (
            "wg_entity_list",
            _schema(
                "wg_entity_list",
                "List entities tracked in the wiki (technologies, people, projects, ...). Browse before adding facts to avoid duplicates.",
                {
                    "type": "object",
                    "properties": {
                        "limit": {"type": "integer", "description": "Default 50."},
                    },
                },
            ),
            _entity_list,
            "List entities tracked in the wiki (technologies, people, projects, ...). Browse before adding facts to avoid duplicates.",
            "📚",
        ),
        (
            "wg_traverse",
            _schema(
                "wg_traverse",
                "Walk the graph forward and backward from a known entity to surface related entities.",
                {
                    "type": "object",
                    "properties": {
                        "entity": {"type": "string"},
                        "depth": {"type": "integer", "description": "Default 2."},
                    },
                    "required": ["entity"],
                },
            ),
            _traverse,
            "Walk the graph forward and backward from a known entity to surface related entities.",
            "🕸️",
        ),
        (
            "wg_fact_add",
            _schema(
                "wg_fact_add",
                "Append a fact to the wiki. Always link to existing entities (call wg_entity_list first if unsure).",
                {
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "The fact text - short, declarative."},
                        "entities": {"type": "array", "items": {"type": "string"}, "description": "Entity names this fact links to."},
                        "fact_type": {"type": "string", "description": "decision | pattern | convention | claim | note | question. Default note."},
                        "tags": {"type": "array", "items": {"type": "string"}},
                        "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id. Use with wg_query/wg_search source_id filters to isolate shared-store reads."},
                    },
                    "required": ["content"],
                },
            ),
            _fact_add,
            "Append a fact to the wiki. Always link to existing entities (call wg_entity_list first if unsure).",
            "📝",
        ),
        (
            "wg_lint",
            _schema(
                "wg_lint",
                "Graph health check - orphan facts, missing entities, broken links.",
                {"type": "object", "properties": {}},
            ),
            _lint,
            "Graph health check - orphan facts, missing entities, broken links.",
            "🩺",
        ),
    ]


def register_all(ctx: Any, client: WgClient) -> None:
    """Register every wg tool with the given Hermes plugin context."""
    for name, schema, handler, description, emoji in _make_handlers(client):
        ctx.register_tool(
            name=name,
            toolset=TOOLSET,
            schema=schema,
            handler=handler,
            description=description,
            emoji=emoji,
        )


# Pretty-printing helper used by slash commands and CLI subcommands -
# keeps formatting consistent across surfaces.
def to_pretty_json(value: Any) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False, default=str)
