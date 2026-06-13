#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_SQLITE_ADVANCED_SURFACE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-sqlite-advanced-surface.XXXXXX")}"
BIN="$ROOT_DIR/target/debug/aidememo"

cleanup() {
    if [[ "${AIDEMEMO_SQLITE_ADVANCED_SURFACE_KEEP_TMP:-0}" != "1" ]]; then
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

fact_id_from_output() {
    python3 - "$1" <<'PY'
import re
import sys

match = re.search(r"\b([0-9A-HJKMNP-TV-Z]{26})\b", sys.argv[1])
if not match:
    raise SystemExit(f"could not parse fact id from: {sys.argv[1]}")
print(match.group(1))
PY
}

assert_json_contains() {
    python3 - "$1" "$2" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
needle = sys.argv[2]
text = json.dumps(payload, ensure_ascii=False)
if needle not in text:
    raise SystemExit(f"missing {needle!r} in JSON payload: {text[:1000]}")
PY
}

assert_json_field_equals() {
    python3 - "$1" "$2" "$3" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
field = sys.argv[2]
expected_raw = sys.argv[3]
value = payload
for part in field.split("."):
    value = value[part]
try:
    expected = json.loads(expected_raw)
except json.JSONDecodeError:
    expected = expected_raw
if value != expected:
    raise SystemExit(f"{field} = {value!r}, expected {expected!r}")
PY
}

cd "$ROOT_DIR"
cargo build -p aidememo-cli --features sqlite

HOME_DIR="$BASE/home"
WIKI_DIR="$BASE/wiki"
STORE="$BASE/advanced-surface.sqlite"
PENDING_LOG="$BASE/pending/aidememo-pending.jsonl"
mkdir -p "$HOME_DIR" "$WIKI_DIR" "$(dirname "$PENDING_LOG")"

am() {
    HOME="$HOME_DIR" "$BIN" --store "$STORE" "$@"
}

am_json() {
    HOME="$HOME_DIR" "$BIN" --store "$STORE" --json "$@"
}

HOME="$HOME_DIR" "$BIN" config set store.backend sqlite >/dev/null
backend="$(HOME="$HOME_DIR" "$BIN" config get store.backend)"
if [[ "$backend" != "sqlite" ]]; then
    echo "expected sqlite backend, got $backend" >&2
    exit 1
fi

am init --no-ingest "$WIKI_DIR" >/dev/null
am entity add SQLite --type technology >/dev/null
am entity add AdvancedSurface --type project >/dev/null

feedback_fact_out="$(am fact add \
    "SQLite advanced smoke records feedback and adapter state" \
    --entities SQLite,AdvancedSurface \
    --type claim \
    --source-id advanced-smoke)"
FEEDBACK_FACT="$(fact_id_from_output "$feedback_fact_out")"

ttl_fact_out="$(am fact add \
    "SQLite advanced smoke TTL expiry candidate" \
    --entities SQLite,AdvancedSurface \
    --type question \
    --source-id advanced-smoke)"
TTL_FACT="$(fact_id_from_output "$ttl_fact_out")"
sleep 0.02

expect_contains "fact feedback" \
    "$(am fact feedback "$FEEDBACK_FACT" --helpful)" \
    "Recorded helpful feedback"
expect_contains "search feedback" \
    "$(am feedback --helpful advanced-session "$FEEDBACK_FACT")" \
    "Recorded helpful feedback"
expect_contains "adapt status" "$(am adapt status)" "Domain adapter status"
expect_contains "adapt train" "$(am adapt train)" "Domain adapter trained"
expect_contains "adapt eval" "$(am adapt eval)" "Domain adapter evaluation"

extract_preview="$(am_json extract \
    --min-confidence 0.1 \
    --max-candidates 3 \
    "We decided to keep SQLite as the default AideMemo persistence backend.")"
assert_json_field_equals "$extract_preview" "applied" "false"
assert_json_contains "$extract_preview" "SQLite"
assert_json_contains "$extract_preview" "decision"

extract_apply="$(am_json extract \
    --apply \
    --min-confidence 0.1 \
    --max-candidates 3 \
    "We decided to expose SQLite as the SDK-compatible AideMemo backend.")"
assert_json_field_equals "$extract_apply" "applied" "true"
assert_json_contains "$extract_apply" "SQLite"
assert_json_contains \
    "$(am_json search "SDK-compatible AideMemo backend" --limit 5)" \
    "SDK-compatible AideMemo backend"

cat > "$PENDING_LOG" <<'JSONL'
{"ts_ms":1760000000000,"content":"Pending queue promoted a SQLite advanced smoke fact","fact_type":"lesson","confidence":0.91,"source_line":"promote this SQLite fact"}
{"ts_ms":1760000001000,"content":"Pending queue discarded a SQLite advanced smoke fact","fact_type":"note","confidence":0.55,"source_line":"discard this SQLite fact"}
JSONL

pending_list="$(am_json pending list --from "$PENDING_LOG")"
assert_json_field_equals "$pending_list" "count" "2"
assert_json_contains "$pending_list" "Pending queue promoted"

pending_stats="$(am_json pending stats --from "$PENDING_LOG")"
assert_json_field_equals "$pending_stats" "total" "2"
assert_json_contains "$pending_stats" "lesson"

pending_approve="$(am_json pending approve --from "$PENDING_LOG" --indices 1)"
assert_json_field_equals "$pending_approve" "summary.committed" "1"
expect_contains "pending approved search" \
    "$(am search "Pending queue promoted" --limit 5)" \
    "Pending queue promoted"

pending_reject="$(am_json pending reject --from "$PENDING_LOG" --all)"
assert_json_field_equals "$pending_reject" "summary.discarded" "1"
pending_empty="$(am_json pending stats --from "$PENDING_LOG")"
assert_json_field_equals "$pending_empty" "total" "0"

consolidate_dry="$(am consolidate \
    --json \
    --semantic-threshold 0 \
    --ttl question=0 \
    --dry-run)"
assert_json_field_equals "$consolidate_dry" "pairs_found" "0"
assert_json_field_equals "$consolidate_dry" "expired_applied" "1"

consolidate_apply="$(am consolidate \
    --json \
    --semantic-threshold 0 \
    --ttl question=0)"
assert_json_field_equals "$consolidate_apply" "pairs_found" "0"
assert_json_field_equals "$consolidate_apply" "expired_applied" "1"
expect_contains "ttl fact get" \
    "$(am fact get "$TTL_FACT")" \
    "SQLite advanced smoke TTL expiry candidate"

echo "sqlite advanced-surface ok: feedback/adapt/extract/pending/consolidate"
