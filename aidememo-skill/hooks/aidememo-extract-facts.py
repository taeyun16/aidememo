#!/usr/bin/env python3
"""UserPromptSubmit hook for Claude Code — runs aidememo's heuristic
extractor over each user prompt and surfaces high-confidence
candidates as additionalContext. Does NOT auto-apply (the agent
decides whether to commit via aidememo_fact_add). LLM extraction is
opt-in via AIDEMEMO_EXTRACT_LLM=1 (uses extract.provider config).

Cost: heuristic = local zero-LLM (~5 ms). LLM = one OpenAI
chat-completion (~$0.0001 with gpt-4o-mini) — only when explicitly
enabled.

Skips short prompts (< 200 chars) since they're rarely durable
fact-shaped material.

Install:
  cp aidememo-extract-facts.py ~/.claude/hooks/aidememo-extract-facts.py
  chmod +x ~/.claude/hooks/aidememo-extract-facts.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys

MIN_PROMPT_CHARS = 200
MIN_CONFIDENCE = "0.7"
MAX_CANDIDATES = "5"


def run_aidememo(*args: str, stdin: str | None = None, timeout: int = 8) -> str | None:
    cmd = [os.environ.get("AIDEMEMO_BIN", "aidememo")]
    if store := os.environ.get("AIDEMEMO_STORE"):
        cmd += ["--store", store]
    elif project := os.environ.get("AIDEMEMO_PROJECT"):
        cmd += ["--project", project]
    cmd += list(args)
    try:
        result = subprocess.run(
            cmd,
            input=stdin,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        if result.returncode != 0:
            return None
        return result.stdout.rstrip()
    except Exception:
        return None


def main() -> int:
    try:
        payload = json.loads(sys.stdin.read() or "{}")
    except Exception:
        print(json.dumps({"continue": True}))
        return 0

    prompt = payload.get("prompt", "") or payload.get("user_message", "")
    if not isinstance(prompt, str) or len(prompt) < MIN_PROMPT_CHARS:
        print(json.dumps({"continue": True}))
        return 0

    args = [
        "extract",
        "--min-confidence",
        MIN_CONFIDENCE,
        "--max-candidates",
        MAX_CANDIDATES,
        "--from-stdin",
    ]
    if os.environ.get("AIDEMEMO_EXTRACT_LLM") == "1":
        args.append("--llm")

    out = run_aidememo(*args, stdin=prompt)
    if not out or "No candidates" in out:
        print(json.dumps({"continue": True}))
        return 0

    body = (
        "## aidememo extract — candidate facts in this message (preview)\n\n"
        f"{out}\n\n"
        "These are NOT auto-saved. Call `aidememo_fact_add` if any are worth keeping."
    )
    print(json.dumps({"additionalContext": body, "continue": True}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
