#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY_DIR="$ROOT_DIR/crates/wg-python"
EXPECT_VERSION="${WG_PYTHON_EXPECT_VERSION:-}"

run() {
    echo "==> $*"
    "$@"
}

if ! command -v maturin >/dev/null 2>&1; then
    echo "maturin is required for wg-python package smoke" >&2
    exit 1
fi

run "$ROOT_DIR/scripts/wg-python-version.sh"

version="$(
    python3 - "$PY_DIR/pyproject.toml" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as f:
    print(tomllib.load(f)["project"]["version"])
PY
)"

if [[ -n "$EXPECT_VERSION" && "$version" != "$EXPECT_VERSION" ]]; then
    echo "expected wg-python version $EXPECT_VERSION but pyproject.toml has $version" >&2
    exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
wheel_dir="$tmp_dir/wheels"
venv_dir="$tmp_dir/venv"
mkdir -p "$wheel_dir"

run python3 -m venv "$venv_dir"
run bash -lc "cd '$PY_DIR' && maturin build --release -i '$venv_dir/bin/python' -o '$wheel_dir'"

wheel="$(
    find "$wheel_dir" -maxdepth 1 -type f -name 'wg_python-*.whl' | sort | head -n 1
)"
if [[ -z "$wheel" ]]; then
    echo "missing built wg_python wheel in $wheel_dir" >&2
    exit 1
fi

run "$venv_dir/bin/python" -m pip --disable-pip-version-check install "$wheel"
run "$venv_dir/bin/python" "$PY_DIR/tests/smoke.py"
run "$venv_dir/bin/python" - "$version" <<'PY'
import importlib.metadata
import sys

import wg_python

expected = sys.argv[1]
metadata_version = importlib.metadata.version("wg-python")
module_version = wg_python.__version__
if metadata_version != expected:
    raise SystemExit(f"wheel metadata version {metadata_version} != {expected}")
if module_version != expected:
    raise SystemExit(f"wg_python.__version__ {module_version} != {expected}")
print(f"installed wg-python version: {module_version}")
PY

echo "OK: wg-python package smoke passed (wheel=$(basename "$wheel"))"
