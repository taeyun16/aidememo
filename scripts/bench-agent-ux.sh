#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AIDEMEMO_BIN="${AIDEMEMO_BIN:-$ROOT_DIR/target/debug/aidememo}"
CLIENTS="${AIDEMEMO_E2E_CLIENTS:-2}"
N_PER_CLIENT="${AIDEMEMO_E2E_N_PER_CLIENT:-10}"

run() {
    echo "==> $*"
    "$@"
}

cd "$ROOT_DIR"

run cargo build -p aidememo-cli
run cargo check -p aidememo-core -p aidememo-cli
run "$ROOT_DIR/scripts/uvx.sh" --from pytest pytest plugins/hermes/tests -q

# Scenario D exercises the optional redb backend and is run from the redb
# matrix. Keep this compact default-feature gate on SQLite-backed scenarios.

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

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_STORE="/tmp/aidememo-agent-ux-p/cross-agent-handoff.sqlite" \
    python3 bench/multi-agent/scenario_p_cross_agent_handoff.py

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_STORE="/tmp/aidememo-agent-ux-q/multi-account.sqlite" \
    python3 bench/multi-agent/scenario_q_multi_account_handoff.py

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_BASE="/tmp/aidememo-agent-ux-r" \
    python3 bench/multi-agent/scenario_r_hermes_kanban_boundary.py

run env \
    AIDEMEMO_BIN="$AIDEMEMO_BIN" \
    AIDEMEMO_E2E_BASE="/tmp/aidememo-agent-ux-s" \
    python3 bench/multi-agent/scenario_s_external_worker_lane.py
