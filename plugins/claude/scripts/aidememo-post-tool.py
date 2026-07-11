#!/usr/bin/env python3
"""Surface facts related to a file after Claude Code edits it."""

import json
import os
import sys

from aidememo_hook_common import context_output, run_aidememo


def topics(path: str) -> list[str]:
    base = os.path.basename(path)
    name = os.path.splitext(base)[0]
    parts = path.split(os.sep)
    candidates = [name]
    if len(parts) >= 3:
        candidates.append(parts[-3])
    candidates.append(path)
    return list(dict.fromkeys(value for value in candidates if value))


def main() -> int:
    try:
        payload = json.loads(sys.stdin.read() or "{}")
    except Exception:
        print(json.dumps({"continue": True}))
        return 0
    if payload.get("tool_name") not in {"Edit", "Write", "NotebookEdit"}:
        print(json.dumps({"continue": True}))
        return 0
    tool_input = payload.get("tool_input", {})
    path = tool_input.get("file_path") or tool_input.get("notebook_path") or ""
    for topic in topics(path):
        result = run_aidememo(
            "query", topic, "-l", "3", "--recent-limit", "3", "--bm25-only", timeout=4
        )
        if result and "no results" not in result.lower():
            body = f"## AideMemo facts related to `{path}`\n\n{result}"
            print(json.dumps(context_output("PostToolUse", body)))
            return 0
    print(json.dumps({"continue": True}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
