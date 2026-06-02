#!/usr/bin/env python3
"""SessionStart hook for Claude Code — surfaces the aidememo wiki's pinned
context + recent activity at the top of every new conversation.

Modeled after OMEGA's `omega_welcome` (session briefing). Cost: one
local `aidememo overview` + `aidememo recent` shell-out, no API calls. Output is
injected as `additionalContext` so the agent sees it before the user's
first prompt.

Install:
  cp aidememo-session-start.py ~/.claude/hooks/aidememo-session-start.py
  chmod +x ~/.claude/hooks/aidememo-session-start.py
  # Then add the SessionStart entry to ~/.claude/settings.json
  # (see hooks/README.md for the full snippet).
"""
from __future__ import annotations

import json
import os
import subprocess
import sys


def run_aidememo(*args: str, timeout: int = 5) -> str | None:
    """Invoke `aidememo` with optional --store / --project from env, capture
    stdout. Returns None on any failure so the hook stays soft-fail —
    a dead aidememo binary should never block the session."""
    cmd = [os.environ.get("AIDEMEMO_BIN", "aidememo")]
    if store := os.environ.get("AIDEMEMO_STORE"):
        cmd += ["--store", store]
    elif project := os.environ.get("AIDEMEMO_PROJECT"):
        cmd += ["--project", project]
    cmd += list(args)
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout, check=False
        )
        if result.returncode != 0:
            return None
        return result.stdout.rstrip()
    except Exception:
        return None


def main() -> int:
    # Drain stdin (SessionStart input — cwd / session_id / etc. — we
    # don't currently use it, but the hook contract expects us to read).
    try:
        sys.stdin.read()
    except Exception:
        pass

    overview = run_aidememo("overview", "-n", "5")
    recent = run_aidememo("recent", "-n", "10", "--last", "7d")
    pinned = run_aidememo("fact", "pinned", "--limit", "10")

    sections: list[str] = []
    if pinned and pinned.strip() and "no pinned" not in pinned.lower():
        sections.append(f"### Pinned facts (always-on context)\n{pinned}")
    if overview and overview.strip():
        sections.append(f"### Wiki overview\n{overview}")
    if recent and recent.strip():
        sections.append(f"### Recent activity (last 7d)\n{recent}")

    if not sections:
        # Nothing to inject — silent success.
        print(json.dumps({"continue": True}))
        return 0

    body = (
        "## aidememo wiki context (auto-loaded by hook)\n\n"
        + "\n\n".join(sections)
        + "\n\nUse `aidememo_query <topic>` / `aidememo_search <q>` for follow-up retrieval."
    )
    print(json.dumps({"additionalContext": body, "continue": True}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
