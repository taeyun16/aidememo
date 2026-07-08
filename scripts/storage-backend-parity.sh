#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_STORAGE_PARITY_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-storage-parity.XXXXXX")}"
BIN="$ROOT_DIR/target/debug/aidememo"

cleanup() {
    if [[ "${AIDEMEMO_STORAGE_PARITY_KEEP_TMP:-0}" != "1" ]]; then
        rm -rf "$BASE"
    else
        echo "kept temp dir: $BASE" >&2
    fi
}
trap cleanup EXIT

find_port() {
    python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

cd "$ROOT_DIR"
cargo build -p aidememo-cli --features sqlite,redb
cargo test -p aidememo-core --features sqlite,redb sqlite_matches_redb_for_mutation_feedback_and_relation_contract
cargo test -p aidememo-core --features sqlite,redb sync_export_import_is_backend_compatible
cargo test -p aidememo-core --features sqlite,redb,semantic sqlite_import_preserves_redb_export_ids_for_migration_gate

mkdir -p "$BASE"
REDB_HOME="$BASE/home-redb"
SQLITE_HOME="$BASE/home-sqlite"
WIKI_DIR="$BASE/wiki"
REDB_STORE="$BASE/source.redb"
SQLITE_STORE="$BASE/target.sqlite"
EXPORT="$BASE/export.jsonl"
MCP_STORE="$BASE/mcp.sqlite"
DAEMON_HOME="$BASE/home-daemon"
DAEMON_STORE="$BASE/daemon.sqlite"
mkdir -p "$REDB_HOME" "$SQLITE_HOME" "$DAEMON_HOME" "$WIKI_DIR"

HOME="$REDB_HOME" "$BIN" config set store.backend redb >/dev/null

printf '%s\n' \
    'Redis references [[Sentinel]].' \
    '' \
    '## Note: availability' \
    '' \
    'Redis and Sentinel are linked for storage backend parity.' \
    > "$WIKI_DIR/Redis.md"

archive_fact_out="$(HOME="$REDB_HOME" "$BIN" --store "$REDB_STORE" fact add \
    "Redis handles hot cache keys" \
    --entities Redis \
    --type claim)"
archive_fact_id="$(python3 - "$archive_fact_out" <<'PY'
import re
import sys

match = re.search(r"ID ([0-9A-HJKMNP-TV-Z]{26})", sys.argv[1])
if not match:
    raise SystemExit(f"could not parse fact id from: {sys.argv[1]}")
print(match.group(1))
PY
)"
HOME="$REDB_HOME" "$BIN" --store "$REDB_STORE" fact add \
    "Sentinel monitors Redis availability" \
    --entities Sentinel,Redis \
    --type note >/dev/null
HOME="$REDB_HOME" "$BIN" --store "$REDB_STORE" ingest "$WIKI_DIR" >/dev/null
HOME="$REDB_HOME" "$BIN" --store "$REDB_STORE" export --output "$EXPORT" >/dev/null

HOME="$SQLITE_HOME" "$BIN" config set store.backend sqlite >/dev/null
HOME="$SQLITE_HOME" "$BIN" config set search.auto_hybrid false >/dev/null
backend="$(HOME="$SQLITE_HOME" "$BIN" config get store.backend)"
if [[ "$backend" != "sqlite" ]]; then
    echo "expected sqlite backend, got $backend" >&2
    exit 1
fi

HOME="$SQLITE_HOME" "$BIN" --store "$SQLITE_STORE" import "$EXPORT" >/dev/null

redb_stats="$(HOME="$REDB_HOME" "$BIN" --store "$REDB_STORE" --json stats)"
sqlite_stats="$(HOME="$SQLITE_HOME" "$BIN" --store "$SQLITE_STORE" --json stats)"
python3 - "$redb_stats" "$sqlite_stats" <<'PY'
import json
import sys

redb = json.loads(sys.argv[1])
sqlite = json.loads(sys.argv[2])
for key in ("entity_count", "fact_count", "relation_count"):
    if redb[key] != sqlite[key]:
        raise SystemExit(f"{key} mismatch: redb={redb[key]} sqlite={sqlite[key]}")
if sqlite["relation_count"] < 1:
    raise SystemExit("expected migrated fixture to include at least one relation")
PY

search_out="$(HOME="$SQLITE_HOME" "$BIN" --store "$SQLITE_STORE" search "hot cache" --limit 3)"
if [[ "$search_out" != *"Redis handles hot cache keys"* ]]; then
    echo "SQLite search did not return migrated Redis fact" >&2
    echo "$search_out" >&2
    exit 1
fi

HOME="$SQLITE_HOME" "$BIN" --store "$SQLITE_STORE" fact archive --ids "$archive_fact_id" >/dev/null
if [[ ! -f "$SQLITE_STORE.cold.sqlite" ]]; then
    echo "expected SQLite cold-tier file: $SQLITE_STORE.cold.sqlite" >&2
    exit 1
fi
archived_search="$(HOME="$SQLITE_HOME" "$BIN" --store "$SQLITE_STORE" search "hot cache" --include-archive --limit 3)"
if [[ "$archived_search" != *"Redis handles hot cache keys"* ]]; then
    echo "SQLite include-archive search did not return archived Redis fact" >&2
    echo "$archived_search" >&2
    exit 1
fi

PORT="$(find_port)"
LOG="$BASE/mcp.log"
nohup env HOME="$SQLITE_HOME" "$BIN" --store "$MCP_STORE" mcp-serve --port "$PORT" \
    >"$LOG" 2>&1 < /dev/null &
