"""Tool definitions for the wg plugin.

Each tool returns plain JSON-serialisable Python (Hermes serialises
to the model). Schemas are minimal — the description is the most
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
    """
    return [
        (
            "wg_query",
            {
                "type": "object",
                "properties": {
                    "topic": {"type": "string", "description": "The entity name or topic to gather context on."},
                    "limit": {"type": "integer", "default": 5, "description": "Top-N search hits (default 5)."},
                    "depth": {"type": "integer", "default": 2, "description": "Graph traversal depth (default 2)."},
                    "recent_limit": {"type": "integer", "default": 5, "description": "How many recent facts to include (default 5)."},
                },
                "required": ["topic"],
            },
            lambda topic, limit=5, depth=2, recent_limit=5: client.query(
                topic, limit=limit, depth=depth, recent_limit=recent_limit
            ),
            "One-shot context fetch: search + traverse + recent for a topic. Prefer over chaining wg_search/wg_traverse/wg_recent.",
            "🧠",
        ),
        (
            "wg_search",
            {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Free-text query (BM25 + semantic vectors)."},
                    "limit": {"type": "integer", "default": 10},
                },
                "required": ["query"],
            },
            lambda query, limit=10: client.search(query, limit=limit),
            "Hybrid BM25 + semantic search across all facts. Use when the user asks about a concept rather than a specific entity.",
            "🔍",
        ),
        (
            "wg_recent",
            {
                "type": "object",
                "properties": {
                    "last": {"type": "string", "default": "7d", "description": "Window like 7d / 24h / 30d."},
                    "limit": {"type": "integer", "default": 10},
                },
            },
            lambda last="7d", limit=10: client.recent(last=last, limit=limit),
            "Most recent facts within a time window. Useful for 'what's changed lately' questions.",
            "🕒",
        ),
        (
            "wg_entity_list",
            {
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "default": 50},
                },
            },
            lambda limit=50: client.entity_list(limit=limit),
            "List entities tracked in the wiki (technologies, people, projects, …). Browse before adding facts to avoid duplicates.",
            "📚",
        ),
        (
            "wg_traverse",
            {
                "type": "object",
                "properties": {
                    "entity": {"type": "string"},
                    "depth": {"type": "integer", "default": 2},
                },
                "required": ["entity"],
            },
            lambda entity, depth=2: client.traverse(entity, depth=depth),
            "Walk the graph forward and backward from a known entity to surface related entities.",
            "🕸️",
        ),
        (
            "wg_fact_add",
            {
                "type": "object",
                "properties": {
                    "content": {"type": "string", "description": "The fact text — short, declarative."},
                    "entities": {"type": "array", "items": {"type": "string"}, "description": "Entity names this fact links to."},
                    "fact_type": {"type": "string", "default": "note", "description": "decision | pattern | convention | claim | note | question"},
                    "tags": {"type": "array", "items": {"type": "string"}, "default": []},
                },
                "required": ["content"],
            },
            lambda content, entities=None, fact_type="note", tags=None: {
                "id": client.fact_add(content, entities=entities, fact_type=fact_type, tags=tags)
            },
            "Append a fact to the wiki. Always link to existing entities (call wg_entity_list first if unsure).",
            "📝",
        ),
        (
            "wg_lint",
            {"type": "object", "properties": {}},
            lambda: client.lint(),
            "Graph health check — orphan facts, missing entities, broken links.",
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


# Pretty-printing helper used by slash commands and CLI subcommands —
# keeps formatting consistent across surfaces.
def to_pretty_json(value: Any) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False, default=str)
