#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MATURIN_SPEC="${AIDEMEMO_MATURIN_SPEC:-maturin==1.14.0}"

if ! command -v uvx >/dev/null 2>&1; then
    echo "uvx is required to run $MATURIN_SPEC. Run 'mise install' or install uv." >&2
    exit 1
fi

exec "$ROOT_DIR/scripts/uvx.sh" --from "$MATURIN_SPEC" maturin "$@"
