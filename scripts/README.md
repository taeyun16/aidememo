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
| `bindings-release-smoke.sh` | Cross-binding release readiness: Rust checks for Python/Node/Elixir/C bindings, npm version/pack/install smoke, and optional Python/Elixir/C package smokes. |
| `workflow-release-smoke.sh` | Release-oriented workflow memory smoke: builds `wg`, runs Scenario F + I, then asserts `wg doctor --json` workflow readiness on a fixture store. |
| `wg-napi-version.sh` | Verify or update every `wg-napi` npm package version: root package, platform packages, and root `optionalDependencies`. |
| `wg-napi-pack-smoke.sh` | Build, test, pack root `wg-napi`, pack the current platform package, then install both tarballs and verify `require("wg-napi")` resolves through the platform optional dependency. |
| `wg-napi-publish.sh` | Shared npm publish engine for root/platform `wg-napi` packages. Defaults to dry-run; set `WG_NAPI_PUBLISH_MODE=publish` only from the trusted-publisher workflow. |
| `wg-napi-publish-dry-run.sh` | Build, test, and `npm publish --dry-run --access public` root + current platform packages, verifying root excludes `.node` and the platform payload includes exactly one `.node`. |

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
