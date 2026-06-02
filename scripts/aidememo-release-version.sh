#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$#" -gt 1 ]]; then
    echo "usage: $0 [semver]" >&2
    echo "example: $0 0.1.1" >&2
    exit 1
fi

VERSION="${1:-}"
if [[ -n "$VERSION" ]]; then
    "$ROOT_DIR/scripts/aidememo-python-version.sh" "$VERSION"
    "$ROOT_DIR/scripts/aidememo-napi-version.sh" "$VERSION"
    "$ROOT_DIR/scripts/aidememo-nif-version.sh" "$VERSION"
else
    "$ROOT_DIR/scripts/aidememo-python-version.sh"
    "$ROOT_DIR/scripts/aidememo-napi-version.sh"
    "$ROOT_DIR/scripts/aidememo-nif-version.sh"
fi

python3 - "$ROOT_DIR" <<'PY'
import sys
import tomllib
from pathlib import Path

root = Path(sys.argv[1])
with (root / "Cargo.toml").open("rb") as f:
    version = tomllib.load(f)["workspace"]["package"]["version"]

print(
    "OK: aidememo release version pinned at "
    f"{version} across Cargo, Python, npm, and NIF packages "
    "(aidememo-ffi uses Cargo metadata)"
)
PY
