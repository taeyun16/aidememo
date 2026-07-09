#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_SQLITE_ADVANCED_SURFACE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-sqlite-advanced-surface.XXXXXX")}"
BACKEND="${AIDEMEMO_SQLITE_ADVANCED_SURFACE_BACKEND:-libsqlite}"
BIN="$ROOT_DIR/target/debug/aidememo"
SQLITE_LOCK_PID=""
SQLITE_FACT_PID=""

cleanup() {
    if [[ -n "${SQLITE_FACT_PID:-}" ]] && kill -0 "$SQLITE_FACT_PID" 2>/dev/null; then
        kill "$SQLITE_FACT_PID" 2>/dev/null || true
    fi
    if [[ -n "${SQLITE_LOCK_PID:-}" ]] && kill -0 "$SQLITE_LOCK_PID" 2>/dev/null; then
        kill "$SQLITE_LOCK_PID" 2>/dev/null || true
    fi
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
cargo build -p aidememo-cli --no-default-features --features sqlite

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

HOME="$HOME_DIR" "$BIN" config set store.backend "$BACKEND" >/dev/null
HOME="$HOME_DIR" "$BIN" config set store.lock_retry_ms 3000 >/dev/null
HOME="$HOME_DIR" "$BIN" config set search.auto_hybrid false >/dev/null
HOME="$HOME_DIR" "$BIN" config set search.semantic_weight 0 >/dev/null
backend="$(HOME="$HOME_DIR" "$BIN" config get store.backend)"
if [[ "$backend" != "$BACKEND" ]]; then
    echo "expected $BACKEND backend, got $backend" >&2
    exit 1
fi
lock_retry_ms="$(HOME="$HOME_DIR" "$BIN" config get store.lock_retry_ms)"
if [[ "$lock_retry_ms" != "3000" ]]; then
    echo "expected lock_retry_ms=3000, got $lock_retry_ms" >&2
    exit 1
fi

am init --no-ingest "$WIKI_DIR" >/dev/null
am entity add SQLite --type technology >/dev/null
am entity add AdvancedSurface --type project >/dev/null

LOCK_READY="$BASE/sqlite-lock-ready"
LOCK_RELEASE="$BASE/sqlite-lock-release"
LOCK_RESULT="$BASE/sqlite-lock-result"
LOCK_FACT_OUT="$BASE/sqlite-lock-fact.out"
LOCK_FACT_ERR="$BASE/sqlite-lock-fact.err"

python3 - "$STORE" "$LOCK_READY" "$LOCK_RELEASE" "$LOCK_RESULT" <<'PY' &
import pathlib
import sqlite3
import sys
import time
import traceback

store, ready, release, result = sys.argv[1:]
try:
    conn = sqlite3.connect(store, timeout=5.0, isolation_level=None)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("BEGIN IMMEDIATE")
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)",
        ("sqlite_busy_timeout_gate", b"held"),
    )
    pathlib.Path(ready).write_text("locked")
    deadline = time.time() + 10
    while time.time() < deadline and not pathlib.Path(release).exists():
        time.sleep(0.05)
    if not pathlib.Path(release).exists():
        raise TimeoutError("timed out waiting for release signal")
    conn.commit()
    pathlib.Path(result).write_text("released")
except Exception:
    pathlib.Path(result).write_text(traceback.format_exc())
    raise
finally:
    try:
        conn.close()
    except Exception:
        pass
PY
SQLITE_LOCK_PID=$!

for _ in {1..100}; do
    if [[ -f "$LOCK_READY" ]]; then
        break
    fi
    if ! kill -0 "$SQLITE_LOCK_PID" 2>/dev/null; then
        cat "$LOCK_RESULT" >&2 2>/dev/null || true
        echo "sqlite lock holder exited before acquiring lock" >&2
        exit 1
    fi
    sleep 0.05
done
if [[ ! -f "$LOCK_READY" ]]; then
    echo "sqlite lock holder did not report ready" >&2
    exit 1
fi

am fact add \
    "SQLite lock retry waited for a busy writer" \
    --entities SQLite,AdvancedSurface \
    --type lesson \
    --source-id advanced-smoke \
    >"$LOCK_FACT_OUT" 2>"$LOCK_FACT_ERR" &
