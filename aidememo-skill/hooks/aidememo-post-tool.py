#!/usr/bin/env python3
"""PostToolUse hook for Claude Code (matcher: Edit|Write) — surfaces
aidememo facts related to the file just edited so the agent sees the
relevant decisions / patterns / conventions before responding.

Cost: one local `aidememo query` per matching tool call. No API.

Install:
  cp aidememo-post-tool.py ~/.claude/hooks/aidememo-post-tool.py
  chmod +x ~/.claude/hooks/aidememo-post-tool.py
  # Settings entry uses matcher: "Edit|Write" so it only fires for
  # file-mutating tools (not for Bash, Grep, etc.).
"""
from __future__ import annotations

import json
import os
import subprocess
import sys


def run_aidememo(*args: str, timeout: int = 4) -> str | None:
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


def candidate_topics(file_path: str) -> list[str]:
    """Pull a few naming-based search terms from a file path.
    'crates/aidememo-core/src/lib.rs' →
    ['lib', 'aidememo-core', 'aidememo-core/src/lib.rs']
    so aidememo can match against entity names (`aidememo-core`, `lib.rs`) AND
    on path-shaped facts."""
    base = os.path.basename(file_path)
    name = os.path.splitext(base)[0]
    parts = file_path.split(os.sep)
    topics: list[str] = []
    if name and name not in topics:
        topics.append(name)
    # The directory just above the file is a strong topic signal
    # for repo-shaped wikis (crates/aidememo-core/src/lib.rs → "aidememo-core").
    if len(parts) >= 3:
        parent = parts[-3]
        if parent and parent not in topics:
            topics.append(parent)
    if file_path not in topics:
        topics.append(file_path)
    return topics


def main() -> int:
    try:
        payload = json.loads(sys.stdin.read() or "{}")
    except Exception:
        print(json.dumps({"continue": True}))
        return 0

    tool = payload.get("tool_name", "")
    if tool not in ("Edit", "Write", "MultiEdit", "NotebookEdit"):
        print(json.dumps({"continue": True}))
        return 0

    file_path = (
        payload.get("tool_input", {}).get("file_path")
        or payload.get("tool_input", {}).get("notebook_path")
        or ""
    )
    if not file_path:
        print(json.dumps({"continue": True}))
        return 0

    # Try the most-specific topic first (filename), fall back to
    # the parent dir if nothing matches.
    for topic in candidate_topics(file_path):
        out = run_aidememo("query", topic, "-l", "3", "--recent-limit", "3")
        if out and "no results" not in out.lower() and out.strip():
            body = (
                f"## aidememo facts related to `{file_path}` (via topic '{topic}')\n\n"
                f"{out}\n\n"
                "If anything contradicts the change you just made, consider "
                "`aidememo_fact_supersede` to retire the stale fact."
            )
            print(json.dumps({"additionalContext": body, "continue": True}))
            return 0

    print(json.dumps({"continue": True}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
