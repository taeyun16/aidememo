#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${AIDEMEMO_RELEASE_PREFLIGHT_PROFILE:-local}" # local | full
BASE="${AIDEMEMO_RELEASE_PREFLIGHT_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-release-preflight.XXXXXX")}"
SUMMARY_TSV="$BASE/release-preflight.tsv"

if [[ "$#" -gt 1 ]]; then
    echo "usage: $0 [semver]" >&2
    echo "example: AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full $0 0.1.1" >&2
    exit 1
fi

VERSION="${1:-}"

case "$PROFILE" in
    local | full) ;;
    *)
        echo "AIDEMEMO_RELEASE_PREFLIGHT_PROFILE must be local or full (got $PROFILE)" >&2
        exit 1
        ;;
esac

if [[ "$PROFILE" == "full" ]]; then
    RUN_PUBLISH="${AIDEMEMO_RELEASE_PREFLIGHT_PUBLISH:-1}"
    RUN_BINDINGS_OPTIONAL="${AIDEMEMO_RELEASE_PREFLIGHT_BINDINGS_OPTIONAL:-1}"
    REQUIRE_ACTIONLINT="${AIDEMEMO_RELEASE_PREFLIGHT_REQUIRE_ACTIONLINT:-1}"
else
    RUN_PUBLISH="${AIDEMEMO_RELEASE_PREFLIGHT_PUBLISH:-0}"
    RUN_BINDINGS_OPTIONAL="${AIDEMEMO_RELEASE_PREFLIGHT_BINDINGS_OPTIONAL:-0}"
    REQUIRE_ACTIONLINT="${AIDEMEMO_RELEASE_PREFLIGHT_REQUIRE_ACTIONLINT:-0}"
fi

RUN_BINDINGS="${AIDEMEMO_RELEASE_PREFLIGHT_BINDINGS:-1}"
RUN_CHANGELOG="${AIDEMEMO_RELEASE_PREFLIGHT_CHANGELOG:-1}"
RUN_WORKFLOW="${AIDEMEMO_RELEASE_PREFLIGHT_WORKFLOW:-1}"
RUN_DOCS="${AIDEMEMO_RELEASE_PREFLIGHT_DOCS:-1}"
RUN_STORAGE_BACKEND="${AIDEMEMO_RELEASE_PREFLIGHT_STORAGE_BACKEND:-1}"
RUN_ACTIONLINT="${AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT:-1}"
RUN_SDK_PROMOTION="${AIDEMEMO_RELEASE_PREFLIGHT_SDK_PROMOTION:-1}"
RUN_SDK_PROMOTION_SMOKE="${AIDEMEMO_RELEASE_PREFLIGHT_SDK_SMOKE:-0}"
RUN_SDK_PROMOTION_SCENARIO_K="${AIDEMEMO_RELEASE_PREFLIGHT_SDK_SCENARIO_K:-0}"
RUN_SDK_PROMOTION_REQUIRE_PUBLIC="${AIDEMEMO_RELEASE_PREFLIGHT_SDK_REQUIRE_PUBLIC:-0}"
ACTIONLINT_BIN="${AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT_BIN:-actionlint}"

have() {
    command -v "$1" >/dev/null 2>&1
}

record_skip() {
    local label="$1"
    local reason="$2"
    echo "==> skip: $label ($reason)"
    printf "skip\t-\t%s\t%s\n" "$label" "$reason" >> "$SUMMARY_TSV"
}

record_fail() {
    local label="$1"
    local reason="$2"
    echo "==> fail: $label ($reason)" >&2
    printf "fail\t0.00\t%s\t%s\n" "$label" "$reason" >> "$SUMMARY_TSV"
}

