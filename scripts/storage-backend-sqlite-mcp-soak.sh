#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_SQLITE_MCP_SOAK_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-sqlite-mcp-soak.XXXXXX")}"
BIN="$ROOT_DIR/target/debug/aidememo"
BACKEND="${AIDEMEMO_SQLITE_MCP_SOAK_BACKEND:-libsqlite}"
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
cargo build -p aidememo-cli --no-default-features --features sqlite

HOME_DIR="$BASE/home"
STORE="$BASE/soak.sqlite"
WIKI_DIR="$BASE/wiki"
LOG="$BASE/mcp.log"
mkdir -p "$HOME_DIR" "$WIKI_DIR"

HOME="$HOME_DIR" "$BIN" config set store.backend "$BACKEND" >/dev/null
HOME="$HOME_DIR" "$BIN" config set search.semantic_weight 0 >/dev/null
backend="$(HOME="$HOME_DIR" "$BIN" config get store.backend)"
if [[ "$backend" != "$BACKEND" ]]; then
    echo "expected $BACKEND backend, got $backend" >&2
    exit 1
fi
cat > "$WIKI_DIR/McpPlatform.md" <<'MD'
# McpPlatform

[[McpPlatform]] uses [[McpRedis]] for SQLite MCP graph smoke.

## Decision

McpPlatform uses McpRedis through the SQLite MCP smoke.
MD
cat > "$WIKI_DIR/McpRedis.md" <<'MD'
# McpRedis

McpRedis is the cache dependency referenced by McpPlatform.
MD
HOME="$HOME_DIR" "$BIN" --store "$STORE" ingest "$WIKI_DIR" >/dev/null
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

python3 - "$PORT" <<'PY'
import json
import sys
import urllib.request

port = int(sys.argv[1])
url = f"http://127.0.0.1:{port}/mcp"
next_id = 1


def call_text(name, arguments):
    global next_id
    payload = {
        "jsonrpc": "2.0",
        "id": next_id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }
    next_id += 1
    req = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=20) as resp:
        body = json.loads(resp.read().decode("utf-8"))
    if "error" in body:
        raise RuntimeError(f"{name} JSON-RPC error: {body['error']}")
    result = body.get("result", {})
    if result.get("isError") or result.get("is_error"):
        raise RuntimeError(f"{name} tool error: {result}")
    blocks = result.get("content") or []
    if not blocks:
        raise RuntimeError(f"{name} returned no content: {body}")
    return blocks[0].get("text", "")


def call_json(name, arguments):
    text = call_text(name, arguments)
    try:
        return json.loads(text)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"{name} did not return JSON: {text[:1000]}") from exc


def assert_contains(label, haystack, needle):
    if needle not in haystack:
        raise AssertionError(f"{label} missing {needle!r}: {haystack[:1000]}")


batch = call_json(
    "aidememo_fact_add_many",
    {
        "source_id": "mcp-smoke",
        "items": [
            {
                "content": "SQLite MCP tool smoke primary Redis fact",
                "entities": ["McpRedis", "McpPlatform"],
                "fact_type": "claim",
            },
            {
                "content": "SQLite MCP tool smoke superseded Redis fact",
                "entities": ["McpRedis"],
                "fact_type": "note",
            },
            {
                "content": "SQLite MCP tool smoke replacement Redis fact",
                "entities": ["McpRedis"],
                "fact_type": "note",
            },
            {
                "content": "SQLite MCP tool smoke archive Redis fact",
                "entities": ["McpRedis"],
                "fact_type": "note",
            },
            {
                "content": "Spent $12 on SQLite MCP tool smoke",
                "entities": ["McpRedis"],
                "fact_type": "note",
            },
        ],
    },
)
facts = batch.get("facts", [])
if batch.get("count") != 5 or len(facts) != 5:
    raise AssertionError(f"unexpected add_many payload: {batch}")
primary_id, old_id, replacement_id, archive_id, spend_id = [fact["id"] for fact in facts]

assert_contains(
    "entity describe",
    call_text(
        "aidememo_entity_describe",
        {"name": "McpRedis", "summary": "SQLite MCP tool smoke summary"},
    ),
    "Updated summary",
)
entity = call_json("aidememo_entity_get", {"name": "McpRedis"})
if entity.get("summary") != "SQLite MCP tool smoke summary":
    raise AssertionError(f"summary did not persist through MCP: {entity}")

