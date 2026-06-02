"""Slash commands exposed to chat sessions.

``/aidememo <topic>`` — one-shot context fetch (search + traverse + recent).
``/aidememo-context [topic]`` — full top-of-turn context envelope.
``/aidememo-aggregate <query>`` — exact count / sum / timeline over matching facts.
``/aidememo-start <title>`` — start an issue/ticket workflow and return context.
``/aidememo-add <content>`` — append a quick fact (defaults to type=note).
``/aidememo-recent`` — last 7 days of facts.
``/aidememo-doctor`` — setup / sharing / graph health diagnostics.
``/aidememo-pending`` — review or commit the dry-run pending log.

Handlers receive the raw argument string (everything after the command
word) and return a string which Hermes renders as assistant output.
"""

from __future__ import annotations

import shlex
from typing import Any

from . import pending
from .client import CLIENT_ERRORS, AideMemoClient
from .tools import to_pretty_json


def _aidememo_handler(client: AideMemoClient):
    def handle(raw_args: str) -> str:
        topic = raw_args.strip()
        if not topic:
            return "Usage: /aidememo <topic> [--source-id ID]  — gathers search + traverse + recent for the topic."
        topic, source_id = _parse_topic_args(raw_args)
        try:
            ctx = client.query(topic, limit=5, depth=2, recent_limit=5, source_id=source_id)
        except CLIENT_ERRORS as exc:
            return f"aidememo_query failed: {exc}"
        return to_pretty_json(ctx)

    return handle


def _aidememo_context_handler(client: AideMemoClient):
    def handle(raw_args: str) -> str:
        topic, source_id = _parse_topic_args(raw_args)
        try:
            ctx = client.context(
                topic=topic or None,
                limit=8,
                recent_limit=8,
                source_id=source_id,
                format="text",
                max_chars=6000,
            )
        except CLIENT_ERRORS as exc:
            return f"aidememo_context failed: {exc}"
        return ctx if isinstance(ctx, str) else to_pretty_json(ctx)

    return handle


def _aidememo_aggregate_handler(client: AideMemoClient):
    def handle(raw_args: str) -> str:
        query, op, fact_type, entity, source_id = _parse_aggregate_args(raw_args)
        if not query:
            return (
                "Usage: /aidememo-aggregate <query> [--op count|enumerate|by_entity|sum_currency|sum_duration|count_distinct_dates|timeline] "
                "[--type decision] [--entity Redis] [--source-id ID]"
            )
        try:
            result = client.aggregate(query, op=op, fact_type=fact_type, entity=entity, source_id=source_id)
        except CLIENT_ERRORS as exc:
            return f"aidememo_aggregate failed: {exc}"
        return to_pretty_json(result)

    return handle


def _aidememo_add_handler(client: AideMemoClient):
    def handle(raw_args: str) -> str:
        if not raw_args.strip():
            return (
                "Usage: /aidememo-add <content> [--entities A,B] [--type decision|note|...] [--source-id ID]\n"
                "Example: /aidememo-add \"HNSW is the default index\" --entities aidememo --type decision"
            )
        content, entities, fact_type, tags, source_id = _parse_add_args(raw_args)
        try:
            fid = client.fact_add(
                content,
                entities=entities,
                fact_type=fact_type,
                tags=tags,
                source_id=source_id,
            )
        except CLIENT_ERRORS as exc:
            return f"aidememo_fact_add failed: {exc}"
        link = ", ".join(entities) if entities else "(no entities)"
        return f"Recorded {fid}  — type={fact_type}, entities={link}"

    return handle


def _aidememo_start_handler(client: AideMemoClient):
    def handle(raw_args: str) -> str:
        title, body, source, source_id = _parse_start_args(raw_args)
        if not title:
            return (
                "Usage: /aidememo-start <title> [--body TEXT] [--source github:org/repo#123] [--source-id ID]\n"
                "Example: /aidememo-start \"Fix Redis timeout in worker\" --source github:org/repo#123"
            )
        try:
            pack = client.workflow_start(
                title,
                body=body,
                source=source,
                source_id=source_id,
            )
        except CLIENT_ERRORS as exc:
            return f"aidememo_workflow_start failed: {exc}"
        return to_pretty_json(pack)

    return handle


def _aidememo_recent_handler(client: AideMemoClient):
    def handle(raw_args: str) -> str:
        last = raw_args.strip() or "7d"
        try:
            facts = client.recent(last=last, limit=10)
        except CLIENT_ERRORS as exc:
            return f"aidememo_recent failed: {exc}"
        if not facts:
            return f"No facts in the last {last}."
        lines = [f"Recent facts (last {last}):"]
        for f in facts:
            content = (f.get("content") or "").strip()
            ftype = f.get("fact_type") or "note"
            lines.append(f"  - [{ftype}] {content}")
        return "\n".join(lines)

    return handle


