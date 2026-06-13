#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_SQLITE_MCP_SOAK_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-sqlite-mcp-soak.XXXXXX")}"
BIN="$ROOT_DIR/target/debug/aidememo"
WRITES="${AIDEMEMO_SQLITE_MCP_SOAK_WRITES:-200}"
WORKERS="${AIDEMEMO_SQLITE_MCP_SOAK_WORKERS:-16}"

cleanup() {
    if [[ "${AIDEMEMO_SQLITE_MCP_SOAK_KEEP_TMP:-0}" != "1" ]]; then
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

for port in range(38921, 39020):
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
    print("no free port found for SQLite MCP soak", file=sys.stderr)
    raise SystemExit(1)
PY
}

cd "$ROOT_DIR"
cargo build -p aidememo-cli --features sqlite

HOME_DIR="$BASE/home"
STORE="$BASE/soak.sqlite"
LOG="$BASE/mcp.log"
mkdir -p "$HOME_DIR"

HOME="$HOME_DIR" "$BIN" config set store.backend sqlite >/dev/null
HOME="$HOME_DIR" "$BIN" --store "$STORE" fact add \
    "SQLite MCP soak seed" \
    --entities SQLiteSoak \
    --type claim >/dev/null

PORT="$(find_port)"
HOME="$HOME_DIR" "$BIN" --store "$STORE" mcp-serve --port "$PORT" >"$LOG" 2>&1 &
PID=$!
mcp_cleanup() {
    kill "$PID" >/dev/null 2>&1 || true
    wait "$PID" >/dev/null 2>&1 || true
}
trap 'mcp_cleanup; cleanup' EXIT

for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null

python3 - "$PORT" "$WRITES" "$WORKERS" <<'PY'
import concurrent.futures
import json
import sys
import time
import urllib.request

port = int(sys.argv[1])
writes = int(sys.argv[2])
workers = int(sys.argv[3])
url = f"http://127.0.0.1:{port}/mcp"
started = time.perf_counter()


def post(i: int) -> str:
    payload = {
        "jsonrpc": "2.0",
        "id": i + 1,
        "method": "tools/call",
        "params": {
            "name": "aidememo_fact_add",
            "arguments": {
                "content": f"SQLite MCP soak fact {i:04d}",
                "entities": ["SQLiteSoak"],
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
    with urllib.request.urlopen(req, timeout=20) as resp:
        body = json.loads(resp.read().decode("utf-8"))
    if "error" in body:
        raise RuntimeError(body["error"])
    result = body.get("result", {})
    if result.get("isError") or result.get("is_error"):
        raise RuntimeError(result)
    blocks = result.get("content", [])
    if not blocks:
        raise RuntimeError(f"missing content block: {body}")
    parsed = json.loads(blocks[0].get("text", ""))
    fact_id = parsed.get("id")
    if not fact_id:
        raise RuntimeError(parsed)
    return fact_id


with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
    ids = list(pool.map(post, range(writes)))

elapsed = time.perf_counter() - started
if len(set(ids)) != writes:
    raise SystemExit(f"duplicate ids: unique={len(set(ids))} writes={writes}")
print(json.dumps({"writes": writes, "workers": workers, "elapsed_s": round(elapsed, 3)}))
PY

stats="$(HOME="$HOME_DIR" "$BIN" --store "$STORE" --json stats)"
python3 - "$stats" "$WRITES" <<'PY'
import json
import sys

stats = json.loads(sys.argv[1])
writes = int(sys.argv[2])
expected = writes + 1
if stats["fact_count"] != expected:
    raise SystemExit(f"expected {expected} facts, got {stats['fact_count']}: {stats}")
if stats["entity_count"] != 1:
    raise SystemExit(f"expected one entity, got {stats['entity_count']}: {stats}")
PY

search_out="$(HOME="$HOME_DIR" "$BIN" --store "$STORE" search "soak fact 0199" --limit 5)"
if [[ "$search_out" != *"SQLite MCP soak fact 0199"* ]]; then
    echo "soak search did not return the tail fact" >&2
    echo "$search_out" >&2
    exit 1
fi

mcp_cleanup
trap cleanup EXIT

echo "sqlite mcp soak ok: ${WRITES} writes across ${WORKERS} workers"
