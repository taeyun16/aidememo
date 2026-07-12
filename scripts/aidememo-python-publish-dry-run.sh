#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY_DIR="$ROOT_DIR/crates/aidememo-python"
EXPECT_VERSION="${AIDEMEMO_PYTHON_EXPECT_VERSION:-}"
BASE="${AIDEMEMO_PYTHON_PUBLISH_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-python-publish.XXXXXX")}"
SUMMARY_TSV="$BASE/aidememo-python-publish.tsv"
tmp_dir=""

# shellcheck source=scripts/pyo3-python.sh
source "$ROOT_DIR/scripts/pyo3-python.sh"
PYO3_PYTHON_BIN="$(aidememo_resolve_pyo3_python)"
export PYO3_PYTHON="$PYO3_PYTHON_BIN"

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

run_labeled() {
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

    python3 - "$SUMMARY_TSV" <<'PY'
from pathlib import Path
import os
import sys

rows = []
for line in Path(sys.argv[1]).read_text().splitlines():
    status, elapsed, label, detail = line.split("\t", 3)
    rows.append((status, elapsed, label, detail))

total = sum(float(elapsed) for _, elapsed, _, _ in rows if elapsed != "-")
lines = [
    "## aidememo-python-publish-dry-run",
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

if ! command -v uvx >/dev/null 2>&1; then
    record_fail "uvx availability" "uvx is required for aidememo-python publish dry-run; run mise install"
    exit 1
fi

run_labeled "aidememo-python version gate" "$ROOT_DIR/scripts/aidememo-python-version.sh"
run_labeled "uv version" "$ROOT_DIR/scripts/uv.sh" --version
echo "PYO3_PYTHON=$PYO3_PYTHON ($(aidememo_pyo3_python_version "$PYO3_PYTHON"))"
run_labeled "maturin version" "$ROOT_DIR/scripts/maturin.sh" --version

version="$(
    python3 - "$PY_DIR/pyproject.toml" <<'PY'
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
if [[ -z "${AIDEMEMO_PYTHON_DIST_DIR:-}" ]]; then
    dist_dir="$tmp_dir/dist"
else
    dist_dir="$AIDEMEMO_PYTHON_DIST_DIR"
    if [[ "$dist_dir" != /* ]]; then
        dist_dir="$ROOT_DIR/$dist_dir"
    fi
    rm -rf "$dist_dir"
fi
venv_dir="$tmp_dir/venv"
mkdir -p "$dist_dir"

run_labeled "create virtualenv" "$ROOT_DIR/scripts/uv.sh" venv --seed -p "$PYO3_PYTHON" "$venv_dir"
run_labeled "maturin build --release --sdist" bash -lc "cd '$PY_DIR' && '$ROOT_DIR/scripts/maturin.sh' build --release --sdist -i '$venv_dir/bin/python' -o '$dist_dir'"
run_labeled "validate publish payload" "$ROOT_DIR/scripts/aidememo_python_publish_check.py" "$dist_dir" "$version"

echo "OK: aidememo-python publish dry-run passed"