def _aidememo_doctor_handler(client: AideMemoClient):
    def handle(raw_args: str) -> str:
        try:
            return to_pretty_json(client.doctor())
        except CLIENT_ERRORS as exc:
            return f"aidememo_doctor failed: {exc}"

    return handle


def _parse_topic_args(raw: str) -> tuple[str, str | None]:
    try:
        tokens = shlex.split(raw)
    except ValueError:
        tokens = raw.split()
    topic_parts: list[str] = []
    source_id: str | None = None
    i = 0
    while i < len(tokens):
        t = tokens[i]
        if t == "--source-id" and i + 1 < len(tokens):
            source_id = tokens[i + 1]
            i += 2
            continue
        topic_parts.append(t)
        i += 1
    return " ".join(topic_parts).strip(), source_id


def _parse_add_args(raw: str) -> tuple[str, list[str] | None, str, list[str], str | None]:
    """Pull ``--entities``, ``--type``, ``--tag`` flags out of the raw
    arg string and return ``(content, entities, fact_type, tags)``.
    Anything not consumed by a flag is treated as the fact content.
    """
    try:
        tokens = shlex.split(raw)
    except ValueError:
        # Unbalanced quotes — fall back to whitespace split.
        tokens = raw.split()
    content_parts: list[str] = []
    entities: list[str] | None = None
    fact_type = "note"
    tags: list[str] = []
    source_id: str | None = None
    i = 0
    while i < len(tokens):
        t = tokens[i]
        if t == "--entities" and i + 1 < len(tokens):
            entities = [s.strip() for s in tokens[i + 1].split(",") if s.strip()]
            i += 2
            continue
        if t == "--type" and i + 1 < len(tokens):
            fact_type = tokens[i + 1]
            i += 2
            continue
        if t == "--tag" and i + 1 < len(tokens):
            tags.append(tokens[i + 1])
            i += 2
            continue
        if t == "--source-id" and i + 1 < len(tokens):
            source_id = tokens[i + 1]
            i += 2
            continue
        content_parts.append(t)
        i += 1
    content = " ".join(content_parts).strip()
    return content, entities, fact_type, tags, source_id


def _parse_aggregate_args(raw: str) -> tuple[str, str, str | None, str | None, str | None]:
    try:
        tokens = shlex.split(raw)
    except ValueError:
        tokens = raw.split()
    query_parts: list[str] = []
    op = "count"
    fact_type: str | None = None
    entity: str | None = None
    source_id: str | None = None
    i = 0
    while i < len(tokens):
        t = tokens[i]
        if t == "--op" and i + 1 < len(tokens):
            op = tokens[i + 1]
            i += 2
            continue
        if t == "--type" and i + 1 < len(tokens):
            fact_type = tokens[i + 1]
            i += 2
            continue
        if t == "--entity" and i + 1 < len(tokens):
            entity = tokens[i + 1]
            i += 2
            continue
        if t == "--source-id" and i + 1 < len(tokens):
            source_id = tokens[i + 1]
            i += 2
            continue
        query_parts.append(t)
        i += 1
    return " ".join(query_parts).strip(), op, fact_type, entity, source_id


def _parse_start_args(raw: str) -> tuple[str, str | None, str | None, str | None]:
    try:
        tokens = shlex.split(raw)
    except ValueError:
        tokens = raw.split()
    title_parts: list[str] = []
    body: str | None = None
    source: str | None = None
    source_id: str | None = None
    i = 0
    while i < len(tokens):
        t = tokens[i]
        if t == "--body" and i + 1 < len(tokens):
            body = tokens[i + 1]
            i += 2
            continue
        if t == "--source" and i + 1 < len(tokens):
            source = tokens[i + 1]
            i += 2
            continue
        if t == "--source-id" and i + 1 < len(tokens):
            source_id = tokens[i + 1]
            i += 2
            continue
        title_parts.append(t)
        i += 1
    return " ".join(title_parts).strip(), body, source, source_id


def _format_pending_list(entries: list[pending.PendingEntry]) -> str:
    if not entries:
        return "No pending detections. Run a session with `dry_run: true` to populate this log."
    lines = [f"{len(entries)} pending detection(s):"]
    for e in entries:
        lines.append(f"  #{e.idx}  [{e.fact_type}, {e.confidence:.2f}]  {e.content}")
    lines.append("")
    lines.append("Commands: `/aidememo-pending commit all|<N>` or `/aidememo-pending clear all|<N>`")
    return "\n".join(lines)


