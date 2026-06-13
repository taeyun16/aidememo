#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_SQLITE_FULL_SURFACE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-sqlite-full-surface.XXXXXX")}"
BACKEND="${AIDEMEMO_SQLITE_FULL_SURFACE_BACKEND:-libsqlite}"
BIN="$ROOT_DIR/target/debug/aidememo"

cleanup() {
    if [[ "${AIDEMEMO_SQLITE_FULL_SURFACE_KEEP_TMP:-0}" != "1" ]]; then
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

session_id_from_output() {
    python3 - "$1" <<'PY'
import re
import sys

match = re.search(r"AIDEMEMO_SESSION_ID=(session-[0-9A-HJKMNP-TV-Z]{26}|[0-9A-HJKMNP-TV-Z]{26})", sys.argv[1])
if not match:
    raise SystemExit(f"could not parse session id from: {sys.argv[1]}")
print(match.group(1))
PY
}

assert_stats_at_least() {
    python3 - "$1" "$2" "$3" "$4" <<'PY'
import json
import sys

stats = json.loads(sys.argv[1])
min_entities = int(sys.argv[2])
min_facts = int(sys.argv[3])
min_relations = int(sys.argv[4])
if stats["entity_count"] < min_entities:
    raise SystemExit(f"entity_count too small: {stats}")
if stats["fact_count"] < min_facts:
    raise SystemExit(f"fact_count too small: {stats}")
if stats["relation_count"] < min_relations:
    raise SystemExit(f"relation_count too small: {stats}")
PY
}

assert_json_contains_fact() {
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

cd "$ROOT_DIR"
cargo build -p aidememo-cli --features sqlite

HOME_DIR="$BASE/home"
WIKI_DIR="$BASE/wiki"
STORE="$BASE/full-surface.sqlite"
IMPORT_STORE="$BASE/imported.sqlite"
EXPORT="$BASE/export.jsonl"
mkdir -p "$HOME_DIR" "$WIKI_DIR"

am() {
    HOME="$HOME_DIR" "$BIN" --store "$STORE" "$@"
}

am_json() {
    HOME="$HOME_DIR" "$BIN" --store "$STORE" --json "$@"
}

HOME="$HOME_DIR" "$BIN" config set store.backend "$BACKEND" >/dev/null
backend="$(HOME="$HOME_DIR" "$BIN" config get store.backend)"
if [[ "$backend" != "$BACKEND" ]]; then
    echo "expected $BACKEND backend, got $backend" >&2
    exit 1
fi

cat > "$WIKI_DIR/Platform.md" <<'MD'
# Platform

[[Platform]] uses [[Redis]] for the SQLite full surface smoke.

## Decision

The platform uses SQLite as the default AideMemo store.
MD

cat > "$WIKI_DIR/Redis.md" <<'MD'
# Redis

Redis is the cache dependency referenced by Platform.
MD

am init --no-ingest "$WIKI_DIR" >/dev/null
assert_stats_at_least "$(am_json stats)" 0 0 0

am ingest "$WIKI_DIR" >/dev/null
assert_stats_at_least "$(am_json stats)" 2 1 1

am sync ingest "$WIKI_DIR" >/dev/null
assert_stats_at_least "$(am_json stats)" 2 1 1

am entity add CacheLayer --type technology --aliases cache >/dev/null
am entity alias CacheLayer cache-layer >/dev/null
am entity describe CacheLayer "Compiled summary stored through the SQLite backend." >/dev/null
expect_contains "entity get alias" "$(am entity get cache-layer)" "CacheLayer"
expect_contains "entity show" "$(am entity show CacheLayer)" "Compiled summary"

fact_a_out="$(am fact add \
    "SQLite full surface smoke writes warm cache facts" \
    --entities CacheLayer,Redis \
    --type claim \
    --source-id smoke \
    --observed-at 2026-01-01)"
FACT_A="$(fact_id_from_output "$fact_a_out")"

fact_b_out="$(am fact add \
    "SQLite full surface smoke chose WAL mode for local tests" \
    --entities CacheLayer \
    --type decision \
    --source-id smoke \
    --observed-at 2026-01-02)"
FACT_B="$(fact_id_from_output "$fact_b_out")"

fact_c_out="$(am fact add \
    "SQLite full surface smoke chose bundled SQLite for local tests" \
    --entities CacheLayer \
    --type decision \
    --source-id smoke \
    --observed-at 2026-01-03)"
FACT_C="$(fact_id_from_output "$fact_c_out")"

am fact supersede "$FACT_B" "$FACT_C" >/dev/null
am edit fact "$FACT_A" --append "Edited via SQLite full-surface smoke." >/dev/null
expect_contains "fact get" "$(am fact get "$FACT_A")" "Edited via SQLite"

am fact pin "$FACT_A" >/dev/null
expect_contains "pinned facts" "$(am fact pinned --limit 5)" "$FACT_A"
am fact unpin "$FACT_A" >/dev/null

assert_json_contains_fact "$(am_json fact list --source-id smoke --limit 10)" "$FACT_A"
assert_json_contains_fact "$(am recent --json --limit 10 --last 365d)" "$FACT_C"
expect_contains "search" "$(am search "warm cache facts" --source-id smoke --limit 5)" "warm cache facts"
expect_contains "query" "$(am query "CacheLayer" -m naive --limit 5 --recent-limit 5)" "CacheLayer"
expect_contains "traverse" "$(am traverse Platform --depth 2)" "Redis"
expect_contains "path" "$(am path Platform Redis)" "Redis"
expect_contains "graph" "$(am graph --from Platform --depth 2)" "Platform"

assert_json_contains_fact "$(am overview --json --recent-days 365)" "CacheLayer"
am lint --json >/dev/null
am doctor --json >/dev/null

session_out="$(am session new "SQLite full surface session")"
SESSION_ID="$(session_id_from_output "$session_out")"
AIDEMEMO_SESSION_ID="$SESSION_ID" HOME="$HOME_DIR" "$BIN" --store "$STORE" fact add \
    "Session-scoped SQLite full surface fact" \
    --entities CacheLayer \
    --type lesson \
    --source-id smoke >/dev/null
expect_contains "session current" "$(AIDEMEMO_SESSION_ID="$SESSION_ID" HOME="$HOME_DIR" "$BIN" --store "$STORE" session current)" "$SESSION_ID"
expect_contains "session list" "$(am session list)" "$SESSION_ID"

workflow_out="$(am workflow start \
    "SQLite full surface workflow" \
    --body "Validate the SQLite default storage backend." \
    --source github:aidememo/smoke#1 \
    --source-id smoke \
    --bm25-only \
    --max-chars 4000)"
expect_contains "workflow start" "$workflow_out" "SQLite full surface workflow"

am export --output "$EXPORT" >/dev/null
if [[ ! -s "$EXPORT" ]]; then
    echo "expected non-empty export at $EXPORT" >&2
    exit 1
fi

HOME="$HOME_DIR" "$BIN" --store "$IMPORT_STORE" import "$EXPORT" >/dev/null
expect_contains "imported search" \
    "$(HOME="$HOME_DIR" "$BIN" --store "$IMPORT_STORE" search "bundled SQLite" --source-id smoke --limit 5)" \
    "bundled SQLite"

am fact archive --ids "$FACT_A" >/dev/null
if [[ ! -f "$STORE.cold.sqlite" ]]; then
    echo "expected SQLite cold-tier file: $STORE.cold.sqlite" >&2
    exit 1
fi
expect_contains "archived fact get" "$(am fact get "$FACT_A")" "warm cache facts"
expect_contains "archive search" "$(am search "warm cache facts" --include-archive --limit 5)" "warm cache facts"
expect_contains "archive dry-run after cold move" "$(am fact archive --ids "$FACT_A" --dry-run)" "No hot facts matched"

assert_stats_at_least "$(am_json stats)" 4 5 1

echo "sqlite full-surface ok ($BACKEND): init/ingest/read/write/search/graph/session/workflow/archive/export/import"
