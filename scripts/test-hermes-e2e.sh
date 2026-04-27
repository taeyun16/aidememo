#!/usr/bin/env bash
# End-to-end test for the hermes-wg plugin against a real Hermes Agent
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
#   - `wg` CLI built at $REPO/target/debug/wg (we'll cargo build if
#     missing).
#   - `hermes` on PATH at $HOME/.local/bin/hermes (or wherever).
#   - Hermes's bundled venv lives at the standard `~/.hermes/hermes-agent/venv/`.
#   - The plugin source tree at `$REPO/plugins/hermes/src/hermes_wg/`.
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
PLUGIN_SRC="$REPO/plugins/hermes/src/hermes_wg"
WG_BIN_DIR="$REPO/target/debug"
WG_BIN="$WG_BIN_DIR/wg"

# Use a unique throwaway HERMES_HOME so we never touch the operator's
# real ~/.hermes. Cleaned up on exit (success or failure).
TEST_HOME="$(mktemp -d -t wg-hermes-e2e.XXXXXX)"
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

if [[ ! -x "$WG_BIN" ]]; then
    log "wg binary not found, building (cargo build -p wg-cli)…"
    (cd "$REPO" && cargo build -p wg-cli)
fi
ok "wg binary at $WG_BIN"

# ----------------------------------------------------------------------
# Wire the isolated Hermes profile
# ----------------------------------------------------------------------

mkdir -p "$TEST_HOME/plugins"
ln -s "$PLUGIN_SRC" "$TEST_HOME/plugins/wg"
log "isolated HERMES_HOME=$TEST_HOME"

# Hermes spawns plugin code inheriting our $PATH, so wg must be
# reachable. We shadow the operator's real PATH-prepending here so
# the test sees only the binary we just built.
export PATH="$WG_BIN_DIR:$PATH"
export HERMES_HOME="$TEST_HOME"
# Force the plugin's WgClient to use the test wiki (not whatever the
# operator's wg config defaults to).
export WG_STORE="$TEST_HOME/wiki.redb"

# ----------------------------------------------------------------------
# 1. Plugin shows up in `hermes plugins list`
# ----------------------------------------------------------------------

log "checking hermes plugins list discovers wg"
# Capture the output rather than streaming through `grep -q`:
#   - `grep -q` exits on the first match and closes its end of the
#     pipe; hermes is still writing → SIGPIPE → exit 141
#   - with `set -o pipefail` (which we want for the rest of the
#     script), that 141 surfaces as the pipeline status
# Capturing into a variable sidesteps the pipe entirely. `-a` forces
# text-mode matching past macOS BSD grep's UTF-8 binary detection,
# and the literal " wg " is unique to the plugin's own row in the
# unicode box-drawing table.
plugin_list="$("$HERMES_BIN" plugins list 2>&1 || true)"
if ! printf '%s' "$plugin_list" | grep -aqF '│ wg '; then
    err "wg plugin not discovered by hermes plugins list"
    printf '%s\n' "$plugin_list" >&2
    exit 1
fi
ok "discovery"

# ----------------------------------------------------------------------
# 2. Enable wires the plugin without complaint
# ----------------------------------------------------------------------

log "enabling plugin"
"$HERMES_BIN" plugins enable wg > /dev/null
ok "enabled"

# ----------------------------------------------------------------------
# 3. PluginManager actually loads the module + register() registers
#    every advertised surface (7 tools, 2 hooks, 4 commands).
# ----------------------------------------------------------------------

log "loading plugin via Hermes's PluginManager"
"$HERMES_VENV_PY" - <<PY
import os, sys
os.environ.setdefault("HERMES_HOME", "$TEST_HOME")
os.environ.setdefault("WG_STORE", "$TEST_HOME/wiki.redb")
sys.path.insert(0, "$HERMES_AGENT_ROOT")

from hermes_cli.plugins import discover_plugins, get_plugin_manager

discover_plugins(force=True)
mgr = get_plugin_manager()

wg = next((p for p in mgr.list_plugins() if p.get("name") == "wg"), None)
if wg is None:
    raise SystemExit("FAIL: wg plugin missing from manager.list_plugins()")

print(f"  enabled  = {wg['enabled']}")
print(f"  tools    = {wg['tools']}")
print(f"  hooks    = {wg['hooks']}")
print(f"  commands = {wg['commands']}")
print(f"  error    = {wg['error']!r}")

if not wg["enabled"]:
    raise SystemExit("FAIL: plugin reports enabled=False")
if wg["error"]:
    raise SystemExit(f"FAIL: plugin reported load error: {wg['error']}")
if wg["tools"] != 7:
    raise SystemExit(f"FAIL: expected 7 tools, got {wg['tools']}")
if wg["hooks"] != 2:
    raise SystemExit(f"FAIL: expected 2 hooks, got {wg['hooks']}")
if wg["commands"] != 4:
    raise SystemExit(f"FAIL: expected 4 commands, got {wg['commands']}")
PY
ok "register(ctx) ran cleanly — 7 tools / 2 hooks / 4 commands"

# ----------------------------------------------------------------------
# Done
# ----------------------------------------------------------------------

ok "all checks passed"
echo
echo "  Manual follow-ups (need an LLM-configured profile):"
echo "    /wg redis"
echo "    /wg-add \"…\" --type decision --entities …"
echo "    /wg-pending"
echo "    on_session_start auto-context preamble"
