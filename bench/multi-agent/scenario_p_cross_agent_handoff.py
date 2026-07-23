#!/usr/bin/env python3
"""Scenario P - cross-agent handoff protocol and context pressure.

This zero-token scenario validates the deterministic contract an orchestrator
needs before model-level handoff quality is worth measuring:

1. Start one source-scoped workflow and attach verbose task observations.
2. Add explicit decision / lesson / error / question checkpoints.
3. Attach a neighbouring-source fact to the same session to probe leakage.
4. Export one receiver-specific packet through CLI, MCP, and agent SDK.
5. Keep quality gates (evidence/route/parity/isolation) separate from context
   efficiency (bytes and estimated tokens versus raw thread / session canvas).

The scenario does not claim that a downstream model completes the task better;
it proves the portable handoff artifact preserves the preregistered evidence
contract with bounded context and no neighbouring-source leakage.
"""

from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
AM = os.environ.get("AIDEMEMO_BIN", str(REPO / "target" / "debug" / "aidememo"))
STORE = os.environ.get(
    "AIDEMEMO_E2E_STORE",
    str(Path(tempfile.gettempdir()) / "aidememo-e2e-p" / "cross-agent-handoff.sqlite"),
)
SOURCE_ID = "release-team"
NEIGHBOUR_SOURCE_ID = "release-team-beta"
SESSION_LIMIT = 16

CRITICAL_EVIDENCE = {
    "decision": "package smoke before the full release preflight",
    "lesson": "stale wheel metadata, not Rust compilation",
    "error": "installed wheel version differs from workspace metadata",
    "question": "rerun the Docusaurus link check",
}
ROUTE_MARKERS = [
    "from_agent: codex",
    "from_profile: coding",
    "to_agent: hermes",
    "to_profile: reviewer",
]
FOCUS = "Verify package metadata, then run release preflight if the package smoke passes."
DONE_WHEN = "The installed wheel version matches workspace metadata and the release preflight passes."
FORBIDDEN = "Beta-only instruction: skip package smoke and publish immediately."


def run(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: int = 30,
) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(
        cmd,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
        env=env,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\n"
            f"stdout={proc.stdout[:1200]}\nstderr={proc.stderr[:1800]}"
        )
    return proc


def reset_store() -> None:
    path = Path(STORE)
    path.parent.mkdir(parents=True, exist_ok=True)
    for sibling in path.parent.iterdir():
        if sibling.name.startswith(path.name):
            if sibling.is_dir():
                shutil.rmtree(sibling)
            else:
                sibling.unlink()


def stats() -> dict[str, Any]:
    return json.loads(run([AM, "--store", STORE, "--json", "stats"]).stdout)


def fact_add(
    content: str,
    fact_type: str,
    entities: list[str],
    *,
    source_id: str = SOURCE_ID,
    env: dict[str, str] | None = None,
) -> str:
    cmd = [
        AM,
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
    return str(json.loads(run(cmd, env=env).stdout)["id"])


def mcp_tool(name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    requests = [
        {
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {"clientInfo": {"name": "scenario-p", "version": "0"}},
        },
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        },
    ]
    proc = run(
        [AM, "--store", STORE, "mcp"],
        input_text="".join(json.dumps(request) + "\n" for request in requests),
    )
    responses = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
    response = next((row for row in responses if row.get("id") == 1), None)
    if response is None or response.get("error"):
        raise RuntimeError(f"MCP {name} failed: {response or proc.stdout[:1200]}")
    result = response.get("result") or {}
    if result.get("isError"):
        raise RuntimeError(f"MCP {name} returned isError: {result}")
    blocks = result.get("content") or []
    text = "\n".join(
        str(block.get("text") or "")
        for block in blocks
        if isinstance(block, dict) and block.get("type") == "text"
    ).strip()
    if not text:
        raise RuntimeError(f"MCP {name} returned no text")
    return json.loads(text)


def prepare_sdk() -> Any:
    sdk_src = REPO / "packages" / "aidememo-agent-sdk" / "src"
    sys.path.insert(0, str(sdk_src))
    binary = Path(AM).resolve()
    if binary.name == "aidememo":
        bin_dir = binary.parent
    else:
        bin_dir = Path(tempfile.mkdtemp(prefix="aidememo-scenario-p-sdk-bin-"))
        (bin_dir / "aidememo").symlink_to(binary)
    os.environ["PATH"] = f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}"

    from aidememo_agent import AideMemoClient, Memory  # noqa: PLC0415

    client = AideMemoClient(
        store_path=STORE,
        source_id=SOURCE_ID,
        storage_backend="libsqlite",
        lock_retry_ms=5000,
    )
    client._py = None
    return Memory(client)


