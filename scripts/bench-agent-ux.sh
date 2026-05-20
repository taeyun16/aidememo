#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WG_BIN="${WG_BIN:-$ROOT_DIR/target/debug/wg}"
PROCESSES="${WG_E2E_PROCESSES:-2}"
N_PER_PROC="${WG_E2E_N_PER_PROC:-10}"
CLIENTS="${WG_E2E_CLIENTS:-2}"
N_PER_CLIENT="${WG_E2E_N_PER_CLIENT:-10}"

run() {
    echo "==> $*"
    "$@"
}

cd "$ROOT_DIR"

run cargo build -p wg-cli
run cargo check -p wg-core -p wg-cli
run python3 -m pytest plugins/hermes/tests -q

run env \
    WG_BIN="$WG_BIN" \
    WG_E2E_STORE="/tmp/wg-agent-ux-d/wiki.redb" \
    WG_E2E_PROCESSES="$PROCESSES" \
    WG_E2E_N_PER_PROC="$N_PER_PROC" \
    python3 bench/multi-agent/scenario_d_concurrent_writers.py

run env \
    WG_BIN="$WG_BIN" \
    WG_E2E_STORE="/tmp/wg-agent-ux-e/wiki.redb" \
    WG_E2E_CLIENTS="$CLIENTS" \
    WG_E2E_N_PER_CLIENT="$N_PER_CLIENT" \
    python3 bench/multi-agent/scenario_e_http_shared.py