edit = call_json(
    "aidememo_fact_edit",
    {"id": primary_id, "append": "Edited through SQLite MCP tool smoke."},
)
if not edit.get("applied"):
    raise AssertionError(f"fact edit did not apply: {edit}")
primary = call_json("aidememo_fact_get", {"id": primary_id})
assert_contains("fact get after edit", primary.get("content", ""), "Edited through SQLite")

pin = call_json("aidememo_fact_pin", {"id": primary_id, "pinned": True})
if pin.get("pinned") is not True:
    raise AssertionError(f"pin did not apply: {pin}")
pinned = call_json("aidememo_pinned_context", {"limit": 5})
if primary_id not in json.dumps(pinned):
    raise AssertionError(f"pinned context missing primary fact: {pinned}")

supersede = call_json("aidememo_fact_supersede", {"old_id": old_id, "new_id": replacement_id})
if not supersede.get("applied"):
    raise AssertionError(f"supersede did not apply: {supersede}")
listed = call_json(
    "aidememo_fact_list",
    {"entity": "McpRedis", "source_id": "mcp-smoke", "limit": 20},
)
listed_ids = {fact["id"] for fact in listed.get("facts", [])}
if old_id in listed_ids or replacement_id not in listed_ids:
    raise AssertionError(f"current fact list did not reflect supersede: {listed}")

search = call_json(
    "aidememo_search",
    {
        "query": "primary Redis",
        "bm25_only": True,
        "source_id": "mcp-smoke",
        "limit": 5,
    },
)
if primary_id not in json.dumps(search):
    raise AssertionError(f"search missing primary fact: {search}")
session_id = search.get("session_id")
if not session_id:
    raise AssertionError(f"search did not return a session id: {search}")
feedback = call_json(
    "aidememo_feedback",
    {"session_id": session_id, "fact_id": primary_id, "helpful": True},
)
if feedback.get("ok") is not True:
    raise AssertionError(f"feedback did not persist: {feedback}")

query = call_json(
    "aidememo_query",
    {
        "topic": "McpRedis",
        "mode": "naive",
        "bm25_only": True,
        "source_id": "mcp-smoke",
        "limit": 5,
    },
)
if not query.get("search") or "mcp-smoke" not in json.dumps(query):
    raise AssertionError(f"query missing SQLite MCP smoke facts: {query}")

context = call_json(
    "aidememo_context",
    {
        "topic": "McpRedis",
        "bm25_only": True,
        "source_id": "mcp-smoke",
        "limit": 5,
        "recent_limit": 5,
    },
)
if "topic" not in context or "mcp-smoke" not in json.dumps(context):
    raise AssertionError(f"context missing topic evidence: {context}")

recent = call_json("aidememo_recent", {"limit": 10, "last": "1d"})
if primary_id not in json.dumps(recent):
    raise AssertionError(f"recent missing primary fact: {recent}")

entities = call_json("aidememo_entity_list", {"limit": 20})
if "McpRedis" not in json.dumps(entities):
    raise AssertionError(f"entity list missing McpRedis: {entities}")

overview = call_json("aidememo_overview", {"top_n": 10, "recent_days": 1})
if overview.get("stats", {}).get("facts", 0) < 5:
    raise AssertionError(f"overview stats too small: {overview}")

doctor = call_json("aidememo_doctor", {})
if "sharing" not in doctor or "stats" not in doctor:
    raise AssertionError(f"doctor missing sharing/stats: {doctor}")

session_start = call_json(
    "aidememo_session_start",
    {"pinned_limit": 5, "recent_limit": 5, "top_entities_limit": 5},
)
if "pinned" not in session_start or "recent" not in session_start:
    raise AssertionError(f"session_start missing sections: {session_start}")

traverse_text = call_text("aidememo_traverse", {"entity": "McpPlatform", "depth": 2})
assert_contains("traverse", traverse_text, "McpRedis")
path = call_json("aidememo_path", {"from": "McpPlatform", "to": "McpRedis"})
if "McpRedis" not in json.dumps(path):
    raise AssertionError(f"path missing McpRedis: {path}")

aggregate = call_json(
    "aidememo_aggregate",
    {
        "query": "SQLite MCP tool smoke",
        "op": "count",
        "limit": 20,
        "source_id": "mcp-smoke",
    },
)
if aggregate.get("matched", 0) < 4:
    raise AssertionError(f"aggregate count too small: {aggregate}")

