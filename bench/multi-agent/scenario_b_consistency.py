#!/usr/bin/env python3
"""Scenario B — cross-client store consistency.

We spawn three independent `wg mcp` processes pointed at the SAME
store, each one shaped like a real agent's invocation:

  - claude-code-shape: ./target/debug/wg mcp <STORE>
  - codex-shape:       ~/.local/bin/wg mcp <STORE>      (release)
  - hermes-shape:      Python WgClient(store_path)      (CLI subprocess)

Test
----
1. Reset the e2e store.
2. claude-shape inserts an entity ("Redis") and a fact.
3. codex-shape lists facts → must see the inserted fact.
4. hermes-shape (via WgClient) lists facts → same.
5. codex-shape inserts a new fact under "Postgres".
6. claude-shape and hermes-shape must each see both facts.
7. Compare normalized fact-id sets across all three reads.

What this proves
----------------
Three different agent integration paths (one in-process via mcp_stdio
called from a subagent's stdio JSON-RPC, one Codex-shaped invocation,
one Python plugin path) all see the same redb store and round-trip
data faithfully — i.e. an agent on this machine can write something
that another agent reads back.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

STORE = "/Users/mixlink/.wg-e2e/wiki.redb"
WG_DEBUG = "/Users/mixlink/dev/wg/target/debug/wg"
WG_RELEASE = "/Users/mixlink/.local/bin/wg"


def reset_store() -> None:
    """Wipe redb files so each run starts clean."""
    p = Path(STORE)
    p.parent.mkdir(parents=True, exist_ok=True)
    for sibling in p.parent.iterdir():
        if sibling.name.startswith(p.name):
            sibling.unlink()


def jsonrpc(cmd: list[str], calls: list[dict]) -> list[dict]:
    """Run a one-shot stdio MCP session and return parsed responses."""
    payload = "".join(json.dumps(c) + "\n" for c in calls)
    proc = subprocess.run(cmd, input=payload, capture_output=True, text=True, timeout=20)
    out = []
    for line in proc.stdout.strip().splitlines():
        if not line.strip():
            continue
        try:
            out.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    if not out:
        raise RuntimeError(f"no response from {cmd[0]}: stderr={proc.stderr.strip()[:300]}")
    return out


def call_tool(cmd: list[str], name: str, args: dict, call_id: int = 1) -> dict:
    """Call an MCP tool and normalize the response.

    wg's tool responses come in two shapes:
      - JSON object as text  → returned as dict
      - plain string         → returned as {"text": "..."}, with the
                                 special case of "Fact added: <ULID>"
                                 unpacked into {"id": "<ULID>", ...}
    All write/read tools now return JSON-shaped payloads (`wg_fact_add`
    returns {"id": "..."}, `wg_recent` returns {"facts": [...]}).
    """
    calls = [
        {"jsonrpc": "2.0", "id": 0, "method": "initialize",
         "params": {"protocolVersion": "2024-11-05", "capabilities": {}}},
        {"jsonrpc": "2.0", "id": call_id, "method": "tools/call",
         "params": {"name": name, "arguments": args}},
    ]
    responses = jsonrpc(cmd, calls)
    by_id = {r.get("id"): r for r in responses if "id" in r}
    response = by_id.get(call_id, {})
    if "error" in response:
        raise RuntimeError(f"{name} error: {response['error']}")
    result = response.get("result", {})
    content = result.get("content") or []
    if content and content[0].get("type") == "text":
        text = content[0]["text"]
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return {"_raw": text}
    return result


def claude_shape(name: str, args: dict) -> dict:
    return call_tool([WG_DEBUG, "mcp", STORE], name, args)


def codex_shape(name: str, args: dict) -> dict:
    return call_tool([WG_RELEASE, "mcp", STORE], name, args)


def hermes_shape(name: str, args: dict) -> dict:
    """The hermes-wg plugin's Python WgClient calls the wg CLI directly,
    not via MCP. Use the CLI surface that backs each MCP tool to mirror
    that path faithfully."""
    if name == "wg_fact_add":
        cmd = [WG_RELEASE, "--store", STORE, "fact", "add", args["content"]]
        if args.get("entities"):
            cmd += ["--entities", ",".join(args["entities"])]
        cmd += ["--json"]
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        return json.loads(out.stdout) if out.stdout.strip() else {}
    if name == "wg_recent":
        cmd = [WG_RELEASE, "--store", STORE, "recent", "--last", "1h",
               "-n", str(args.get("limit", 50)), "--json"]
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        return {"facts": json.loads(out.stdout) if out.stdout.strip() else []}
    if name == "wg_query":
        cmd = [WG_RELEASE, "--store", STORE, "query", args["topic"],
               "-l", str(args.get("limit", 5)), "--json"]
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        return json.loads(out.stdout) if out.stdout.strip() else {}
    raise ValueError(f"hermes_shape: unknown tool {name}")


@dataclass
class Step:
    n: int
    actor: str
    action: str
    detail: str = ""
    fact_ids_seen: list[str] = field(default_factory=list)
    error: str = ""


def normalize_fact_ids(payload) -> list[str]:
    """Pull fact IDs from any of the response shapes we use.

    wg_recent returns a bare JSON list; CLI `wg recent` we wrap as
    {"facts": [...]}. Handle both, plus the legacy {"recent_facts": [...]}
    that some tools use.
    """
    if isinstance(payload, list):
        items = payload
    elif isinstance(payload, dict):
        items = payload.get("facts") or payload.get("recent_facts") or []
    else:
        items = []
    return sorted(f.get("id", "") for f in items if isinstance(f, dict))


def main() -> int:
    reset_store()
    steps: list[Step] = []

    # 1. claude-shape inserts entity + fact
    res = claude_shape("wg_fact_add", {
        "content": "Redis Sentinel provides high availability",
        "entities": ["Redis"],
    })
    fact_a = res.get("id", "")
    steps.append(Step(1, "claude-shape", "wg_fact_add",
                      detail=f"id={fact_a}"))

    # 2. codex-shape reads recent
    # Use the new `last` DSL surface added by issue #2.
    res = codex_shape("wg_recent", {"limit": 50, "last": "1h"})
    seen_codex_1 = normalize_fact_ids(res)
    steps.append(Step(2, "codex-shape", "wg_recent",
                      detail=f"facts={len(seen_codex_1)}",
                      fact_ids_seen=seen_codex_1))

    # 3. hermes-shape reads recent (via CLI)
    res = hermes_shape("wg_recent", {"limit": 50})
    seen_hermes_1 = normalize_fact_ids(res)
    steps.append(Step(3, "hermes-shape", "wg recent",
                      detail=f"facts={len(seen_hermes_1)}",
                      fact_ids_seen=seen_hermes_1))

    # 4. codex-shape inserts a second fact under Postgres
    res = codex_shape("wg_fact_add", {
        "content": "Postgres logical replication shipped to prod",
        "entities": ["Postgres"],
    })
    fact_b = res.get("id", "")
    steps.append(Step(4, "codex-shape", "wg_fact_add",
                      detail=f"id={fact_b}"))

    # 5. claude-shape reads everything
    res = claude_shape("wg_recent", {"limit": 50, "last": "1h"})
    seen_claude_2 = normalize_fact_ids(res)
    steps.append(Step(5, "claude-shape", "wg_recent",
                      detail=f"facts={len(seen_claude_2)}",
                      fact_ids_seen=seen_claude_2))

    # 6. hermes-shape reads everything
    res = hermes_shape("wg_recent", {"limit": 50})
    seen_hermes_2 = normalize_fact_ids(res)
    steps.append(Step(6, "hermes-shape", "wg recent",
                      detail=f"facts={len(seen_hermes_2)}",
                      fact_ids_seen=seen_hermes_2))

    # 7. cross-client query for Redis must include fact_a
    res_q_codex = codex_shape("wg_query", {"topic": "Redis", "limit": 5, "depth": 1})
    res_q_hermes = hermes_shape("wg_query", {"topic": "Redis", "limit": 5})

    def query_has_fact(payload: dict, fact_id: str) -> bool:
        for key in ("recent_facts", "related"):
            block = payload.get(key) or []
            if any(isinstance(f, dict) and f.get("id") == fact_id for f in block):
                return True
        search = payload.get("search") or []
        return any(isinstance(s, dict) and s.get("fact_id") == fact_id for s in search)

    invariants = {
        "step2_sees_fact_a": fact_a in seen_codex_1,
        "step3_sees_fact_a": fact_a in seen_hermes_1,
        "step5_sees_both": fact_a in seen_claude_2 and fact_b in seen_claude_2,
        "step6_sees_both": fact_a in seen_hermes_2 and fact_b in seen_hermes_2,
        "all_three_agree_on_set": (
            sorted(seen_codex_1) != sorted(seen_hermes_1)
            and False  # placeholder — see below; we only require step2/3 sets equal AT THAT POINT
        ),
    }
    # Replace the buggy placeholder with the real invariant: at step
    # 2/3 (after the first insert), codex and hermes both see exactly
    # one fact and that fact equals fact_a.
    invariants["all_three_agree_on_set"] = (
        seen_codex_1 == seen_hermes_1 == [fact_a]
        and seen_claude_2 == seen_hermes_2 == sorted([fact_a, fact_b])
    )
    invariants["query_codex_sees_redis_fact"] = query_has_fact(res_q_codex, fact_a)
    invariants["query_hermes_sees_redis_fact"] = query_has_fact(res_q_hermes, fact_a)

    out = {
        "scenario": "B — cross-client store consistency",
        "store": STORE,
        "fact_ids": {"a": fact_a, "b": fact_b},
        "steps": [s.__dict__ for s in steps],
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
        },
    }
    out_path = Path("bench/multi-agent/results/scenario_b.json")
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
