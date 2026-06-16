#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE="${1:-}"
tmp_dir=""

case "$PACKAGE" in
    aidememo-agent-sdk)
        PACKAGE_DIR="$ROOT_DIR/packages/aidememo-agent-sdk"
        EXPECT_VERSION="${AIDEMEMO_AGENT_SDK_EXPECT_VERSION:-}"
        BASE="${AIDEMEMO_AGENT_SDK_PUBLISH_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-agent-sdk-publish.XXXXXX")}"
        DIST_OVERRIDE="${AIDEMEMO_AGENT_SDK_DIST_DIR:-}"
        ;;
    hermes-aidememo)
        PACKAGE_DIR="$ROOT_DIR/plugins/hermes"
        EXPECT_VERSION="${HERMES_AIDEMEMO_EXPECT_VERSION:-}"
        BASE="${HERMES_AIDEMEMO_PUBLISH_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/hermes-aidememo-publish.XXXXXX")}"
        DIST_OVERRIDE="${HERMES_AIDEMEMO_DIST_DIR:-}"
        ;;
    *)
        echo "usage: $0 <aidememo-agent-sdk|hermes-aidememo>" >&2
        exit 1
        ;;
esac

SUMMARY_TSV="$BASE/$PACKAGE-publish.tsv"

timer_now() {
    python3 - <<'PY'
import time
print(time.perf_counter())
PY
}

elapsed_since() {
    python3 - "$1" <<'PY'
import sys
import time
start = float(sys.argv[1])
print(f"{time.perf_counter() - start:.2f}")
PY
}

record_summary_row() {
    local status elapsed label row_status detail
    status="$1"
    elapsed="$2"
    label="$3"
    if [[ "$status" == "0" ]]; then
        row_status="ok"
        detail=""
    else
        row_status="fail"
        detail="exit $status"
    fi
    printf "%s\t%s\t%s\t%s\n" "$row_status" "$elapsed" "$label" "$detail" >> "$SUMMARY_TSV"
}

run_timed() {
    local label start status elapsed
    label="$1"
    shift
    echo "==> $label"
    start="$(timer_now)"
    set +e
    "$@"
    status="$?"
    set -e
    elapsed="$(elapsed_since "$start")"
    record_summary_row "$status" "$elapsed" "$label"
    echo "    elapsed: ${elapsed}s"
    return "$status"
}

record_fail() {
    local label="$1"
    local reason="$2"
    echo "==> fail: $label ($reason)" >&2
    printf "fail\t0.00\t%s\t%s\n" "$label" "$reason" >> "$SUMMARY_TSV"
}

print_summary() {
    if [[ ! -s "$SUMMARY_TSV" ]]; then
        return
    fi
    python3 - "$SUMMARY_TSV" "$PACKAGE" <<'PY'
from pathlib import Path
import os
import sys

rows = []
for line in Path(sys.argv[1]).read_text().splitlines():
    status, elapsed, label, detail = line.split("\t", 3)
    rows.append((status, elapsed, label, detail))

total = sum(float(elapsed) for _, elapsed, _, _ in rows if elapsed != "-")
lines = [
    f"## {sys.argv[2]}-publish-dry-run",
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

cleanup() {
    print_summary
    if [[ -n "$tmp_dir" ]]; then
        rm -rf "$tmp_dir"
    fi
}

mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap cleanup EXIT

version="$(
    python3 - "$PACKAGE_DIR/pyproject.toml" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as f:
    print(tomllib.load(f)["project"]["version"])
PY
)"

if [[ -n "$EXPECT_VERSION" && "$version" != "$EXPECT_VERSION" ]]; then
    record_fail "version expectation" "expected $EXPECT_VERSION but pyproject.toml has $version"
    exit 1
fi

tmp_dir="$(mktemp -d)"
venv_dir="$tmp_dir/venv"
if [[ -n "$DIST_OVERRIDE" ]]; then
    dist_dir="$DIST_OVERRIDE"
    rm -rf "$dist_dir"
else
    dist_dir="$tmp_dir/dist"
fi
mkdir -p "$dist_dir"

run_timed "create virtualenv" python3 -m venv "$venv_dir"
run_timed "install build backend" "$venv_dir/bin/python" -m pip --disable-pip-version-check install build hatchling
run_timed "build wheel + sdist" "$venv_dir/bin/python" -m build --wheel --sdist --outdir "$dist_dir" "$PACKAGE_DIR"
run_timed "validate publish payload" "$ROOT_DIR/scripts/python_package_publish_check.py" "$PACKAGE" "$dist_dir" "$version"

echo "OK: $PACKAGE publish dry-run passed"
