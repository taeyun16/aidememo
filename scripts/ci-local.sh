#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLCHAIN="${AIDEMEMO_CI_TOOLCHAIN:-$(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT_DIR/rust-toolchain.toml" | head -n1)}"
BASE="${AIDEMEMO_CI_LOCAL_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-ci-local.XXXXXX")}"
SUMMARY_TSV="$BASE/ci-local.tsv"

cargo_cmd=(cargo)
if [[ -n "${TOOLCHAIN}" ]]; then
    cargo_cmd+=("+${TOOLCHAIN}")
fi

run_timed() {
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

run() {
    run_timed "$*" "$@"
}

run_labeled() {
    local label="$1"
    shift
    run_timed "$label" "$@"
}

run_without_child_summary() {
    local label="$1"
    shift
    run_labeled "$label" env GITHUB_STEP_SUMMARY= "$@"
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

total = sum(float(elapsed) for _, elapsed, _, _ in rows)
lines = [
    "## ci-local timings",
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

lint() {
    run "${cargo_cmd[@]}" fmt --all -- --check
    run "${cargo_cmd[@]}" clippy --workspace --all-targets --features semantic -- -D warnings
    run env RUSTDOCFLAGS=-D\ warnings "${cargo_cmd[@]}" doc --workspace --no-deps --features semantic
}

tests() {
    run "${cargo_cmd[@]}" test --workspace --no-default-features
    run "${cargo_cmd[@]}" test -p aidememo-core --features semantic
    run "${cargo_cmd[@]}" test -p aidememo-core --features sqlite,semantic,semantic-adapt
    run "${cargo_cmd[@]}" check -p aidememo-cli --features sqlite
    run "${cargo_cmd[@]}" test -p aidememo-cli --bin aidememo
    run "$ROOT_DIR/scripts/storage-backend-parity.sh"
    run "$ROOT_DIR/scripts/storage-backend-real-corpus-diff.sh"
    run "$ROOT_DIR/scripts/storage-backend-sqlite-mcp-soak.sh"
    run "$ROOT_DIR/scripts/storage-backend-sdk-bindings-check.sh"
}

sdk() {
    run_without_child_summary "sdk promotion check" "$ROOT_DIR/scripts/sdk-promotion-check.sh"
}

docs() {
    run "$ROOT_DIR/scripts/docs-feature-gate.py"
    run npm --prefix "$ROOT_DIR/website" run build
}

demo() {
    run "$ROOT_DIR/scripts/demo-workflow.sh"
}

case "${1:-all}" in
    lint)
        lint
        ;;
    test)
        tests
        ;;
    sdk)
        sdk
        ;;
    docs)
        docs
        ;;
    demo)
        demo
        ;;
    all)
        lint
        docs
        demo
        sdk
        tests
        ;;
    *)
        echo "usage: $0 [lint|test|sdk|docs|demo|all]" >&2
        exit 1
        ;;
esac
