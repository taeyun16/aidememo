from __future__ import annotations

from pathlib import Path

from hermes_aidememo import capture_adapter, pending
from hermes_aidememo.decisions import DetectedFact


def test_plugin_config_defaults_to_disabled() -> None:
    cfg = capture_adapter.config_from_plugin({})
    assert cfg.enabled is False
    assert cfg.mode == "pending"


def test_plugin_config_dry_run_opts_into_pending(tmp_path: Path) -> None:
    cfg = capture_adapter.config_from_plugin(
        {"dry_run": True, "pending_log": str(tmp_path / "pending.jsonl")}
    )
    assert cfg.enabled is True
    assert cfg.mode == "pending"
    assert cfg.pending_path == tmp_path / "pending.jsonl"


def test_plugin_config_auto_capture_direct_is_explicit() -> None:
    cfg = capture_adapter.config_from_plugin(
        {
            "auto_capture": {
                "enabled": True,
                "mode": "direct",
                "detect_in": "assistant",
                "default_entities": "Hermes,OpenClaw",
            }
        }
    )
    assert cfg.enabled is True
    assert cfg.mode == "direct"
    assert cfg.detect_in == "assistant"
    assert cfg.default_entities == ["Hermes", "OpenClaw"]


def test_openclaw_payload_can_queue_pending_entries(tmp_path: Path) -> None:
    log = tmp_path / "aidememo-pending.jsonl"
    payload = {
        "messages": [
            {"role": "user", "content": "Should we keep automatic capture off by default?"},
            {"role": "assistant", "content": "Decision: keep auto-capture opt-in and queue to pending by default."},
        ]
    }
    cfg = capture_adapter.CaptureConfig(
        enabled=True,
        mode="pending",
        provider="openclaw",
        confidence_floor=0.85,
        detect_in="assistant",
        pending_path=log,
    )

    result = capture_adapter.capture_from_payload(None, payload, cfg)

    assert result.queued == 1
    entries = pending.read(log)
    assert len(entries) == 1
    assert entries[0].fact_type == "decision"
    assert "opt-in" in entries[0].content


def test_direct_capture_requires_explicit_mode_and_uses_provider_tags() -> None:
    captured = []

    class Client:
        def fact_add(self, *args, **kwargs):
            captured.append((args, kwargs))
            return "fact-1"

    cfg = capture_adapter.CaptureConfig(
        enabled=True,
        mode="direct",
        provider="openclaw",
        default_entities=["OpenClaw"],
    )
    result = capture_adapter.capture_detections(
        Client(),
        [
            DetectedFact(
                content="keep direct capture explicit",
                fact_type="decision",
                confidence=0.95,
                source_line="Decision: keep direct capture explicit",
            )
        ],
        cfg,
    )

    assert result.recorded == 1
    assert captured[0][1]["entities"] == ["OpenClaw"]
    assert captured[0][1]["tags"] == ["auto-recorded", "openclaw-capture"]


def test_disabled_capture_has_no_side_effects(tmp_path: Path) -> None:
    log = tmp_path / "aidememo-pending.jsonl"
    result = capture_adapter.capture_from_payload(
        None,
        {"prompt": "Decision: this would be captured if enabled"},
        capture_adapter.CaptureConfig(enabled=False, pending_path=log),
    )
    assert result.enabled is False
    assert not log.exists()