extract_preview = call_json(
    "aidememo_extract",
    {
        "text": "Decided to keep SQLite as the MCP storage default.",
        "max_candidates": 5,
        "min_confidence": 0.0,
    },
)
if extract_preview.get("applied") is not False or "candidates" not in extract_preview:
    raise AssertionError(f"extract preview shape changed: {extract_preview}")

workflow = call_json(
    "aidememo_workflow_start",
    {
        "title": "SQLite MCP tool smoke workflow",
        "body": "Validate the SQLite MCP tool surface.",
        "source": "local:mcp-smoke",
        "source_id": "mcp-smoke",
        "bm25_only": True,
        "limit": 3,
    },
)
if not workflow.get("session_id"):
    raise AssertionError(f"workflow_start missing session_id: {workflow}")

archive_preview = call_json("aidememo_fact_archive", {"ids": [archive_id], "dry_run": True})
if archive_preview.get("moved") != 1 or archive_preview.get("dry_run") is not True:
    raise AssertionError(f"archive dry-run mismatch: {archive_preview}")
archive = call_json("aidememo_fact_archive", {"ids": [archive_id]})
if archive.get("moved") != 1:
    raise AssertionError(f"archive did not move one fact: {archive}")
archived_fact = call_json("aidememo_fact_get", {"id": archive_id})
assert_contains("archived fact get", archived_fact.get("content", ""), "archive Redis fact")
archived_search = call_json(
    "aidememo_search",
    {
        "query": "archive Redis fact",
        "bm25_only": True,
        "source_id": "mcp-smoke",
        "include_archive": True,
        "limit": 5,
    },
)
if archive_id not in json.dumps(archived_search):
    raise AssertionError(f"include_archive search missing archived fact: {archived_search}")

print(
    json.dumps(
        {
            "mcp_tool_smoke": "ok",
            "primary_id": primary_id,
            "spend_id": spend_id,
            "workflow_session": workflow["session_id"],
        }
    )
)
PY

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
minimum = writes + 1
if stats["fact_count"] < minimum:
    raise SystemExit(f"expected at least {minimum} facts, got {stats['fact_count']}: {stats}")
if stats["entity_count"] < 1:
    raise SystemExit(f"expected at least one entity, got {stats['entity_count']}: {stats}")
PY

TAIL_INDEX="$(printf '%04d' "$((WRITES - 1))")"
search_out="$(HOME="$HOME_DIR" "$BIN" --store "$STORE" search "soak fact $TAIL_INDEX" --limit 5)"
if [[ "$search_out" != *"SQLite MCP soak fact $TAIL_INDEX"* ]]; then
    echo "soak search did not return the tail fact" >&2
    echo "$search_out" >&2
    exit 1
fi

mcp_cleanup
trap cleanup EXIT

DAEMON_PORT="$(find_port)"
HOME="$HOME_DIR" "$BIN" --backend "$BACKEND" --store "$STORE" daemon start \
    --port "$DAEMON_PORT" >/dev/null
daemon_cleanup() {
    HOME="$HOME_DIR" "$BIN" --backend "$BACKEND" --store "$STORE" daemon stop >/dev/null 2>&1 || true
}
trap 'daemon_cleanup; cleanup' EXIT

daemon_status="$(HOME="$HOME_DIR" "$BIN" --backend "$BACKEND" --store "$STORE" daemon status)"
if [[ "$daemon_status" != *"matches CLI store/backend"* ]]; then
    echo "SQLite-only daemon did not report a matching backend" >&2
    echo "$daemon_status" >&2
    exit 1
fi

daemon_search="$(HOME="$HOME_DIR" "$BIN" --backend "$BACKEND" --store "$STORE" search "SQLite MCP soak seed" --limit 5)"
if [[ "$daemon_search" != *"SQLite MCP soak seed"* ]]; then
    echo "SQLite-only daemon-discovered search did not return the seed fact" >&2
    echo "$daemon_search" >&2
    exit 1
fi

daemon_stop="$(HOME="$HOME_DIR" "$BIN" --backend "$BACKEND" --store "$STORE" daemon stop)"
if [[ "$daemon_stop" != *"aidememo daemon stopped"* ]]; then
    echo "SQLite-only daemon stop did not report success" >&2
    echo "$daemon_stop" >&2
    exit 1
fi
trap cleanup EXIT

echo "sqlite mcp soak ok (${BACKEND}): ${WRITES} writes across ${WORKERS} workers + daemon lifecycle"
