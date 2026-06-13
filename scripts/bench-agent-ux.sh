#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AIDEMEMO_BIN="${AIDEMEMO_BIN:-$ROOT_DIR/target/debug/aidememo}"
PROCESSES="${AIDEMEMO_E2E_PROCESSES:-2}"
N_PER_PROC="${AIDEMEMO_E2E_N_PER_PROC:-10}"
CLIENTS="${AIDEMEMO_E2E_CLIENTS:-2}"
N_PER_CLIENT="${AIDEMEMO_E2E_N_PER_CLIENT:-10}"

run() {
    echo "==> $*"
    "$@"
}

cd "$ROOT_DIR"

run cargo build -p aidememo-cli
run cargo check -p aidememo-core -p aidememo-cli
run python3 -m pytest plugins/hermes/tests -q

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_STORE="/tmp/aidememo-agent-ux-d/wiki.sqlite" \
    AIDEMEMO_E2E_PROCESSES="$PROCESSES" \
    AIDEMEMO_E2E_N_PER_PROC="$N_PER_PROC" \
    python3 bench/multi-agent/scenario_d_concurrent_writers.py

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_STORE="/tmp/aidememo-agent-ux-e/wiki.sqlite" \
    AIDEMEMO_E2E_CLIENTS="$CLIENTS" \
    AIDEMEMO_E2E_N_PER_CLIENT="$N_PER_CLIENT" \
    python3 bench/multi-agent/scenario_e_http_shared.py

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_BASE="/tmp/aidememo-agent-ux-m" \
    python3 bench/multi-agent/scenario_m_mcp_install_source_defaults.py
