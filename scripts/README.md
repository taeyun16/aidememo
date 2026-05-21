# Scripts

Scripts are grouped by purpose. Prefer keeping durable benchmark outputs in
`bench/**/results`, `benchmarks/results`, or `docs/MEASUREMENTS.md`; avoid
adding scratch note files.

## Daily Checks

| Script | Purpose |
|---|---|
| `ci-local.sh` | Local CI parity: fmt, clippy, docs, and tests. |
| `openai_check.sh` | Quick OpenAI-compatible API smoke. |
| `bench-agent-ux.sh` | Small agent-memory UX regression: Rust check, Hermes tests, and zero-token multi-agent scenarios. |
| `workflow-release-smoke.sh` | Release-oriented workflow memory smoke: builds `wg`, runs Scenario F + I, then asserts `wg doctor --json` workflow readiness on a fixture store. |
| `wg-napi-pack-smoke.sh` | Build, test, and `npm pack` the local `wg-napi` package, then verify the tarball contains JS, types, and the platform `.node` binary. |

## Install And Hermes

| Script | Purpose |
|---|---|
| `install.sh` | One-line installer used by README. |
| `setup-hermes-test-env.sh` | Create an isolated Hermes profile for plugin smoke tests. |
| `test-hermes-e2e.sh` | Verify Hermes plugin registration, tools, hooks, and slash commands. |

## Bench And Analysis

These are research harnesses, not the first-stop user path.

| Family | Scripts |
|---|---|
| LongMemEval | `longmemeval_*.py`, `agent_eval_*.py`, `merge_retrievals.py`, `analyze_retrievals.py` |
| Multi-hop readers | `hotpotqa_reader.py`, `multihop_rag_reader.py`, `locomo_reader.py` |
| Query expansion / decomposition | `expand_queries.py`, `longmemeval_decompose_queries.py`, `longmemeval_hyde_questions.py` |
| Consolidation analysis | `gac_analyze.py` |
| Overview eval | `overview_eval.py` |

The Rust benchmark binaries live in `benchmarks/src/bin`. Scenario-style
multi-agent checks live in `bench/multi-agent`.
