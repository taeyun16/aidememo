#!/usr/bin/env python3
"""Scenario H — natural-language workflow adoption across agents.

This is the token-burning follow-up to Scenario F/G. It asks whether
real coding agents naturally call the workflow entry point when a
sparse ticket arrives, rather than merely proving the tool works.

For each agent, the script resets and seeds the same store, sends one
prompt, then inspects both:

  - Store side effect: a ``workflow-start`` question fact was created.
  - Final answer: at least two seeded project-memory facts are reflected.

Expected agents:

  - Claude Code via a temporary project ``.mcp.json`` with AIDEMEMO_SOURCE_ID
  - Codex CLI via an isolated ``aidememo mcp-install --source-id`` config
  - Hermes via the aidememo plugin, explicit ``aidememo`` toolset, and AIDEMEMO_SOURCE_ID override

This burns model tokens. Do not put it in default CI.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

STORE = os.environ.get("AIDEMEMO_E2E_STORE", "/Users/mixlink/.aidememo-e2e/wiki.redb")
WG = os.environ.get("AIDEMEMO_BIN", "/Users/mixlink/dev/aidememo/target/debug/aidememo")
CLAUDE_BIN = os.environ.get("CLAUDE_BIN", "/Users/mixlink/.local/bin/claude")
CODEX_BIN = os.environ.get("CODEX_BIN", "/opt/homebrew/bin/codex")
HERMES_BIN = os.environ.get("HERMES_BIN", "/Users/mixlink/.local/bin/hermes")
TIMEOUT_S = int(os.environ.get("AIDEMEMO_E2E_AGENT_TIMEOUT", "240"))
AGENT_FILTER = {
    name.strip()
    for name in os.environ.get("AIDEMEMO_E2E_AGENTS", "").split(",")
    if name.strip()
}
SETUP_ONLY = os.environ.get("AIDEMEMO_E2E_SETUP_ONLY") == "1"

SOURCE_ID = "workflow-alpha"
FORBIDDEN_SOURCE_ID = "workflow-beta"

SEED = [
    (
        "Decision: Redis timeout fixes must go through the Worker job wrapper, not the Redis client.",
        "decision",
        ["Redis", "Worker"],
        SOURCE_ID,
    ),
    (
        "Lesson: The last Worker Redis timeout was DNS resolution, not pool size.",
        "lesson",
        ["Redis", "Worker"],
        SOURCE_ID,
    ),
    (
        "Error: Avoid increasing Redis pool size before checking DNS metrics.",
        "error",
        ["Redis", "Worker"],
        SOURCE_ID,
    ),
    (
        "Decision: Redis timeout fixes for beta edge traffic belong in edge cache config.",
        "decision",
        ["Redis", "EdgeCache"],
        FORBIDDEN_SOURCE_ID,
    ),
]

EXPECTED_PHRASES = [
    "Worker job wrapper",
    "DNS resolution",
    "DNS metrics",
]
FORBIDDEN_PHRASES = [
    "edge cache config",
    "beta edge traffic",
]

PROMPT = f"""
Issue #123: Fix Redis timeout in worker.

The issue body is intentionally sparse: worker jobs intermittently time
out against Redis.

