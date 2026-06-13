# multi-agent e2e

Scenarios A-N exercise the integration between `aidememo` and the three
agents installed on this machine: Claude Code, Codex, and Hermes.

| script | what it does | model cost |
|---|---|---|
| `scenario_a_mcp_smoke.py` | Each client's MCP config can spawn `aidememo mcp`, handshake, list tools, call `aidememo_query`. | 0 |
| `scenario_b_consistency.py` | Three clients write/read against one store, agree on the data they see. | 0 |
| `scenario_c_natural_prompt.py` | Each agent receives the same natural-language prompt; verifies it actually called aidememo and quoted the seeded fact contents. | ~50–100k tokens (claude+codex+hermes) |
| `scenario_d_concurrent_writers.py` | Multi-process write contention; verifies lock failures are explicit and non-destructive. | 0 |
| `scenario_e_http_shared.py` | One `aidememo mcp-serve` + 4 HTTP clients × 25 inserts; the recommended shared-write pattern. | 0 |
| `scenario_f_workflow_triggers.py` | Starts multiple unrelated sparse tickets through CLI, MCP, and Hermes plugin paths; verifies unique sessions, ticket facts, topical priors, and `source_id` isolation. | 0 |
| `scenario_g_hermes_binding.py` | Compares Hermes workflow-start CLI fallback vs `aidememo-python` in-process path for shape parity, leakage, and latency. Requires local `aidememo-python` install. | 0 |
| `scenario_h_workflow_natural_prompt.py` | Sends sparse-ticket chat prompts to Claude, Codex, and Hermes; verifies workflow-start side effects, prior-memory reflection, and forbidden-source leakage with source defaults configured per runtime. | token-burning |
| `scenario_i_workflow_doctor.py` | Starts workflow tickets through CLI, MCP, and Hermes, then validates `aidememo doctor --json` readiness, recent ticket count, summaries, and setup hints from an isolated HOME. | 0 |
| `scenario_j_lock_retry_sweep.py` | Sweeps 1/2/4/8 serverless CLI writers against the optional redb backend with `store.lock_retry_ms=0` vs `5000` to find when retry remains smooth and when users should switch to a shared daemon. Requires an `aidememo` binary built with `--features redb`. | 0 |
| `scenario_k_sdk_workflow_parity.py` | Compares `workflow_start` shape parity across CLI, `aidememo-python`, and `aidememo-napi`; verifies scoped priors, unique sessions/tickets, and forbidden-source leakage. | 0 |
| `scenario_l_self_extraction.py` | Simulates an agent-classified `aidememo_fact_add_many` batch, then verifies typed facts drive sparse-ticket workflow context and `source_id` isolation. | 0 |
| `scenario_m_mcp_install_source_defaults.py` | Installs MCP configs into an isolated HOME, verifies `AIDEMEMO_SOURCE_ID` is written for file targets / printed for shell targets, then proves the installed env scopes MCP write/search calls. | 0 |
| `scenario_n_hermes_memory_as_code.py` | Exercises the shared `aidememo_agent.Memory` research profile for Hermes/Codex/Claude-style code execution: fanout row collection, dedupe, coverage, `remember` batch write, aggregate, and source isolation. | 0 |

## Running locally

```bash
cargo build -p aidememo-cli --release
ln -sf "$PWD/target/release/aidememo" ~/.local/bin/aidememo

# zero-cost default SQLite scenarios
python3 bench/multi-agent/scenario_a_mcp_smoke.py
python3 bench/multi-agent/scenario_b_consistency.py
python3 bench/multi-agent/scenario_e_http_shared.py
python3 bench/multi-agent/scenario_f_workflow_triggers.py
python3 bench/multi-agent/scenario_i_workflow_doctor.py
python3 bench/multi-agent/scenario_l_self_extraction.py
python3 bench/multi-agent/scenario_m_mcp_install_source_defaults.py
python3 bench/multi-agent/scenario_n_hermes_memory_as_code.py

# optional-redb lock scenarios
cargo build -p aidememo-cli --release --features redb
ln -sf "$PWD/target/release/aidememo" ~/.local/bin/aidememo
python3 bench/multi-agent/scenario_d_concurrent_writers.py
python3 bench/multi-agent/scenario_j_lock_retry_sweep.py

# binding / SDK comparisons (require local native packages)
(cd crates/aidememo-python && maturin build --release -o /tmp/aidememo-python-wheel)
python3 -m pip install --force-reinstall /tmp/aidememo-python-wheel/*.whl
(cd crates/aidememo-napi && npm install && npm run build)
python3 bench/multi-agent/scenario_g_hermes_binding.py
python3 bench/multi-agent/scenario_k_sdk_workflow_parity.py

# compact local UX regression
scripts/bench-agent-ux.sh

# token-burning scenario (one-shot demo / opt-in regression)
python3 bench/multi-agent/scenario_c_natural_prompt.py
python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py

# Scenario H setup-only source-default check (no model calls)
AIDEMEMO_E2E_SETUP_ONLY=1 python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py

# debug one agent in Scenario H
AIDEMEMO_E2E_AGENTS=hermes python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py
```

Override the binary / store paths via env vars (defaults match this
machine):

```bash
AIDEMEMO_BIN=/path/to/aidememo \
CLAUDE_BIN=/path/to/claude \
CODEX_BIN=/path/to/codex \
HERMES_BIN=/path/to/hermes \
AIDEMEMO_E2E_STORE=/tmp/aidememo-e2e/wiki.sqlite \
python3 bench/multi-agent/scenario_c_natural_prompt.py
```

## CI integration

| scenario | workflow | trigger | runner |
|---|---|---|---|
| D | `.github/workflows/ci.yml` (`scenario-d` job) | every push | ubuntu-latest |
| C | `.github/workflows/e2e-natural-prompt.yml` | `workflow_dispatch` or PR label `run-e2e-c` | `[self-hosted, macos]` |

A, B, E are not in CI — they need either MCP clients (A), a real
multi-agent setup (B), or an HTTP server bind (E). Run them locally
when changing the MCP surface.

### Setting up the self-hosted runner for scenario C

The C workflow needs claude / codex / hermes installed and
authenticated. Spin up a self-hosted runner on this machine:

1. GitHub repo → Settings → Actions → Runners → "New self-hosted runner"
2. Pick **macOS** + **arm64**, follow the displayed `./config.sh` /
   `./run.sh` steps.
3. When prompted for labels, add **`macos`** so the workflow's
   `runs-on: [self-hosted, macos]` selector matches.
4. Confirm the three agents are still on PATH for the runner user
   (`claude --version`, `codex --version`, `hermes --version`).
5. To trigger:
   - PR: add the `run-e2e-c` label.
   - Manual: GitHub UI → Actions → "e2e — natural-language prompt"
     → Run workflow.

Result lands as a workflow artifact (`scenario-c-results/scenario_c.json`)
and, on PR runs, as a markdown summary comment.
