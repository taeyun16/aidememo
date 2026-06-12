#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

python3 - "$ROOT_DIR" <<'PY'
import json
import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(sys.argv[1])
RUN_SMOKE = os.environ.get("AIDEMEMO_SDK_PROMOTION_RUN_SMOKE", "0") == "1"
RUN_SCENARIO_K = os.environ.get("AIDEMEMO_SDK_PROMOTION_RUN_SCENARIO_K", "0") == "1"
REQUIRE_PUBLIC = os.environ.get("AIDEMEMO_SDK_PROMOTION_REQUIRE_PUBLIC", "0") == "1"
JSON_OUT = os.environ.get("AIDEMEMO_SDK_PROMOTION_JSON", "0") == "1"


@dataclass
class Check:
    package: str
    criterion: str
    status: str
    detail: str


def run(cmd: list[str], timeout: int = 300) -> tuple[bool, str]:
    proc = subprocess.run(
        cmd,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
    )
    return proc.returncode == 0, proc.stdout.strip()


def has_tokens(path: str, tokens: list[str]) -> tuple[bool, str]:
    text = (ROOT / path).read_text(encoding="utf-8")
    missing = [token for token in tokens if token not in text]
    if missing:
        return False, "missing: " + ", ".join(missing)
    return True, "contains: " + ", ".join(tokens)


def append_status(
    checks: list[Check],
    package: str,
    criterion: str,
    ok: bool,
    ok_detail: str,
    fail_detail: str,
) -> None:
    checks.append(Check(package, criterion, "ok" if ok else "fail", ok_detail if ok else fail_detail))


def public_install_check(package: str, marker_env: str, registry: str) -> Check:
    if os.environ.get(marker_env, "0") == "1":
        return Check(package, "public registry install", "ok", f"{registry} install verified by {marker_env}=1")
    return Check(
        package,
        "public registry install",
        "blocked",
        f"requires first real {registry} release; set {marker_env}=1 after verification",
    )


def version_gate(checks: list[Check], package: str, script: str) -> None:
    ok, out = run([str(ROOT / script)], timeout=120)
    detail = out.splitlines()[-1] if out else script
    append_status(checks, package, "version gate", ok, detail, detail)


def optional_smoke(checks: list[Check], package: str, script: str, detail: str) -> None:
    if not RUN_SMOKE:
        checks.append(Check(package, "package install smoke", "ready", f"set AIDEMEMO_SDK_PROMOTION_RUN_SMOKE=1 to run {script}"))
        return
    ok, out = run([str(ROOT / script)], timeout=900)
    last = out.splitlines()[-1] if out else detail
    append_status(checks, package, "package install smoke", ok, last, last)


def optional_scenario_k(checks: list[Check]) -> None:
    package = "aidememo-python/aidememo-napi"
    if not RUN_SCENARIO_K:
        checks.append(
            Check(
                package,
                "workflow parity scenario",
                "ready",
                "set AIDEMEMO_SDK_PROMOTION_RUN_SCENARIO_K=1 to run Scenario K",
            )
        )
        return
    ok, out = run(["python3", "bench/multi-agent/scenario_k_sdk_workflow_parity.py"], timeout=300)
    detail = "Scenario K failed"
    if out:
        try:
            payload = json.loads(out)
            summary = payload.get("summary", {})
            summaries = payload.get("summaries", {})
            detail = (
                f"{summary.get('passed')}/{summary.get('total')} invariants; "
                f"p50 cli={summaries.get('cli', {}).get('p50_ms')}ms "
                f"python={summaries.get('python', {}).get('p50_ms')}ms "
                f"node={summaries.get('node', {}).get('p50_ms')}ms"
            )
        except json.JSONDecodeError:
            detail = out.splitlines()[-1]
    append_status(checks, package, "workflow parity scenario", ok, detail, detail)


def md_cell(value: object) -> str:
    return str(value).replace("|", "\\|").replace("\n", " ")


def markdown_summary(payload: dict) -> str:
    summary = payload["summary"]
    lines = [
        "## SDK promotion check",
        "",
        "| Package | Status | Criterion | Detail |",
        "|---|---|---|---|",
    ]
    for check in payload["checks"]:
        lines.append(
            "| "
            + " | ".join(
                md_cell(check[key])
                for key in ("package", "status", "criterion", "detail")
            )
            + " |"
        )
    lines.extend(
        [
            "",
            "| Metric | Value |",
            "|---|---:|",
            f"| ok | {summary['ok']} |",
            f"| ready | {summary['ready']} |",
            f"| blocked | {summary['blocked']} |",
            f"| fail | {summary['fail']} |",
            f"| total | {summary['total']} |",
            f"| local_ready | {str(summary['local_ready']).lower()} |",
            f"| sdk_promotable | {str(summary['sdk_promotable']).lower()} |",
        ]
    )
    return "\n".join(lines)


checks: list[Check] = []

