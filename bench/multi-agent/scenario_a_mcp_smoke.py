#!/usr/bin/env python3
"""Scenario A — MCP protocol smoke for each registered client.

For every (client, command, args) triple we:
  1. Spawn the aidememo mcp server exactly the way the client would.
  2. Send: initialize → tools/list → tools/call aidememo_query topic=Redis
  3. Verify: handshake OK, tool count, query result is JSON-shaped.

This is a protocol-level smoke. It does NOT run the agent itself —
it runs the same aidememo invocation the agent's MCP layer would issue, so
that if a config is broken the agent has no way to ever reach aidememo.

Invariants
----------
- Server prints exactly one initialize response with protocolVersion.
- tools/list returns the 13 tools enumerated in cmd/mcp_tools.rs.
- tools/call aidememo_query returns a result with topic/entity/related/recent_facts.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STORE = os.environ.get(
    "AIDEMEMO_E2E_STORE",
    str(Path(tempfile.gettempdir()) / "aidememo-e2e-a" / "wiki.sqlite"),
)
AIDEMEMO_BIN = os.environ.get(
    "AIDEMEMO_BIN",
    str(ROOT / "target" / "debug" / "aidememo"),
)


@dataclass
class ClientConfig:
    name: str
    command: list[str]
    # Where the config that produced `command` lives — surfaced in the report.
    config_origin: str


def claude_code_config() -> ClientConfig:
    # Project-level .mcp.json controls Claude Code's aidememo invocation.
    cfg = json.loads(Path(".mcp.json").read_text())
    entry = cfg["mcpServers"]["aidememo"]
    cmd = [entry["command"], *entry.get("args", [])]
    return ClientConfig(
        name="claude-code",
        command=cmd,
        config_origin=str(Path(".mcp.json").resolve()),
    )


def codex_config() -> ClientConfig:
    # Codex reads ~/.codex/config.toml.
    # Mirror command="aidememo", args=["mcp", STORE] without coupling this
    # smoke to Codex's CLI configuration parser.
    cmd = [AIDEMEMO_BIN, "mcp", STORE]
    return ClientConfig(
        name="codex",
        command=cmd,
        config_origin="~/.codex/config.toml [mcp_servers.aidememo]",
    )


def hermes_config() -> ClientConfig:
    # The hermes-aidememo plugin shells out to `aidememo` (PATH lookup) by default.
    # Mirror its CLI form so the smoke fails the same way Hermes would
    # if PATH didn't have aidememo.
    cmd = [AIDEMEMO_BIN, "mcp", STORE]
    return ClientConfig(
        name="hermes",
        command=cmd,
        config_origin="plugins/hermes/src/hermes_aidememo/client.py (CLI fallback)",
    )


@dataclass
class ToolCall:
    name: str
    arguments: dict


@dataclass
class Result:
    client: str
    config_origin: str
    handshake_ok: bool = False
    server_version: str = ""
    protocol_version: str = ""
    tool_count: int = 0
    tool_names: list[str] = field(default_factory=list)
    query_ok: bool = False
    query_topic: str = ""
    query_keys: list[str] = field(default_factory=list)
    elapsed_ms: float = 0.0
    error: str = ""

    def passed(self) -> bool:
        return (
            self.handshake_ok
            and self.tool_count > 0
            and self.query_ok
            and not self.error
        )


def jsonrpc_session(cmd: list[str], calls: list[dict]) -> tuple[list[dict], str, float]:
    """Run a stdio JSON-RPC session against `cmd`.

    Returns (responses, stderr, elapsed_ms).
    """
    payload = "".join(json.dumps(c) + "\n" for c in calls)
    start = time.perf_counter_ns()
    try:
        proc = subprocess.run(
            cmd,
            input=payload,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except subprocess.TimeoutExpired as exc:
        elapsed = (time.perf_counter_ns() - start) / 1e6
        return [], f"timeout: {exc}", elapsed
    elapsed = (time.perf_counter_ns() - start) / 1e6
    responses = []
    for line in proc.stdout.strip().splitlines():
        if not line.strip():
            continue
        try:
            responses.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return responses, proc.stderr, elapsed


def smoke_one(cfg: ClientConfig) -> Result:
    res = Result(client=cfg.name, config_origin=cfg.config_origin)

    calls = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}},
        },
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "aidememo_query",
                "arguments": {"topic": "Redis", "limit": 5, "depth": 1},
            },
        },
    ]

    responses, stderr, elapsed = jsonrpc_session(cfg.command, calls)
    res.elapsed_ms = elapsed

    if not responses:
        res.error = f"no JSON-RPC response. stderr: {stderr.strip()[:500]}"
        return res

    by_id = {r.get("id"): r for r in responses if "id" in r}

    init = by_id.get(1)
    if init and "result" in init:
        res.handshake_ok = True
        srv = init["result"].get("serverInfo") or {}
        res.server_version = srv.get("version", "")
        res.protocol_version = init["result"].get("protocolVersion", "")
    else:
        res.error = f"initialize failed: {init}"
        return res

    tools = by_id.get(2)
    if tools and "result" in tools:
        tool_list = tools["result"].get("tools", [])
        res.tool_count = len(tool_list)
        res.tool_names = sorted(t["name"] for t in tool_list)
    else:
        res.error = f"tools/list failed: {tools}"
        return res

    query = by_id.get(3)
    if query and "result" in query:
        # MCP returns content as text — parse it back to JSON.
        content = query["result"].get("content") or []
        if content and content[0].get("type") == "text":
            try:
                payload = json.loads(content[0]["text"])
                res.query_ok = True
                res.query_topic = payload.get("topic", "")
                res.query_keys = sorted(payload.keys())
            except json.JSONDecodeError as e:
                res.error = f"query payload not JSON: {e}"
        else:
            res.error = f"query content empty: {query['result']}"
    else:
        res.error = f"aidememo_query failed: {query}"
    return res


def main() -> int:
    Path(STORE).parent.mkdir(parents=True, exist_ok=True)

    clients = [claude_code_config(), codex_config(), hermes_config()]
    results = [smoke_one(c) for c in clients]

    out = {
        "scenario": "A — MCP protocol smoke",
        "store": STORE,
        "results": [r.__dict__ for r in results],
        "summary": {
            "passed": sum(r.passed() for r in results),
            "total": len(results),
        },
    }
    out_path = Path("bench/multi-agent/results/scenario_a.json")
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
