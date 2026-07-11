"""Shared, dependency-free helpers for the AideMemo Claude Code hooks."""

from __future__ import annotations

import os
import subprocess
from typing import Any


def run_aidememo(
    *args: str, stdin: str | None = None, timeout: int = 5
) -> str | None:
    command = [os.environ.get("AIDEMEMO_BIN", "aidememo")]
    if store := os.environ.get("AIDEMEMO_STORE"):
        command += ["--store", store]
    elif project := os.environ.get("AIDEMEMO_PROJECT"):
        command += ["--project", project]
    command += list(args)
    try:
        result = subprocess.run(
            command,
            input=stdin,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except Exception:
        return None
    return result.stdout.rstrip() if result.returncode == 0 else None


def context_output(event: str, body: str) -> dict[str, Any]:
    return {
        "continue": True,
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": body,
        },
    }
