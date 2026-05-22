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
    echo "maturin is required for wg-python publish dry-run" >&2
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
if [[ -z "${WG_PYTHON_DIST_DIR:-}" ]]; then
    trap 'rm -rf "$tmp_dir"' EXIT
    dist_dir="$tmp_dir/dist"
else
    trap 'rm -rf "$tmp_dir"' EXIT
    dist_dir="$WG_PYTHON_DIST_DIR"
    rm -rf "$dist_dir"
fi
venv_dir="$tmp_dir/venv"
mkdir -p "$dist_dir"

run python3 -m venv "$venv_dir"
run bash -lc "cd '$PY_DIR' && maturin build --release --sdist -i '$venv_dir/bin/python' -o '$dist_dir'"
run "$ROOT_DIR/scripts/wg_python_publish_check.py" "$dist_dir" "$version"

echo "OK: wg-python publish dry-run passed"
