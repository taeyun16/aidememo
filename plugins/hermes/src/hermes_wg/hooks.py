"""Lifecycle hooks.

Two hooks, both verified against Hermes 0.11's actual fire sites
in ``run_agent.py``:

``pre_llm_call`` — fires before every LLM API call. Hermes passes
``is_first_turn`` in kwargs and *uses our return value* as injected
context: any ``str`` we return (or a dict with a ``"context"``
key) is appended to the user's message before the model sees it.
Returning the recent-facts preamble on the first turn gets the
model loaded with wg state without a tool call.

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

import logging
from pathlib import Path
from typing import Any

from . import pending
from .client import CLIENT_ERRORS, WgClient
from .decisions import detect

log = logging.getLogger("hermes_wg")


def _format_recent_block(facts: list[dict]) -> str | None:
    """Render a compact, model-friendly preamble. Returns ``None`` if
    there's nothing worth injecting (no recent facts)."""
    if not facts:
        return None
    lines = [
        "## wg - recent knowledge graph snapshot",
        "",
        "Auto-loaded by the wg Hermes plugin. The most recent facts "
        "from your knowledge base are below; consult them before "
        "answering questions about prior decisions, conventions, or "
        "ongoing topics.",
        "",
    ]
    for f in facts[:10]:
        content = (f.get("content") or "").strip()
        if not content:
            continue
        ftype = f.get("fact_type") or "note"
        lines.append(f"- ({ftype}) {content}")
    if len(lines) <= 4:  # only the header survived - nothing to show
        return None
    return "\n".join(lines)


def make_pre_llm_call(client: WgClient, last: str = "7d", limit: int = 10):
    """Build the ``pre_llm_call`` callback.

    Returns a dict with a ``context`` key on the first turn so the
    recent-facts preamble lands in front of the user's message;
    returns ``None`` on every other turn so we don't keep
    re-injecting state the model already saw.
    """

    def pre_llm_call(**kwargs: Any) -> dict[str, str] | None:
        if not kwargs.get("is_first_turn"):
            return None
        try:
            facts = client.recent(last=last, limit=limit)
        except CLIENT_ERRORS as exc:
            log.warning("wg recent failed at pre_llm_call: %s", exc)
            return None
        block = _format_recent_block(facts)
        if not block:
            return None
        return {"context": block}

    return pre_llm_call


def make_post_llm_call(
    client: WgClient,
    *,
    enable_auto_record: bool = True,
    dry_run: bool = False,
    confidence_floor: float = 0.85,
    default_entities: list[str] | None = None,
    pending_path: Path | None = None,
):
    """Build the ``post_llm_call`` callback.

    Runs the decision detector against the just-finished turn's text
    (``user_message`` + ``assistant_response``). Matches at or above
    ``confidence_floor`` get either auto-recorded as wg facts (the
    default) or appended to ``pending_path`` for offline review
    when ``dry_run`` is on.
    """

    def post_llm_call(**kwargs: Any) -> None:
        if not enable_auto_record:
            return
        text_chunks: list[str] = []
        for key in ("user_message", "assistant_response"):
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
                log.warning("wg dry-run could not write pending log: %s", exc)
                return
            log.info(
                "wg dry-run captured %d detection(s) (logged to %s - review with `/wg-pending`, then `/wg-pending commit all` or `clear all`)",
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
                )
            except CLIENT_ERRORS as exc:
                log.warning("wg fact_add (auto-record) failed: %s", exc)

    return post_llm_call


# Back-compat shim: the older `make_on_session_end` API is still
# referenced by test_plugin.py's dry-run cases. The new hook lives
# at post_llm_call but the in-process behaviour (detect → append /
# fact_add) is identical, so we expose the same logic under the
# old name so existing tests keep their assertions meaningful.
def make_on_session_end(
    client: WgClient,
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


def register_all(ctx: Any, client: WgClient, config: dict | None = None) -> None:
    cfg = config or {}
    last = cfg.get("recent_window", "7d")
    limit = int(cfg.get("recent_limit", 10))
    auto_record = bool(cfg.get("auto_record", True))
    dry_run = bool(cfg.get("dry_run", False))
    floor = float(cfg.get("confidence_floor", 0.85))
    default_entities = cfg.get("default_entities")
    pending_path_cfg = cfg.get("pending_log")
    pending_path = Path(pending_path_cfg) if pending_path_cfg else None

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
        ),
    )
