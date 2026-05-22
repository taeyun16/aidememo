# Scripts

Scripts are grouped by purpose. Prefer keeping durable benchmark outputs in
`bench/**/results`, `benchmarks/results`, or `docs/MEASUREMENTS.md`; avoid
adding scratch note files.

## Daily Checks

| Script | Purpose |
|---|---|
| `ci-local.sh` | Local CI parity: fmt, clippy, docs, SDK promotion check, and tests. Use `ci-local.sh sdk` for just the SDK wording gate. |
| `demo-workflow.sh` | Zero-token first-run demo: seeds a temp store, starts a sparse ticket, and verifies decision + lesson + error context. |
| `openai_check.sh` | Quick OpenAI-compatible API smoke. |
| `bench-agent-ux.sh` | Small agent-memory UX regression: Rust check, Hermes tests, and zero-token multi-agent scenarios. |
| `bindings-release-smoke.sh` | Cross-binding release readiness: Rust checks for Python/Node/Elixir/C bindings, npm version/pack/install smoke, and optional Python/Elixir/C package smokes. |
| `release-preflight.sh` | One-command release gate. Local profile runs version, workflow lint, binding smoke, workflow smoke, and SDK promotion check; full profile adds optional binding smokes plus Python/npm publish dry-runs. |
| `sdk-promotion-check.sh` | SDK wording gate for `wg-python` and `wg-napi`: checks local criteria, optional package smokes / Scenario K, and public-registry blockers. Also runs as the CI `SDK promotion check` job and writes `$GITHUB_STEP_SUMMARY` when available. |
| `workflow-release-smoke.sh` | Release-oriented workflow memory smoke: builds `wg`, runs Scenario F + I, then asserts `wg doctor --json` workflow readiness on a fixture store. |
| `wg-release-version.sh` | Verify or update the release version across Cargo, Python, npm, and Elixir/NIF packages. |
| `wg-python-version.sh` | Verify or update `wg-python` package version pins across the Rust workspace version and Python `pyproject.toml`. |
| `wg-python-pack-smoke.sh` | Build a `wg-python` wheel, install it into a temp venv, run the Python binding smoke, and verify wheel metadata matches `wg_python.__version__`. |
| `wg-python-publish-dry-run.sh` | Build `wg-python` wheel + sdist publish payloads and validate their metadata/file contents without uploading to PyPI. |
| `.github/workflows/wg-python-publish.yml` | Trusted-publisher release path: build/validate distributions first, then publish through PyPA's OIDC action only when `dry_run=false`. |
| `wg-napi-version.sh` | Verify or update every `wg-napi` npm package version: root package, platform packages, and root `optionalDependencies`. |
| `wg-napi-pack-smoke.sh` | Build, test, pack root `wg-napi`, pack the current platform package, then install both tarballs and verify `require("wg-napi")` resolves through the platform optional dependency. |
| `wg-napi-publish.sh` | Shared npm publish engine for root/platform `wg-napi` packages. Defaults to dry-run; set `WG_NAPI_PUBLISH_MODE=publish` only from the trusted-publisher workflow. |
| `wg-napi-publish-dry-run.sh` | Build, test, and `npm publish --dry-run --access public` root + current platform packages, verifying root excludes `.node` and the platform payload includes exactly one `.node`. |
| `wg-nif-version.sh` | Verify or update `wg-nif` package version pins across the Rust workspace version and Elixir `mix.exs`. |

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
