#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CANDIDATE="${AIDEMEMO_SKILLOPT_CANDIDATE:-$ROOT_DIR/aidememo-skill/SKILL.md}"
RUN_SCENARIOS="${AIDEMEMO_SKILLOPT_RUN_SCENARIOS:-0}"
BASE="${AIDEMEMO_SKILLOPT_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-skillopt-lite.XXXXXX")}"
SUMMARY_TSV="$BASE/skillopt-lite.tsv"
AIDEMEMO_BIN="${AIDEMEMO_BIN:-$ROOT_DIR/target/debug/aidememo}"

mkdir -p "$BASE"
: > "$SUMMARY_TSV"

record() {
    local status elapsed label detail
    status="$1"
    elapsed="$2"
    label="$3"
    detail="${4:-}"
    printf "%s\t%s\t%s\t%s\n" "$status" "$elapsed" "$label" "$detail" >> "$SUMMARY_TSV"
}

run_timed() {
    local label start end status elapsed
    label="$1"
    shift
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
print(f"{float(sys.argv[2]) - float(sys.argv[1]):.2f}")
PY
)"
    if [[ "$status" == "0" ]]; then
        record ok "$elapsed" "$label" ""
    else
        record fail "$elapsed" "$label" "exit $status"
    fi
    echo "    elapsed: ${elapsed}s"
    return "$status"
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
    rows.append((status, elapsed, label, detail))

total = sum(float(elapsed) for _, elapsed, _, _ in rows)
lines = [
    "## skillopt-lite check",
    "",
    "| Status | Step | Seconds | Detail |",
    "|---|---|---:|---|",
]
for status, elapsed, label, detail in rows:
    lines.append(f"| {status} | `{label}` | {elapsed} | {detail} |")
lines.append(f"| total | | {total:.2f} | |")

text = "\n".join(lines)
print(text)

summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
if summary_path:
    with open(summary_path, "a", encoding="utf-8") as handle:
        handle.write(text)
        handle.write("\n")
PY
}

trap print_summary EXIT

if [[ ! -f "$CANDIDATE" ]]; then
    echo "candidate not found: $CANDIDATE" >&2
    exit 1
fi

candidate_check() {
    python3 - "$CANDIDATE" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
required = [
    "aidememo_workflow_start",
    "aidememo_context",
    "aidememo_query",
    "aidememo_aggregate",
    "aidememo_fact_add",
    "source_id",
]

missing = [token for token in required if token not in text]
if missing:
    raise SystemExit("candidate missing required memory workflow tokens: " + ", ".join(missing))

sdk_tokens = ["Memory.open", "search_rows", "remember"]
if any(token in text for token in sdk_tokens):
    missing_sdk = [token for token in sdk_tokens if token not in text]
    if missing_sdk:
        raise SystemExit("candidate has partial SDK profile; missing: " + ", ".join(missing_sdk))

if "aidememo_aggregate" in text and "simple recall" not in text and "simple retrieval" not in text:
    raise SystemExit("candidate mentions aidememo_aggregate but lacks a simple-recall/simple-retrieval guardrail")

print(f"candidate ok: {path}")
PY
}

skill_check() {
    if [[ ! -x "$AIDEMEMO_BIN" ]]; then
        cargo build -p aidememo-cli
    fi
    "$AIDEMEMO_BIN" skill check "$CANDIDATE"
}

cd "$ROOT_DIR"

run_timed "candidate memory profile tokens" candidate_check
run_timed "aidememo skill check candidate" skill_check
run_timed "git diff whitespace check" git diff --check
run_timed "cargo check aidememo-cli" cargo check -p aidememo-cli
run_timed "zero-token workflow demo" "$ROOT_DIR/scripts/demo-workflow.sh"
run_timed "sdk promotion gate" env GITHUB_STEP_SUMMARY= "$ROOT_DIR/scripts/sdk-promotion-check.sh"

if [[ "$RUN_SCENARIOS" == "1" ]]; then
    run_timed "scenario L self extraction" python3 bench/multi-agent/scenario_l_self_extraction.py
    run_timed "scenario M source/backend defaults" python3 bench/multi-agent/scenario_m_mcp_install_source_defaults.py
    run_timed "scenario N memory as code" python3 bench/multi-agent/scenario_n_hermes_memory_as_code.py
else
    record ready 0.00 "optional scenarios L/M/N" "set AIDEMEMO_SKILLOPT_RUN_SCENARIOS=1"
fi
