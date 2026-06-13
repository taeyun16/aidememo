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
import sys

for port in range(38873, 38920):
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind(("127.0.0.1", port))
    except OSError:
        continue
    finally:
        sock.close()
    print(port)
    break
else:
    print("no free port found for MCP smoke", file=sys.stderr)
    raise SystemExit(1)
PY
}

cd "$ROOT_DIR"
cargo build -p aidememo-cli --features sqlite,redb

mkdir -p "$BASE"
REDB_HOME="$BASE/home-redb"
SQLITE_HOME="$BASE/home-sqlite"
WIKI_DIR="$BASE/wiki"
REDB_STORE="$BASE/source.redb"
SQLITE_STORE="$BASE/target.sqlite"
EXPORT="$BASE/export.jsonl"
MCP_STORE="$BASE/mcp.sqlite"
mkdir -p "$REDB_HOME" "$SQLITE_HOME" "$WIKI_DIR"

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
HOME="$SQLITE_HOME" "$BIN" --store "$MCP_STORE" mcp-serve --port "$PORT" >"$LOG" 2>&1 &
PID=$!
mcp_cleanup() {
    kill "$PID" >/dev/null 2>&1 || true
    wait "$PID" >/dev/null 2>&1 || true
}
trap 'mcp_cleanup; cleanup' EXIT

for _ in $(seq 1 80); do
    if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null
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

echo "storage backend parity ok: redb export/import -> SQLite + relation + archive + MCP concurrency"
