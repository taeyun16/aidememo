#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_CARGO_PACKAGE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-cargo-package.XXXXXX")}"
SUMMARY_TSV="$BASE/cargo-package-readiness.tsv"
RUN_VERSION_GATE="${AIDEMEMO_CARGO_PACKAGE_VERSION_GATE:-1}"
ALLOW_DIRTY="${AIDEMEMO_CARGO_PACKAGE_ALLOW_DIRTY:-1}"
VERIFY_PACKAGE="${AIDEMEMO_CARGO_PACKAGE_VERIFY:-1}"
CHECK_DEPENDENTS="${AIDEMEMO_CARGO_PACKAGE_CHECK_DEPENDENTS:-0}"

CORE_PACKAGE="aidememo-core"
DEPENDENT_PACKAGES=(
    aidememo-cli
    aidememo-ffi
    aidememo-napi
    aidememo-nif
    aidememo-python
)

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

workspace_version() {
    python3 - "$ROOT_DIR" <<'PY'
import sys
import tomllib
from pathlib import Path

root = Path(sys.argv[1])
with (root / "Cargo.toml").open("rb") as handle:
    print(tomllib.load(handle)["workspace"]["package"]["version"])
PY
}

record_row() {
    local status="$1"
    local elapsed="$2"
    local label="$3"
    local detail="$4"
    printf "%s\t%s\t%s\t%s\n" "$status" "$elapsed" "$label" "$detail" >> "$SUMMARY_TSV"
}

record_skip() {
    local label="$1"
    local reason="$2"
    echo "==> skip: $label ($reason)"
    record_row "skip" "-" "$label" "$reason"
}

run_timed() {
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
    if [[ "$status" == "0" ]]; then
        record_row "ok" "$elapsed" "$label" ""
    else
        record_row "fail" "$elapsed" "$label" "exit $status"
    fi
    echo "    elapsed: ${elapsed}s"
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

total = sum(float(elapsed) for status, elapsed, _, _ in rows if status != "skip")
lines = [
    "## cargo-package-readiness",
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

normalize_bool() {
    case "$1" in
        1 | true | TRUE | yes | YES | on | ON) echo "1" ;;
        0 | false | FALSE | no | NO | off | OFF) echo "0" ;;
        *)
            echo "expected boolean-like value, got: $1" >&2
            exit 1
            ;;
    esac
}

publish_flags() {
    local flags=()
    if [[ "$ALLOW_DIRTY" == "1" ]]; then
        flags+=(--allow-dirty)
    fi
    if [[ "$VERIFY_PACKAGE" == "0" ]]; then
        flags+=(--no-verify)
    fi
    printf '%s\n' "${flags[@]}"
}

run_cargo_publish_dry_run() {
    local package="$1"
    local flags=()
    while IFS= read -r flag; do
        if [[ -n "$flag" ]]; then
            flags+=("$flag")
        fi
    done < <(publish_flags)
    run_timed "cargo publish --dry-run $package" cargo publish -p "$package" --dry-run "${flags[@]}"
}

cd "$ROOT_DIR"
mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap print_summary EXIT

ALLOW_DIRTY="$(normalize_bool "$ALLOW_DIRTY")"
VERIFY_PACKAGE="$(normalize_bool "$VERIFY_PACKAGE")"
RUN_VERSION_GATE="$(normalize_bool "$RUN_VERSION_GATE")"
CHECK_DEPENDENTS="$(normalize_bool "$CHECK_DEPENDENTS")"
VERSION="$(workspace_version)"

echo "cargo publish dry-run readiness"
echo "version: $VERSION"
echo "allow_dirty: $ALLOW_DIRTY"
echo "verify_package: $VERIFY_PACKAGE"
echo "check_dependents: $CHECK_DEPENDENTS"
echo "base: $BASE"
echo

if [[ "$RUN_VERSION_GATE" == "1" ]]; then
    run_timed "release version gate" "$ROOT_DIR/scripts/aidememo-release-version.sh"
else
    record_skip "release version gate" "AIDEMEMO_CARGO_PACKAGE_VERSION_GATE=0"
fi

run_cargo_publish_dry_run "$CORE_PACKAGE"

if [[ "$CHECK_DEPENDENTS" == "1" ]]; then
    for package in "${DEPENDENT_PACKAGES[@]}"; do
        run_cargo_publish_dry_run "$package"
    done
else
    for package in "${DEPENDENT_PACKAGES[@]}"; do
        record_skip \
            "cargo publish --dry-run $package" \
            "publish $CORE_PACKAGE $VERSION first, then set AIDEMEMO_CARGO_PACKAGE_CHECK_DEPENDENTS=1"
    done
fi

echo
echo "OK: cargo publish dry-run readiness completed"
