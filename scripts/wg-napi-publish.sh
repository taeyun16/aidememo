#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAPI_DIR="$ROOT_DIR/crates/wg-napi"
MODE="${WG_NAPI_PUBLISH_MODE:-dry-run}" # dry-run | publish
SCOPE="${WG_NAPI_PUBLISH_SCOPE:-both}"   # root | platform | both
EXPECT_VERSION="${WG_NAPI_EXPECT_VERSION:-}"

run() {
    echo "==> $*"
    "$@"
}

case "$MODE" in
    dry-run | publish) ;;
    *)
        echo "WG_NAPI_PUBLISH_MODE must be dry-run or publish (got $MODE)" >&2
        exit 1
        ;;
esac

case "$SCOPE" in
    root | platform | both) ;;
    *)
        echo "WG_NAPI_PUBLISH_SCOPE must be root, platform, or both (got $SCOPE)" >&2
        exit 1
        ;;
esac

publish_args=(--access public --json)
if [[ "$MODE" == "dry-run" ]]; then
    publish_args=(--dry-run "${publish_args[@]}")
fi

cd "$NAPI_DIR"

run npm install
run "$ROOT_DIR/scripts/wg-napi-version.sh"
run npm run build
run npm test

root_version="$(node -p "require('./package.json').version")"
if [[ -n "$EXPECT_VERSION" && "$root_version" != "$EXPECT_VERSION" ]]; then
    echo "expected wg-napi version $EXPECT_VERSION but package.json has $root_version" >&2
    exit 1
fi

node_files=()
while IFS= read -r node_path; do
    node_files+=("$node_path")
done < <(find "$NAPI_DIR" -maxdepth 1 -type f -name 'wg-napi.*.node' | sort)
if [[ "${#node_files[@]}" -ne 1 ]]; then
    printf 'expected exactly one built wg-napi.*.node file, found %s\n' "${#node_files[@]}" >&2
    printf '  %s\n' "${node_files[@]}" >&2
    exit 1
fi
node_file="${node_files[0]}"
node_base="$(basename "$node_file")"
platform_pkg="${node_base%.node}"
platform_pkg="${platform_pkg/./-}"
platform_dir="$NAPI_DIR/npm/$platform_pkg"
if [[ ! -f "$platform_dir/package.json" ]]; then
    echo "missing platform package scaffold: $platform_dir/package.json" >&2
    exit 1
fi
platform_version="$(node -p "require('$platform_dir/package.json').version")"
if [[ "$platform_version" != "$root_version" ]]; then
    echo "$platform_pkg version $platform_version does not match root $root_version" >&2
    exit 1
fi
cp "$node_file" "$platform_dir/$node_base"

publish_platform() {
    cd "$platform_dir"
    rm -f "$platform_pkg"-*.tgz
    echo "==> npm publish ${publish_args[*]} ($platform_pkg)"
    local publish_json
    publish_json="$(npm publish "${publish_args[@]}")"
    echo "$publish_json"
    PUBLISH_JSON="$publish_json" NODE_BASE="$node_base" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["PUBLISH_JSON"])
files = payload.get("files") or []
paths = {item.get("path") for item in files}
required = {"package.json", os.environ["NODE_BASE"]}
missing = sorted(required - paths)
if missing:
    raise SystemExit(f"platform publish payload missing required files: {missing}")

node_files = sorted(path for path in paths if path and path.endswith(".node"))
if node_files != [os.environ["NODE_BASE"]]:
    raise SystemExit(f"platform publish payload has unexpected node files: {node_files}")

pkg = payload.get("name", "<unknown>")
version = payload.get("version", "<unknown>")
size = payload.get("size", 0)
unpacked = payload.get("unpackedSize", 0)
print(
    "OK: platform npm publish payload package="
    f"{pkg}@{version} files={len(files)} node_files={','.join(node_files)} "
    f"size={size} unpacked={unpacked}"
)
PY
}

publish_root() {
    cd "$NAPI_DIR"
    rm -f wg-napi-*.tgz
    echo "==> npm publish ${publish_args[*]} (wg-napi)"
    local publish_json
    publish_json="$(npm publish "${publish_args[@]}")"
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
    raise SystemExit(f"root publish payload missing required files: {missing}")
if node_files:
    raise SystemExit(f"root publish payload must not include platform binaries: {node_files}")

pkg = payload.get("name", "wg-napi")
version = payload.get("version", "<unknown>")
size = payload.get("size", 0)
unpacked = payload.get("unpackedSize", 0)
print(
    "OK: root npm publish payload package="
    f"{pkg}@{version} files={len(files)} size={size} unpacked={unpacked}"
)
PY
}

case "$SCOPE" in
    platform)
        publish_platform
        ;;
    root)
        publish_root
        ;;
    both)
        publish_platform
        publish_root
        ;;
esac
