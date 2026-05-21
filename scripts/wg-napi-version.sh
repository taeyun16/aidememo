#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAPI_DIR="$ROOT_DIR/crates/wg-napi"
VERSION="${1:-}"

if [[ -n "$VERSION" && ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
    echo "usage: $0 [semver]" >&2
    echo "example: $0 0.1.1" >&2
    exit 1
fi

node "$ROOT_DIR/scripts/wg_napi_version.mjs" "$NAPI_DIR" "$VERSION"