PID=$!
mcp_cleanup() {
    kill "$PID" >/dev/null 2>&1 || true
    wait "$PID" >/dev/null 2>&1 || true
}
trap 'mcp_cleanup; cleanup' EXIT

for _ in $(seq 1 300); do
    if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$PID" >/dev/null 2>&1; then
        echo "mcp-serve exited before becoming healthy on port $PORT" >&2
        if [[ -s "$LOG" ]]; then
            cat "$LOG" >&2
        fi
        wait "$PID" >/dev/null 2>&1 || true
        exit 1
    fi
    sleep 0.1
done
if ! curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null; then
    echo "mcp-serve did not become healthy on port $PORT" >&2
    if [[ -s "$LOG" ]]; then
        cat "$LOG" >&2
    fi
    exit 1
fi
curl -fsS -X POST "http://127.0.0.1:$PORT/mcp" \
    -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"aidememo_fact_add","arguments":{"content":"SQLite MCP parity fact","entities":["SQLite"],"fact_type":"claim","dedup_check":false}}}' \
    | python3 -c 'import json, sys; payload=json.load(sys.stdin); assert "error" not in payload, payload'

python3 - "$PORT" <<'PY'
import concurrent.futures
import json
import sys
import urllib.request

port = sys.argv[1]
url = f"http://127.0.0.1:{port}/mcp"

def post(i: int) -> str:
    payload = {
        "jsonrpc": "2.0",
        "id": i + 100,
        "method": "tools/call",
        "params": {
            "name": "aidememo_fact_add",
            "arguments": {
                "content": f"SQLite MCP concurrent parity fact {i}",
                "entities": ["SQLite"],
                "fact_type": "claim",
                "dedup_check": False,
            },
        },
    }
    req = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        body = json.loads(resp.read().decode("utf-8"))
    if "error" in body:
        raise RuntimeError(body["error"])
    result = body.get("result", {})
    if result.get("isError") or result.get("is_error"):
        raise RuntimeError(result)
    blocks = result.get("content", [])
    if not blocks:
        raise RuntimeError(f"missing content block: {body}")
    text = blocks[0].get("text", "")
    parsed = json.loads(text)
    if "id" not in parsed:
        raise RuntimeError(parsed)
    return parsed["id"]

with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
    ids = list(pool.map(post, range(24)))

if len(set(ids)) != 24:
    raise SystemExit("concurrent MCP writes returned duplicate fact ids")
PY

mcp_stats="$(HOME="$SQLITE_HOME" "$BIN" --store "$MCP_STORE" --json stats)"
python3 - "$mcp_stats" <<'PY'
import json
import sys

stats = json.loads(sys.argv[1])
if stats["fact_count"] != 25:
    raise SystemExit(f"expected 25 MCP facts after concurrency soak, got {stats['fact_count']}")
PY

mcp_cleanup
trap cleanup EXIT

DAEMON_PORT="$(find_port)"
HOME="$DAEMON_HOME" "$BIN" config set search.auto_hybrid false >/dev/null
HOME="$DAEMON_HOME" "$BIN" --backend libsqlite daemon start \
    --store "$DAEMON_STORE" \
    --port "$DAEMON_PORT" >/dev/null
daemon_cleanup() {
    HOME="$DAEMON_HOME" "$BIN" --backend libsqlite daemon stop >/dev/null 2>&1 || true
}
trap 'daemon_cleanup; cleanup' EXIT

python3 - "$DAEMON_HOME/.aidememo/daemon.json" "$DAEMON_STORE" <<'PY'
import json
import pathlib
import sys

registry_path = pathlib.Path(sys.argv[1])
expected_store = pathlib.Path(sys.argv[2])
registry = json.loads(registry_path.read_text())
if registry.get("backend") != "sqlite":
    raise SystemExit(f"expected daemon registry backend=sqlite, got {registry.get('backend')!r}")
if pathlib.Path(registry.get("store", "")) != expected_store:
    raise SystemExit(
        f"expected daemon registry store {expected_store}, got {registry.get('store')!r}"
    )
PY

daemon_same="$(HOME="$DAEMON_HOME" "$BIN" --backend sqlite --store "$DAEMON_STORE" daemon status)"
if [[ "$daemon_same" != *"matches CLI store/backend"* ]]; then
    echo "expected sqlite daemon status to match backend" >&2
    echo "$daemon_same" >&2
    exit 1
fi

daemon_alias="$(HOME="$DAEMON_HOME" "$BIN" --backend libsqlite --store "$DAEMON_STORE" daemon status)"
if [[ "$daemon_alias" != *"matches CLI store/backend"* ]]; then
    echo "expected libsqlite daemon status to match sqlite backend alias" >&2
    echo "$daemon_alias" >&2
    exit 1
fi

daemon_mismatch="$(HOME="$DAEMON_HOME" "$BIN" --backend redb --store "$DAEMON_STORE" daemon status)"
if [[ "$daemon_mismatch" != *"same store but DIFFERENT/unknown backend"* ]]; then
    echo "expected redb daemon status to reject sqlite backend registry" >&2
    echo "$daemon_mismatch" >&2
    exit 1
fi

daemon_cleanup
trap cleanup EXIT

echo "storage backend parity ok: redb/SQLite/libsqlite mutation/feedback/sync + redb export/import -> SQLite/libsqlite + relation + archive + MCP concurrency + daemon backend registry"
