"""Slash commands exposed to chat sessions.

``/wg <topic>`` — one-shot context fetch (search + traverse + recent).
``/wg-add <content>`` — append a quick fact (defaults to type=note).
``/wg-recent`` — last 7 days of facts.
``/wg-pending`` — review or commit the dry-run pending log.

Handlers receive the raw argument string (everything after the command
word) and return a string which Hermes renders as assistant output.
"""

from __future__ import annotations

import shlex
from typing import Any

from hermes_wg import pending
from hermes_wg.client import CLIENT_ERRORS, WgClient
from hermes_wg.tools import to_pretty_json


def _wg_handler(client: WgClient):
    def handle(raw_args: str) -> str:
        topic = raw_args.strip()
        if not topic:
            return "Usage: /wg <topic>  — gathers search + traverse + recent for the topic."
        try:
            ctx = client.query(topic, limit=5, depth=2, recent_limit=5)
        except CLIENT_ERRORS as exc:
            return f"wg_query failed: {exc}"
        return to_pretty_json(ctx)

    return handle


def _wg_add_handler(client: WgClient):
    def handle(raw_args: str) -> str:
        if not raw_args.strip():
            return (
                "Usage: /wg-add <content> [--entities A,B] [--type decision|note|...]\n"
                "Example: /wg-add \"HNSW is the default index\" --entities wg --type decision"
            )
        content, entities, fact_type, tags = _parse_add_args(raw_args)
        try:
            fid = client.fact_add(
                content, entities=entities, fact_type=fact_type, tags=tags
            )
        except CLIENT_ERRORS as exc:
            return f"wg_fact_add failed: {exc}"
        link = ", ".join(entities) if entities else "(no entities)"
        return f"Recorded {fid}  — type={fact_type}, entities={link}"

    return handle


def _wg_recent_handler(client: WgClient):
    def handle(raw_args: str) -> str:
        last = raw_args.strip() or "7d"
        try:
            facts = client.recent(last=last, limit=10)
        except CLIENT_ERRORS as exc:
            return f"wg_recent failed: {exc}"
        if not facts:
            return f"No facts in the last {last}."
        lines = [f"Recent facts (last {last}):"]
        for f in facts:
            content = (f.get("content") or "").strip()
            ftype = f.get("fact_type") or "note"
            lines.append(f"  - [{ftype}] {content}")
        return "\n".join(lines)

    return handle


def _parse_add_args(raw: str) -> tuple[str, list[str] | None, str, list[str]]:
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
        content_parts.append(t)
        i += 1
    content = " ".join(content_parts).strip()
    return content, entities, fact_type, tags


def _format_pending_list(entries: list[pending.PendingEntry]) -> str:
    if not entries:
        return "No pending detections. Run a session with `dry_run: true` to populate this log."
    lines = [f"{len(entries)} pending detection(s):"]
    for e in entries:
        lines.append(f"  #{e.idx}  [{e.fact_type}, {e.confidence:.2f}]  {e.content}")
    lines.append("")
    lines.append("Commands: `/wg-pending commit all|<N>` or `/wg-pending clear all|<N>`")
    return "\n".join(lines)


def _wg_pending_handler(client: WgClient):
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
            "  /wg-pending                 — list pending detections\n"
            "  /wg-pending commit all      — commit every entry to wg\n"
            "  /wg-pending commit <N>      — commit a single entry by index\n"
            "  /wg-pending clear all       — discard every entry\n"
            "  /wg-pending clear <N>       — discard a single entry"
        )

    return handle


def _handle_commit(client: WgClient, target: str) -> str:
    if not target:
        return "Usage: /wg-pending commit all|<N>"
    if target == "all":
        committed, leftover = pending.commit_all(client)
        if committed == 0 and not leftover:
            return "No pending detections to commit."
        if leftover:
            return (
                f"Committed {committed} fact(s) to wg; {len(leftover)} entry(ies) "
                "failed and remain in the pending log — run `/wg-pending` to review."
            )
        return f"Committed {committed} fact(s) to wg; pending log cleared."
    try:
        idx = int(target)
    except ValueError:
        return f"Invalid index `{target}` — expected `all` or a number."
    try:
        entry = pending.commit_one(client, idx)
    except IndexError as exc:
        return f"{exc}. Run `/wg-pending` to see valid indices."
    except CLIENT_ERRORS as exc:
        return f"wg fact_add failed for #{idx}; entry kept in pending log: {exc}"
    return f"Committed #{idx} ([{entry.fact_type}]) to wg."


def _handle_clear(target: str) -> str:
    if not target:
        return "Usage: /wg-pending clear all|<N>"
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
        return f"{exc}. Run `/wg-pending` to see valid indices."
    return f"Discarded #{idx} ([{entry.fact_type}]) without writing to wg."


def register_all(ctx: Any, client: WgClient) -> None:
    ctx.register_command(
        name="wg",
        handler=_wg_handler(client),
        description="One-shot wg context fetch (search + graph + recent).",
        args_hint="<topic>",
    )
    ctx.register_command(
        name="wg-add",
        handler=_wg_add_handler(client),
        description="Append a fact to the wg knowledge graph.",
        args_hint="<content> [--entities A,B] [--type decision]",
    )
    ctx.register_command(
        name="wg-recent",
        handler=_wg_recent_handler(client),
        description="Show recent wg facts (default last 7 days).",
        args_hint="[7d|24h|30d]",
    )
    ctx.register_command(
        name="wg-pending",
        handler=_wg_pending_handler(client),
        description="Review / commit / clear the dry-run pending detections log.",
        args_hint="[commit all|N] [clear all|N]",
    )
