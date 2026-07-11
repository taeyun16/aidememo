#!/usr/bin/env python3
"""Load a compact AideMemo briefing when a Claude Code session starts."""

import json
import sys

from aidememo_hook_common import context_output, run_aidememo


def main() -> int:
    sys.stdin.read()
    sections: list[str] = []
    pinned = run_aidememo("fact", "pinned", "--limit", "10")
    overview = run_aidememo("overview", "-n", "5")
    recent = run_aidememo("recent", "-n", "10", "--last", "7d")
    if pinned and "no pinned" not in pinned.lower():
        sections.append(f"### Pinned facts\n{pinned}")
    if overview:
        sections.append(f"### Wiki overview\n{overview}")
    if recent:
        sections.append(f"### Recent activity\n{recent}")
    if not sections:
        print(json.dumps({"continue": True}))
        return 0
    body = "## AideMemo context\n\n" + "\n\n".join(sections)
    print(json.dumps(context_output("SessionStart", body)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
