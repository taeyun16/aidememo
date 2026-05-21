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

node_file="$(find "$NAPI_DIR" -maxdepth 1 -type f -name 'wg-napi.*.node' | head -n 1)"
if [[ -z "$node_file" ]]; then
    echo "missing built wg-napi.*.node file" >&2
    exit 1
fi
node_base="$(basename "$node_file")"
platform_pkg="${node_base%.node}"
platform_pkg="${platform_pkg/./-}"
platform_dir="$NAPI_DIR/npm/$platform_pkg"
if [[ ! -f "$platform_dir/package.json" ]]; then
    echo "missing platform package scaffold: $platform_dir/package.json" >&2
    exit 1
fi
cp "$node_file" "$platform_dir/$node_base"

rm -f wg-napi-*.tgz
pack_json="$(npm pack --json)"
echo "$pack_json"

package_file="$(PACK_JSON="$pack_json" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["PACK_JSON"])
if not payload:
    raise SystemExit("npm pack returned no package entries")
files = {item["path"] for item in payload[0].get("files", [])}
required = {"package.json", "index.js", "index.d.ts"}
missing = sorted(required - files)
if missing:
    raise SystemExit(f"root package missing required files: {missing}")
node_files = sorted(path for path in files if path.endswith(".node"))
if node_files:
    raise SystemExit(f"root package must not include platform binaries: {node_files}")
print(payload[0]["filename"])
PY
)"

test -f "$package_file"

cd "$platform_dir"
rm -f "$platform_pkg"-*.tgz
platform_pack_json="$(npm pack --json)"
echo "$platform_pack_json"
platform_package_file="$(PACK_JSON="$platform_pack_json" NODE_BASE="$node_base" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["PACK_JSON"])
if not payload:
    raise SystemExit("platform npm pack returned no package entries")
files = {item["path"] for item in payload[0].get("files", [])}
required = {"package.json", os.environ["NODE_BASE"]}
missing = sorted(required - files)
if missing:
    raise SystemExit(f"platform package missing required files: {missing}")
print(payload[0]["filename"])
PY
)"
test -f "$platform_package_file"

tmp_project="$(mktemp -d)"
cd "$tmp_project"
run npm install "$NAPI_DIR/$package_file" "$platform_dir/$platform_package_file"
run node -e "const wg = require('wg-napi'); console.log('installed wg-napi version:', wg.version())"

echo "OK: wg-napi package smoke passed (root=$NAPI_DIR/$package_file platform=$platform_pkg)"
