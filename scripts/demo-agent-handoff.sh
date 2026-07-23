#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -n "${AIDEMEMO_BIN:-}" ]]; then
    AM="$AIDEMEMO_BIN"
elif [[ -f "$ROOT_DIR/Cargo.toml" ]] && command -v cargo >/dev/null 2>&1; then
    cargo build -p aidememo-cli >/dev/null
    AM="$ROOT_DIR/target/debug/aidememo"
else
    AM="$(command -v aidememo || true)"
fi

if [[ -z "$AM" || ! -x "$AM" ]]; then
    echo "aidememo binary not found. Set AIDEMEMO_BIN=/path/to/aidememo." >&2
    exit 1
fi

DEMO_DIR="${AIDEMEMO_HANDOFF_DEMO_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-handoff.XXXXXX")}"
STORE="$DEMO_DIR/team.sqlite"
PACKET="$DEMO_DIR/codex-to-hermes.md"

cleanup() {
    if [[ "${AIDEMEMO_DEMO_KEEP:-0}" != "1" ]]; then
        rm -rf "$DEMO_DIR"
    fi
}
trap cleanup EXIT

am() {
    "$AM" --store "$STORE" "$@"
}

workflow_json="$(
    am --json workflow start "Harden release preflight" \
        --body "The orchestrator assigned Codex to diagnose the flaky package gate." \
        --source "orchestrator:demo/run-01" \
        --source-id "release-team" \
        --bm25-only
)"

AIDEMEMO_SESSION_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["session_id"])' <<<"$workflow_json")"
export AIDEMEMO_SESSION_ID

am fact add \
    "Decision: run the package smoke before the full release preflight." \
    --type decision --entities Release --source-id release-team >/dev/null
am fact add \
    "Lesson: the previous failure came from stale wheel metadata, not Rust compilation." \
    --type lesson --entities Release,Python --source-id release-team >/dev/null
am fact add \
    "Error: do not publish until the installed wheel version matches workspace metadata." \
    --type error --entities Release,Python --source-id release-team >/dev/null

am session handoff \
    --from codex/coding \
    --to hermes/reviewer \
    --focus "Verify package metadata, then run the release preflight if the smoke passes." \
    --done-when "The installed wheel matches workspace metadata and release preflight passes." \
    --source-id release-team \
    --output "$PACKET" \
    "$AIDEMEMO_SESSION_ID" >/dev/null

python3 - "$PACKET" "$AIDEMEMO_SESSION_ID" <<'PY'
from pathlib import Path
import sys

packet = Path(sys.argv[1]).read_text()
session_id = sys.argv[2]

required = [
    "# AideMemo Agent Handoff",
    f"session: `{session_id}`",
    "from_agent: codex",
    "from_profile: coding",
    "to_agent: hermes",
    "to_profile: reviewer",
    "## Resume Contract",
    "## Definition of Done",
    "installed wheel matches workspace metadata",
    "eval \"$(aidememo session resume --source-id 'release-team'",
    "run the package smoke before the full release preflight",
    "stale wheel metadata",
    "do not publish until",
    "aidememo fact get <fact_id>",
]
missing = [value for value in required if value not in packet]
if missing:
    raise SystemExit("handoff packet missing: " + ", ".join(missing))

print("aidememo agent handoff demo")
print(f"session: {session_id}")
print("route: codex/coding -> hermes/reviewer")
print("scope: release-team")
print("OK: receiver got focus + decision + lesson + error with fact-id verification")
PY

SESSION_ID="$AIDEMEMO_SESSION_ID"
unset AIDEMEMO_SESSION_ID AIDEMEMO_SOURCE_ID
eval "$(am session resume --source-id release-team "$SESSION_ID")"
if [[ "$AIDEMEMO_SESSION_ID" != "$SESSION_ID" || "$AIDEMEMO_SOURCE_ID" != "release-team" ]]; then
    echo "session resume did not activate the handoff environment" >&2
    exit 1
fi
echo "OK: one-command resume activated session + source scope"

if [[ "${AIDEMEMO_DEMO_KEEP:-0}" == "1" ]]; then
    echo "packet: $PACKET"
fi
