#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY_DIR="$ROOT_DIR/crates/aidememo-python"
EXPECT_VERSION="${AIDEMEMO_PYTHON_EXPECT_VERSION:-}"
BUILD_FEATURES="${AIDEMEMO_PYTHON_PACK_SMOKE_FEATURES:-}"
NO_DEFAULT_FEATURES="${AIDEMEMO_PYTHON_PACK_SMOKE_NO_DEFAULT_FEATURES:-0}"
SMOKE_BACKEND="${AIDEMEMO_PYTHON_SMOKE_BACKEND:-sqlite}"
SMOKE_EXPECT_DEFAULT_BACKEND="${AIDEMEMO_PYTHON_SMOKE_EXPECT_DEFAULT_BACKEND:-}"
BASE="${AIDEMEMO_PYTHON_PACK_SMOKE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-python-pack-smoke.XXXXXX")}"
SUMMARY_TSV="$BASE/aidememo-python-pack-smoke.tsv"
tmp_dir=""

# shellcheck source=scripts/pyo3-python.sh
source "$ROOT_DIR/scripts/pyo3-python.sh"
PYO3_PYTHON_BIN="$(aidememo_resolve_pyo3_python)"
export PYO3_PYTHON="$PYO3_PYTHON_BIN"

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

run() {
    run_timed "$*" "$@"
}

run_labeled() {
    local label="$1"
    shift
    run_timed "$label" "$@"
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
    "## aidememo-python-pack-smoke",
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
    record_fail "uvx availability" "uvx is required for aidememo-python package smoke; run mise install"
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
wheel_dir="$tmp_dir/wheels"
venv_dir="$tmp_dir/venv"
mkdir -p "$wheel_dir"

run_labeled "create virtualenv" "$ROOT_DIR/scripts/uv.sh" venv --seed -p "$PYO3_PYTHON" "$venv_dir"

build_args=(build --release)
build_label="maturin build --release"
if [[ "$NO_DEFAULT_FEATURES" == "1" || "$NO_DEFAULT_FEATURES" == "true" ]]; then
    build_args+=(--no-default-features)
    build_label+=" --no-default-features"
fi
if [[ -n "$BUILD_FEATURES" ]]; then
    build_args+=(--features "$BUILD_FEATURES")
    build_label+=" --features $BUILD_FEATURES"
fi
build_args+=(-i "$venv_dir/bin/python" -o "$wheel_dir")

if [[ -z "$SMOKE_EXPECT_DEFAULT_BACKEND" ]]; then
    normalized_features="$(tr ',' ' ' <<<"$BUILD_FEATURES")"
    if [[ ("$NO_DEFAULT_FEATURES" == "1" || "$NO_DEFAULT_FEATURES" == "true") && " $normalized_features " == *" redb "* && " $normalized_features " != *" sqlite "* ]]; then
        SMOKE_EXPECT_DEFAULT_BACKEND="redb"
    else
        SMOKE_EXPECT_DEFAULT_BACKEND="sqlite"
    fi
fi

run_labeled "$build_label" \
    bash -lc 'cd "$1" && shift && exec "$@"' \
    _ "$PY_DIR" "$ROOT_DIR/scripts/maturin.sh" "${build_args[@]}"

wheel="$(
    find "$wheel_dir" -maxdepth 1 -type f -name 'aidememo_python-*.whl' | sort | head -n 1
)"
if [[ -z "$wheel" ]]; then
    record_fail "wheel artifact" "missing built aidememo_python wheel in $wheel_dir"
    exit 1
fi

run_labeled "install built wheel" "$venv_dir/bin/python" -m pip --disable-pip-version-check install "$wheel"
run_labeled "run Python binding smoke ($SMOKE_BACKEND)" env \
    AIDEMEMO_PYTHON_SMOKE_BACKEND="$SMOKE_BACKEND" \
    AIDEMEMO_PYTHON_SMOKE_EXPECT_DEFAULT_BACKEND="$SMOKE_EXPECT_DEFAULT_BACKEND" \
    "$venv_dir/bin/python" "$PY_DIR/tests/smoke.py"
run_labeled "verify installed aidememo-python version" "$venv_dir/bin/python" - "$version" <<'PY'
import importlib.metadata
import sys

import aidememo_python

expected = sys.argv[1]
metadata_version = importlib.metadata.version("aidememo-python")
module_version = aidememo_python.__version__
if metadata_version != expected:
    raise SystemExit(f"wheel metadata version {metadata_version} != {expected}")
if module_version != expected:
    raise SystemExit(f"aidememo_python.__version__ {module_version} != {expected}")
print(f"installed aidememo-python version: {module_version}")
PY

echo "OK: aidememo-python package smoke passed (wheel=$(basename "$wheel"))"
