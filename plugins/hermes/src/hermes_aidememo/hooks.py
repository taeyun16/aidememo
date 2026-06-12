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
``user_message`` and ``assistant_response`` as kwargs. We run the
decision detector on each turn's text and either auto-record the
match (default) or append it to the dry-run pending log. Per-turn
firing is actually a stronger guarantee than the old session-end
design — captures land before the user pivots away from the
relevant context, and a session that gets killed mid-conversation
still has its earlier decisions persisted.

Both hooks degrade gracefully — any exception is logged but never
propagates back into Hermes's agent loop.
"""

from __future__ import annotations

import json
import logging
import re
from pathlib import Path
from typing import Any

from . import pending
from .client import CLIENT_ERRORS, AideMemoClient
from .decisions import detect

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


def _format_recent_block(facts: list[dict]) -> str:
    """Render a compact, model-friendly preamble."""
    lines = [
        "## AideMemo - workflow memory",
        "",
        "Auto-loaded by the AideMemo Hermes plugin. When the user gives a "
        "sparse issue, ticket, PR, or automation trigger, call "
        "`aidememo_workflow_start` before making a plan. Pass any user-provided "
        "`source_id` through to the tool so neighbouring project memory "
        "does not leak in.",
        "",
    ]
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
        if _looks_like_workflow_trigger(user_message):
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
        block = _format_recent_block(facts)
        return {"context": block}

    return pre_llm_call


def make_post_llm_call(
    client: AideMemoClient,
    *,
    enable_auto_record: bool = True,
    dry_run: bool = False,
    confidence_floor: float = 0.85,
    default_entities: list[str] | None = None,
    pending_path: Path | None = None,
    detect_in: str = "both",
):
    """Build the ``post_llm_call`` callback.

    Runs the decision detector against the just-finished turn's text
    and either auto-records each match as an AideMemo fact (the default) or
    appends it to ``pending_path`` for offline review when
    ``dry_run`` is on.

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

    valid_modes = {"both", "user", "assistant"}
    if detect_in not in valid_modes:
        log.warning(
            "aidememo detect_in=%r unknown — falling back to 'both' (valid: %s)",
            detect_in,
            sorted(valid_modes),
        )
        detect_in = "both"

    def post_llm_call(**kwargs: Any) -> None:
        user_message = str(kwargs.get("user_message") or "")
        if _looks_like_workflow_trigger(user_message):
            try:
                client.workflow_start(
                    _extract_title(user_message),
                    body=user_message,
                    source_id=_extract_source_id(user_message),
                )
            except CLIENT_ERRORS as exc:
                log.warning("aidememo workflow_start capture failed at post_llm_call: %s", exc)

        if not enable_auto_record:
            return
        text_chunks: list[str] = []
        wanted_keys = {
            "both": ("user_message", "assistant_response"),
            "user": ("user_message",),
            "assistant": ("assistant_response",),
        }[detect_in]
        for key in wanted_keys:
            value = kwargs.get(key)
            if isinstance(value, str) and value.strip():
                text_chunks.append(value)
        if not text_chunks:
            return

        transcript = "\n".join(text_chunks)
        detections = detect(transcript, confidence_floor=confidence_floor)
        if not detections:
            return

        if dry_run:
            try:
                path = pending.append(detections, pending_path)
            except OSError as exc:
                log.warning("aidememo dry-run could not write pending log: %s", exc)
                return
            log.info(
                "aidememo dry-run captured %d detection(s) (logged to %s - review with `/aidememo-pending`, then `/aidememo-pending commit all` or `clear all`)",
                len(detections),
                path,
            )
            for d in detections:
                log.info(
                    "  [%s, %.2f] %s",
                    d.fact_type,
                    d.confidence,
                    d.content,
                )
            return

        for d in detections:
            try:
                client.fact_add(
                    d.content,
                    entities=default_entities,
                    fact_type=d.fact_type,
                    tags=["auto-recorded", "hermes-session"],
                    # Forward the detector's per-pattern weight so the
                    # wiki sees the same confidence the auto-recorder
                    # used to gate the entry. Otherwise aidememo defaults to
                    # 0.5 and a 0.95-marker decision lands at 0.5 in
                    # search ranking — bug masquerading as feature.
                    confidence=d.confidence,
                )
            except CLIENT_ERRORS as exc:
                log.warning("aidememo fact_add (auto-record) failed: %s", exc)

    return post_llm_call


# Back-compat shim: the older `make_on_session_end` API is still
# referenced by test_plugin.py's dry-run cases. The new hook lives
# at post_llm_call but the in-process behaviour (detect → append /
# fact_add) is identical, so we expose the same logic under the
# old name so existing tests keep their assertions meaningful.
def make_on_session_end(
    client: AideMemoClient,
    *,
    enable_auto_record: bool = True,
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
    auto_record = bool(cfg.get("auto_record", True))
    dry_run = bool(cfg.get("dry_run", False))
    floor = float(cfg.get("confidence_floor", 0.85))
    default_entities = cfg.get("default_entities")
    pending_path_cfg = cfg.get("pending_log")
    pending_path = Path(pending_path_cfg) if pending_path_cfg else None
    detect_in = str(cfg.get("detect_in") or "both")

    ctx.register_hook(
        "pre_llm_call",
        make_pre_llm_call(client, last=last, limit=limit),
    )
    ctx.register_hook(
        "post_llm_call",
        make_post_llm_call(
            client,
            enable_auto_record=auto_record,
            dry_run=dry_run,
            confidence_floor=floor,
            default_entities=default_entities,
            pending_path=pending_path,
            detect_in=detect_in,
        ),
    )
