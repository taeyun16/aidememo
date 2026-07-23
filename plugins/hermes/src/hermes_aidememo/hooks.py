"""Lifecycle hooks.

Two hooks, both verified against Hermes 0.11's actual fire sites
in ``run_agent.py``:

``pre_llm_call`` — fires before every LLM API call. Hermes passes
``is_first_turn`` in kwargs and *uses our return value* as injected
context: any ``str`` we return (or a dict with a ``"context"``
key) is appended to the user's message before the model sees it.
Returning the recent-facts preamble on the first turn gets the
model loaded with aidememo state without a tool call.

This replaces the earlier ``on_session_start`` design. That hook
fires too — Hermes calls it at session creation — but it doesn't
give plugins a way to influence the system prompt or the first
user message; ``inject_message`` only works in interactive CLI
mode (an idle ``HermesCLI`` queue), so the preamble silently
disappeared in ``hermes chat -q``, gateway, and TUI sessions.

``post_llm_call`` — fires after every LLM API call with
``user_message`` and ``assistant_response`` as kwargs. When the
operator explicitly enables the capture adapter, we run the decision
detector on each turn's text and either queue matches to the pending
review log (safe default) or write directly when configured. Per-turn
firing keeps candidate captures close to the relevant context, but the
default remains disabled so explicit ``fact_add`` / SDK / MCP writes
stay canonical.

Both hooks degrade gracefully — any exception is logged but never
propagates back into Hermes's agent loop.
"""

from __future__ import annotations

import json
import logging
import os
import re
from pathlib import Path
from typing import Any

from . import capture_adapter
from .client import CLIENT_ERRORS, AideMemoClient

log = logging.getLogger("hermes_aidememo")


_SOURCE_ID_QUOTED_RE = re.compile(
    r"\bsource[_ -]?id\b.{0,80}?[\"'`]([^\"'`]+)[\"'`]",
    re.IGNORECASE | re.DOTALL,
)
_SOURCE_ID_RE = re.compile(
    r"\bsource[_ -]?id\b\s*[:=]\s*([A-Za-z0-9_.:/#-]+)",
    re.IGNORECASE,
)
_TICKET_RE = re.compile(r"\b(issue|ticket|pr|pull request|automation trigger)\b", re.IGNORECASE)


def _looks_like_workflow_trigger(message: str) -> bool:
    return bool(_TICKET_RE.search(message))


def _extract_source_id(message: str) -> str | None:
    match = _SOURCE_ID_QUOTED_RE.search(message) or _SOURCE_ID_RE.search(message)
    return match.group(1) if match else None


def _extract_title(message: str) -> str:
    for line in message.splitlines():
        stripped = line.strip()
        if stripped:
            return stripped[:160]
    return "Hermes workflow"


def _format_workflow_pack(pack: dict[str, Any]) -> str:
    lines = [
        "## AideMemo - workflow context pack",
        "",
        "`aidememo_workflow_start` was run automatically for this sparse "
        "ticket. Base the plan on the scoped prior memory below.",
        "",
    ]
    sections = [
        ("Relevant decisions", pack.get("relevant_decisions") or []),
        ("Prior lessons", pack.get("prior_lessons") or []),
        ("Prior errors", pack.get("prior_errors") or []),
    ]
    for heading, facts in sections:
        if not facts:
            continue
        lines.extend([f"### {heading}", ""])
        for fact in facts[:5]:
            content = str(fact.get("content") or "").strip()
            if content:
                lines.append(f"- {content}")
        lines.append("")
    lines.extend(
        [
            "Raw context pack:",
            json.dumps(pack, ensure_ascii=False, default=str)[:6000],
        ]
    )
    return "\n".join(lines).strip()


def _format_recent_block(
    facts: list[dict],
    *,
    kanban_task: str | None = None,
    kanban_board: str | None = None,
) -> str:
    """Render a compact, model-friendly preamble."""
    lines = [
        "## AideMemo - workflow memory",
        "",
        "Auto-loaded by the AideMemo Hermes plugin.",
        "",
    ]
    if kanban_task:
        board = kanban_board or "<active board>"
        lines.extend(
            [
                f"Hermes Kanban worker detected: board `{board}`, task `{kanban_task}`.",
                "Kanban is the canonical owner of claim, dependency, retry, comment, "
                "and completion state. Use `kanban_show` / `kanban_comment` / "
                "`kanban_complete` for that lifecycle; do not create an AideMemo "
                "dispatch/inbox assignment for an internal Hermes profile transition.",
                "Use AideMemo for durable project decisions, lessons, errors, and "
                "fact-linked evidence. If the card or parent handoff carries an "
                "AideMemo session id, pass it to fact writes. Use AideMemo dispatch "
                "only when work crosses to an external agent installation such as "
                "Codex or Claude.",
                "",
            ]
        )
    else:
        lines.extend(
            [
                "When the user gives a sparse issue, ticket, PR, or automation "
                "trigger, call `aidememo_workflow_start` before making a plan. "
                "Pass any user-provided `source_id` through to the tool so "
                "neighbouring project memory does not leak in.",
                "",
            ]
        )
    if not facts:
        return "\n".join(lines).strip()

    lines.extend(
        [
            "Recent facts from your knowledge base are below; consult "
            "them before answering questions about prior decisions, "
            "conventions, or ongoing topics.",
            "",
        ]
    )
    for f in facts[:10]:
        content = (f.get("content") or "").strip()
        if not content:
            continue
        ftype = f.get("fact_type") or "note"
        lines.append(f"- ({ftype}) {content}")
    return "\n".join(lines)