def estimated_tokens(text: str) -> int:
    return math.ceil(len(text.encode("utf-8")) / 4)


def reduction_percent(candidate_bytes: int, baseline_bytes: int) -> float:
    if baseline_bytes <= 0:
        return 0.0
    return round((1.0 - candidate_bytes / baseline_bytes) * 100.0, 1)


def main() -> int:
    reset_store()
    if not Path(AM).exists():
        raise RuntimeError(f"AIDEMEMO_BIN does not exist: {AM}")

    workflow = json.loads(
        run(
            [
                AM,
                "--store",
                STORE,
                "--json",
                "workflow",
                "start",
                "Harden release preflight",
                "--body",
                "Codex is diagnosing a flaky package gate before a Hermes reviewer takes over.",
                "--source",
                "orchestrator:scenario-p/run-01",
                "--source-id",
                SOURCE_ID,
                "--bm25-only",
            ]
        ).stdout
    )
    session_id = str(workflow["session_id"])
    session_env = os.environ.copy()
    session_env["AIDEMEMO_SESSION_ID"] = session_id

    for idx in range(36):
        fact_add(
            (
                f"Tool observation {idx:02d}: package probe captured verbose resolver output, "
                f"wheel candidate paths, cache state, and retry metadata for attempt {idx}. "
                "This deterministic filler represents tool history that should not all cross "
                "the agent boundary."
            ),
            "note",
            ["Release", "Packaging"],
            env=session_env,
        )

    fact_add(
        "Decision: run the package smoke before the full release preflight.",
        "decision",
        ["Release", "Packaging"],
        env=session_env,
    )
    fact_add(
        "Lesson: the previous failure came from stale wheel metadata, not Rust compilation.",
        "lesson",
        ["Release", "Python"],
        env=session_env,
    )
    fact_add(
        "Error: do not publish when the installed wheel version differs from workspace metadata.",
        "error",
        ["Release", "Python"],
        env=session_env,
    )
    fact_add(
        "Open question: should the reviewer rerun the Docusaurus link check after package validation?",
        "question",
        ["Release", "Docs"],
        env=session_env,
    )
    fact_add(
        FORBIDDEN,
        "error",
        ["Release", "Packaging"],
        source_id=NEIGHBOUR_SOURCE_ID,
        env=session_env,
    )

    before = stats()
    raw_thread = run(
        [
            AM,
            "--store",
            STORE,
            "--json",
            "fact",
            "list",
            "--entity",
            session_id,
            "--limit",
            "500",
        ]
    ).stdout

    out_dir = Path(tempfile.mkdtemp(prefix="aidememo-scenario-p-"))
    canvas_path = out_dir / "session_canvas.md"
    handoff_path = out_dir / "agent_handoff.md"

    run(
        [
            AM,
            "--store",
            STORE,
            "session",
            "canvas",
            "--limit",
            str(SESSION_LIMIT),
            "--output",
            str(canvas_path),
            session_id,
        ]
    )

    route_args: dict[str, Any] = {
        "session_id": session_id,
        "from": "codex/coding",
        "to": "hermes/reviewer",
        "focus": FOCUS,
        "done_when": DONE_WHEN,
        "source_id": SOURCE_ID,
        "limit": SESSION_LIMIT,
    }
    export_start = time.perf_counter_ns()
    cli_meta = json.loads(
        run(
            [
                AM,
                "--store",
                STORE,
                "--json",
                "session",
                "handoff",
                "--from",
                "codex/coding",
                "--to",
                "hermes/reviewer",
                "--focus",
                FOCUS,
                "--done-when",
                DONE_WHEN,
                "--source-id",
                SOURCE_ID,
                "--limit",
                str(SESSION_LIMIT),
                "--output",
                str(handoff_path),
                session_id,
            ]
        ).stdout
    )
    handoff = handoff_path.read_text(encoding="utf-8")
    mcp_payload = mcp_tool("aidememo_handoff", route_args)
    sdk = prepare_sdk()
    sdk_handoff = sdk.handoff(
        session_id,
        from_route="codex/coding",
        to_route="hermes/reviewer",
        focus=FOCUS,
        done_when=DONE_WHEN,
        source_id=SOURCE_ID,
        limit=SESSION_LIMIT,
    )
    sdk_packet = sdk.handoff_packet(
        session_id,
        from_route="codex/coding",
        to_route="hermes/reviewer",
        focus=FOCUS,
        done_when=DONE_WHEN,
        source_id=SOURCE_ID,
        limit=SESSION_LIMIT,
    )
    export_ms = (time.perf_counter_ns() - export_start) / 1e6
    canvas = canvas_path.read_text(encoding="utf-8")
    after = stats()

    evidence_hits = {
        name: marker in handoff for name, marker in CRITICAL_EVIDENCE.items()
    }
    route_hits = {marker: marker in handoff for marker in ROUTE_MARKERS}
    quality_gates = {
        "critical_evidence_recall_4_of_4": all(evidence_hits.values()),
        "route_recall_4_of_4": all(route_hits.values()),
        "focus_preserved": FOCUS in handoff,
        "definition_of_done_preserved": DONE_WHEN in handoff,
        "session_resume_preserved": f"aidememo session resume --source-id '{SOURCE_ID}' '{session_id}'" in handoff,
        "source_scope_preserved": f"--source-id '{SOURCE_ID}'" in handoff,
        "fact_id_verification_present": "aidememo fact get <fact_id>" in handoff,
        "neighbour_source_leakage_zero": FORBIDDEN not in handoff,
        "cli_mcp_parity": mcp_payload["content"] == handoff,
        "cli_sdk_parity": sdk_handoff == handoff,
        "sdk_structured_packet_preserved": (
            sdk_packet.get("content") == handoff
            and sdk_packet.get("session_id") == session_id
            and sdk_packet.get("source_id") == SOURCE_ID
            and sdk_packet.get("done_when") == DONE_WHEN
        ),
        "artifact_read_only": before == after,
    }

    raw_bytes = len(raw_thread.encode("utf-8"))
    canvas_bytes = len(canvas.encode("utf-8"))
    handoff_bytes = len(handoff.encode("utf-8"))
    efficiency_gates = {
        "handoff_smaller_than_raw_thread": handoff_bytes < raw_bytes,
        "handoff_smaller_than_session_canvas": handoff_bytes < canvas_bytes,
        "raw_context_reduction_at_least_50_percent": reduction_percent(handoff_bytes, raw_bytes)
        >= 50.0,
    }

    out = {
        "scenario": "P - cross-agent handoff protocol",
        "claim_boundary": (
            "Zero-token protocol evidence only: this validates evidence preservation, routing, "
            "source isolation, parity, and context pressure; it does not measure downstream model task success."
        ),
        "store": STORE,
        "session_id": session_id,
        "route": {
            "from": "codex/coding",
            "to": "hermes/reviewer",
            "source_id": SOURCE_ID,
        },
        "quality": {
            "critical_evidence_hits": evidence_hits,
            "critical_evidence_recall": f"{sum(evidence_hits.values())}/{len(evidence_hits)}",
            "route_hits": route_hits,
            "route_recall": f"{sum(route_hits.values())}/{len(route_hits)}",
            "forbidden_leakage_count": handoff.count(FORBIDDEN),
        },
        "context": {
            "raw_thread_bytes": raw_bytes,
            "session_canvas_bytes": canvas_bytes,
            "handoff_bytes": handoff_bytes,
            "raw_thread_estimated_tokens": estimated_tokens(raw_thread),
            "session_canvas_estimated_tokens": estimated_tokens(canvas),
            "handoff_estimated_tokens": estimated_tokens(handoff),
            "handoff_reduction_vs_raw_percent": reduction_percent(handoff_bytes, raw_bytes),
            "handoff_reduction_vs_canvas_percent": reduction_percent(handoff_bytes, canvas_bytes),
        },
        "timing_ms": {"handoff_cli_mcp_sdk": round(export_ms, 2)},
        "cli_meta": cli_meta,
        "mcp_meta": {key: value for key, value in mcp_payload.items() if key != "content"},
        "quality_gates": quality_gates,
        "efficiency_gates": efficiency_gates,
        "summary": {
            "quality_passed": sum(quality_gates.values()),
            "quality_total": len(quality_gates),
            "efficiency_passed": sum(efficiency_gates.values()),
            "efficiency_total": len(efficiency_gates),
            "ok": all(quality_gates.values()) and all(efficiency_gates.values()),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_p.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
