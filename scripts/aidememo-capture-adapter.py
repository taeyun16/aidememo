#!/usr/bin/env python3
"""Opt-in AideMemo capture adapter for Hermes/OpenClaw-style hook JSON.

Default behavior is deliberately inert. Pass ``--enable`` (or set
``AIDEMEMO_CAPTURE_ENABLE=1``) and choose ``--mode pending`` to queue candidate
facts for review. ``--mode direct`` writes immediately through the SDK and is
therefore a separate explicit opt-in.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "packages" / "aidememo-agent-sdk" / "src"))
sys.path.insert(0, str(ROOT / "plugins" / "hermes" / "src"))

from hermes_aidememo.capture_adapter import CaptureConfig, capture_from_payload  # noqa: E402
from hermes_aidememo.client import AideMemoClient  # noqa: E402


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--enable", action="store_true", help="Enable capture for this invocation.")
    parser.add_argument(
        "--provider",
        default=os.environ.get("AIDEMEMO_CAPTURE_PROVIDER", "generic"),
        choices=["hermes", "openclaw", "generic"],
        help="Provider label used for tagging/log metadata.",
    )
    parser.add_argument(
        "--mode",
        default=os.environ.get("AIDEMEMO_CAPTURE_MODE", "pending"),
        choices=["pending", "direct", "off"],
        help="pending queues JSONL for review; direct writes facts immediately.",
    )
    parser.add_argument("--pending-log", type=Path, help="Override pending JSONL path.")
    parser.add_argument("--store", help="AideMemo store path for direct mode.")
    parser.add_argument("--backend", help="AideMemo storage backend selector for direct mode.")
    parser.add_argument("--lock-retry-ms", type=int, default=5000)
    parser.add_argument("--source-id", help="Default source_id for direct mode writes.")
    parser.add_argument("--entity", action="append", default=[], help="Default entity for direct mode. Repeatable.")
    parser.add_argument("--confidence-floor", type=float, default=0.85)
    parser.add_argument("--detect-in", choices=["both", "user", "assistant"], default="both")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    payload = _read_payload()
    enabled = args.enable or os.environ.get("AIDEMEMO_CAPTURE_ENABLE") == "1"
    config = CaptureConfig(
        enabled=enabled,
        mode=args.mode,
        provider=args.provider,
        confidence_floor=args.confidence_floor,
        detect_in=args.detect_in,
        default_entities=args.entity or None,
        pending_path=args.pending_log,
    )

    client: AideMemoClient | None = None
    if enabled and args.mode == "direct":
        client = AideMemoClient(
            store_path=args.store or os.environ.get("AIDEMEMO_STORE"),
            source_id=args.source_id or os.environ.get("AIDEMEMO_SOURCE_ID"),
            storage_backend=args.backend,
            lock_retry_ms=args.lock_retry_ms,
        )

    result = capture_from_payload(client, payload, config)
    print(json.dumps(result.__dict__, ensure_ascii=False, sort_keys=True))
    return 0


def _read_payload() -> Any:
    raw = sys.stdin.read()
    if not raw.strip():
        return {}
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return {"text": raw}


if __name__ == "__main__":
    raise SystemExit(main())