def _aidememo_pending_handler(client: AideMemoClient):
    def handle(raw_args: str) -> str:
        parts = raw_args.strip().split()
        if not parts:
            return _format_pending_list(pending.read())

        action = parts[0].lower()
        target = parts[1].lower() if len(parts) >= 2 else ""

        if action == "commit":
            return _handle_commit(client, target)
        if action == "clear":
            return _handle_clear(target)

        return (
            "Usage:\n"
            "  /aidememo-pending                 — list pending detections\n"
            "  /aidememo-pending commit all      — commit every entry to aidememo\n"
            "  /aidememo-pending commit <N>      — commit a single entry by index\n"
            "  /aidememo-pending clear all       — discard every entry\n"
            "  /aidememo-pending clear <N>       — discard a single entry"
        )

    return handle


def _handle_commit(client: AideMemoClient, target: str) -> str:
    if not target:
        return "Usage: /aidememo-pending commit all|<N>"
    if target == "all":
        committed, leftover = pending.commit_all(client)
        if committed == 0 and not leftover:
            return "No pending detections to commit."
        if leftover:
            return (
                f"Committed {committed} fact(s) to aidememo; {len(leftover)} entry(ies) "
                "failed and remain in the pending log — run `/aidememo-pending` to review."
            )
        return f"Committed {committed} fact(s) to aidememo; pending log cleared."
    try:
        idx = int(target)
    except ValueError:
        return f"Invalid index `{target}` — expected `all` or a number."
    try:
        entry = pending.commit_one(client, idx)
    except IndexError as exc:
        return f"{exc}. Run `/aidememo-pending` to see valid indices."
    except CLIENT_ERRORS as exc:
        return f"aidememo fact_add failed for #{idx}; entry kept in pending log: {exc}"
    return f"Committed #{idx} ([{entry.fact_type}]) to aidememo."


def _handle_clear(target: str) -> str:
    if not target:
        return "Usage: /aidememo-pending clear all|<N>"
    if target == "all":
        n = pending.clear_all()
        return f"Discarded {n} pending entry(ies)."
    try:
        idx = int(target)
    except ValueError:
        return f"Invalid index `{target}` — expected `all` or a number."
    try:
        entry = pending.clear_one(idx)
    except IndexError as exc:
        return f"{exc}. Run `/aidememo-pending` to see valid indices."
    return f"Discarded #{idx} ([{entry.fact_type}]) without writing to aidememo."


def register_all(ctx: Any, client: AideMemoClient) -> None:
    ctx.register_command(
        name="aidememo",
        handler=_aidememo_handler(client),
        description="One-shot aidememo context fetch (search + graph + recent).",
        args_hint="<topic> [--source-id ID]",
    )
    ctx.register_command(
        name="aidememo-context",
        handler=_aidememo_context_handler(client),
        description="Top-of-turn aidememo context envelope (pinned + personalisation + recent + optional topic).",
        args_hint="[topic] [--source-id ID]",
    )
    ctx.register_command(
        name="aidememo-aggregate",
        handler=_aidememo_aggregate_handler(client),
        description="Exact count / sum / timeline over matching aidememo facts.",
        args_hint="<query> [--op count] [--type decision] [--entity Redis]",
    )
    ctx.register_command(
        name="aidememo-start",
        handler=_aidememo_start_handler(client),
        description="Start an issue/ticket workflow and return aidememo project context.",
        args_hint="<title> [--body TEXT] [--source URL] [--source-id ID]",
    )
    ctx.register_command(
        name="aidememo-add",
        handler=_aidememo_add_handler(client),
        description="Append a fact to the aidememo knowledge graph.",
        args_hint="<content> [--entities A,B] [--type decision] [--source-id ID]",
    )
    ctx.register_command(
        name="aidememo-recent",
        handler=_aidememo_recent_handler(client),
        description="Show recent aidememo facts (default last 7 days).",
        args_hint="[7d|24h|30d]",
    )
    ctx.register_command(
        name="aidememo-doctor",
        handler=_aidememo_doctor_handler(client),
        description="Show aidememo/Hermes setup, sharing, and graph diagnostics.",
        args_hint="",
    )
    ctx.register_command(
        name="aidememo-pending",
        handler=_aidememo_pending_handler(client),
        description="Review / commit / clear the dry-run pending detections log.",
        args_hint="[commit all|N] [clear all|N]",
    )
