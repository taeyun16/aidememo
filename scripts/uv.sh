#!/usr/bin/env bash

set -euo pipefail

UV_SPEC="${AIDEMEMO_UV_SPEC:-uv==0.11.21}"

if ! command -v uvx >/dev/null 2>&1; then
    echo "uvx is required to run $UV_SPEC. Run 'mise install' or install uv." >&2
    exit 1
fi

exec uvx --from "$UV_SPEC" uv "$@"