Before making a plan, call the aidememo workflow-start tool for this ticket
(`aidememo_workflow_start` in MCP/Hermes, not a shell command).
Do not pass a source_id argument; the aidememo MCP server / Hermes plugin is
already configured with the project source namespace. The resulting
workflow context should still be scoped so neighbouring project memory
does not leak in.
Then answer with a concise plan that explicitly cites the relevant prior
decision, lesson, and error if they exist. Do not guess prior context.
""".strip()


@dataclass
class AgentSpec:
    name: str
    cmd: list[str]
    prompt: str = PROMPT
    cwd: str | None = None
    extra_env: dict[str, str] | None = None


def run(cmd: list[str], *, timeout: int = 20) -> subprocess.CompletedProcess:
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\nstdout={proc.stdout[:500]}\nstderr={proc.stderr[:1000]}"
        )
    return proc


def run_with_env(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    timeout: int = 20,
) -> subprocess.CompletedProcess:
    child_env = os.environ.copy()
    if env:
        child_env.update(env)
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, env=child_env)
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\nstdout={proc.stdout[:500]}\nstderr={proc.stderr[:1000]}"
        )
    return proc


def reset_store() -> None:
    p = Path(STORE)
    p.parent.mkdir(parents=True, exist_ok=True)
    for sibling in p.parent.iterdir():
        if sibling.name.startswith(p.name):
            if sibling.is_dir():
                shutil.rmtree(sibling)
            else:
                sibling.unlink()


def fact_add(content: str, fact_type: str, entities: list[str], source_id: str) -> str:
    cmd = [
        WG,
        "--store",
        STORE,
        "--json",
        "fact",
        "add",
        content,
        "--type",
        fact_type,
        "--entities",
        ",".join(entities),
        "--source-id",
        source_id,
    ]
    return json.loads(run(cmd).stdout)["id"]


def seed_store() -> list[str]:
    return [fact_add(*item) for item in SEED]


def workflow_question_facts() -> list[dict[str, Any]]:
    proc = run([WG, "--store", STORE, "--json", "fact", "list", "--type", "question", "-l", "100"])
    payload = json.loads(proc.stdout or "[]")
    facts = payload.get("facts") if isinstance(payload, dict) else payload
    return [
        f
        for f in facts
        if isinstance(f, dict) and "workflow-start" in (f.get("tags") or [])
    ]


def write_claude_project(tmpdir: Path) -> None:
    (tmpdir / ".mcp.json").write_text(
        json.dumps(
            {
                "mcpServers": {
                    "aidememo": {
                        "type": "stdio",
                        "command": WG,
                        "args": ["--store", STORE, "mcp"],
                        "env": {"AIDEMEMO_SOURCE_ID": SOURCE_ID},
                    }
                }
            },
            indent=2,
        )
    )
    settings = tmpdir / ".claude" / "settings.local.json"
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text(
        json.dumps(
            {
                "enableAllProjectMcpServers": True,
                "permissions": {"allow": ["mcp__aidememo"]},
            },
            indent=2,
        )
    )


def prepare_hermes_home(tmpdir: Path) -> Path:
    home = tmpdir / "hermes-home"
    home.mkdir(parents=True, exist_ok=True)

    real_home = Path.home() / ".hermes"
    for name in ("auth.json", ".env"):
        src = real_home / name
        if src.exists():
            shutil.copy(src, home / name)

    (home / "config.yaml").write_text(
        "\n".join(
            [
                "plugins:",
                "  aidememo:",
                f"    store_path: {STORE}",
                f"    source_id: {SOURCE_ID}",
                "",
                "mcp_servers:",
                "  aidememo:",
                f"    command: {WG}",
                "    args:",
                "      - --store",
                f"      - {STORE}",
                "      - mcp",
                "    env:",
                f"      AIDEMEMO_SOURCE_ID: {SOURCE_ID}",
                "    enabled: true",
                "",
            ]
        )
    )
    return home


def prepare_codex_home(tmpdir: Path) -> tuple[Path, Path, dict[str, Any]]:
    home = tmpdir / "codex-home"
    codex_home = home / ".codex"
    codex_home.mkdir(parents=True, exist_ok=True)

    real_home = Path.home() / ".codex"
    auth = real_home / "auth.json"
    if auth.exists():
        shutil.copy(auth, codex_home / "auth.json")

    (codex_home / "config.toml").write_text(
        "\n".join(
            [
                'model = "gpt-5.5"',
                'model_reasoning_effort = "medium"',
                'sandbox_mode = "danger-full-access"',
                'approval_policy = "never"',
                "",
                "[features]",
                "apps = true",
                "",
                f'[projects."{tmpdir}"]',
                'trust_level = "trusted"',
                "",
            ]
        )
    )
    report = json.loads(
        run_with_env(
            [
                WG,
                "--json",
                "mcp-install",
                "--target",
                "codex",
                "--source-id",
                SOURCE_ID,
                "--force",
                "--no-verify",
            ],
            env={"HOME": str(home)},
        ).stdout
    )
    run_with_env(
        [WG, "config", "set", "store.path", STORE],
        env={"HOME": str(home)},
    )
    return home, codex_home, report


def setup_summary(
    project_dir: Path,
    codex_home: Path,
    hermes_home: Path,
    codex_install: dict[str, Any],
) -> dict[str, Any]:
    claude = json.loads((project_dir / ".mcp.json").read_text())
    codex_config = (codex_home / "config.toml").read_text()
    hermes_config = (hermes_home / "config.yaml").read_text()
    invariants = {
        "claude_project_mcp_has_source_env": claude["mcpServers"]["aidememo"]["env"]["AIDEMEMO_SOURCE_ID"]
        == SOURCE_ID,
        "codex_install_report_has_source_id": codex_install.get("source_id") == SOURCE_ID,
        "codex_config_has_source_env": "AIDEMEMO_SOURCE_ID" in codex_config
        and SOURCE_ID in codex_config,
        "hermes_plugin_config_has_source_id": f"source_id: {SOURCE_ID}" in hermes_config,
        "hermes_mcp_config_has_source_env": "AIDEMEMO_SOURCE_ID" in hermes_config
        and SOURCE_ID in hermes_config,
    }
    return {
        "codex_mcp_install": codex_install,
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for ok in invariants.values() if ok),
            "total": len(invariants),
        },
    }


def run_agent(spec: AgentSpec) -> dict[str, Any]:
    env = os.environ.copy()
    if spec.extra_env:
        env.update(spec.extra_env)
    start = time.perf_counter_ns()
    try:
        proc = subprocess.run(
            [*spec.cmd, spec.prompt],
            cwd=spec.cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_S,
        )
        wall_ms = (time.perf_counter_ns() - start) / 1e6
        return {
            "agent": spec.name,
            "returncode": proc.returncode,
            "wall_ms": round(wall_ms, 2),
            "stdout": proc.stdout,
            "stderr": proc.stderr[-3000:],
        }
    except subprocess.TimeoutExpired as exc:
        return {
            "agent": spec.name,
            "returncode": -1,
            "wall_ms": -1,
            "stdout": exc.stdout or "",
            "stderr": f"TIMEOUT after {TIMEOUT_S}s",
        }


def evaluate_run(raw: dict[str, Any]) -> dict[str, Any]:
    stdout = raw.get("stdout") or ""
    questions = workflow_question_facts()
    reflected = [phrase for phrase in EXPECTED_PHRASES if phrase in stdout]
    forbidden = [phrase for phrase in FORBIDDEN_PHRASES if phrase in stdout]
    scoped_questions = [
        f for f in questions if f.get("source_id") == SOURCE_ID
    ]
    raw.update(
        {
            "workflow_fact_count": len(questions),
            "scoped_workflow_fact_count": len(scoped_questions),
            "reflected_expected": reflected,
            "reflected_expected_count": len(reflected),
            "forbidden_mentions": forbidden,
            "forbidden_mentions_count": len(forbidden),
            "passed": (
                raw.get("returncode") == 0
                and len(scoped_questions) >= 1
                and len(reflected) >= 2
                and len(forbidden) == 0
            ),
        }
    )
    return raw


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="aidememo-e2e-h-claude-") as td:
        td_path = Path(td)
        write_claude_project(td_path)
        codex_home_root, codex_home, codex_install = prepare_codex_home(td_path)
        hermes_home = prepare_hermes_home(td_path)
        setup = setup_summary(td_path, codex_home, hermes_home, codex_install)

        if SETUP_ONLY:
            out = {
                "scenario": "H setup — source-default natural prompt config",
                "store": STORE,
                "source_id": SOURCE_ID,
                "setup": setup,
            }
            out_path = Path("bench/multi-agent/results/scenario_h_setup.json")
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
            print(json.dumps(out, indent=2, ensure_ascii=False))
            return 0 if setup["summary"]["passed"] == setup["summary"]["total"] else 1

        agents = [
            AgentSpec(
                name="claude",
                cmd=[
                    CLAUDE_BIN,
                    "--print",
                    "--permission-mode",
                    "bypassPermissions",
                ],
                cwd=str(td_path),
            ),
            AgentSpec(
                name="codex",
                cmd=[
                    CODEX_BIN,
                    "exec",
                    "--cd",
                    str(td_path),
                    "--skip-git-repo-check",
                    "--dangerously-bypass-approvals-and-sandbox",
                ],
                extra_env={"HOME": str(codex_home_root), "CODEX_HOME": str(codex_home)},
            ),
            AgentSpec(
                name="hermes",
                cmd=[
                    HERMES_BIN,
                    "chat",
                    "--max-turns",
                    "10",
                    "--accept-hooks",
                    "-Q",
                    "-q",
                ],
                cwd=str(td_path),
                extra_env={
                    "HERMES_HOME": str(hermes_home),
                    "AIDEMEMO_STORE": STORE,
                    "AIDEMEMO_SOURCE_ID": SOURCE_ID,
                },
            ),
        ]
        if AGENT_FILTER:
            agents = [spec for spec in agents if spec.name in AGENT_FILTER]

        runs = []
        seeded_by_agent: dict[str, list[str]] = {}
        for spec in agents:
            print(f"# running {spec.name}", file=sys.stderr)
            reset_store()
            seeded_by_agent[spec.name] = seed_store()
            raw = run_agent(spec)
            evaluated = evaluate_run(raw)
            print(
                f"#   {spec.name}: rc={evaluated['returncode']} "
                f"workflow={evaluated['scoped_workflow_fact_count']} "
                f"reflected={evaluated['reflected_expected_count']} "
                f"forbidden={evaluated['forbidden_mentions_count']} "
                f"passed={evaluated['passed']}",
                file=sys.stderr,
            )
            runs.append(evaluated)

    invariants = {
        "all_agents_created_workflow_fact": all(r["scoped_workflow_fact_count"] >= 1 for r in runs),
        "all_agents_reflected_memory": all(r["reflected_expected_count"] >= 2 for r in runs),
        "no_agent_leaked_forbidden_source": all(r["forbidden_mentions_count"] == 0 for r in runs),
        "all_agents_returned_success": all(r["returncode"] == 0 for r in runs),
    }

    out = {
        "scenario": "H — natural-language workflow adoption",
        "store": STORE,
        "prompt": PROMPT,
        "source_id": SOURCE_ID,
        "forbidden_source_id": FORBIDDEN_SOURCE_ID,
        "setup": setup,
        "seeded_by_agent": seeded_by_agent,
        "agents": runs,
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for v in invariants.values() if v),
            "total": len(invariants),
            "agents_passed": sum(1 for r in runs if r["passed"]),
            "agents_total": len(runs),
        },
    }
    out_path = Path("bench/multi-agent/results/scenario_h.json")
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
