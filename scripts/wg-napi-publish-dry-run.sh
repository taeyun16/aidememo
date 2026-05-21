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

echo "==> npm publish --dry-run --access public"
publish_json="$(npm publish --dry-run --access public --json)"
echo "$publish_json"

PUBLISH_JSON="$publish_json" python3 - <<'PY'
import json
import os
import sys

payload = json.loads(os.environ["PUBLISH_JSON"])
files = payload.get("files") or []
paths = {item.get("path") for item in files}
required = {"package.json", "index.js", "index.d.ts"}
missing = sorted(required - paths)
node_files = sorted(path for path in paths if path and path.endswith(".node"))

if missing:
    raise SystemExit(f"publish dry-run missing required files: {missing}")
if not node_files:
    raise SystemExit("publish dry-run missing platform .node binary")

pkg = payload.get("name", "wg-napi")
version = payload.get("version", "<unknown>")
size = payload.get("size", 0)
unpacked = payload.get("unpackedSize", 0)

print(
    "OK: npm publish dry-run package="
    f"{pkg}@{version} files={len(files)} node_files={','.join(node_files)} "
    f"size={size} unpacked={unpacked}"
)

if len(node_files) == 1:
    print(
        "NOTE: dry-run validates the current platform package shape. "
        "A cross-platform npm release still needs platform package publishing "
        "or a merged artifact release step.",
        file=sys.stderr,
    )
PY
