"""Opt-in auto-capture adapter shared by Hermes and generic hook callers.

The canonical write path remains explicit ``fact_add`` / SDK / MCP calls.
This module only runs when the operator opts in, and its safe default is the
pending review log. Direct writes are still available for teams that explicitly
choose them.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from . import pending
from .client import CLIENT_ERRORS
from .decisions import DetectedFact, detect

log = logging.getLogger("hermes_aidememo")


@dataclass(frozen=True)
class CaptureConfig:
    enabled: bool = False
    mode: str = "pending"
    provider: str = "hermes"
    confidence_floor: float = 0.85
    detect_in: str = "both"
    default_entities: list[str] | None = None
    pending_path: Path | None = None


@dataclass(frozen=True)
class CaptureResult:
    enabled: bool
    mode: str
    provider: str
    detected: int = 0
    queued: int = 0
    recorded: int = 0
    pending_path: str | None = None


def config_from_plugin(config: dict[str, Any] | None, *, provider: str = "hermes") -> CaptureConfig:
    """Build capture config from new ``auto_capture`` keys plus legacy keys.

    New installs are opt-in: absent config means disabled. Legacy explicit
    ``auto_record: true`` still works, and explicit ``dry_run: true`` opts into
    pending capture because that is the review-first mode.
    """

    cfg = config or {}
    nested = cfg.get("auto_capture")
    if nested is None:
        nested = cfg.get("capture")
    if not isinstance(nested, dict):
        nested = {}

    has_nested = bool(nested)
    if has_nested:
        enabled = bool(nested.get("enabled", False))
        mode = str(nested.get("mode") or nested.get("destination") or "pending")
    elif "auto_record" in cfg:
        enabled = bool(cfg.get("auto_record"))
        mode = "pending" if bool(cfg.get("dry_run", False)) else "direct"
    elif bool(cfg.get("dry_run", False)):
        enabled = True
        mode = "pending"
    else:
        enabled = False
        mode = "pending"

    if mode == "off":
        enabled = False
    if mode not in {"pending", "direct", "off"}:
        log.warning("unknown aidememo auto_capture mode=%r; using pending", mode)
        mode = "pending"

    confidence_floor = _float_from(nested.get("confidence_floor"), cfg.get("confidence_floor"), 0.85)
    detect_in = str(nested.get("detect_in") or cfg.get("detect_in") or "both")
    if detect_in not in {"both", "user", "assistant"}:
        log.warning("unknown aidememo detect_in=%r; using both", detect_in)
        detect_in = "both"

    default_entities = nested.get("default_entities", cfg.get("default_entities"))
    if isinstance(default_entities, str):
        default_entities = [s.strip() for s in default_entities.split(",") if s.strip()]
    if not isinstance(default_entities, list):
        default_entities = None

    pending_path_cfg = nested.get("pending_log", cfg.get("pending_log"))
    pending_path = Path(pending_path_cfg) if pending_path_cfg else None
    provider_name = str(nested.get("provider") or cfg.get("capture_provider") or provider)

    return CaptureConfig(
        enabled=enabled,
        mode=mode,
        provider=provider_name,
        confidence_floor=confidence_floor,
        detect_in=detect_in,
        default_entities=default_entities,
        pending_path=pending_path,
    )


def capture_from_payload(client: Any, payload: Any, config: CaptureConfig) -> CaptureResult:
    if not config.enabled:
        return CaptureResult(enabled=False, mode=config.mode, provider=config.provider)

    text = extract_text(payload, detect_in=config.detect_in)
    if not text.strip():
        return CaptureResult(enabled=True, mode=config.mode, provider=config.provider)

    detections = detect(text, confidence_floor=config.confidence_floor)
    if not detections:
        return CaptureResult(enabled=True, mode=config.mode, provider=config.provider)

    return capture_detections(client, detections, config)


def capture_detections(client: Any, detections: list[DetectedFact], config: CaptureConfig) -> CaptureResult:
    if not config.enabled or not detections:
        return CaptureResult(enabled=config.enabled, mode=config.mode, provider=config.provider)

    if config.mode == "pending":
        path = pending.append(detections, config.pending_path)
        return CaptureResult(
            enabled=True,
            mode=config.mode,
            provider=config.provider,
            detected=len(detections),
            queued=len(detections),
            pending_path=str(path),
        )

    if config.mode != "direct":
        return CaptureResult(enabled=False, mode=config.mode, provider=config.provider)

    if client is None:
        raise RuntimeError("direct capture requires an AideMemoClient")

    recorded = 0
    tags = _capture_tags(config.provider)
    for detection in detections:
        try:
            client.fact_add(
                detection.content,
                entities=config.default_entities,
                fact_type=detection.fact_type,
                tags=tags,
                confidence=detection.confidence,
            )
        except CLIENT_ERRORS as exc:
            log.warning("aidememo direct auto-capture fact_add failed: %s", exc)
            continue
        recorded += 1

    return CaptureResult(
        enabled=True,
        mode=config.mode,
        provider=config.provider,
        detected=len(detections),
        recorded=recorded,
    )


def extract_text(payload: Any, *, detect_in: str = "both") -> str:
    """Extract user/assistant text from Hermes, OpenClaw, or generic hook JSON."""

    if isinstance(payload, str):
        return payload
    if not isinstance(payload, dict):
        return ""

    direct_text = payload.get("transcript") or payload.get("text")
    if isinstance(direct_text, str) and direct_text.strip():
        return direct_text

    chunks: list[str] = []
    wanted_roles = {
        "both": {"user", "assistant", "model"},
        "user": {"user"},
        "assistant": {"assistant", "model"},
    }.get(detect_in, {"user", "assistant", "model"})

    messages = payload.get("messages") or payload.get("conversation") or payload.get("turns")
    if isinstance(messages, list):
        for message in messages:
            if not isinstance(message, dict):
                continue
            role = str(message.get("role") or message.get("speaker") or "").lower()
            if role and role not in wanted_roles:
                continue
            content = message.get("content") or message.get("text") or message.get("message")
            if isinstance(content, str) and content.strip():
                chunks.append(content)

    key_groups = {
        "user": ("user_message", "prompt", "input", "request", "question"),
        "assistant": ("assistant_response", "response", "output", "answer", "completion"),
    }
    if detect_in in {"both", "user"}:
        chunks.extend(_string_values(payload, key_groups["user"]))
    if detect_in in {"both", "assistant"}:
        chunks.extend(_string_values(payload, key_groups["assistant"]))

    content = payload.get("content")
    if isinstance(content, str) and content.strip() and not chunks:
        chunks.append(content)

    return "\n".join(_dedupe_preserve_order(chunks))


def _string_values(payload: dict[str, Any], keys: tuple[str, ...]) -> list[str]:
    values: list[str] = []
    for key in keys:
        value = payload.get(key)
        if isinstance(value, str) and value.strip():
            values.append(value)
    return values


def _dedupe_preserve_order(values: list[str]) -> list[str]:
    out: list[str] = []
    seen: set[str] = set()
    for value in values:
        key = value.strip()
        if not key or key in seen:
            continue
        seen.add(key)
        out.append(key)
    return out


def _float_from(primary: Any, fallback: Any, default: float) -> float:
    for value in (primary, fallback):
        if value is None:
            continue
        try:
            return float(value)
        except (TypeError, ValueError):
            continue
    return default


def _capture_tags(provider: str) -> list[str]:
    provider = (provider or "generic").strip().lower()
    if provider == "hermes":
        return ["auto-recorded", "hermes-session"]
    return ["auto-recorded", f"{provider}-capture"]
