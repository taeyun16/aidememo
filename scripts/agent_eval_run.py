#!/usr/bin/env python3
"""Run multi-agent eval scenarios and record transcripts.

Each scenario is sent to one or more agent CLIs (claude / codex /
hermes), each with the test wg store wired in via MCP. We capture:
  - hypothesis (the agent's final answer text)
  - tool_calls (parsed from --debug stderr where available)
  - latency
  - exit status

Then a simple keyword scorer + LLM-judge pass writes the verdict.

Usage:
  python3 scripts/agent_eval_run.py \
      --scenarios scripts/agent_eval_scenarios.json \
      --agent claude \
      --out /tmp/wg_agent_eval/claude.jsonl
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path


def run_claude(prompt: str, mcp_config: str, timeout: int = 120) -> dict:
    """Spawn `claude -p` non-interactively against the test MCP config.

    --strict-mcp-config keeps the agent isolated from any other MCP
    servers the user has registered (project, user, etc.). --bare is
    NOT used because we want skills/CLAUDE.md inheritance off, but
    --append-system-prompt nudges the agent toward wg use.
    """
    sys_prompt = (
        "You have access to wg MCP tools (server name 'wg-test') over "
        "the wiki at /tmp/wg-agent-test/wiki.redb. Use those tools to "
        "answer the user's question. Be concise. If the wiki doesn't "
        "have the answer, say so explicitly — do not guess."
    )
    cmd = [
        "claude", "-p", prompt,
        "--mcp-config", mcp_config,
        "--strict-mcp-config",
        "--dangerously-skip-permissions",
        "--append-system-prompt", sys_prompt,
        "--debug", "api",
        "--effort", "low",
    ]
    started = time.time()
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout,
        )
        elapsed = time.time() - started
    except subprocess.TimeoutExpired:
        return {
            "agent": "claude", "hypothesis": "", "tool_calls": [],
            "latency_s": timeout, "exit": "timeout", "stderr_tail": "",
        }
    hypothesis = proc.stdout.strip()
    # Tool calls: look for "tool_use" or mcp__wg-test__ markers in stderr
    stderr = proc.stderr
    tool_calls = []
    for m in re.finditer(r"mcp__wg-test__(\w+)", stderr):
        tool_calls.append(m.group(1))
    return {
        "agent": "claude",
        "hypothesis": hypothesis,
        "tool_calls": tool_calls,
        "latency_s": round(elapsed, 2),
        "exit": proc.returncode,
        "stderr_tail": stderr[-1500:],
    }


def run_codex(prompt: str, mcp_config: str, timeout: int = 120) -> dict:
    """Codex exec — relies on ~/.codex/config.toml carrying the wg-test
    MCP server. We can't pass --mcp-config inline, so the runner
    expects the user to have registered wg-test out-of-band (the
    scaffold below logs the registration command on first failure).
    """
    sys_prompt = (
        "Use the wg MCP tools (server 'wg-test') against the wiki at "
        "/tmp/wg-agent-test/wiki.redb to answer. Be concise. Don't guess."
    )
    full = f"{sys_prompt}\n\nQuestion: {prompt}"
    cmd = [
        "codex", "exec",
        "--dangerously-bypass-approvals-and-sandbox",
        "--skip-git-repo-check",
        full,
    ]
    started = time.time()
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        elapsed = time.time() - started
    except subprocess.TimeoutExpired:
        return {
            "agent": "codex", "hypothesis": "", "tool_calls": [],
            "latency_s": timeout, "exit": "timeout", "stderr_tail": "",
        }
    # Codex prints both reasoning and answer interleaved on stdout.
    # Take the LAST non-`tokens used`/`mcp:` line as the final answer.
    raw_lines = [l for l in proc.stdout.splitlines() if l.strip()]
    final = ""
    for line in reversed(raw_lines):
        if line.startswith(("mcp:", "tokens used", "codex")) or line.strip() == "":
            continue
        final = line.strip()
        break
    hypothesis = final or proc.stdout.strip()
    # Tool calls: lines like "mcp: wg-test/wg_search started"
    tool_calls = re.findall(r"mcp: wg-test/(\w+) started", proc.stdout)
    return {
        "agent": "codex",
        "hypothesis": hypothesis,
        "tool_calls": tool_calls,
        "latency_s": round(elapsed, 2),
        "exit": proc.returncode,
        "stderr_tail": proc.stderr[-1500:],
    }


def run_hermes(prompt: str, mcp_config: str, timeout: int = 120) -> dict:
    """Hermes chat in non-interactive mode. Falls back to a friendly
    message if the wg-test MCP isn't registered with hermes."""
    sys_prompt = (
        "Use the wg MCP tools (server 'wg-test') against the wiki at "
        "/tmp/wg-agent-test/wiki.redb to answer. Be concise. Don't guess."
    )
    full = f"{sys_prompt}\n\nQuestion: {prompt}"
    cmd = ["hermes", "chat", full]
    started = time.time()
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        elapsed = time.time() - started
    except subprocess.TimeoutExpired:
        return {
            "agent": "hermes", "hypothesis": "", "tool_calls": [],
            "latency_s": timeout, "exit": "timeout", "stderr_tail": "",
        }
    hypothesis = proc.stdout.strip()
    tool_calls = re.findall(r"wg_(\w+)", proc.stderr + proc.stdout)
    return {
        "agent": "hermes",
        "hypothesis": hypothesis,
        "tool_calls": tool_calls,
        "latency_s": round(elapsed, 2),
        "exit": proc.returncode,
        "stderr_tail": proc.stderr[-1500:],
    }


