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

echo "==> npm publish --dry-run --access public"
publish_json="$(npm publish --dry-run --access public --json)"
echo "$publish_json"

PUBLISH_JSON="$publish_json" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["PUBLISH_JSON"])
files = payload.get("files") or []
paths = {item.get("path") for item in files}
required = {"package.json", "index.js", "index.d.ts"}
missing = sorted(required - paths)
node_files = sorted(path for path in paths if path and path.endswith(".node"))

if missing:
    raise SystemExit(f"root publish dry-run missing required files: {missing}")
if node_files:
    raise SystemExit(f"root publish dry-run must not include platform binaries: {node_files}")

pkg = payload.get("name", "wg-napi")
version = payload.get("version", "<unknown>")
size = payload.get("size", 0)
unpacked = payload.get("unpackedSize", 0)

print(
    "OK: root npm publish dry-run package="
    f"{pkg}@{version} files={len(files)} size={size} unpacked={unpacked}"
)
PY

(
    cd "$platform_dir"
    rm -f "$platform_pkg"-*.tgz
    echo "==> npm publish --dry-run --access public ($platform_pkg)"
    platform_publish_json="$(npm publish --dry-run --access public --json)"
    echo "$platform_publish_json"
    PUBLISH_JSON="$platform_publish_json" NODE_BASE="$node_base" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["PUBLISH_JSON"])
files = payload.get("files") or []
paths = {item.get("path") for item in files}
required = {"package.json", os.environ["NODE_BASE"]}
missing = sorted(required - paths)
if missing:
    raise SystemExit(f"platform publish dry-run missing required files: {missing}")

node_files = sorted(path for path in paths if path and path.endswith(".node"))
if node_files != [os.environ["NODE_BASE"]]:
    raise SystemExit(f"platform publish dry-run has unexpected node files: {node_files}")

pkg = payload.get("name", "<unknown>")
version = payload.get("version", "<unknown>")
size = payload.get("size", 0)
unpacked = payload.get("unpackedSize", 0)

print(
    "OK: platform npm publish dry-run package="
    f"{pkg}@{version} files={len(files)} node_files={','.join(node_files)} "
    f"size={size} unpacked={unpacked}"
)
PY
)
