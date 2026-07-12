#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAPI_DIR="$ROOT_DIR/crates/aidememo-napi"
MODE="${AIDEMEMO_NAPI_PUBLISH_MODE:-dry-run}" # dry-run | publish
SCOPE="${AIDEMEMO_NAPI_PUBLISH_SCOPE:-both}"   # root | platform | both
EXPECT_VERSION="${AIDEMEMO_NAPI_EXPECT_VERSION:-}"
EXPECT_PLATFORM_PACKAGE="${AIDEMEMO_NAPI_EXPECT_PLATFORM_PACKAGE:-}"
BOOTSTRAP="${AIDEMEMO_NAPI_BOOTSTRAP:-0}"
BOOTSTRAP_TOKEN="${AIDEMEMO_NAPI_BOOTSTRAP_TOKEN:-}"
BASE="${AIDEMEMO_NAPI_PUBLISH_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-napi-publish.XXXXXX")}"
SUMMARY_TSV="$BASE/aidememo-napi-publish.tsv"

run() {
    local label start end status elapsed
    label="$*"
    echo "==> $label"
    start="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    set +e
    "$@"
    status="$?"
    set -e
    end="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    elapsed="$(python3 - "$start" "$end" <<'PY'
import sys
start = float(sys.argv[1])
end = float(sys.argv[2])
print(f"{end - start:.2f}")
PY
)"
    if [[ "$status" == "0" ]]; then
        printf "ok\t%s\t%s\t\n" "$elapsed" "$label" >> "$SUMMARY_TSV"
    else
        printf "fail\t%s\t%s\texit %s\n" "$elapsed" "$label" "$status" >> "$SUMMARY_TSV"
    fi
    echo "    elapsed: ${elapsed}s"
    return "$status"
}

run_labeled() {
    local label start end status elapsed
    label="$1"
    shift
    echo "==> $label"
    start="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    set +e
    "$@"
    status="$?"
    set -e
    end="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    elapsed="$(python3 - "$start" "$end" <<'PY'
import sys
start = float(sys.argv[1])
end = float(sys.argv[2])
print(f"{end - start:.2f}")
PY
)"
    if [[ "$status" == "0" ]]; then
        printf "ok\t%s\t%s\t\n" "$elapsed" "$label" >> "$SUMMARY_TSV"
    else
        printf "fail\t%s\t%s\texit %s\n" "$elapsed" "$label" "$status" >> "$SUMMARY_TSV"
    fi
    echo "    elapsed: ${elapsed}s"
    return "$status"
}

run_capture() {
    local outvar label start end status elapsed output
    outvar="$1"
    shift
    label="$1"
    shift
    echo "==> $label"
    start="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    set +e
    output="$("$@")"
    status="$?"
    set -e
    end="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    elapsed="$(python3 - "$start" "$end" <<'PY'
import sys
start = float(sys.argv[1])
end = float(sys.argv[2])
print(f"{end - start:.2f}")
PY
)"
    if [[ "$status" == "0" ]]; then
        printf "ok\t%s\t%s\t\n" "$elapsed" "$label" >> "$SUMMARY_TSV"
    else
        printf "fail\t%s\t%s\texit %s\n" "$elapsed" "$label" "$status" >> "$SUMMARY_TSV"
    fi
    printf '%s\n' "$output"
    echo "    elapsed: ${elapsed}s"
    printf -v "$outvar" '%s' "$output"
    return "$status"
}

record_skip() {
    local label="$1"
    local detail="$2"
    echo "==> skip: $label ($detail)"
    printf "skip\t-\t%s\t%s\n" "$label" "$detail" >> "$SUMMARY_TSV"
}

run_npm_publish_capture() {
    local outvar="$1"
    local label="$2"
    shift 2
    if [[ "$BOOTSTRAP" == "1" ]]; then
        run_capture "$outvar" "$label" env NODE_AUTH_TOKEN="$BOOTSTRAP_TOKEN" npm publish "$@"
    else
        run_capture "$outvar" "$label" npm publish "$@"
    fi
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
    "## aidememo-napi-publish",
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

case "$MODE" in
    dry-run | publish) ;;
    *)
        echo "AIDEMEMO_NAPI_PUBLISH_MODE must be dry-run or publish (got $MODE)" >&2
        exit 1
        ;;
esac

case "$SCOPE" in
    root | platform | both) ;;
    *)
        echo "AIDEMEMO_NAPI_PUBLISH_SCOPE must be root, platform, or both (got $SCOPE)" >&2
        exit 1
        ;;
esac

