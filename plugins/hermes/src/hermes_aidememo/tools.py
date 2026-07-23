"""Tool definitions for the AideMemo plugin.

Each tool returns plain JSON-serialisable Python (Hermes serialises
to the model). Schemas are minimal - the description is the most
load-bearing field for the model's tool selection, so we keep them
crisp and grounded in actual user phrasings.
"""

from __future__ import annotations

import json
from typing import Any, Callable

from .client import AideMemoClient

TOOLSET = "aidememo"


def _make_handlers(client: AideMemoClient) -> list[tuple[str, dict, Callable[..., Any], str, str]]:
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
        # gets a structured payload because every aidememo call shape
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
                actor_id=args.get("actor_id"),
                parent_session_id=args.get("parent_session_id"),
            )
        )

    def _context(args: dict, **_: Any) -> str:
        return _serialize(
            client.context(
                topic=args.get("topic"),
                limit=int(args.get("limit") or 10),
                pinned_limit=int(args.get("pinned_limit") or 10),
                recent_limit=int(args.get("recent_limit") or 10),
                recent_days=int(args.get("recent_days") or 7),
                depth=int(args.get("depth") or 2),
                source_id=args.get("source_id"),
                format=str(args.get("format") or "full"),
                preview_chars=int(args.get("preview_chars") or 160),
                max_chars=int(args["max_chars"]) if args.get("max_chars") is not None else None,
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
                source_id=args.get("source_id"),
            )
        )

    def _workflow_start(args: dict, **_: Any) -> str:
        return _serialize(
            client.workflow_start(
                str(args.get("title") or ""),
                body=args.get("body"),
                source=args.get("source"),
                source_id=args.get("source_id"),
                limit=int(args.get("limit") or 8),
                depth=int(args.get("depth") or 2),
                recent_limit=int(args.get("recent_limit") or 5),
                bm25_only=bool(args.get("bm25_only", False)),
            )
        )

    def _handoff(args: dict, **_: Any) -> str:
        kwargs = {
            "from_actor": args.get("from_actor"),
            "from_route": args.get("from") or args.get("from_route"),
            "to_route": args.get("to") or args.get("to_route"),
            "from_agent": args.get("from_agent"),
            "from_profile": args.get("from_profile"),
            "to_agent": args.get("to_agent"),
            "to_profile": args.get("to_profile"),
            "to_actor": args.get("to_actor"),
            "focus": args.get("focus"),
            "done_when": args.get("done_when"),
            "dispatch": bool(args.get("dispatch", False)),
            "source_id": args.get("source_id"),
            "limit": int(args.get("limit") or 40),
            "include_superseded": bool(args.get("include_superseded", False)),
        }
        if args.get("kanban_task"):
            kwargs["kanban_task"] = args["kanban_task"]
        if args.get("kanban_board"):
            kwargs["kanban_board"] = args["kanban_board"]
        return _serialize(
            client.handoff_packet(
                args.get("session") or args.get("session_id"),
                **kwargs,
            )
        )

    def _handoff_inbox(args: dict, **_: Any) -> str:
        action = args.get("action") or "list"
        if action == "list":
            return _serialize(
                {
                    "assignments": client.handoff_inbox(
                        actor_id=args.get("actor_id"),
                        source_id=args.get("source_id"),
                        include_completed=bool(args.get("include_completed", False)),
                        limit=int(args.get("limit") or 20),
                    )
                }
            )
        if action == "outbox":
            return _serialize(
                {
                    "assignments": client.handoff_outbox(
                        actor_id=args.get("actor_id"),
                        source_id=args.get("source_id"),
                        include_completed=bool(args.get("include_completed", True)),
                        limit=int(args.get("limit") or 20),
                    )
                }
            )
        if action == "board":
            return _serialize(
                client.handoff_board(
                    actor_id=args.get("actor_id"),
                    source_id=args.get("source_id"),
                    stale_after=str(args.get("stale_after") or "1h"),
                    include_completed=bool(args.get("include_completed", False)),
                    limit=int(args.get("limit") or 50),
                )
            )
        handoff_id = args.get("handoff_id")
        if not handoff_id:
            raise ValueError(f"handoff_id required for action={action}")
        if action == "show":
            return _serialize(client.handoff_show(handoff_id))
        if action == "heartbeat":
            return _serialize(
                client.handoff_heartbeat(handoff_id, actor_id=args.get("actor_id"))
            )
        if action == "accept":
            return _serialize(client.handoff_accept(handoff_id, actor_id=args.get("actor_id")))
        if action == "status":
            return _serialize(client.handoff_status(handoff_id, actor_id=args.get("actor_id")))
        if action == "return":
            result_fact_id = args.get("result_fact_id")
            outcome = args.get("outcome")
            if not result_fact_id or not outcome:
                raise ValueError("result_fact_id and outcome required for action=return")
            return _serialize(
                client.handoff_return(
                    handoff_id,
                    result_fact_id,
                    outcome=outcome,
                    actor_id=args.get("actor_id"),
                )
            )
        if action == "complete":
            return _serialize(client.handoff_complete(handoff_id, actor_id=args.get("actor_id")))
        raise ValueError(f"unknown handoff inbox action: {action}")

    def _aggregate(args: dict, **_: Any) -> str:
        return _serialize(
            client.aggregate(
                str(args.get("query") or ""),
                op=str(args.get("op") or "count"),
                limit=int(args.get("limit") or 50),
                fact_type=args.get("fact_type"),
                entity=args.get("entity"),
                since=args.get("since"),
                source_id=args.get("source_id"),
                current_only=bool(args.get("current_only", True)),
                preview_chars=int(args.get("preview_chars") or 120),
                relevance_threshold=(
                    float(args["relevance_threshold"])
                    if args.get("relevance_threshold") is not None
                    else None
                ),
            )
        )

    def _entity_list(args: dict, **_: Any) -> str:
        return _serialize(
            client.entity_list(
                limit=int(args.get("limit") or 50),
                source_id=args.get("source_id"),
            )
        )

    def _traverse(args: dict, **_: Any) -> str:
        return _serialize(
            client.traverse(
                str(args.get("entity") or ""),
                depth=int(args.get("depth") or 2),
                source_id=args.get("source_id"),
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
                    fact_type=args.get("fact_type"),
                    tags=tags,
                    source_id=args.get("source_id"),
                    actor_id=args.get("actor_id"),
                    session_id=args.get("session_id"),
                )
            }
        )

    def _fact_add_many(args: dict, **_: Any) -> str:
        items = args.get("items") or []
        if not isinstance(items, list):
            items = []
        source_id = args.get("source_id")
        actor_id = args.get("actor_id")
        session_id = args.get("session_id")
        normalised = []
        for item in items:
            if not isinstance(item, dict):
                continue
            row = dict(item)
            if source_id is not None and not row.get("source_id"):
                row["source_id"] = source_id
            if actor_id is not None and not row.get("actor_id"):
                row["actor_id"] = actor_id
            if session_id is not None and not row.get("session_id"):
                row["session_id"] = session_id
            normalised.append(row)
        ids = client.fact_add_many(normalised)
        return _serialize({"count": len(ids), "ids": ids})

    def _doctor(args: dict, **_: Any) -> str:
        return _serialize(client.doctor())

    def _lint(args: dict, **_: Any) -> str:
        return _serialize(client.lint())

    def _schema(name: str, description: str, parameters: dict) -> dict:
        return {"name": name, "description": description, "parameters": parameters}

    return [
        (
            "aidememo_workflow_start",
            _schema(
                "aidememo_workflow_start",
                "Start an issue/PR/ticket-driven coding workflow. Creates a tracked session, stores the trigger, and returns project memory: relevant decisions, lessons, errors, and search hits. Use before planning when the user gives a sparse issue or automation trigger.",
                {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Issue / PR / ticket title or short workflow trigger text."},
                        "body": {"type": "string", "description": "Optional issue / PR / ticket body."},
                        "source": {"type": "string", "description": "Optional upstream source id, e.g. github:org/repo#123 or linear:ENG-42."},
                        "source_id": {"type": "string", "description": "Optional project or upstream namespace for shared-store scoping. If omitted, Hermes falls back to plugins.aidememo.source_id or AIDEMEMO_SOURCE_ID when set."},
                        "actor_id": {"type": "string", "description": "Optional writer identity, such as hermes:account-a. Falls back to plugins.aidememo.actor_id or AIDEMEMO_ACTOR_ID."},
                        "parent_session_id": {"type": "string", "description": "Optional prior workflow session to link with continued_from."},
                        "limit": {"type": "integer", "description": "Max context search hits (default 8)."},
                        "depth": {"type": "integer", "description": "Graph traversal depth (default 2)."},
                        "recent_limit": {"type": "integer", "description": "Max recent facts attached to a resolved entity (default 5)."},
                    },
                    "required": ["title"],
                },
            ),
            _workflow_start,
            "Start a ticket/issue workflow and return the project-memory context pack.",
            "🚦",
        ),
        (
            "aidememo_handoff",
            _schema(
                "aidememo_handoff",
                "Create a structured evidence-linked handoff packet before work crosses to another coding-agent installation or external worker lane. Keeps the same session id, focus, done_when, resume state, grouped decisions/questions/risks, prompt-ready content, and fact ids for verification. Inside one Hermes Kanban board, keep claim/retry/status/completion in Kanban and use a read-only packet only when compact evidence is useful; do not dispatch a second AideMemo assignment for an internal profile transition.",
                {
                    "type": "object",
                    "properties": {
                        "session": {"type": "string", "description": "Tracked session id. Falls back to AIDEMEMO_SESSION_ID."},
                        "from_actor": {"type": "string", "description": "Sending account/installation alias. Falls back to AIDEMEMO_ACTOR_ID."},
                        "from": {"type": "string", "description": "Producing route shorthand AGENT[/PROFILE], e.g. hermes/coding."},
                        "to": {"type": "string", "description": "Receiving route shorthand AGENT[/PROFILE], e.g. hermes/reviewer."},
                        "from_agent": {"type": "string", "description": "Producing agent, e.g. hermes or codex."},
                        "from_profile": {"type": "string", "description": "Producing Hermes profile, e.g. coding."},
                        "to_agent": {"type": "string", "description": "Receiving agent, e.g. claude-code or hermes."},
                        "to_profile": {"type": "string", "description": "Receiving profile, e.g. reviewer."},
                        "to_actor": {"type": "string", "description": "Receiving account/installation alias. Required when dispatch=true."},
                        "focus": {"type": "string", "description": "Next objective for the receiver."},
                        "done_when": {"type": "string", "description": "Observable completion condition before the receiver returns the workflow."},
                        "kanban_task": {"type": "string", "description": "Upstream Hermes Kanban task. Usually inferred from HERMES_KANBAN_TASK."},
                        "kanban_board": {"type": "string", "description": "Upstream Hermes Kanban board. Usually inferred from HERMES_KANBAN_BOARD."},
                        "dispatch": {"type": "boolean", "description": "Persist a pull-based assignment pointer; default false keeps read-only preview."},
                        "source_id": {"type": "string", "description": "Shared memory namespace. Unlike profile names, this scopes retrieval and falls back to plugin config or AIDEMEMO_SOURCE_ID."},
                        "limit": {"type": "integer", "description": "Max session facts (default 40)."},
                        "include_superseded": {"type": "boolean", "description": "Include superseded evidence for audit replay."},
                    },
                },
            ),
            _handoff,
            "Bridge a tracked workflow across agent installations with structured resume state and evidence intact.",
            "🤝",
        ),
        (
            "aidememo_handoff_inbox",
            _schema(
                "aidememo_handoff_inbox",
                "Pull, inspect, heartbeat, or return cross-installation session assignments. The board action derives a minimal view for runtimes without Kanban; Hermes Kanban remains the canonical card lifecycle.",
                {
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["list", "outbox", "show", "heartbeat", "board", "status", "accept", "return", "complete"], "default": "list"},
                        "actor_id": {"type": "string", "description": "Current account/installation alias. Falls back to AIDEMEMO_ACTOR_ID."},
                        "handoff_id": {"type": "string", "description": "Required for show, status, accept, return, or complete. show does not require actor_id."},
                        "result_fact_id": {"type": "string", "description": "Persisted worker result/error fact for action=return."},
                        "outcome": {"type": "string", "enum": ["succeeded", "failed"]},
                        "source_id": {"type": "string"},
                        "stale_after": {"type": "string", "description": "For board, inactivity threshold (default 1h)."},
                        "include_completed": {"type": "boolean"},
                        "limit": {"type": "integer"},
                    },
                },
            ),
            _handoff_inbox,
            "Pull or acknowledge assignments for this agent installation.",
            "📥",
        ),
        (
            "aidememo_context",
            _schema(
                "aidememo_context",
                "Top-of-turn context envelope: pinned facts, personalisation, recent activity, and optional topic search/lessons/errors. Use as Hermes' broad opening read before planning a normal coding/research turn.",
                {
                    "type": "object",
                    "properties": {
                        "topic": {"type": "string", "description": "Optional topic for focused context."},
                        "limit": {"type": "integer", "description": "Topic search hit limit (default 10)."},
                        "pinned_limit": {"type": "integer", "description": "Pinned fact limit (default 10)."},
                        "recent_limit": {"type": "integer", "description": "Recent fact limit (default 10)."},
                        "recent_days": {"type": "integer", "description": "Recent window in days (default 7)."},
                        "depth": {"type": "integer", "description": "Graph traversal depth for topic context (default 2)."},
                        "format": {"type": "string", "description": "full JSON or text summary. Default full."},
                        "max_chars": {"type": "integer", "description": "Optional hard cap for text output."},
                        "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id filter. If omitted, Hermes falls back to plugins.aidememo.source_id or AIDEMEMO_SOURCE_ID when set."},
                    },
                },
            ),
            _context,
            "Top-of-turn context envelope for Hermes: pinned, personalisation, recent, and optional topic context.",
            "🧭",
        ),
        (
            "aidememo_query",
            _schema(
                "aidememo_query",
                "One-shot context fetch: search + traverse + recent for a topic. Prefer over chaining aidememo_search/aidememo_traverse/aidememo_recent.",
                {
                    "type": "object",
                    "properties": {
                        "topic": {"type": "string", "description": "The entity name or topic to gather context on."},
                        "limit": {"type": "integer", "description": "Top-N search hits (default 5)."},
                        "depth": {"type": "integer", "description": "Graph traversal depth (default 2)."},
                        "recent_limit": {"type": "integer", "description": "How many recent facts to include (default 5)."},
                        "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id filter. If omitted, Hermes falls back to plugins.aidememo.source_id or AIDEMEMO_SOURCE_ID when set."},
                    },
                    "required": ["topic"],
                },
            ),
            _query,
            "One-shot context fetch: search + traverse + recent for a topic. Prefer over chaining aidememo_search/aidememo_traverse/aidememo_recent.",
            "🧠",
        ),
        (
            "aidememo_search",
            _schema(
                "aidememo_search",
                "Hybrid BM25 + semantic search across all facts. Use when the user asks about a concept rather than a specific entity.",
                {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Free-text query (BM25 + semantic vectors)."},
                        "limit": {"type": "integer", "description": "Default 10."},
                        "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id filter. If omitted, Hermes falls back to plugins.aidememo.source_id or AIDEMEMO_SOURCE_ID when set."},
                    },
                    "required": ["query"],
                },
            ),
            _search,
            "Hybrid BM25 + semantic search across all facts. Use when the user asks about a concept rather than a specific entity.",
            "🔍",
        ),
        (
            "aidememo_recent",
            _schema(
                "aidememo_recent",
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
            "aidememo_aggregate",
            _schema(
                "aidememo_aggregate",
                "Deterministic count/sum/timeline over matching facts. Call for 'how many', 'how much total', 'distinct days', or chronological event questions; do not call for simple recall.",
                {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Fact search query to aggregate over."},
                        "op": {"type": "string", "description": "count | enumerate | by_entity | sum_currency | sum_duration | count_distinct_dates | timeline. Default count."},
                        "limit": {"type": "integer", "description": "Max facts considered (default 50)."},
                        "fact_type": {"type": "string", "description": "Optional fact type filter, e.g. decision or lesson."},
                        "entity": {"type": "string", "description": "Optional entity name filter."},
                        "since": {"type": "string", "description": "Optional lower time bound (YYYY-MM-DD or RFC3339)."},
                        "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id filter. If omitted, Hermes falls back to plugins.aidememo.source_id or AIDEMEMO_SOURCE_ID when set."},
                        "current_only": {"type": "boolean", "description": "Exclude superseded facts by default."},
                        "relevance_threshold": {"type": "number", "description": "Optional semantic relevance cutoff for structured value ops."},
                    },
                    "required": ["query"],
                },
            ),
            _aggregate,
            "Exact aggregation over AideMemo facts for counts, sums, dates, and timelines.",
            "🧮",
        ),
        (
            "aidememo_entity_list",
            _schema(
                "aidememo_entity_list",
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
            "aidememo_traverse",
            _schema(
                "aidememo_traverse",
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
            "aidememo_fact_add",
            _schema(
                "aidememo_fact_add",
                "Append a fact to the wiki. Pick fact_type when you know it; if omitted, aidememo infers strong preference/lesson/error/decision/convention cues and returns type metadata. Pass session_id from the workflow or Kanban parent handoff so later workers recover the same evidence thread.",
                {
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "The fact text - short, declarative."},
                        "entities": {"type": "array", "items": {"type": "string"}, "description": "Entity names this fact links to."},
                        "fact_type": {"type": "string", "description": "decision | pattern | convention | claim | note | question | preference | lesson | error. Omit to let strong cues infer the type; explicit note is preserved."},
                        "tags": {"type": "array", "items": {"type": "string"}},
                        "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id. If omitted, Hermes falls back to plugins.aidememo.source_id or AIDEMEMO_SOURCE_ID when set."},
                        "actor_id": {"type": "string", "description": "Optional writer identity. Falls back to plugins.aidememo.actor_id or AIDEMEMO_ACTOR_ID."},
                        "session_id": {"type": "string", "description": "Optional tracked workflow session id. In a Kanban worker, reuse the id carried by the card or parent handoff; Kanban remains the task lifecycle owner."},
                    },
                    "required": ["content"],
                },
            ),
            _fact_add,
            "Append a typed fact to the wiki; omitted fact_type uses strong-cue inference.",
            "📝",
        ),
        (
            "aidememo_fact_add_many",
            _schema(
                "aidememo_fact_add_many",
                "Append many facts in one batch. Prefer already-classified decisions, lessons, errors, and preferences; omitted fact_type uses strong-cue inference and returns fact_type_source.",
                {
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "content": {"type": "string"},
                                    "entities": {"type": "array", "items": {"type": "string"}},
                                    "fact_type": {"type": "string"},
                                    "tags": {"type": "array", "items": {"type": "string"}},
                                    "confidence": {"type": "number"},
                                    "source_id": {"type": "string"},
                                    "actor_id": {"type": "string"},
                                    "session_id": {"type": "string"},
                                },
                                "required": ["content"],
                            },
                        },
                        "source_id": {"type": "string", "description": "Default source namespace for items that omit source_id."},
                        "actor_id": {"type": "string", "description": "Default writer identity for items that omit actor_id."},
                        "session_id": {"type": "string", "description": "Workflow session id returned by aidememo_workflow_start; applies to items that omit session_id."},
                    },
                    "required": ["items"],
                },
            ),
            _fact_add_many,
            "Batch append classified facts from Hermes self-extraction.",
            "📦",
        ),
        (
            "aidememo_doctor",
            _schema(
                "aidememo_doctor",
                "Health and setup diagnostics for the AideMemo store and Hermes integration. Use when memory reads/writes fail, source scoping looks wrong, or shared-store lock contention appears.",
                {"type": "object", "properties": {}},
            ),
            _doctor,
            "Health and setup diagnostics for aidememo/Hermes.",
            "🩺",
        ),
        (
            "aidememo_lint",
            _schema(
                "aidememo_lint",
                "Deprecated raw graph lint. Prefer aidememo_doctor for setup, sharing, and graph health guidance.",
                {"type": "object", "properties": {}},
            ),
            _lint,
            "Deprecated raw graph lint. Prefer aidememo_doctor.",
            "🩺",
        ),
    ]


def register_all(ctx: Any, client: AideMemoClient) -> None:
    """Register every aidememo tool with the given Hermes plugin context."""
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
