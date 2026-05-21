#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAPI_DIR="$ROOT_DIR/crates/wg-napi"

run() {
    echo "==> $*"
    "$@"
}

cd "$NAPI_DIR"

run npm install
run npm run build
run npm test

rm -f wg-napi-*.tgz
pack_json="$(npm pack --json)"
echo "$pack_json"

package_file="$(PACK_JSON="$pack_json" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["PACK_JSON"])
if not payload:
    raise SystemExit("npm pack returned no package entries")
print(payload[0]["filename"])
PY
)"

test -f "$package_file"
tar -tzf "$package_file" | grep -E 'package/(index\.js|index\.d\.ts|wg-napi\..*\.node)$' >/dev/null

echo "OK: wg-napi package smoke passed ($NAPI_DIR/$package_file)"
