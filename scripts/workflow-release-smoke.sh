#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AIDEMEMO_BIN="${AIDEMEMO_BIN:-$ROOT_DIR/target/debug/aidememo}"
BASE="${AIDEMEMO_RELEASE_SMOKE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-release-smoke.XXXXXX")}"
SUMMARY_TSV="$BASE/workflow-release-smoke-timings.tsv"

run() {
    local label start end status elapsed
    label="$*"
    echo "==> $label"
    start="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    set +e
    "$@"
    status="$?"
    set -e
    end="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    elapsed="$(python3 - "$start" "$end" <<'PY'
import sys
start = float(sys.argv[1])
end = float(sys.argv[2])
print(f"{end - start:.2f}")
PY
)"
    if [[ "$status" == "0" ]]; then
        printf "ok\t%s\t%s\t\n" "$elapsed" "$label" >> "$SUMMARY_TSV"
    else
        printf "fail\t%s\t%s\texit %s\n" "$elapsed" "$label" "$status" >> "$SUMMARY_TSV"
    fi
    echo "    elapsed: ${elapsed}s"
    return "$status"
}

json_assert() {
    python3 - "$@"
}

print_summary() {
    if [[ ! -s "$SUMMARY_TSV" ]]; then
        return
    fi

    python3 - "$SUMMARY_TSV" <<'PY'
from pathlib import Path
import os
import sys

rows = []
for line in Path(sys.argv[1]).read_text().splitlines():
    status, elapsed, label, detail = line.split("\t", 3)
    rows.append((status, float(elapsed), label, detail))
total = sum(elapsed for _, elapsed, _, _ in rows)
lines = [
    "## workflow-release-smoke timings",
    "",
    "| Status | Step | Seconds | Detail |",
    "|---|---|---:|---|",
    *[
        f"| {status} | `{label}` | {elapsed:.2f} | {detail} |"
        for status, elapsed, label, detail in rows
    ],
    f"| total | | {total:.2f} | |",
]
text = "\n".join(lines)
print(text)

summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
if summary_path:
    with open(summary_path, "a", encoding="utf-8") as handle:
        handle.write(text)
        handle.write("\n")
PY
}

cd "$ROOT_DIR"
mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap print_summary EXIT

run cargo build -p aidememo-cli
run bash -n scripts/demo-workflow.sh
run python3 -m py_compile \
    bench/multi-agent/scenario_f_workflow_triggers.py \
    bench/multi-agent/scenario_i_workflow_doctor.py

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_DEMO_BASE="$BASE/demo-workflow" \
    scripts/demo-workflow.sh

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_STORE="$BASE/scenario-f/workflow.redb" \
    python3 bench/multi-agent/scenario_f_workflow_triggers.py

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_BASE="$BASE/scenario-i" \
    python3 bench/multi-agent/scenario_i_workflow_doctor.py

FIXTURE_STORE="$BASE/doctor-fixture/wiki.redb"
FIXTURE_HOME="$BASE/home"
mkdir -p "$FIXTURE_HOME/.codex"
cat > "$FIXTURE_HOME/.codex/config.toml" <<EOF
[mcp_servers.aidememo]
command = "$AIDEMEMO_BIN"
args = ["--store", "$FIXTURE_STORE", "mcp"]
EOF

run "$AIDEMEMO_BIN" \
    --store "$FIXTURE_STORE" \
    --json \
    workflow start \
    "Release smoke ticket" \
    --body "Verify workflow doctor readiness before release." \
    --source "smoke:release" \
    --source-id "release" \
    --bm25-only

doctor_json="$(
    HOME="$FIXTURE_HOME" \
    PATH="/nonexistent" \
    "$AIDEMEMO_BIN" --store "$FIXTURE_STORE" --json doctor
)"

json_assert "$doctor_json" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
workflow = payload.get("workflow") or {}
hints = [h.get("code") for h in workflow.get("hints") or []]

assert workflow.get("ready") is True, workflow
assert workflow.get("mcp_ready") is True, workflow
assert workflow.get("recent_ticket_count", 0) >= 1, workflow
assert "workflow_no_mcp_agent" not in hints, hints
assert "workflow_no_recent_tickets" not in hints, hints

print(json.dumps({
    "workflow_ready": workflow.get("ready"),
    "recent_ticket_count": workflow.get("recent_ticket_count"),
    "hint_codes": hints,
}, ensure_ascii=False))
PY

echo "OK: workflow release smoke passed"
echo "base: $BASE"
