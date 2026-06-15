#!/usr/bin/env python3
"""Scenario O - session canvas token-pressure regression.

This zero-token scenario covers the TencentDB-Agent-Memory comparison gap we
want without changing AideMemo's core posture: keep explicit typed facts as the
source of truth, then derive bounded read-only artifacts for long-session
continuation.

The script:

  1. Seeds durable project decisions / lessons / errors.
  2. Starts a workflow session and attaches many verbose task facts to it.
  3. Exports a bounded Markdown + Mermaid session canvas.
  4. Exports a read-only project profile.
  5. Verifies both artifacts keep fact-id drill-down and do not mutate the store.
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

REPO = Path(__file__).resolve().parents[2]
WG = os.environ.get("AIDEMEMO_BIN", str(REPO / "target" / "debug" / "aidememo"))
STORE = os.environ.get(
    "AIDEMEMO_E2E_STORE",
    str(Path(tempfile.gettempdir()) / "aidememo-e2e-o" / "session-canvas.sqlite"),
)


def run(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: int = 30,
) -> subprocess.CompletedProcess:
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
            f"{cmd!r} exited {proc.returncode}\nstdout={proc.stdout[:1000]}\nstderr={proc.stderr[:1600]}"
        )
    return proc


def mcp_tool(name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    init = {
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {"clientInfo": {"name": "scenario-o", "version": "0"}},
    }
    call = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }
    proc = run(
        [WG, "--store", STORE, "mcp"],
        input_text=f"{json.dumps(init)}\n{json.dumps(call)}\n",
    )
    responses = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
    response = next((row for row in responses if row.get("id") == 1), None)
    if response is None:
        raise RuntimeError(f"MCP returned no response for {name}: {proc.stdout[:1000]}")
    if response.get("error"):
        raise RuntimeError(f"MCP {name} failed: {response['error']}")
    result = response.get("result") or {}
    if result.get("isError"):
        raise RuntimeError(f"MCP {name} returned isError: {result}")
    text = "\n".join(
        str(block.get("text") or "")
        for block in result.get("content") or []
        if isinstance(block, dict) and block.get("type") == "text"
    ).strip()
    if not text:
        raise RuntimeError(f"MCP {name} returned no text content")
    return json.loads(text)


def reset_store() -> None:
    path = Path(STORE)
    path.parent.mkdir(parents=True, exist_ok=True)
    for sibling in path.parent.iterdir():
        if sibling.name.startswith(path.name):
            if sibling.is_dir():
                shutil.rmtree(sibling)
            else:
                sibling.unlink()


def fact_add(
    content: str,
    fact_type: str,
    entities: list[str],
    *,
    env: dict[str, str] | None = None,
) -> str:
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
        "scenario-o",
    ]
    return json.loads(run(cmd, env=env).stdout)["id"]


def stats() -> dict[str, Any]:
    return json.loads(run([WG, "--store", STORE, "--json", "stats"]).stdout)


def prepare_sdk_import() -> None:
    sdk_src = REPO / "packages" / "aidememo-agent-sdk" / "src"
    sys.path.insert(0, str(sdk_src))
    wg_path = Path(WG).resolve()
    if wg_path.name == "aidememo":
        bin_dir = wg_path.parent
    else:
        bin_dir = Path(tempfile.mkdtemp(prefix="aidememo-scenario-o-sdk-bin-"))
        (bin_dir / "aidememo").symlink_to(wg_path)
    os.environ["PATH"] = f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}"


def main() -> int:
    reset_store()
    if not Path(WG).exists():
        raise RuntimeError(f"AIDEMEMO_BIN does not exist: {WG}")

    seed_start = time.perf_counter_ns()
    seed_ids = [
        fact_add(
            "Decision: Long workflow continuation uses a bounded session canvas before deep fact drill-down.",
            "decision",
            ["AideMemo", "SessionCanvas"],
        ),
        fact_add(
            "Lesson: Long task recovery should keep fact ids visible so agents can verify details with fact_get.",
            "lesson",
            ["AideMemo", "SessionCanvas"],
        ),
        fact_add(
            "Error: Do not replace typed facts with an irreversible persona summary.",
            "error",
            ["AideMemo", "ProjectProfile"],
        ),
    ]
    seed_ms = (time.perf_counter_ns() - seed_start) / 1e6

    workflow = json.loads(
        run(
            [
                WG,
                "--store",
                STORE,
                "--json",
                "workflow",
                "start",
                "Continue a long Redis timeout investigation",
                "--body",
                "The agent needs to resume after many verbose tool observations without loading the whole thread.",
                "--source",
                "bench:scenario-o",
                "--source-id",
                "scenario-o",
                "--bm25-only",
            ]
        ).stdout
    )
    session_id = workflow["session_id"]
    env = os.environ.copy()
    env["AIDEMEMO_SESSION_ID"] = session_id

    for idx in range(60):
        fact_type = "lesson" if idx % 10 == 0 else "note"
        content = (
            f"Tool note {idx:02d}: Redis timeout probe captured verbose diagnostic block "
            f"with resolver state, retry window, queue lag, and mitigation candidate {idx}. "
            "The full detail intentionally repeats enough operational text to create "
            "session-token pressure while remaining deterministic for this benchmark."
        )
        fact_add(content, fact_type, ["Redis", "Worker", "SessionCanvas"], env=env)

    before = stats()
    raw_thread = run(
        [
            WG,
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

    out_dir = Path(tempfile.mkdtemp(prefix="aidememo-scenario-o-"))
    canvas_path = out_dir / "session_canvas.md"
    profile_path = out_dir / "project_profile.md"

    export_start = time.perf_counter_ns()
    canvas_meta = run(
        [
            WG,
            "--store",
            STORE,
            "--json",
            "session",
            "canvas",
            session_id,
            "--limit",
            "12",
            "--output",
            str(canvas_path),
        ]
    ).stdout
    profile_meta = run(
        [
            WG,
            "--store",
            STORE,
            "--json",
            "profile",
            "export",
            "--source-id",
            "scenario-o",
            "--limit",
            "20",
            "--output",
            str(profile_path),
        ]
    ).stdout

    canvas = canvas_path.read_text(encoding="utf-8")
    profile = profile_path.read_text(encoding="utf-8")

    mcp_canvas_payload = mcp_tool("aidememo_session_canvas", {"session": session_id, "limit": 12})
    mcp_profile_payload = mcp_tool(
        "aidememo_profile_export",
        {"source_id": "scenario-o", "limit": 20},
    )
    mcp_canvas = mcp_canvas_payload["content"]
    mcp_profile = mcp_profile_payload["content"]

    prepare_sdk_import()
    from aidememo_agent import AideMemoClient, Memory  # noqa: PLC0415

    sdk_client = AideMemoClient(
        store_path=STORE,
        source_id="scenario-o",
        storage_backend="libsqlite",
        lock_retry_ms=5000,
    )
    sdk_client._py = None
    sdk = Memory(sdk_client)
    sdk_canvas = sdk.session_canvas(session_id, limit=12)
    sdk_profile = sdk.project_profile(limit=20)

    export_ms = (time.perf_counter_ns() - export_start) / 1e6
    after = stats()

    mermaid = canvas.split("## Evidence Thread", 1)[0]
    canvas_verify_count = canvas.count("aidememo fact get")

    invariants = {
        "seed_ids_created": len(seed_ids) == 3,
        "session_id_recorded": session_id.startswith("session-"),
        "canvas_has_mermaid": "```mermaid" in canvas,
        "canvas_has_session": session_id in canvas,
        "canvas_has_drilldown": canvas_verify_count >= 12,
        "profile_has_evidence_contract": "## Evidence Contract" in profile,
        "profile_has_project_decision": "bounded session canvas" in profile,
        "artifacts_read_only": before == after,
        "mcp_canvas_matches_cli": mcp_canvas == canvas,
        "mcp_profile_matches_cli": mcp_profile == profile,
        "sdk_canvas_matches_cli": sdk_canvas == canvas,
        "sdk_profile_matches_cli": sdk_profile == profile,
        "bounded_canvas_smaller_than_full_thread": len(canvas) < len(raw_thread),
        "mermaid_map_smaller_than_full_thread": len(mermaid) < len(raw_thread) // 3,
    }

    out = {
        "scenario": "O - session canvas token pressure",
        "store": STORE,
        "session_id": session_id,
        "counts": {
            "seed_ids": len(seed_ids),
            "session_facts_raw_json_bytes": len(raw_thread),
            "session_canvas_bytes": len(canvas),
            "session_canvas_verify_refs": canvas_verify_count,
            "project_profile_bytes": len(profile),
            "mermaid_map_bytes": len(mermaid),
            "mcp_session_canvas_bytes": len(mcp_canvas),
            "mcp_project_profile_bytes": len(mcp_profile),
            "sdk_session_canvas_bytes": len(sdk_canvas),
            "sdk_project_profile_bytes": len(sdk_profile),
        },
        "timing_ms": {
            "seed": round(seed_ms, 2),
            "artifact_export": round(export_ms, 2),
        },
        "canvas_meta": json.loads(canvas_meta),
        "profile_meta": json.loads(profile_meta),
        "mcp_canvas_meta": {
            key: value for key, value in mcp_canvas_payload.items() if key != "content"
        },
        "mcp_profile_meta": {
            key: value for key, value in mcp_profile_payload.items() if key != "content"
        },
        "invariants": invariants,
        "ok": all(invariants.values()),
    }
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
