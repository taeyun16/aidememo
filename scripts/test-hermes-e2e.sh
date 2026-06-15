#!/usr/bin/env bash
# End-to-end test for the hermes-aidememo plugin against a real Hermes Agent
# install — verifies that our `register(ctx)` actually fires inside
# Hermes's plugin host and registers every surface (tools, hooks,
# commands) with no errors.
#
# Why a shell script and not a pytest case: Hermes's plugin host
# expects to be the orchestrator. Running it under pytest would mean
# patching its private init paths and risk false positives. Driving
# it from outside via the canonical PluginManager API gives us the
# same signal a real user would see when they enable the plugin.
#
# Layout assumptions:
#   - `aidememo` CLI built at $REPO/target/debug/aidememo (we'll cargo build if
#     missing).
#   - `hermes` on PATH at $HOME/.local/bin/hermes (or wherever).
#   - Hermes's bundled venv lives at the standard `~/.hermes/hermes-agent/venv/`.
#   - The plugin source tree at `$REPO/plugins/hermes/src/hermes_aidememo/`.
#
# What we *don't* do here: actually call an LLM. That requires a
# provider credential and is slow + flaky for a regression test.
# The plugin's load surface is what we care about — broken modules
# get caught here without paying for a real chat round-trip.

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
HERMES_BIN="${HERMES_BIN:-$(command -v hermes || true)}"
HERMES_VENV_PY="${HERMES_VENV_PY:-$HOME/.hermes/hermes-agent/venv/bin/python3}"
HERMES_AGENT_ROOT="${HERMES_AGENT_ROOT:-$HOME/.hermes/hermes-agent}"
PLUGIN_SRC="$REPO/plugins/hermes/src/hermes_aidememo"
AIDEMEMO_BIN_DIR="$REPO/target/debug"
AIDEMEMO_BIN="$AIDEMEMO_BIN_DIR/aidememo"

# Use a unique throwaway HERMES_HOME so we never touch the operator's
# real ~/.hermes. Cleaned up on exit (success or failure).
TEST_HOME="$(mktemp -d -t aidememo-hermes-e2e.XXXXXX)"
trap 'rm -rf "$TEST_HOME"' EXIT

log() { printf '\033[36m▶\033[0m %s\n' "$*"; }
ok()  { printf '\033[32m✓\033[0m %s\n' "$*"; }
err() { printf '\033[31m✗\033[0m %s\n' "$*" >&2; }

# ----------------------------------------------------------------------
# Preflight
# ----------------------------------------------------------------------

if [[ -z "$HERMES_BIN" ]]; then
    err "hermes CLI not on PATH. Install Hermes Agent first or set HERMES_BIN."
    exit 1
fi
if [[ ! -x "$HERMES_VENV_PY" ]]; then
    err "Hermes venv python not found at $HERMES_VENV_PY"
    err "Override with HERMES_VENV_PY=/path/to/python3"
    exit 1
fi
if [[ ! -d "$PLUGIN_SRC" ]]; then
    err "Plugin source missing at $PLUGIN_SRC — running from a wrong checkout?"
    exit 1
fi

if [[ ! -x "$AIDEMEMO_BIN" ]]; then
    log "aidememo binary not found, building (cargo build -p aidememo-cli)…"
    (cd "$REPO" && cargo build -p aidememo-cli)
fi
ok "aidememo binary at $AIDEMEMO_BIN"

# ----------------------------------------------------------------------
# Wire the isolated Hermes profile
# ----------------------------------------------------------------------

mkdir -p "$TEST_HOME/plugins"
ln -s "$PLUGIN_SRC" "$TEST_HOME/plugins/aidememo"
log "isolated HERMES_HOME=$TEST_HOME"

# Hermes spawns plugin code inheriting our $PATH, so aidememo must be
# reachable. We shadow the operator's real PATH-prepending here so
# the test sees only the binary we just built.
export PATH="$AIDEMEMO_BIN_DIR:$PATH"
export HERMES_HOME="$TEST_HOME"
# Force the plugin's AideMemoClient to use the test wiki (not whatever the
# operator's aidememo config defaults to).
export AIDEMEMO_STORE="$TEST_HOME/wiki.sqlite"

