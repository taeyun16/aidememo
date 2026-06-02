#!/usr/bin/env python3
"""Scenario M - installed MCP source defaults are usable.

P3.38/P3.39 made `aidememo mcp-install --source-id` the smooth path for shared-store
MCP agents. This zero-token scenario validates the install contract at the
product boundary:

  1. Install file-edit MCP targets into an isolated HOME.
  2. Verify Codex / Cursor / OpenCode configs contain AIDEMEMO_SOURCE_ID.
  3. Verify shell-out targets report the env injection in --print mode.
  4. Feed the installed Codex env into `aidememo mcp` and prove writes/searches are
     source-scoped without explicit source_id tool arguments.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback.
    tomllib = None  # type: ignore[assignment]


REPO = Path(__file__).resolve().parents[2]
WG = os.environ.get("AIDEMEMO_BIN", str(REPO / "target" / "debug" / "aidememo"))
BASE = Path(os.environ.get("AIDEMEMO_E2E_BASE", str(Path(tempfile.gettempdir()) / "aidememo-e2e-m")))
HOME = BASE / "home"
STORE = str(BASE / "install-source-defaults.redb")
SOURCE_ID = "agent-alpha"


def run(
    cmd: list[str],
    *,
    input_text: str | None = None,
    env: dict[str, str] | None = None,
    timeout: int = 30,
) -> subprocess.CompletedProcess:
    child_env = os.environ.copy()
    child_env.update({"HOME": str(HOME)})
    if env:
        child_env.update(env)
    proc = subprocess.run(
        cmd,
        input=input_text,
        capture_output=True,
        text=True,
        env=child_env,
        timeout=timeout,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\n"
            f"stdout={proc.stdout[:1000]}\nstderr={proc.stderr[:1600]}"
        )
    return proc


def reset() -> None:
    if BASE.exists():
        shutil.rmtree(BASE)
    HOME.mkdir(parents=True, exist_ok=True)


def install(target: str, *, print_only: bool = False) -> dict[str, Any]:
    cmd = [
        WG,
        "--json",
        "mcp-install",
        "--target",
        target,
        "--source-id",
        SOURCE_ID,
        "--no-verify",
    ]
    if print_only:
        cmd.append("--print")
    return json.loads(run(cmd).stdout)


def read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text())


def read_codex_source_id() -> str | None:
    path = HOME / ".codex" / "config.toml"
    text = path.read_text()
    if tomllib is not None:
        return (
            tomllib.loads(text)
            .get("mcp_servers", {})
            .get("aidememo", {})
            .get("env", {})
            .get("AIDEMEMO_SOURCE_ID")
        )
    for line in text.splitlines():
        if line.strip().startswith("AIDEMEMO_SOURCE_ID"):
            return line.split("=", 1)[1].strip().strip('"')
    return None


def mcp_tool_call(
    name: str,
    args: dict[str, Any],
    *,
    env: dict[str, str] | None = None,
) -> dict[str, Any]:
    calls = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}},
        },
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        },
    ]
    proc = run(
        [WG, "--store", STORE, "mcp"],
        input_text="".join(json.dumps(call) + "\n" for call in calls),
        env=env,
    )
    responses = [
        json.loads(line) for line in proc.stdout.splitlines() if line.strip().startswith("{")
    ]
    response = {item.get("id"): item for item in responses}.get(2) or {}
    if "error" in response:
        raise RuntimeError(f"MCP {name} failed: {response['error']}")
    content = response.get("result", {}).get("content") or []
    if not content:
        raise RuntimeError(f"MCP {name} returned no content: {response}")
    return json.loads(content[0]["text"])


def main() -> int:
    reset()
    start = time.perf_counter_ns()

    file_reports = {target: install(target) for target in ["codex", "cursor", "opencode"]}
    shell_reports = {
        target: install(target, print_only=True)
        for target in ["claude", "hermes", "openclaw"]
    }

    codex_source_id = read_codex_source_id()
    cursor = read_json(HOME / ".cursor" / "mcp.json")
    opencode = read_json(HOME / ".config" / "opencode" / "opencode.json")

    env = {"AIDEMEMO_SOURCE_ID": codex_source_id or ""}
    add_payload = mcp_tool_call(
        "aidememo_fact_add",
        {
            "content": "Decision: installed MCP source defaults scope agent writes.",
            "fact_type": "decision",
            "entities": ["McpInstallDefaults"],
        },
        env=env,
    )
    search_payload = mcp_tool_call(
        "aidememo_search",
        {
            "query": "installed MCP source defaults",
            "bm25_only": True,
            "limit": 5,
        },
        env=env,
    )

    elapsed_ms = (time.perf_counter_ns() - start) / 1e6
    results = search_payload.get("results", [])

    invariants = {
        "codex_report_verified": file_reports["codex"].get("verified") is True,
        "cursor_report_verified": file_reports["cursor"].get("verified") is True,
        "opencode_report_verified": file_reports["opencode"].get("verified") is True,
        "codex_config_has_source_id": codex_source_id == SOURCE_ID,
        "cursor_config_has_source_id": cursor["mcpServers"]["aidememo"]["env"]["AIDEMEMO_SOURCE_ID"]
        == SOURCE_ID,
        "opencode_config_has_source_id": opencode["mcp"]["aidememo"]["env"]["AIDEMEMO_SOURCE_ID"]
        == SOURCE_ID,
        "claude_print_has_env": "AIDEMEMO_SOURCE_ID=agent-alpha"
        in shell_reports["claude"].get("detail", ""),
        "hermes_print_has_env": "--env AIDEMEMO_SOURCE_ID=agent-alpha"
        in shell_reports["hermes"].get("detail", ""),
        "openclaw_print_has_env": "AIDEMEMO_SOURCE_ID"
        in shell_reports["openclaw"].get("detail", ""),
        "mcp_write_used_installed_source_id": add_payload.get("source_id") == SOURCE_ID,
        "mcp_search_used_installed_source_id": any(
            row.get("source_id") == SOURCE_ID
            and "installed MCP source defaults" in row.get("content", "")
            for row in results
        ),
        "mcp_add_returned_fact_id": isinstance(add_payload.get("id"), str),
    }

    out = {
        "scenario": "M - installed MCP source defaults are usable",
        "home": str(HOME),
        "store": STORE,
        "latency_ms": round(elapsed_ms, 2),
        "file_reports": file_reports,
        "shell_reports": shell_reports,
        "installed_env": {"AIDEMEMO_SOURCE_ID": codex_source_id},
        "mcp": {
            "add_source_id": add_payload.get("source_id"),
            "search_hit_count": len(results),
            "search_source_ids": sorted(
                {row.get("source_id") for row in results if row.get("source_id")}
            ),
        },
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for ok in invariants.values() if ok),
            "total": len(invariants),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_m.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
