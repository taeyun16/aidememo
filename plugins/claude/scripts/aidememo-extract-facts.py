#!/usr/bin/env python3
"""Preview durable fact candidates from longer Claude Code prompts."""

import json
import os
import sys

from aidememo_hook_common import context_output, run_aidememo


def main() -> int:
    try:
        payload = json.loads(sys.stdin.read() or "{}")
    except Exception:
        print(json.dumps({"continue": True}))
        return 0
    prompt = payload.get("prompt", "") or payload.get("user_message", "")
    if not isinstance(prompt, str) or len(prompt) < 200:
        print(json.dumps({"continue": True}))
        return 0
    args = [
        "extract",
        "--min-confidence",
        "0.7",
        "--max-candidates",
        "5",
        "--from-stdin",
    ]
    if os.environ.get("AIDEMEMO_EXTRACT_LLM") == "1":
        args.append("--llm")
    result = run_aidememo(*args, stdin=prompt, timeout=8)
    if not result or "No candidates" in result:
        print(json.dumps({"continue": True}))
        return 0
    body = (
        "## AideMemo candidate facts (not saved)\n\n"
        f"{result}\n\nSave only durable items with `aidememo_fact_add`."
    )
    print(json.dumps(context_output("UserPromptSubmit", body)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
