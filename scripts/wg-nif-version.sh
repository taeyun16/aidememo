#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-}"

python3 "$ROOT_DIR/scripts/wg_nif_version.py" "$ROOT_DIR" "$VERSION"