def make_pre_llm_call(client: AideMemoClient, last: str = "7d", limit: int = 10):
    """Build the ``pre_llm_call`` callback.

    Returns a dict with a ``context`` key on the first turn so the
    recent-facts preamble lands in front of the user's message;
    returns ``None`` on every other turn so we don't keep
    re-injecting state the model already saw.
    """

    def pre_llm_call(**kwargs: Any) -> dict[str, str] | None:
        user_message = str(kwargs.get("user_message") or "")
        kanban_task = (os.environ.get("HERMES_KANBAN_TASK") or "").strip() or None
        kanban_board = (os.environ.get("HERMES_KANBAN_BOARD") or "").strip() or None
        if _looks_like_workflow_trigger(user_message) and not kanban_task:
            try:
                pack = client.workflow_start(
                    _extract_title(user_message),
                    body=user_message,
                    source_id=_extract_source_id(user_message),
                )
            except CLIENT_ERRORS as exc:
                log.warning("aidememo workflow_start failed at pre_llm_call: %s", exc)
            else:
                log.info("aidememo workflow_start auto-context injected for sparse trigger")
                return {"context": _format_workflow_pack(pack)}

        if not kwargs.get("is_first_turn"):
            return None
        try:
            facts = client.recent(last=last, limit=limit)
        except CLIENT_ERRORS as exc:
            log.warning("aidememo recent failed at pre_llm_call: %s", exc)
            return None
        block = _format_recent_block(
            facts,
            kanban_task=kanban_task,
            kanban_board=kanban_board,
        )
        return {"context": block}

    return pre_llm_call


def make_post_llm_call(
    client: AideMemoClient,
    *,
    enable_auto_record: bool = False,
    dry_run: bool = False,
    confidence_floor: float = 0.85,
    default_entities: list[str] | None = None,
    pending_path: Path | None = None,
    detect_in: str = "both",
    capture_config: capture_adapter.CaptureConfig | None = None,
):
    """Build the ``post_llm_call`` callback.

    Runs the decision detector against the just-finished turn's text only when
    capture is explicitly enabled. The safe mode appends matches to the pending
    review log; direct writes require explicit opt-in.

    ``detect_in`` controls which side of the turn is scanned:

      - ``"both"`` (default) — user_message + assistant_response.
        Highest recall: catches both user-typed decisions and
        agent-confirmed ones (e.g. "should we go with X?" → "let's
        go with X"). Detector dedup collapses superficial echo
        differences (markdown formatting, trailing punctuation).
      - ``"user"`` — user_message only. Highest precision: every
        record is something the user explicitly typed. Skips agent
        confirmations entirely.
      - ``"assistant"`` — assistant_response only. Captures decisions
        the agent commits to but the user merely prompted for.
    """

    if capture_config is None:
        capture_config = capture_adapter.CaptureConfig(
            enabled=enable_auto_record,
            mode="pending" if dry_run else "direct",
            provider="hermes",
            confidence_floor=confidence_floor,
            detect_in=detect_in,
            default_entities=default_entities,
            pending_path=pending_path,
        )

    def post_llm_call(**kwargs: Any) -> None:
        try:
            result = capture_adapter.capture_from_payload(client, kwargs, capture_config)
        except (OSError, RuntimeError) as exc:
            log.warning("aidememo auto-capture adapter failed: %s", exc)
            return

        if result.queued:
            path = result.pending_path or str(pending_path or "pending log")
            log.info(
                "aidememo auto-capture queued %d detection(s) to %s; review with `/aidememo-pending`",
                result.queued,
                path,
            )
            return

        if result.recorded:
            log.info("aidememo auto-capture recorded %d detection(s)", result.recorded)

    return post_llm_call


# Back-compat shim: the older `make_on_session_end` API is still
# referenced by test_plugin.py's dry-run cases. The new hook lives
# at post_llm_call but the in-process behaviour (detect → append /
# fact_add) is identical, so we expose the same logic under the
# old name so existing tests keep their assertions meaningful.
def make_on_session_end(
    client: AideMemoClient,
    *,
    enable_auto_record: bool = False,
    dry_run: bool = False,
    confidence_floor: float = 0.85,
    default_entities: list[str] | None = None,
    pending_path: Path | None = None,
):
    """Compat wrapper around :func:`make_post_llm_call` for tests
    that still pass ``transcript=...`` directly to the callback."""
    inner = make_post_llm_call(
        client,
        enable_auto_record=enable_auto_record,
        dry_run=dry_run,
        confidence_floor=confidence_floor,
        default_entities=default_entities,
        pending_path=pending_path,
    )

    def on_session_end(
        ctx: Any = None, transcript: str | None = None, **kwargs: Any
    ) -> None:
        # Old tests passed a single `transcript` string; map it onto
        # the new kwarg shape so detection still happens.
        if transcript:
            inner(user_message=transcript, assistant_response="")
        else:
            inner(**kwargs)

    return on_session_end


def register_all(ctx: Any, client: AideMemoClient, config: dict | None = None) -> None:
    cfg = config or {}
    last = cfg.get("recent_window", "7d")
    limit = int(cfg.get("recent_limit", 10))
    capture_config = capture_adapter.config_from_plugin(cfg, provider="hermes")

    ctx.register_hook(
        "pre_llm_call",
        make_pre_llm_call(client, last=last, limit=limit),
    )
    ctx.register_hook(
        "post_llm_call",
        make_post_llm_call(
            client,
            capture_config=capture_config,
        ),
    )