run() {
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

run_without_child_summary() {
    local label="$1"
    shift
    run "$label" env GITHUB_STEP_SUMMARY= "$@"
}

print_summary() {
    python3 - "$SUMMARY_TSV" <<'PY'
from pathlib import Path
import os
import sys

rows = []
for line in Path(sys.argv[1]).read_text().splitlines():
    status, elapsed, label, detail = line.split("\t", 3)
    rows.append((status, elapsed, label, detail))

timed = [float(elapsed) for status, elapsed, _, _ in rows if status != "skip"]
total = sum(timed)
lines = [
    "## release-preflight",
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

cd "$ROOT_DIR"
mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap print_summary EXIT

echo "release preflight"
echo "profile: $PROFILE"
echo "base: $BASE"
echo

if [[ -n "$VERSION" ]]; then
    run "release version gate" "$ROOT_DIR/scripts/aidememo-release-version.sh" "$VERSION"
else
    run "release version gate" "$ROOT_DIR/scripts/aidememo-release-version.sh"
fi
if [[ "$RUN_CHANGELOG" == "1" ]]; then
    if [[ -n "$VERSION" ]]; then
        run "changelog release gate" python3 "$ROOT_DIR/scripts/changelog-release-check.py" "$VERSION"
    else
        run "changelog release gate" python3 "$ROOT_DIR/scripts/changelog-release-check.py"
    fi
else
    record_skip "changelog release gate" "AIDEMEMO_RELEASE_PREFLIGHT_CHANGELOG=0"
fi
run "registry readiness gate" python3 "$ROOT_DIR/scripts/registry-readiness-check.py"

if [[ "$RUN_ACTIONLINT" == "1" ]]; then
    if have "$ACTIONLINT_BIN"; then
        run "workflow syntax lint" "$ACTIONLINT_BIN" .github/workflows/*.yml
    elif [[ "$REQUIRE_ACTIONLINT" == "1" ]]; then
        record_fail "workflow syntax lint" "$ACTIONLINT_BIN not installed"
        exit 1
    else
        record_skip "workflow syntax lint" "$ACTIONLINT_BIN not installed"
    fi
else
    record_skip "workflow syntax lint" "AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT=0"
fi

if [[ "$RUN_DOCS" == "1" ]]; then
    run "docs feature gate" "$ROOT_DIR/scripts/docs-feature-gate.py"
    run "docs site e2e" "$ROOT_DIR/scripts/docs-site-e2e.py"
else
    record_skip "docs feature gate" "AIDEMEMO_RELEASE_PREFLIGHT_DOCS=0"
    record_skip "docs site e2e" "AIDEMEMO_RELEASE_PREFLIGHT_DOCS=0"
fi

if [[ "$RUN_STORAGE_BACKEND" == "1" ]]; then
    run "storage backend feature gate" "$ROOT_DIR/scripts/storage-backend-feature-gate.sh"
    run "storage backend SQLite full surface" "$ROOT_DIR/scripts/storage-backend-sqlite-full-surface.sh"
    run "storage backend SQLite advanced surface" "$ROOT_DIR/scripts/storage-backend-sqlite-advanced-surface.sh"
    run "storage backend parity" "$ROOT_DIR/scripts/storage-backend-parity.sh"
    run "storage backend real corpus diff" "$ROOT_DIR/scripts/storage-backend-real-corpus-diff.sh"
    run "storage backend SQLite MCP soak" "$ROOT_DIR/scripts/storage-backend-sqlite-mcp-soak.sh"
    run "storage backend SDK binding check" "$ROOT_DIR/scripts/storage-backend-sdk-bindings-check.sh"
else
    record_skip "storage backend preflight" "AIDEMEMO_RELEASE_PREFLIGHT_STORAGE_BACKEND=0"
fi

if [[ "$RUN_BINDINGS" == "1" ]]; then
    run_without_child_summary "binding release smoke" env \
        AIDEMEMO_BINDINGS_SMOKE_OPTIONAL="$RUN_BINDINGS_OPTIONAL" \
        "$ROOT_DIR/scripts/bindings-release-smoke.sh"
else
    record_skip "binding release smoke" "AIDEMEMO_RELEASE_PREFLIGHT_BINDINGS=0"
fi

if [[ "$RUN_WORKFLOW" == "1" ]]; then
    run_without_child_summary "workflow release smoke" "$ROOT_DIR/scripts/workflow-release-smoke.sh"
else
    record_skip "workflow release smoke" "AIDEMEMO_RELEASE_PREFLIGHT_WORKFLOW=0"
fi

if [[ "$RUN_SDK_PROMOTION" == "1" ]]; then
    run_without_child_summary "sdk promotion check" env \
        AIDEMEMO_SDK_PROMOTION_RUN_SMOKE="$RUN_SDK_PROMOTION_SMOKE" \
        AIDEMEMO_SDK_PROMOTION_RUN_SCENARIO_K="$RUN_SDK_PROMOTION_SCENARIO_K" \
        AIDEMEMO_SDK_PROMOTION_REQUIRE_PUBLIC="$RUN_SDK_PROMOTION_REQUIRE_PUBLIC" \
        "$ROOT_DIR/scripts/sdk-promotion-check.sh"
else
    record_skip "sdk promotion check" "AIDEMEMO_RELEASE_PREFLIGHT_SDK_PROMOTION=0"
fi

if [[ "$RUN_PUBLISH" == "1" ]]; then
    run_without_child_summary "aidememo-python publish dry-run" "$ROOT_DIR/scripts/aidememo-python-publish-dry-run.sh"
    run_without_child_summary "aidememo-agent-sdk publish dry-run" "$ROOT_DIR/scripts/aidememo-agent-sdk-publish-dry-run.sh"
    run_without_child_summary "hermes-aidememo publish dry-run" "$ROOT_DIR/scripts/hermes-aidememo-publish-dry-run.sh"
    run_without_child_summary "aidememo-napi publish dry-run" "$ROOT_DIR/scripts/aidememo-napi-publish-dry-run.sh"
else
    record_skip "aidememo-python publish dry-run" "set AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full"
    record_skip "aidememo-agent-sdk publish dry-run" "set AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full"
    record_skip "hermes-aidememo publish dry-run" "set AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full"
    record_skip "aidememo-napi publish dry-run" "set AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full"
fi

echo
echo "OK: release preflight completed"
