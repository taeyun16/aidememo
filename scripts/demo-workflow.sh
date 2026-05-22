#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -n "${WG_BIN:-}" ]]; then
    WG="$WG_BIN"
elif [[ -f "$ROOT_DIR/Cargo.toml" ]] && command -v cargo >/dev/null 2>&1; then
    cargo build -p wg-cli >/dev/null
    WG="$ROOT_DIR/target/debug/wg"
elif [[ -x "$ROOT_DIR/target/debug/wg" ]]; then
    WG="$ROOT_DIR/target/debug/wg"
elif [[ -x "$ROOT_DIR/target/release/wg" ]]; then
    WG="$ROOT_DIR/target/release/wg"
else
    WG="$(command -v wg || true)"
fi

if [[ -z "$WG" || ! -x "$WG" ]]; then
    echo "wg binary not found. Set WG_BIN=/path/to/wg or run: cargo build -p wg-cli" >&2
    exit 1
fi

BASE="${WG_DEMO_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/wg-demo-workflow.XXXXXX")}"
STORE="$BASE/wiki.redb"

cleanup() {
    if [[ "${WG_DEMO_KEEP:-0}" != "1" ]]; then
        rm -rf "$BASE"
    fi
}
trap cleanup EXIT

run_wg() {
    "$WG" --store "$STORE" "$@"
}

add_fact() {
    local fact_type="$1"
    local content="$2"
    run_wg --json fact add "$content" \
        --type "$fact_type" \
        --entities Redis,Worker \
        --source-id demo >/dev/null
}

echo "wg workflow demo"
echo "store: $STORE"
echo

add_fact decision "Decision: Redis timeout fixes must go through the Worker job wrapper."
add_fact lesson "Lesson: The last Worker Redis timeout was DNS resolution, not pool size."
add_fact error "Error: Avoid increasing Redis pool size before checking DNS metrics."

start_ms="$(python3 - <<'PY'
import time
print(time.perf_counter_ns() // 1_000_000)
PY
)"
pack_json="$(
    run_wg --json workflow start "Fix Redis timeout in worker" \
        --body "Worker jobs intermittently time out. The issue body has no more detail." \
        --source github:example/app#123 \
        --source-id demo \
        --limit 8 \
        --depth 2 \
        --recent-limit 5 \
        --bm25-only
)"
end_ms="$(python3 - <<'PY'
import time
print(time.perf_counter_ns() // 1_000_000)
PY
)"

python3 - "$pack_json" "$((end_ms - start_ms))" <<'PY'
import json
import sys

pack = json.loads(sys.argv[1])
elapsed_ms = int(sys.argv[2])
lessons = pack.get("prior_lessons") or []
errors = pack.get("prior_errors") or []
decisions = pack.get("relevant_decisions") or []
search = (pack.get("context") or {}).get("search") or []

checks = {
    "session": str(pack.get("session_id", "")).startswith("session-"),
    "ticket_fact": isinstance(pack.get("ticket_fact_id"), str),
    "decision": any("Worker job wrapper" in hit.get("content", "") for hit in decisions),
    "lesson": any("DNS resolution" in hit.get("content", "") for hit in lessons),
    "error": any("DNS metrics" in hit.get("content", "") for hit in errors),
    "source_id": pack.get("source_id") == "demo",
}

print(f"ticket: {pack.get('title')}")
print(f"session: {pack.get('session_id')}")
print(f"ticket_fact: {pack.get('ticket_fact_id')}")
print(f"latency_ms: {elapsed_ms}")
print()
print("context surfaced:")
print(f"- decisions: {len(decisions)}")
print(f"- lessons: {len(lessons)}")
print(f"- errors: {len(errors)}")
print(f"- search_hits: {len(search)}")
print()
for label, rows in (("decision", decisions), ("lesson", lessons), ("error", errors)):
    if rows:
        print(f"{label}: {rows[0].get('content', '')}")

failed = [name for name, ok in checks.items() if not ok]
if failed:
    print()
    print("FAILED checks: " + ", ".join(failed), file=sys.stderr)
    raise SystemExit(1)

print()
print("OK: sparse ticket recovered decision + lesson + error context")
PY
