#!/usr/bin/env bash
# Bootstrap a reusable Hermes Agent test profile for wg plugin
# development. Sits between the throwaway-tempdir checks in
# `test-hermes-e2e.sh` (LLM-free CI smoke) and a fully manual
# `~/.hermes` setup — gives operators a stable, isolated profile
# they can iterate against across sessions without polluting their
# real Hermes state.
#
# Usage:
#
#   ./scripts/setup-hermes-test-env.sh setup [--inherit-auth] [--ollama MODEL]
#   eval "$(./scripts/setup-hermes-test-env.sh env)"
#   ./scripts/setup-hermes-test-env.sh seed
#   ./scripts/setup-hermes-test-env.sh teardown
#
# The four subcommands compose:
#   setup     creates HERMES_HOME, builds wg, symlinks the plugin,
#             enables it, and (optionally) wires a provider.
#   env       prints export lines for HERMES_HOME / PATH / WG_STORE
#             so a single eval lights up the shell for chat sessions.
#   seed      adds a small set of sample facts to the test wiki —
#             enough material for /wg, /wg-recent, and wg_query to
#             return non-empty results.
#   teardown  removes HERMES_HOME and exits.
#
# `--inherit-auth` copies the operator's existing
# `~/.hermes/auth.json` + `.env` into the test profile so whatever
# provider they have configured (MiniMax, Anthropic, OpenAI, …)
# works without re-running `hermes auth`.
#
# `--ollama MODEL` points the test profile at a local Ollama server
# at http://localhost:11434/v1 with the named model. Useful for
# secret-free testing — pair with `ollama pull <model>` first.

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
TEST_HOME="${WG_HERMES_TEST_HOME:-/tmp/wg-hermes-test}"
PLUGIN_SRC="$REPO/plugins/hermes/src/hermes_wg"
WG_BIN="$REPO/target/debug/wg"
WG_BIN_DIR="$REPO/target/debug"
HERMES_BIN="${HERMES_BIN:-$(command -v hermes || true)}"

# ----------------------------------------------------------------------
# Helpers (only emit ANSI when stdout is a TTY, so `eval` of `env` is clean)
# ----------------------------------------------------------------------

if [[ -t 1 ]]; then
    log() { printf '\033[36m▶\033[0m %s\n' "$*" >&2; }
    ok()  { printf '\033[32m✓\033[0m %s\n' "$*" >&2; }
    err() { printf '\033[31m✗\033[0m %s\n' "$*" >&2; }
else
    log() { printf '▶ %s\n' "$*" >&2; }
    ok()  { printf '✓ %s\n' "$*" >&2; }
    err() { printf '✗ %s\n' "$*" >&2; }
fi

# ----------------------------------------------------------------------
# Subcommand: setup
# ----------------------------------------------------------------------

cmd_setup() {
    local inherit_auth=0
    local ollama_model=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --inherit-auth)
                inherit_auth=1
                shift
                ;;
            --ollama)
                ollama_model="${2:-}"
                if [[ -z "$ollama_model" ]]; then
                    err "--ollama requires a model name (e.g. --ollama gemma4:e4b)"
                    exit 2
                fi
                shift 2
                ;;
            *)
                err "unknown setup flag: $1"
                exit 2
                ;;
        esac
    done

    if [[ -z "$HERMES_BIN" ]]; then
        err "hermes CLI not on PATH. Install Hermes Agent first or set HERMES_BIN."
        exit 1
    fi
    if [[ ! -d "$PLUGIN_SRC" ]]; then
        err "Plugin source missing at $PLUGIN_SRC — wrong checkout?"
        exit 1
    fi

    if [[ ! -x "$WG_BIN" ]]; then
        log "wg binary not found, building (cargo build -p wg-cli)…"
        (cd "$REPO" && cargo build -p wg-cli)
    fi
    ok "wg binary at $WG_BIN"

    mkdir -p "$TEST_HOME/plugins"
    if [[ ! -e "$TEST_HOME/plugins/wg" ]]; then
        ln -s "$PLUGIN_SRC" "$TEST_HOME/plugins/wg"
    fi
    ok "HERMES_HOME=$TEST_HOME (plugin symlinked at plugins/wg)"

    if [[ "$inherit_auth" -eq 1 ]]; then
        local src="${HERMES_HOME_REAL:-$HOME/.hermes}"
        if [[ -f "$src/auth.json" ]]; then
            cp "$src/auth.json" "$TEST_HOME/auth.json"
            ok "copied auth.json from $src"
        else
            err "no auth.json at $src — run hermes auth first"
        fi
        if [[ -f "$src/.env" ]]; then
            cp "$src/.env" "$TEST_HOME/.env"
            ok "copied .env from $src"
        fi
    fi

    HERMES_HOME="$TEST_HOME" PATH="$WG_BIN_DIR:$PATH" "$HERMES_BIN" plugins enable wg \
        > /dev/null 2>&1 || true
    ok "wg plugin enabled"

    if [[ -n "$ollama_model" ]]; then
        if ! curl -sf http://localhost:11434/api/tags > /dev/null 2>&1; then
            err "Ollama not reachable at http://localhost:11434 — start it with `ollama serve`"
            exit 1
        fi
        # Append a `local-ollama` provider to the existing config.yaml.
        # `hermes plugins enable` writes a baseline config, so by this
        # point the file exists.
        "$HERMES_BIN" --help > /dev/null 2>&1   # ensure binary is healthy
        local cfg="$TEST_HOME/config.yaml"
        # Use python to avoid YAML hand-edits — Hermes ships PyYAML.
        local hermes_py
        hermes_py="$(dirname "$HERMES_BIN")/../share/hermes/venv/bin/python3"
        [[ -x "$hermes_py" ]] || hermes_py="$HOME/.hermes/hermes-agent/venv/bin/python3"
        [[ -x "$hermes_py" ]] || hermes_py=python3

        OLLAMA_MODEL="$ollama_model" "$hermes_py" - <<PY