checks.append(public_install_check("aidememo-python", "AIDEMEMO_PYTHON_PUBLIC_INSTALL_OK", "PyPI"))
version_gate(checks, "aidememo-python", "scripts/aidememo-python-version.sh")
ok, detail = has_tokens(
    "crates/aidememo-python/README.md",
    ["workflow_start", "source_id", "fact_add", "search", "query", "AideMemoNotFoundError"],
)
append_status(checks, "aidememo-python", "workflow docs", ok, detail, detail)
ok, detail = has_tokens(
    "crates/aidememo-python/README.md",
    ["session_id", "fact_add_many", "fact_pin", "pinned_facts"],
)
append_status(checks, "aidememo-python", "session/pinned docs", ok, detail, detail)
ok, detail = has_tokens(
    "crates/aidememo-python/src/lib.rs",
    ["session_id", "fn pinned_facts", "fn fact_pin", "attach_session_entity"],
)
append_status(checks, "aidememo-python", "session/pinned API", ok, detail, detail)
ok, detail = has_tokens(
    "crates/aidememo-python/src/lib.rs",
    ["create_exception!", "AideMemoNotFoundError", "AideMemoInvalidInputError", "e.code()"],
)
append_status(checks, "aidememo-python", "idiomatic errors", ok, detail, detail)
optional_smoke(checks, "aidememo-python", "scripts/aidememo-python-pack-smoke.sh", "wheel install smoke")

checks.append(public_install_check("aidememo-napi", "AIDEMEMO_NAPI_PUBLIC_INSTALL_OK", "npm"))
version_gate(checks, "aidememo-napi", "scripts/aidememo-napi-version.sh")
ok, detail = has_tokens(
    "crates/aidememo-napi/README.md",
    ["workflowStart", "sourceId", "factAdd", "search", "query", "error.code"],
)
append_status(checks, "aidememo-napi", "workflow docs", ok, detail, detail)
ok, detail = has_tokens(
    "crates/aidememo-napi/README.md",
    ["sessionId", "factAddMany", "factPin", "pinnedFacts"],
)
append_status(checks, "aidememo-napi", "session/pinned docs", ok, detail, detail)
ok, detail = has_tokens(
    "crates/aidememo-napi/src/lib.rs",
    ["session_id", "pub fn pinned_facts", "pub fn fact_pin", "attach_session_entity"],
)
append_status(checks, "aidememo-napi", "session/pinned API", ok, detail, detail)
ok, detail = has_tokens(
    "crates/aidememo-napi/src/lib.rs",
    ["Status::InvalidArg", "e.code()", "fn map_err"],
)
append_status(checks, "aidememo-napi", "idiomatic errors", ok, detail, detail)
optional_smoke(checks, "aidememo-napi", "scripts/aidememo-napi-pack-smoke.sh", "npm pack/install smoke")

ok, detail = has_tokens(
    "packages/aidememo-agent-sdk/README.md",
    ["workflow_start", "remember", "session_id", "source_id"],
)
append_status(checks, "aidememo-agent-sdk", "session workflow docs", ok, detail, detail)
ok, detail = has_tokens(
    "packages/aidememo-agent-sdk/src/aidememo_agent/client.py",
    ["pinned_facts", "session_id", "aidememo_fact_add", "aidememo_fact_add_many"],
)
append_status(checks, "aidememo-agent-sdk", "backend parity API", ok, detail, detail)
ok, detail = has_tokens(
    "packages/aidememo-agent-sdk/tests/test_sdk.py",
    ["test_pyo3_backend_preserves_session_and_context_scope", "pinned_facts", "session_id"],
)
append_status(checks, "aidememo-agent-sdk", "backend parity tests", ok, detail, detail)

optional_scenario_k(checks)

failures = [c for c in checks if c.status == "fail"]
blocking = [c for c in checks if c.status == "blocked"]
ready = [c for c in checks if c.status == "ready"]
ok_count = sum(1 for c in checks if c.status == "ok")

payload = {
    "checks": [c.__dict__ for c in checks],
    "summary": {
        "ok": ok_count,
        "ready": len(ready),
        "blocked": len(blocking),
        "fail": len(failures),
        "total": len(checks),
        "require_public": REQUIRE_PUBLIC,
        "local_ready": not failures,
        "sdk_promotable": not failures and not blocking,
    },
}

summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
if summary_path:
    with open(summary_path, "a", encoding="utf-8") as handle:
        handle.write(markdown_summary(payload))
        handle.write("\n")

if JSON_OUT:
    print(json.dumps(payload, indent=2))
else:
    print("sdk promotion check")
    print()
    print(f"{'package':<18} {'status':<8} {'criterion':<28} detail")
    print(f"{'-------':<18} {'------':<8} {'---------':<28} ------")
    for c in checks:
        print(f"{c.package:<18} {c.status:<8} {c.criterion:<28} {c.detail}")
    print()
    summary = payload["summary"]
    print(
        "summary: "
        f"ok={summary['ok']} ready={summary['ready']} "
        f"blocked={summary['blocked']} fail={summary['fail']} total={summary['total']}"
    )
    print(f"local_ready={str(summary['local_ready']).lower()} sdk_promotable={str(summary['sdk_promotable']).lower()}")
    if blocking and not REQUIRE_PUBLIC:
        print("note: public registry install blockers are expected until the real PyPI/npm releases land")

if failures or (REQUIRE_PUBLIC and blocking):
    sys.exit(1)
PY
