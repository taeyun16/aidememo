# multi-agent e2e

Scenarios A–I exercise the integration between `wg` and the three
agents installed on this machine: Claude Code, Codex, and Hermes.

| script | what it does | model cost |
|---|---|---|
| `scenario_a_mcp_smoke.py` | Each client's MCP config can spawn `wg mcp`, handshake, list tools, call `wg_query`. | 0 |
| `scenario_b_consistency.py` | Three clients write/read against one store, agree on the data they see. | 0 |
| `scenario_c_natural_prompt.py` | Each agent receives the same natural-language prompt; verifies it actually called wg and quoted the seeded fact contents. | ~50–100k tokens (claude+codex+hermes) |
| `scenario_d_concurrent_writers.py` | Multi-process write contention; verifies lock failures are explicit and non-destructive. | 0 |
| `scenario_e_http_shared.py` | One `wg mcp-serve` + 4 HTTP clients × 25 inserts; the recommended shared-write pattern. | 0 |
| `scenario_f_workflow_triggers.py` | Starts multiple unrelated sparse tickets through CLI, MCP, and Hermes plugin paths; verifies unique sessions, ticket facts, topical priors, and `source_id` isolation. | 0 |
| `scenario_g_hermes_binding.py` | Compares Hermes workflow-start CLI fallback vs `wg-python` in-process path for shape parity, leakage, and latency. Requires local `wg-python` install. | 0 |
| `scenario_h_workflow_natural_prompt.py` | Sends sparse-ticket chat prompts to Claude, Codex, and Hermes; verifies workflow-start side effects, prior-memory reflection, and forbidden-source leakage with isolated MCP config per runtime. | token-burning |
| `scenario_i_workflow_doctor.py` | Starts workflow tickets through CLI, MCP, and Hermes, then validates `wg doctor --json` readiness, recent ticket count, summaries, and setup hints from an isolated HOME. | 0 |

## Running locally

```bash
cargo build -p wg-cli --release
ln -sf "$PWD/target/release/wg" ~/.local/bin/wg

# zero-cost scenarios
python3 bench/multi-agent/scenario_a_mcp_smoke.py
python3 bench/multi-agent/scenario_b_consistency.py
python3 bench/multi-agent/scenario_d_concurrent_writers.py
python3 bench/multi-agent/scenario_e_http_shared.py
python3 bench/multi-agent/scenario_f_workflow_triggers.py
python3 bench/multi-agent/scenario_i_workflow_doctor.py

# binding comparison (requires wg-python wheel installed)
(cd crates/wg-python && maturin build --release -o /tmp/wg-python-wheel)
python3 -m pip install --force-reinstall /tmp/wg-python-wheel/*.whl
python3 bench/multi-agent/scenario_g_hermes_binding.py

# compact local UX regression
scripts/bench-agent-ux.sh

# token-burning scenario (one-shot demo / opt-in regression)
python3 bench/multi-agent/scenario_c_natural_prompt.py
python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py

# debug one agent in Scenario H
WG_E2E_AGENTS=hermes python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py
```

Override the binary / store paths via env vars (defaults match this
machine):

```bash
WG_BIN=/path/to/wg \
CLAUDE_BIN=/path/to/claude \
CODEX_BIN=/path/to/codex \
HERMES_BIN=/path/to/hermes \
WG_E2E_STORE=/tmp/wg-e2e/wiki.redb \
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
