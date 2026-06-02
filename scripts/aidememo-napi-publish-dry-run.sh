#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

AIDEMEMO_NAPI_PUBLISH_MODE=dry-run \
AIDEMEMO_NAPI_PUBLISH_SCOPE="${AIDEMEMO_NAPI_PUBLISH_SCOPE:-both}" \
    "$ROOT_DIR/scripts/aidememo-napi-publish.sh"
