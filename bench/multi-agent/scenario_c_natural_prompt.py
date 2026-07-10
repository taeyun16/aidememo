#!/usr/bin/env python3
"""Scenario C — natural-language prompt e2e against three agents.

We seed the e2e store with a small known fact set, then send the
SAME natural-language prompt to each agent's non-interactive CLI:

  - claude   →  claude --print --mcp-config <tmp .mcp.json> ...
  - codex    →  codex exec ...                  (uses ~/.codex/config.toml)
  - hermes   →  hermes chat -q ... -Q           (uses ~/.hermes/config.yaml)

The prompt explicitly forbids guessing — if the agent doesn't call
aidememo, it cannot answer. We capture wall time + stdout, then a human
inspects the per-agent output to judge correctness.

This burns model tokens (one prompt × 3 agents). It is the smallest
useful natural-language e2e — designed to be a one-shot demo, not a
benchmark loop.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STORE = os.environ.get(
    "AIDEMEMO_E2E_STORE",
    str(Path(tempfile.gettempdir()) / "aidememo-e2e-c" / "wiki.sqlite"),
)
WG = os.environ.get("AIDEMEMO_BIN", str(ROOT / "target" / "release" / "aidememo"))
CLAUDE_BIN = os.environ.get("CLAUDE_BIN", "claude")
CODEX_BIN = os.environ.get("CODEX_BIN", "codex")
HERMES_BIN = os.environ.get("HERMES_BIN", "hermes")

# Seed facts — short and unambiguous so we can tell whether the agent
# actually used aidememo vs hallucinated.
SEED = [
    ("Redis", "Redis 7.0 introduces functions, replacing Lua scripts for shared logic"),
    ("Redis", "Redis Sentinel provides high availability with automatic failover"),
    ("Redis", "Redis 8 changes the default eviction policy from noeviction to allkeys-lru"),
    ("Postgres", "Postgres logical replication shipped to prod in 2026 Q1"),
    ("Postgres", "We standardised on Postgres 17 across all services"),
]

PROMPT = (
    "Use the aidememo knowledge graph (aidememo_query / aidememo_search / aidememo_recent tools) "
    "to fetch every fact about 'Redis' that aidememo knows, then summarise them "
    "as a numbered list. Quote each fact's content verbatim and include "
    "its fact id. Do NOT invent or paraphrase — if aidememo returns nothing, "
    "say so explicitly. Keep the answer under 200 words."
)


def reset_and_seed() -> list[str]:
    """Clear the e2e store and insert SEED facts. Returns inserted fact IDs."""
    p = Path(STORE)
    p.parent.mkdir(parents=True, exist_ok=True)
    for sib in p.parent.iterdir():
        if sib.name.startswith(p.name):
            sib.unlink()

    ids = []
    for entity, content in SEED:
        out = subprocess.run(
            [WG, "--store", STORE, "fact", "add", content, "--entities", entity, "--json"],
            capture_output=True, text=True, timeout=15,
        )
        if out.returncode != 0:
            raise RuntimeError(f"seed failed: {out.stderr}")
        try:
            ids.append(json.loads(out.stdout)["id"])
        except (json.JSONDecodeError, KeyError):
            ids.append(out.stdout.strip())
    return ids


@dataclass
class AgentRun:
    name: str
    cmd: list[str]
    cwd: str | None = None
    extra_env: dict[str, str] | None = None


def run_agent(spec: AgentRun, prompt: str, timeout_s: int = 180) -> dict:
    env = os.environ.copy()
    if spec.extra_env:
        env.update(spec.extra_env)
    t = time.perf_counter_ns()
    try:
        proc = subprocess.run(
            spec.cmd + [prompt],
            cwd=spec.cwd, env=env,
            capture_output=True, text=True, timeout=timeout_s,
        )
    except subprocess.TimeoutExpired as exc:
        return {"agent": spec.name, "wall_ms": -1,
                "stdout": "", "stderr": f"TIMEOUT after {timeout_s}s",
                "returncode": -1}
    wall_ms = (time.perf_counter_ns() - t) / 1e6
    return {"agent": spec.name, "wall_ms": wall_ms,
            "stdout": proc.stdout, "stderr": proc.stderr[-2000:],
            "returncode": proc.returncode}


def write_claude_mcp_config(tmpdir: Path) -> Path:
    """Claude Code auto-loads project .mcp.json; we want our e2e aidememo.
    Write a sandbox dir with only the e2e aidememo server defined."""
    cfg = {
        "mcpServers": {
            "aidememo": {
                "type": "stdio",
                "command": WG,
                "args": ["mcp", STORE],
            }
        }
    }
    path = tmpdir / ".mcp.json"
    path.write_text(json.dumps(cfg, indent=2))
    # Trust the project so Claude doesn't prompt about MCP servers.
    settings = tmpdir / ".claude" / "settings.local.json"
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text(json.dumps({
        "enableAllProjectMcpServers": True,
        "permissions": {
            "allow": ["mcp__aidememo"],
        }
    }))
    return path


def main() -> int:
    seeded = reset_and_seed()
    print(f"# seeded {len(seeded)} facts: {seeded}", file=sys.stderr)

    with tempfile.TemporaryDirectory(prefix="aidememo-e2e-claude-") as td:
        td_path = Path(td)
        write_claude_mcp_config(td_path)

        agents = [
            AgentRun(
                name="claude",
                cmd=[CLAUDE_BIN, "--print",
                     "--permission-mode", "bypassPermissions"],
                cwd=str(td_path),
            ),
            AgentRun(
                name="codex",
                # `--full-auto` is not enough — codex still cancels MCP
                # tool invocations under both default and full-auto
                # sandboxes, falling back to the local CLI which then
                # opens the wrong store. The bypass flag is the only
                # way to get codex's non-interactive run to actually
                # call MCP tools.
                cmd=[CODEX_BIN, "exec",
                     "--skip-git-repo-check",
                     "--dangerously-bypass-approvals-and-sandbox"],
            ),
            AgentRun(
                name="hermes",
                cmd=[HERMES_BIN, "chat", "-Q", "-q"],
            ),
        ]

        runs = []
        for spec in agents:
            print(f"# running {spec.name}…", file=sys.stderr)
            r = run_agent(spec, PROMPT)
            print(f"#   {spec.name}: rc={r['returncode']} wall={r['wall_ms']:.0f}ms "
                  f"stdout_chars={len(r['stdout'])}", file=sys.stderr)
            runs.append(r)

    # Two verification signals:
    #   - facts_quoted   : agent reproduced the fact's exact content
    #                      string. Long natural-language strings are
    #                      LLM-friendly to transcribe verbatim, so this
    #                      is the primary signal for "did the agent
    #                      actually call aidememo and use the data?"
    #   - ids_mentioned  : agent included the 26-char ULID. Bonus
    #                      metric — useful but flaky because LLMs
    #                      occasionally drop a character when they
    #                      transcribe long opaque strings. We do NOT
    #                      gate pass/fail on it.
    redis_seeds = [content for entity, content in SEED if entity == "Redis"]
    redis_seed_ids = [
        fid for fid, (entity, _) in zip(seeded, SEED) if entity == "Redis"
    ]
    for r in runs:
        r["facts_quoted"] = [s for s in redis_seeds if s in r["stdout"]]
        r["facts_quoted_count"] = len(r["facts_quoted"])
        r["ids_mentioned"] = [fid for fid in redis_seed_ids if fid in r["stdout"]]
        r["ids_mentioned_count"] = len(r["ids_mentioned"])

    redis_total = sum(1 for entity, _ in SEED if entity == "Redis")
    invariants = {
        # Every agent must have actually used aidememo — i.e. quoted at
        # least one Redis fact verbatim. (Otherwise it hallucinated
        # or said "no facts found".)
        "all_agents_used_aidememo": all(r["facts_quoted_count"] >= 1 for r in runs),
        # Every agent must have surfaced ALL Redis facts aidememo knows.
        # Content matching tolerates LLM ULID-transcription wobbles.
        "all_agents_returned_complete_set": all(
            r["facts_quoted_count"] == redis_total for r in runs
        ),
        # No agent invoked the prompt's "say nothing" escape clause.
        "no_agent_claimed_empty": all(
            "no facts" not in r["stdout"].lower()
            and "could not" not in r["stdout"].lower()
            for r in runs
        ),
    }

    out = {
        "scenario": "C — natural-language prompt e2e",
        "store": STORE,
        "prompt": PROMPT,
        "seeded_facts": [
            {"id": fid, "content": content}
            for fid, (entity, content) in zip(seeded, SEED)
        ],
        "agents": runs,
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
            "agents_total": len(runs),
        },
    }
    out_path = Path("bench/multi-agent/results/scenario_c.json")
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