import os, yaml, sys
path = "$cfg"
with open(path) as fh:
    doc = yaml.safe_load(fh) or {}
doc["model"] = {
    "default": os.environ["OLLAMA_MODEL"],
    "provider": "local-ollama",
    "base_url": "http://localhost:11434/v1",
}
providers = doc.setdefault("providers", {}) or {}
providers["local-ollama"] = {
    "base_url": "http://localhost:11434/v1",
    "api_key": "ollama",
    "api_mode": "openai",
    "default_model": os.environ["OLLAMA_MODEL"],
}
doc["providers"] = providers
with open(path, "w") as fh:
    yaml.safe_dump(doc, fh, sort_keys=False)
PY
        ok "wired Ollama provider (model=$ollama_model)"
    fi

    cat >&2 <<EOF

  Next steps:
    eval "\$($0 env)"            # export HERMES_HOME / PATH / WG_STORE
    $0 seed                      # add a few sample facts (idempotent)
    hermes chat                  # interactive chat
    hermes chat --tui            # TUI mode
    $REPO/scripts/test-hermes-e2e.sh   # LLM-free regression

  When done:
    $0 teardown                  # remove $TEST_HOME

EOF
}

# ----------------------------------------------------------------------
# Subcommand: env  (eval-friendly, no decoration)
# ----------------------------------------------------------------------

cmd_env() {
    if [[ ! -d "$TEST_HOME" ]]; then
        err "$TEST_HOME doesn't exist — run setup first"
        exit 1
    fi
    cat <<EOF
export HERMES_HOME="$TEST_HOME"
export WG_STORE="$TEST_HOME/wiki.redb"
export PATH="$WG_BIN_DIR:\$PATH"
EOF
}

# ----------------------------------------------------------------------
# Subcommand: seed
# ----------------------------------------------------------------------

cmd_seed() {
    if [[ ! -d "$TEST_HOME" ]]; then
        err "$TEST_HOME doesn't exist — run setup first"
        exit 1
    fi
    if [[ ! -x "$WG_BIN" ]]; then
        err "wg binary missing at $WG_BIN — re-run setup"
        exit 1
    fi
    log "seeding wiki at $TEST_HOME/wiki.redb"
    "$WG_BIN" --store "$TEST_HOME/wiki.redb" fact add \
        "HNSW is the default semantic index in wg" \
        --entities wg,hnsw --type decision > /dev/null 2>&1 || true
    "$WG_BIN" --store "$TEST_HOME/wiki.redb" fact add \
        "Hermes plugin auto-records decisions on session_end" \
        --entities wg,hermes --type convention > /dev/null 2>&1 || true
    "$WG_BIN" --store "$TEST_HOME/wiki.redb" fact add \
        "Tool handlers must return strings — Hermes slice-checks results" \
        --entities wg,hermes --type pattern > /dev/null 2>&1 || true
    "$WG_BIN" --store "$TEST_HOME/wiki.redb" fact add \
        "agentskills.io SKILL.md format makes the skill portable across 6 agents" \
        --entities wg,agentskills --type convention > /dev/null 2>&1 || true
    "$WG_BIN" --store "$TEST_HOME/wiki.redb" fact add \
        "wg pending review TUI is the human-driven promotion path" \
        --entities wg,tui --type pattern > /dev/null 2>&1 || true
    local count
    count="$("$WG_BIN" --store "$TEST_HOME/wiki.redb" --json stats | grep -o '"fact_count": *[0-9]*' | grep -o '[0-9]*')"
    ok "seeded — wiki now has $count fact(s)"
}

# ----------------------------------------------------------------------
# Subcommand: teardown
# ----------------------------------------------------------------------

cmd_teardown() {
    if [[ ! -d "$TEST_HOME" ]]; then
        err "$TEST_HOME doesn't exist — nothing to remove"
        exit 0
    fi
    rm -rf "$TEST_HOME"
    ok "removed $TEST_HOME"
}

# ----------------------------------------------------------------------
# Dispatch
# ----------------------------------------------------------------------

if [[ $# -lt 1 ]]; then
    cat >&2 <<EOF
Usage: $0 <setup|env|seed|teardown> [flags…]

Subcommands:
  setup [--inherit-auth] [--ollama MODEL]
                       Create $TEST_HOME, symlink plugin, enable it,
                       optionally inherit auth from ~/.hermes or wire
                       a local Ollama provider.
  env                  Print export lines (use: eval "\$($0 env)").
  seed                 Add 5 sample facts to the test wiki.
  teardown             rm -rf $TEST_HOME.
EOF
    exit 2
fi

case "$1" in
    setup)    shift; cmd_setup "$@" ;;
    env)      shift; cmd_env "$@" ;;
    seed)     shift; cmd_seed "$@" ;;
    teardown) shift; cmd_teardown "$@" ;;
    *)
        err "unknown subcommand: $1"
        exit 2
        ;;
esac
