#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_FRESH_CHECKOUT_SMOKE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-fresh-checkout.XXXXXX")}"
CHECKOUT="$BASE/checkout"
RUN_DIR="$BASE/run"
SUMMARY_TSV="$BASE/fresh-checkout-smoke.tsv"
CAPTURED_OUT=""

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

record_row() {
    local status="$1"
    local elapsed="$2"
    local label="$3"
    local detail="$4"
    printf "%s\t%s\t%s\t%s\n" "$status" "$elapsed" "$label" "$detail" >> "$SUMMARY_TSV"
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

capture_timed() {
    local label start status elapsed err_file
    label="$1"
    shift
    echo "==> $label"
    err_file="$BASE/capture.err"
    : > "$err_file"
    start="$(timer_now)"
    set +e
    CAPTURED_OUT="$("$@" 2>"$err_file")"
    status="$?"
    set -e
    elapsed="$(elapsed_since "$start")"
    if [[ "$status" == "0" ]]; then
        record_row "ok" "$elapsed" "$label" ""
    else
        record_row "fail" "$elapsed" "$label" "exit $status"
        printf '%s\n' "$CAPTURED_OUT" >&2
        cat "$err_file" >&2
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
    "## fresh-checkout-smoke",
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

cleanup() {
    print_summary
    if [[ "${AIDEMEMO_FRESH_CHECKOUT_KEEP_TMP:-0}" != "1" ]]; then
        rm -rf "$BASE"
    else
        echo "kept temp dir: $BASE" >&2
    fi
}
trap cleanup EXIT

expect_contains() {
    local label="$1"
    local haystack="$2"
    local needle="$3"
    if [[ "$haystack" != *"$needle"* ]]; then
        echo "$label did not contain expected text: $needle" >&2
        echo "$haystack" >&2
        exit 1
    fi
}

assert_workflow_json() {
    python3 - "$1" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
for key in ("session_id", "ticket_fact_id", "context"):
    if key not in payload:
        raise SystemExit(f"workflow payload missing {key}: {payload}")
if not str(payload["session_id"]).startswith("session-"):
    raise SystemExit(f"unexpected session_id: {payload['session_id']!r}")
if "Fresh checkout smoke" not in json.dumps(payload, ensure_ascii=False):
    raise SystemExit(f"workflow payload missing ticket text: {payload}")
PY
}

assert_stats_json() {
    python3 - "$1" <<'PY'
import json
import sys

stats = json.loads(sys.argv[1])
if stats["entity_count"] < 2 or stats["fact_count"] < 2:
    raise SystemExit(f"fresh checkout stats too small: {stats}")
PY
}

run_in_checkout() {
    (cd "$CHECKOUT" && "$@")
}

mkdir -p "$CHECKOUT" "$RUN_DIR"
: > "$SUMMARY_TSV"

run_timed "copy checkout without build artifacts" rsync -a --delete \
    --exclude ".git" \
    --exclude "target" \
    --exclude "website/build" \
    --exclude "website/node_modules" \
    --exclude "crates/aidememo-napi/node_modules" \
    --exclude "crates/aidememo-napi/*.tgz" \
    --exclude ".direnv" \
    "$ROOT_DIR/" \
    "$CHECKOUT/"

run_timed "install script syntax" bash -n "$CHECKOUT/scripts/install.sh"
run_timed "cargo build aidememo-cli" run_in_checkout cargo build -p aidememo-cli

BIN="$CHECKOUT/target/debug/aidememo"
STORE="$RUN_DIR/wiki.sqlite"

capture_timed "aidememo --help" "$BIN" --help
expect_contains "help" "$CAPTURED_OUT" "Available commands"

capture_timed "quickstart fact add" "$BIN" --store "$STORE" fact add \
    "Decision: AideMemo stores typed project memory locally." \
    --type decision \
    --entities AideMemo,Onboarding
expect_contains "fact add" "$CAPTURED_OUT" "Added fact"

capture_timed "quickstart search" "$BIN" --store "$STORE" search \
    "typed project memory" \
    --bm25-only \
    --limit 5
expect_contains "search" "$CAPTURED_OUT" "typed project memory"

capture_timed "quickstart query" "$BIN" --store "$STORE" query \
    "typed project memory" \
    --bm25-only \
    --limit 5
expect_contains "query" "$CAPTURED_OUT" "typed project memory"

capture_timed "quickstart workflow start" "$BIN" --store "$STORE" --json workflow start \
    "Fresh checkout smoke" \
    --body "Verify the checkout build and deterministic onboarding path." \
    --source "local:fresh-checkout" \
    --bm25-only
assert_workflow_json "$CAPTURED_OUT"

capture_timed "quickstart stats" "$BIN" --store "$STORE" --json stats
assert_stats_json "$CAPTURED_OUT"

echo "OK: fresh checkout smoke passed"
echo "base: $BASE"