case "$BOOTSTRAP" in
    0 | 1) ;;
    *)
        echo "AIDEMEMO_NAPI_BOOTSTRAP must be 0 or 1 (got $BOOTSTRAP)" >&2
        exit 1
        ;;
esac

if [[ "$MODE" == "publish" && "$BOOTSTRAP" == "1" && -z "$BOOTSTRAP_TOKEN" ]]; then
    echo "bootstrap publish requires the npm-publish environment NPM_TOKEN secret" >&2
    exit 1
fi

publish_args=(--access public --json)
if [[ "$MODE" == "dry-run" ]]; then
    publish_args=(--dry-run "${publish_args[@]}")
fi

mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap print_summary EXIT

cd "$NAPI_DIR"

run npm install
run "$ROOT_DIR/scripts/aidememo-napi-version.sh"
run npm run build
run npm test

root_version="$(node -p "require('./package.json').version")"
if [[ -n "$EXPECT_VERSION" && "$root_version" != "$EXPECT_VERSION" ]]; then
    echo "expected aidememo-napi version $EXPECT_VERSION but package.json has $root_version" >&2
    exit 1
fi

node_files=()
while IFS= read -r node_path; do
    node_files+=("$node_path")
done < <(find "$NAPI_DIR" -maxdepth 1 -type f -name 'aidememo-napi.*.node' | sort)
if [[ "${#node_files[@]}" -ne 1 ]]; then
    printf 'expected exactly one built aidememo-napi.*.node file, found %s\n' "${#node_files[@]}" >&2
    printf '  %s\n' "${node_files[@]}" >&2
    exit 1
fi
node_file="${node_files[0]}"
node_base="$(basename "$node_file")"
platform_pkg="${node_base%.node}"
platform_pkg="${platform_pkg/./-}"
if [[ -n "$EXPECT_PLATFORM_PACKAGE" && "$platform_pkg" != "$EXPECT_PLATFORM_PACKAGE" ]]; then
    echo "expected platform package $EXPECT_PLATFORM_PACKAGE but built $platform_pkg" >&2
    exit 1
fi
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
    if [[ "$MODE" == "publish" ]] && npm view "$platform_pkg@$root_version" version --json >/dev/null 2>&1; then
        record_skip "npm publish ($platform_pkg)" "$platform_pkg@$root_version already exists"
        return
    fi
    local publish_json
    run_npm_publish_capture publish_json "npm publish ${publish_args[*]} ($platform_pkg)" "${publish_args[@]}"
    run_labeled "validate platform publish payload" env PUBLISH_JSON="$publish_json" NODE_BASE="$node_base" python3 - <<'PY'
import json
import os

raw_payload = json.loads(os.environ["PUBLISH_JSON"])
if isinstance(raw_payload, dict) and "files" in raw_payload:
    payload = raw_payload
else:
    candidates = [
        value
        for value in raw_payload.values()
        if isinstance(value, dict) and "files" in value
    ]
    if len(candidates) != 1:
        raise SystemExit(f"could not find package payload in npm JSON: {raw_payload}")
    payload = candidates[0]
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
    rm -f aidememo-napi-*.tgz
    if [[ "$MODE" == "publish" ]] && npm view "aidememo-napi@$root_version" version --json >/dev/null 2>&1; then
        record_skip "npm publish (aidememo-napi)" "aidememo-napi@$root_version already exists"
        return
    fi
    local publish_json
    run_npm_publish_capture publish_json "npm publish ${publish_args[*]} (aidememo-napi)" "${publish_args[@]}"
    run_labeled "validate root publish payload" env PUBLISH_JSON="$publish_json" python3 - <<'PY'
import json
import os

raw_payload = json.loads(os.environ["PUBLISH_JSON"])
if isinstance(raw_payload, dict) and "files" in raw_payload:
    payload = raw_payload
else:
    candidates = [
        value
        for value in raw_payload.values()
        if isinstance(value, dict) and "files" in value
    ]
    if len(candidates) != 1:
        raise SystemExit(f"could not find package payload in npm JSON: {raw_payload}")
    payload = candidates[0]
files = payload.get("files") or []
paths = {item.get("path") for item in files}
required = {"package.json", "index.js", "index.d.ts"}
missing = sorted(required - paths)
node_files = sorted(path for path in paths if path and path.endswith(".node"))

if missing:
    raise SystemExit(f"root publish payload missing required files: {missing}")
if node_files:
    raise SystemExit(f"root publish payload must not include platform binaries: {node_files}")

pkg = payload.get("name", "aidememo-napi")
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
