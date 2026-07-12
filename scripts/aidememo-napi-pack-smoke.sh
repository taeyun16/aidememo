#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAPI_DIR="$ROOT_DIR/crates/aidememo-napi"
BASE="${AIDEMEMO_NAPI_PACK_SMOKE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-napi-pack-smoke.XXXXXX")}"
SUMMARY_TSV="$BASE/aidememo-napi-pack-smoke.tsv"

timer_now() {
    python3 - <<'PY'
import time
print(time.perf_counter())
PY
}

elapsed_since() {
    python3 - "$1" <<'PY'
import sys
import time
start = float(sys.argv[1])
print(f"{time.perf_counter() - start:.2f}")
PY
}

record_summary_row() {
    local status elapsed label row_status detail
    status="$1"
    elapsed="$2"
    label="$3"
    if [[ "$status" == "0" ]]; then
        row_status="ok"
        detail=""
    else
        row_status="fail"
        detail="exit $status"
    fi

    printf "%s\t%s\t%s\t%s\n" "$row_status" "$elapsed" "$label" "$detail" >> "$SUMMARY_TSV"
}

run() {
    run_labeled "$*" "$@"
}

run_labeled() {
    local label start status elapsed
    label="$1"
    shift
    echo "==> $label"
    start="$(timer_now)"
    set +e
    "$@"
    status="$?"
    set -e
    elapsed="$(elapsed_since "$start")"
    record_summary_row "$status" "$elapsed" "$label"
    echo "    elapsed: ${elapsed}s"
    return "$status"
}

run_capture() {
    local outvar label start status elapsed output
    outvar="$1"
    shift
    label="$1"
    shift
    echo "==> $label"
    start="$(timer_now)"
    set +e
    output="$("$@")"
    status="$?"
    set -e
    elapsed="$(elapsed_since "$start")"
    record_summary_row "$status" "$elapsed" "$label"
    printf '%s\n' "$output"
    echo "    elapsed: ${elapsed}s"
    printf -v "$outvar" '%s' "$output"
    return "$status"
}

print_summary() {
    if [[ ! -s "$SUMMARY_TSV" ]]; then
        return
    fi

    python3 - "$SUMMARY_TSV" <<'PY'
from pathlib import Path
import os
import sys

rows = []
for line in Path(sys.argv[1]).read_text().splitlines():
    status, elapsed, label, detail = line.split("\t", 3)
    rows.append((status, elapsed, label, detail))

total = sum(float(elapsed) for _, elapsed, _, _ in rows if elapsed != "-")
lines = [
    "## aidememo-napi-pack-smoke",
    "",
    "| Status | Step | Seconds | Detail |",
    "|---|---|---:|---|",
]
for status, elapsed, label, detail in rows:
    lines.append(f"| {status} | `{label}` | {elapsed} | {detail} |")
lines.append(f"| total | | {total:.2f} | |")

text = "\n".join(lines)
print(text)

summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
if summary_path:
    with open(summary_path, "a", encoding="utf-8") as handle:
        handle.write(text)
        handle.write("\n")
PY
}

mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap print_summary EXIT

cd "$NAPI_DIR"

if [[ ! -x node_modules/.bin/napi ]]; then
    if [[ -f package-lock.json ]]; then
        run npm ci --prefer-offline --no-audit --fund=false
    else
        # The root wrapper depends on platform packages that do not exist in
        # npm until the first release. npm therefore cannot create a complete
        # lockfile during bootstrap; use the same install path as the publish
        # script until those packages are available.
        run npm install --prefer-offline --no-audit --fund=false
    fi
fi
run_labeled "aidememo-napi version gate" "$ROOT_DIR/scripts/aidememo-napi-version.sh"
run npm run build
run npm test

node_file="$(find "$NAPI_DIR" -maxdepth 1 -type f -name 'aidememo-napi.*.node' | head -n 1)"
if [[ -z "$node_file" ]]; then
    echo "missing built aidememo-napi.*.node file" >&2
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

rm -f aidememo-napi-*.tgz
run_capture pack_json "npm pack root aidememo-napi" npm pack --json

package_file="$(PACK_JSON="$pack_json" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["PACK_JSON"])
if not payload:
    raise SystemExit("npm pack returned no package entries")
files = {item["path"] for item in payload[0].get("files", [])}
required = {"package.json", "README.md", "index.js", "index.d.ts"}
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
run_capture platform_pack_json "npm pack $platform_pkg" npm pack --json
platform_package_file="$(PACK_JSON="$platform_pack_json" NODE_BASE="$node_base" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["PACK_JSON"])
if not payload:
    raise SystemExit("platform npm pack returned no package entries")
files = {item["path"] for item in payload[0].get("files", [])}
required = {"package.json", "README.md", os.environ["NODE_BASE"]}
missing = sorted(required - files)
if missing:
    raise SystemExit(f"platform package missing required files: {missing}")
print(payload[0]["filename"])
PY
)"
test -f "$platform_package_file"

tmp_project="$(mktemp -d)"
cd "$tmp_project"
run_labeled "npm install packed aidememo-napi tarballs" npm install --offline --ignore-scripts --package-lock=false --no-audit --fund=false --omit=optional "$NAPI_DIR/$package_file" "$platform_dir/$platform_package_file"
run_labeled "verify installed aidememo-napi" node -e "const aidememo = require('aidememo-napi'); console.log('installed aidememo-napi version:', aidememo.version())"

echo "OK: aidememo-napi package smoke passed (root=$NAPI_DIR/$package_file platform=$platform_pkg)"