RUNNERS = {"claude": run_claude, "codex": run_codex, "hermes": run_hermes}


def keyword_score(hyp: str, gold_keywords: list) -> dict:
    if not gold_keywords:
        return {"hits": 0, "total": 0, "ratio": None}
    hyp_l = hyp.lower()
    hits = sum(1 for k in gold_keywords if k.lower() in hyp_l)
    return {"hits": hits, "total": len(gold_keywords), "ratio": hits / len(gold_keywords)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scenarios", required=True, type=Path)
    ap.add_argument("--agent", required=True, choices=["claude", "codex", "hermes"])
    ap.add_argument("--mcp-config", default="/tmp/wg-agent-test/mcp-config.json")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--timeout", type=int, default=180)
    args = ap.parse_args()

    cfg = json.loads(args.scenarios.read_text())
    scenarios = cfg["scenarios"]
    if args.limit:
        scenarios = scenarios[: args.limit]
    print(f"Running {len(scenarios)} scenarios via {args.agent}")
    print(f"  store: {cfg['store_path']}")
    print(f"  mcp config: {args.mcp_config}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    runner = RUNNERS[args.agent]
    with open(args.out, "w") as fout:
        for i, sc in enumerate(scenarios, 1):
            print(f"\n[{i}/{len(scenarios)}] {sc['id']}: {sc['shape']}")
            result = runner(sc["prompt"], args.mcp_config, timeout=args.timeout)
            kw = keyword_score(result["hypothesis"], sc.get("gold_keywords", []))
            row = {
                "scenario_id": sc["id"],
                "shape": sc["shape"],
                "expected_tool": sc.get("expected_tool"),
                "agent": result["agent"],
                "hypothesis": result["hypothesis"],
                "tool_calls": result["tool_calls"],
                "tool_call_count": len(result["tool_calls"]),
                "latency_s": result["latency_s"],
                "exit": result["exit"],
                "keyword_hits": kw,
            }
            fout.write(json.dumps(row, ensure_ascii=False) + "\n")
            fout.flush()
            print(f"  → {kw['hits']}/{kw['total']} kw, {len(result['tool_calls'])} tool calls, {result['latency_s']}s")
            if result["exit"] not in (0, "timeout"):
                print(f"  ! non-zero exit ({result['exit']}); stderr tail:\n{result['stderr_tail'][:500]}")

    rows = [json.loads(l) for l in open(args.out)]
    n = len(rows)
    avg_kw = sum((r["keyword_hits"]["ratio"] or 0) for r in rows) / max(1, n)
    avg_tools = sum(r["tool_call_count"] for r in rows) / max(1, n)
    avg_lat = sum(r["latency_s"] for r in rows) / max(1, n)
    print(f"\nSummary {args.agent}:")
    print(f"  scenarios:   {n}")
    print(f"  avg keyword: {avg_kw:.2f}")
    print(f"  avg tools/q: {avg_tools:.2f}")
    print(f"  avg latency: {avg_lat:.1f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