# ----------------------------------------------------------------------
# 1. Plugin shows up in `hermes plugins list`
# ----------------------------------------------------------------------

log "checking hermes plugins list discovers aidememo"
# Capture the output rather than streaming through `grep -q`:
#   - `grep -q` exits on the first match and closes its end of the
#     pipe; hermes is still writing → SIGPIPE → exit 141
#   - with `set -o pipefail` (which we want for the rest of the
#     script), that 141 surfaces as the pipeline status
# Capturing into a variable sidesteps the pipe entirely. `-a` forces
# text-mode matching past macOS BSD grep's UTF-8 binary detection,
# and the literal " aidememo " is unique to the plugin's own row in the
# unicode box-drawing table.
plugin_list="$("$HERMES_BIN" plugins list 2>&1 || true)"
if ! printf '%s' "$plugin_list" | grep -aqF '│ aidememo '; then
    err "aidememo plugin not discovered by hermes plugins list"
    printf '%s\n' "$plugin_list" >&2
    exit 1
fi
ok "discovery"

# ----------------------------------------------------------------------
# 2. Enable wires the plugin without complaint
# ----------------------------------------------------------------------

log "enabling plugin"
"$HERMES_BIN" plugins enable aidememo > /dev/null
ok "enabled"

# ----------------------------------------------------------------------
# 3. PluginManager actually loads the module + register() registers
#    every advertised surface (8 tools, 2 hooks, 5 commands).
# ----------------------------------------------------------------------

log "loading plugin via Hermes's PluginManager"
"$HERMES_VENV_PY" - <<PY
import os, sys
os.environ.setdefault("HERMES_HOME", "$TEST_HOME")
os.environ.setdefault("AIDEMEMO_STORE", "$TEST_HOME/wiki.sqlite")
sys.path.insert(0, "$HERMES_AGENT_ROOT")

from hermes_cli.plugins import discover_plugins, get_plugin_manager

discover_plugins(force=True)
mgr = get_plugin_manager()

aidememo = next((p for p in mgr.list_plugins() if p.get("name") == "aidememo"), None)
if aidememo is None:
    raise SystemExit("FAIL: aidememo plugin missing from manager.list_plugins()")

print(f"  enabled  = {aidememo['enabled']}")
print(f"  tools    = {aidememo['tools']}")
print(f"  hooks    = {aidememo['hooks']}")
print(f"  commands = {aidememo['commands']}")
print(f"  error    = {aidememo['error']!r}")

if not aidememo["enabled"]:
    raise SystemExit("FAIL: plugin reports enabled=False")
if aidememo["error"]:
    raise SystemExit(f"FAIL: plugin reported load error: {aidememo['error']}")
if aidememo["tools"] != 8:
    raise SystemExit(f"FAIL: expected 8 tools, got {aidememo['tools']}")
if aidememo["hooks"] != 2:
    raise SystemExit(f"FAIL: expected 2 hooks, got {aidememo['hooks']}")
if aidememo["commands"] != 5:
    raise SystemExit(f"FAIL: expected 5 commands, got {aidememo['commands']}")
PY
ok "register(ctx) ran cleanly — 8 tools / 2 hooks / 5 commands"

# ----------------------------------------------------------------------
# 4. End-to-end through the TUI slash worker — proves slash commands
#    actually pump through Hermes's plugin host and our handlers fire.
#
#    The TUI ships a long-running `tui_gateway.slash_worker` process
#    that reads JSON-RPC lines from stdin and dispatches each "/aidememo…"
#    command through HermesCLI.process_command — which calls into
#    our register_command handlers. Driving that worker directly
#    sidesteps the LLM entirely (no provider auth, no API call) and
#    gives us a deterministic check that every slash surface works.
# ----------------------------------------------------------------------

if [[ ! -d "$HERMES_AGENT_ROOT/tui_gateway" ]]; then
    log "tui_gateway module missing — skipping TUI slash worker phase"
