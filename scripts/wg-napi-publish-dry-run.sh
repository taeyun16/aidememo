#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

WG_NAPI_PUBLISH_MODE=dry-run \
WG_NAPI_PUBLISH_SCOPE="${WG_NAPI_PUBLISH_SCOPE:-both}" \
    "$ROOT_DIR/scripts/wg-napi-publish.sh"