SQLITE_FACT_PID=$!
sleep 0.3
if ! kill -0 "$SQLITE_FACT_PID" 2>/dev/null; then
    echo "fact add exited before SQLite busy lock was released" >&2
    cat "$LOCK_FACT_OUT" >&2 || true
    cat "$LOCK_FACT_ERR" >&2 || true
    exit 1
fi
touch "$LOCK_RELEASE"
if ! wait "$SQLITE_FACT_PID"; then
    SQLITE_FACT_PID=""
    cat "$LOCK_FACT_OUT" >&2 || true
    cat "$LOCK_FACT_ERR" >&2 || true
    exit 1
fi
SQLITE_FACT_PID=""
if ! wait "$SQLITE_LOCK_PID"; then
    SQLITE_LOCK_PID=""
    cat "$LOCK_RESULT" >&2 || true
    exit 1
fi
SQLITE_LOCK_PID=""
expect_contains "sqlite busy timeout fact add" \
    "$(cat "$LOCK_FACT_OUT")" \
    "Added fact"
expect_contains "sqlite busy timeout search" \
    "$(am search "busy writer" --limit 5)" \
    "SQLite lock retry waited for a busy writer"

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

LOCK_READY="$BASE/sqlite-feedback-lock-ready"
LOCK_RELEASE="$BASE/sqlite-feedback-lock-release"
LOCK_RESULT="$BASE/sqlite-feedback-lock-result"
LOCK_FEEDBACK_OUT="$BASE/sqlite-lock-feedback.out"
LOCK_FEEDBACK_ERR="$BASE/sqlite-lock-feedback.err"

python3 - "$STORE" "$LOCK_READY" "$LOCK_RELEASE" "$LOCK_RESULT" <<'PY' &
import pathlib
import sqlite3
import sys
import time
import traceback

store, ready, release, result = sys.argv[1:]
try:
    conn = sqlite3.connect(store, timeout=5.0, isolation_level=None)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("BEGIN IMMEDIATE")
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)",
        ("sqlite_feedback_retry_gate", b"held"),
    )
    pathlib.Path(ready).write_text("locked")
    deadline = time.time() + 10
    while time.time() < deadline and not pathlib.Path(release).exists():
        time.sleep(0.05)
    if not pathlib.Path(release).exists():
        raise TimeoutError("timed out waiting for release signal")
    conn.commit()
    pathlib.Path(result).write_text("released")
except Exception:
    pathlib.Path(result).write_text(traceback.format_exc())
    raise
finally:
    try:
        conn.close()
    except Exception:
        pass
PY
SQLITE_LOCK_PID=$!

for _ in {1..100}; do
    if [[ -f "$LOCK_READY" ]]; then
        break
    fi
    if ! kill -0 "$SQLITE_LOCK_PID" 2>/dev/null; then
        cat "$LOCK_RESULT" >&2 2>/dev/null || true
        echo "sqlite feedback lock holder exited before acquiring lock" >&2
        exit 1
    fi
    sleep 0.05
done
if [[ ! -f "$LOCK_READY" ]]; then
    echo "sqlite feedback lock holder did not report ready" >&2
    exit 1
fi

am fact feedback "$FEEDBACK_FACT" --helpful >"$LOCK_FEEDBACK_OUT" 2>"$LOCK_FEEDBACK_ERR" &
SQLITE_FACT_PID=$!
sleep 0.3
if ! kill -0 "$SQLITE_FACT_PID" 2>/dev/null; then
    echo "fact feedback exited before SQLite busy lock was released" >&2
    cat "$LOCK_FEEDBACK_OUT" >&2 || true
    cat "$LOCK_FEEDBACK_ERR" >&2 || true
    exit 1
fi
touch "$LOCK_RELEASE"
if ! wait "$SQLITE_FACT_PID"; then
    SQLITE_FACT_PID=""
    cat "$LOCK_FEEDBACK_OUT" >&2 || true
    cat "$LOCK_FEEDBACK_ERR" >&2 || true
    exit 1
fi
SQLITE_FACT_PID=""
if ! wait "$SQLITE_LOCK_PID"; then
    SQLITE_LOCK_PID=""
    cat "$LOCK_RESULT" >&2 || true
    exit 1
fi
SQLITE_LOCK_PID=""
expect_contains "fact feedback busy timeout" \
    "$(cat "$LOCK_FEEDBACK_OUT")" \
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

echo "sqlite advanced-surface ok ($BACKEND): busy-timeout/feedback/adapt/extract/pending/consolidate"