else
    log "driving tui_gateway.slash_worker for /aidememo-pending /aidememo-recent /aidememo-add /aidememo-start"
    # Pre-seed two facts so /aidememo-recent has something to show.
    "$AIDEMEMO_BIN" --store "$AIDEMEMO_STORE" fact add "HNSW is the default semantic index" \
        --entities aidememo,hnsw --type decision > /dev/null
    "$AIDEMEMO_BIN" --store "$AIDEMEMO_STORE" fact add "Hermes plugin capture adapter is opt-in" \
        --entities aidememo,hermes --type convention > /dev/null

    slash_log="$TEST_HOME/slash_output.jsonl"
    (
        cd "$HERMES_AGENT_ROOT"
        printf '%s\n%s\n%s\n%s\n%s\n' \
            '{"id":1,"command":"/aidememo-pending"}' \
            '{"id":2,"command":"/aidememo-recent 7d"}' \
            '{"id":3,"command":"/aidememo-add \"HNSW ships as default index\" --type decision --entities aidememo"}' \
            '{"id":4,"command":"/aidememo-recent 7d"}' \
            '{"id":5,"command":"/aidememo-start \"Fix Redis timeout in worker\" --body \"Worker jobs timeout against Redis\" --source github:org/repo#123"}' \
            | timeout 60 "$HERMES_VENV_PY" -m tui_gateway.slash_worker \
                --session-key aidememo-e2e-slash 2>/dev/null > "$slash_log"
    )

    # Each request returns one JSON line. Hand the file to python
    # rather than embedding the output in a heredoc — the slash
    # responses contain backslash-escaped quotes that would otherwise
    # collide with shell or python string literals.
    SLASH_LOG="$slash_log" "$HERMES_VENV_PY" - <<'PY'
import json, os, sys

with open(os.environ["SLASH_LOG"], encoding="utf-8") as fh:
    lines = [l for l in fh.read().splitlines() if l.strip()]

by_id = {json.loads(line)["id"]: json.loads(line) for line in lines}

def must(cond, msg):
    if not cond:
        raise SystemExit(f"FAIL: {msg}")

must(set(by_id) == {1, 2, 3, 4, 5}, f"missing slash responses: got ids {sorted(by_id)}")
must(all(o["ok"] for o in by_id.values()), "at least one slash returned ok=False")

# /aidememo-pending on a fresh log → no detections message
must("No pending detections" in by_id[1]["output"], f"/aidememo-pending unexpected: {by_id[1]['output']!r}")

# /aidememo-recent before add → 2 facts (the seed pair)
recent_before = by_id[2]["output"]
must(recent_before.count("\n  - [") == 2, f"/aidememo-recent should show 2 seeded facts, got: {recent_before!r}")

# /aidememo-add returns a 26-char ULID
add_msg = by_id[3]["output"]
must("Recorded 0" in add_msg and "type=decision" in add_msg, f"/aidememo-add unexpected: {add_msg!r}")

# /aidememo-recent after add → 3 facts (seed + new)
recent_after = by_id[4]["output"]
must(recent_after.count("\n  - [") == 3, f"/aidememo-recent after add should show 3 facts, got: {recent_after!r}")
must("HNSW ships as default index" in recent_after, "newly added fact missing from /aidememo-recent")

start_msg = by_id[5]["output"]
must("session_id" in start_msg and "ticket_fact_id" in start_msg, f"/aidememo-start unexpected: {start_msg!r}")
PY
    ok "/aidememo, /aidememo-start, /aidememo-add, /aidememo-recent, /aidememo-pending all dispatched through the TUI gateway"
fi

# ----------------------------------------------------------------------
# Done
# ----------------------------------------------------------------------

ok "all checks passed"
echo
echo "  LLM-required follow-ups (run with your own provider configured):"
echo "    Tool invocation by the model:"
echo "      hermes chat -q 'Issue #123: Fix Redis timeout in worker. Use the project memory workflow before planning.' -Q"
echo "    Auto-context preamble at session start:"
echo "      hermes chat --tui   # check the system message before your turn"
