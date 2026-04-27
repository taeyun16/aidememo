"""Lifecycle hooks.

``on_session_start`` injects a recent-facts summary into the
conversation so the model has wg context loaded before the user
even types — the highest-leverage win of the plugin route over
plain MCP.

``on_session_end`` walks the just-completed transcript, runs the
decision detector (``hermes_wg.decisions``), and records each match
as a wg fact. Bias is towards precision: a high confidence floor
keeps false positives (and wiki noise) low. Operators who prefer
recall over precision can lower ``confidence_floor`` in the plugin
config.

Both hooks degrade gracefully — any exception during ingest of
context or recording of facts is logged to stderr but never
propagates back into Hermes's session lifecycle, since a noisy
plugin should never break the agent.
"""

from __future__ import annotations

import logging
from typing import Any

from hermes_wg.client import WgClient
from hermes_wg.decisions import detect

log = logging.getLogger("hermes_wg")


def _format_recent_block(facts: list[dict]) -> str | None:
    """Render a compact, model-friendly preamble. Returns ``None`` if
    there's nothing worth injecting (no recent facts)."""
    if not facts:
        return None
    lines = [
        "## wg — recent knowledge graph snapshot",
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
    if len(lines) <= 4:  # only the header survived — nothing to show
        return None
    return "\n".join(lines)


def make_on_session_start(client: WgClient, last: str = "7d", limit: int = 10):
    """Build the ``on_session_start`` callback.

    The callback is what gets registered with Hermes; it captures the
    client (via closure) so we don't have to thread it through
    Hermes's hook signature.
    """

    def on_session_start(ctx: Any = None, **_kwargs: Any) -> None:
        try:
            facts = client.recent(last=last, limit=limit)
        except Exception as exc:  # noqa: BLE001 — never break sessions
            log.warning("wg recent failed at session start: %s", exc)
            return
        block = _format_recent_block(facts)
        if not block:
            return
        try:
            # Hermes injects via the ctx passed at register-time; some
            # versions also expose `ctx.inject_message` on the hook
            # callback directly. Probe both.
            inject = getattr(ctx, "inject_message", None) if ctx is not None else None
            if inject is None:
                from hermes_cli.plugins import get_plugin_context  # type: ignore

                inject = get_plugin_context().inject_message
            inject(block, role="system")
        except Exception as exc:  # noqa: BLE001
            log.warning("wg session-start inject_message failed: %s", exc)

    return on_session_start


def make_on_session_end(
    client: WgClient,
    *,
    enable_auto_record: bool = True,
    confidence_floor: float = 0.85,
    default_entities: list[str] | None = None,
):
    """Build the ``on_session_end`` callback.

    ``enable_auto_record`` defaults to ``True`` but is gated by the
    plugin config; ``confidence_floor`` controls the precision /
    recall trade-off of the decision detector.
    """

    def on_session_end(ctx: Any = None, transcript: str | None = None, **_kwargs: Any) -> None:
        if not enable_auto_record:
            return
        if transcript is None:
            # Hermes versions differ — try a couple of common arg names.
            for kw in ("messages", "session", "history"):
                value = _kwargs.get(kw)
                if value is None:
                    continue
                if isinstance(value, str):
                    transcript = value
                    break
                if isinstance(value, list):
                    transcript = "\n".join(_message_text(m) for m in value)
                    break
        if not transcript:
            return

        detections = detect(transcript, confidence_floor=confidence_floor)
        if not detections:
            return

        for d in detections:
            try:
                client.fact_add(
                    d.content,
                    entities=default_entities,
                    fact_type=d.fact_type,
                    tags=["auto-recorded", "hermes-session"],
                )
            except Exception as exc:  # noqa: BLE001
                log.warning("wg fact_add (auto-record) failed: %s", exc)

    return on_session_end


def _message_text(m: Any) -> str:
    if isinstance(m, dict):
        return str(m.get("content") or "")
    return str(m)


def register_all(ctx: Any, client: WgClient, config: dict | None = None) -> None:
    cfg = config or {}
    last = cfg.get("recent_window", "7d")
    limit = int(cfg.get("recent_limit", 10))
    auto_record = bool(cfg.get("auto_record", True))
    floor = float(cfg.get("confidence_floor", 0.85))
    default_entities = cfg.get("default_entities")

    ctx.register_hook(
        "on_session_start",
        make_on_session_start(client, last=last, limit=limit),
    )
    ctx.register_hook(
        "on_session_end",
        make_on_session_end(
            client,
            enable_auto_record=auto_record,
            confidence_floor=floor,
            default_entities=default_entities,
        ),
    )
