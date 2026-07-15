# Scripts

Scripts are grouped by purpose. Prefer keeping durable benchmark outputs in
`bench/**/results`, `benchmarks/results`, or `docs/MEASUREMENTS.md`; avoid
adding scratch note files.

## Daily Checks

| Script | Purpose |
|---|---|
| `ci-local.sh` | Local CI parity with one top-level timed summary: public portability, fmt, clippy, docs, first-run workflow demo, SDK promotion check, and tests. Use `ci-local.sh demo` for just the workflow onboarding smoke or `ci-local.sh sdk` for just the SDK wording/parity gate. |
| `demo-workflow.sh` | Zero-token first-run demo: seeds a temp store, starts a sparse ticket, and verifies decision + lesson + error context. |
| `openai_check.sh` | Quick OpenAI-compatible API smoke. |
| `bench-agent-ux.sh` | Small agent-memory UX regression: Rust check, Hermes tests, and zero-token multi-agent scenarios. |
| `bindings-release-smoke.sh` | Cross-binding release readiness with one top-level timed summary: Rust checks for Python/Node/Elixir/C bindings, npm version/pack/install smoke, agent SDK wheel smoke, Hermes plugin wheel smoke, and optional Python/Elixir/C package smokes. Python wheel smokes run `maturin` through pinned `uvx`. |
| `release-preflight.sh` | One-command release gate with one top-level timed summary. Local profile runs version, changelog release, registry readiness, public portability, workflow lint, docs feature gate, docs site e2e, storage backend preflight, binding smoke including agent SDK/Hermes wheel smoke, workflow smoke, and SDK promotion check; full profile adds Rust publish dry-run readiness, optional binding smokes, and Python-package/npm publish dry-runs. The same gate is available as `.github/workflows/release-preflight.yml` for runner-backed pre-publish checks. Set `AIDEMEMO_RELEASE_PREFLIGHT_CHANGELOG=0`, `AIDEMEMO_RELEASE_PREFLIGHT_STORAGE_BACKEND=0`, or `AIDEMEMO_RELEASE_PREFLIGHT_CARGO_PACKAGE=0` only for narrow release checks. Set `AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT_BIN` to pin or test the actionlint executable. |
| `package-cli-release.sh` | Smoke-check a built `aidememo` binary and package it with the README and dual-license files into the target-specific archive consumed by `.github/workflows/cli-release-assets.yml`. The workflow validates tag/version alignment before calling it. |
| `public-portability-check.py` | Offline public-source gate: scans first-party tracked text files and rejects developer-specific macOS, Linux, or Windows home paths. Runs in normal CI and release preflight. |
| `fresh-checkout-smoke.sh` | Public onboarding smoke: copies the current checkout to a temp directory without build artifacts, verifies `install.sh` syntax, builds `aidememo-cli`, then runs deterministic quickstart `fact add`, `search --bm25-only`, `query --bm25-only`, `workflow start --bm25-only`, and `stats` checks. Also runs through `.github/workflows/fresh-checkout-smoke.yml` manually and on relevant pull requests. |
| `changelog-release-check.py` | Offline changelog release gate: verifies `CHANGELOG.md` has exactly one empty `[Unreleased]` section, a dated current-version section immediately below it, and non-empty release-note content before publish. |
| `registry-readiness-check.py` | Offline registry setup gate: validates PyPI trusted-publisher workflow/package/environment mappings, npm root/platform package graph, release-doc package lists, OIDC permissions, and absence of long-lived publish-token requirements in first-party publish workflows. |
| `cargo-package-readiness.sh` | Rust publish-order gate with a timed summary. Runs the release version gate and `cargo publish --dry-run -p aidememo-core`; dependent Rust crates are skipped by default until `aidememo-core` is published to crates.io at the matching version. The CI `cargo-package-readiness` job runs this gate with dependent checks off. Set `AIDEMEMO_CARGO_PACKAGE_CHECK_DEPENDENTS=1` after that first publish to dry-run publish `aidememo-cli` and the native binding crates. |
| `public-registry-smoke.sh` | Post-release public install smoke. Default `plan` mode records the exact `cargo install`, `pip install`, and `npm install` checks without touching registries; set `AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE=verify` after the real publish, or run `.github/workflows/public-registry-smoke.yml` manually, to install `aidememo-cli`, `aidememo-agent-sdk`, `aidememo-agent-sdk[binding]`, `hermes-aidememo`, and `aidememo-napi` from public registries in temp environments. |
| `docs-feature-gate.py` | Public documentation drift gate: verifies CLI/MCP feature inventory, dynamic count claims such as MCP tool and CLI command counts across both READMEs, `COMPARE.md`, and the docs corpus, bilingual README navigation, script-inventory paths, Docusaurus sidebar/homepage wiring, the Claude/Codex/Hermes/pi coding-agent setup matrix, architecture/workflow/measurement contract tokens, onboarding/community file contracts, Mermaid diagram coverage/static lint, both READMEs plus English/Korean architecture nodes for opt-in LFM and privacy sidecars, product wording, and the storage positioning that SQLite is default while redb is optional. `--self-test` runs the count-claim and Mermaid-content rejection fixtures without building the CLI. |
| `docs-i18n-status.py` | Korean docs translation drift gate. `check` validates translated/fallback coverage against the public sidebar, rejects stale source SHA-256 fingerprints, preserves inline code tokens for command/API reference translations, and self-tests the rejection paths; `update` refreshes fingerprints only after the matching Korean prose has been reviewed. |
| `docs-site-e2e.py` | Rendered Docusaurus e2e gate: builds both locales and verifies English/Korean sitemap/sidebar/homepage route parity, locale-specific H1s, `html lang` / `hreflang`, baseUrl-safe links/assets/anchors, and repo-local implementation paths named by the architecture docs. |
| `sdk-promotion-check.sh` | SDK wording/parity gate for `aidememo-python`, `aidememo-napi`, and `aidememo-agent-sdk`: checks local criteria, session/pinned surface parity, optional package smokes / Scenario K, and public-registry blockers. When Scenario K is enabled, it builds a temp `aidememo-python` wheel/venv and Node addon instead of requiring a preinstalled binding. Also runs as the CI `SDK promotion check` job and writes `$GITHUB_STEP_SUMMARY` when available. |
| `skillopt-lite-cycle.sh` | Periodic SkillOpt-lite runner: checks the current memory profile or queued candidates, writes accepted/rejected JSONL records under `target/skillopt-lite`, and applies a passing candidate only with `--apply` / `AIDEMEMO_SKILLOPT_APPLY=1`. |
| `skillopt-lite-check.sh` | Offline SkillOpt-style gate for agent memory skill/profile edits: validates required `aidememo` memory workflow tokens, runs `aidememo skill check`, whitespace check, `cargo check -p aidememo-cli`, the first-run demo, and the SDK promotion gate. Set `AIDEMEMO_SKILLOPT_CANDIDATE=/path/to/SKILL.md` for a candidate and `AIDEMEMO_SKILLOPT_RUN_SCENARIOS=1` for Scenario L/M/N. |
| `workflow-release-smoke.sh` | Release-oriented workflow memory smoke with an always-printed timing summary: builds `aidememo`, runs the first-run demo plus Scenario F + I, then asserts `aidememo doctor --json` workflow readiness on a fixture store. |
| `storage-backend-feature-gate.sh` | Storage feature boundary gate: checks default/SQLite-only core, CLI, and SDK builds plus S3-only core builds omit `redb`, while redb still builds only through the explicit `redb` feature. Also smoke-tests a redb-only CLI default config opens `./_meta/wiki.redb` rather than a SQLite store. |
| `storage-backend-parity.sh` | Storage compatibility gate: verifies redb/SQLite mutation, direct feedback, relation removal, sync compatibility including the `libsqlite` alias, redb export/import into SQLite and `libsqlite`, relation preservation, SQLite archive/include-archive, concurrent SQLite MCP writes, and backend-aware daemon registry matching. |
| `storage-backend-real-corpus-diff.sh` | Real-docs storage diff: ingests the repo docs corpus into redb, canonical SQLite, and `libsqlite`, then compares normalized export rows and representative BM25 search results. |
| `storage-backend-sqlite-full-surface.sh` | SQLite-only storage smoke: builds the CLI with `--no-default-features --features sqlite`, then verifies init, ingest, entity/fact writes, entity rename/delete, fact delete, search/query, graph traversal, sessions, workflow start, archive, export, and import against a single SQLite store. Defaults to the `libsqlite` backend alias; set `AIDEMEMO_SQLITE_FULL_SURFACE_BACKEND=sqlite` to exercise the canonical spelling. |
| `storage-backend-sqlite-advanced-surface.sh` | SQLite-only advanced smoke: builds the CLI with `--no-default-features --features sqlite`, then verifies CLI busy-timeout behaviour, feedback/adapt, heuristic extract preview/apply, pending approve/reject, and TTL consolidate against a single SQLite store without requiring model downloads. Defaults to the `libsqlite` backend alias; set `AIDEMEMO_SQLITE_ADVANCED_SURFACE_BACKEND=sqlite` for canonical spelling. |
| `storage-backend-sqlite-mcp-soak.sh` | SQLite-only MCP/daemon gate: builds the CLI with `--no-default-features --features sqlite`, defaults `store.backend` to the `libsqlite` alias, runs representative MCP reads/writes (`fact_add_many`, `search`, `query`, `context`, `aggregate`, `workflow_start`, archive/include-archive, graph reads, feedback, extract preview), verifies concurrent HTTP writes through one SQLite-backed `mcp-serve`, then starts/stops a SQLite-only daemon and checks daemon-discovered search. Set `AIDEMEMO_SQLITE_MCP_SOAK_BACKEND=sqlite` to exercise the canonical spelling. |
| `storage-backend-sdk-bindings-check.sh` | SDK/native binding backend gate: checks Python default/SQLite-only/redb feature builds, and runs backend open/write/header tests for Node, Elixir NIF, and C across omitted/empty backend args, the `libsqlite` alias, and redb feature builds. Python runtime packaging is covered by `aidememo-python-pack-smoke.sh`. |
| `aidememo-release-version.sh` | Verify or update the release version across Cargo, Python, npm, Elixir/NIF, agent SDK, and Hermes packages. |
| `aidememo-python-version.sh` | Verify or update `aidememo-python` package version pins across the Rust workspace version and Python `pyproject.toml`. |
| `uv.sh` / `uvx.sh` | Repo-pinned `uv` runners: execute `uvx --from "$AIDEMEMO_UV_SPEC" uv|uvx`, defaulting to `uv==0.11.21`. |
| `maturin.sh` | Repo-pinned `maturin` runner: executes `scripts/uvx.sh --from "$AIDEMEMO_MATURIN_SPEC" maturin`, defaulting to `maturin==1.14.0`. |
| `aidememo-python-pack-smoke.sh` | Build a `aidememo-python` wheel with pinned `uvx`/`maturin`, install it into a temp venv, run the Python binding smoke, verify wheel metadata matches `aidememo_python.__version__`, and write a timed summary. `AIDEMEMO_PYTHON_SMOKE_BACKEND` accepts `sqlite`, `libsqlite`, or `redb`; set `AIDEMEMO_PYTHON_PACK_SMOKE_FEATURES=redb AIDEMEMO_PYTHON_PACK_SMOKE_NO_DEFAULT_FEATURES=1 AIDEMEMO_PYTHON_SMOKE_BACKEND=redb` to exercise a redb-only wheel. |
| `aidememo-agent-sdk-pack-smoke.sh` | Build a `aidememo-agent-sdk` wheel, install it into a temp venv, verify `Memory`, `AideMemoClient`, `AideMemoMemorySDK`, and first-use methods, and write a timed summary. |
| `hermes-aidememo-pack-smoke.sh` | Build `aidememo-agent-sdk` and `hermes-aidememo` wheels, install both into a temp venv, verify SDK re-export, the Hermes plugin entry point, `plugin.yaml`, and bundled `SKILL.md`, and write a timed summary. |
| `aidememo-python-publish-dry-run.sh` | Build `aidememo-python` wheel + sdist publish payloads with pinned `uvx`/`maturin`, validate their metadata/file contents without uploading to PyPI, and write a timed summary. |
| `aidememo-agent-sdk-publish-dry-run.sh` | Build `aidememo-agent-sdk` wheel + sdist publish payloads, validate their metadata/file contents without uploading to PyPI, and write a timed summary. |
| `hermes-aidememo-publish-dry-run.sh` | Build `hermes-aidememo` wheel + sdist publish payloads, validate metadata, dependency declarations, bundled `plugin.yaml`, and bundled skill files without uploading to PyPI, and write a timed summary. |
| `python-package-publish-dry-run.sh` / `python_package_publish_check.py` | Shared pure-Python publish dry-run engine and payload validator used by `aidememo-agent-sdk` and `hermes-aidememo`. |
| `.github/workflows/aidememo-python-publish.yml`, `.github/workflows/aidememo-agent-sdk-publish.yml`, `.github/workflows/hermes-aidememo-publish.yml` | Trusted-publisher release paths: build/validate distributions first, then publish through PyPA's OIDC action only when `dry_run=false`. |
| `aidememo-napi-version.sh` | Verify or update every `aidememo-napi` npm package version: root package, platform packages, and root `optionalDependencies`. |
| `aidememo-napi-pack-smoke.sh` | Build, test, pack root `aidememo-napi`, pack the current platform package, then install both tarballs, verify `require("aidememo-napi")` resolves through the platform optional dependency, and write a timed summary. |
| `aidememo-napi-publish.sh` | Shared npm publish engine for root/platform `aidememo-napi` packages with a timed summary. Defaults to dry-run; set `AIDEMEMO_NAPI_PUBLISH_MODE=publish` only from the trusted-publisher workflow. |
| `aidememo-napi-publish-dry-run.sh` | Build, test, and `npm publish --dry-run --access public` root + current platform packages, verifying root excludes `.node` and the platform payload includes exactly one `.node`; uses the shared publish summary. |
| `aidememo-nif-version.sh` | Verify or update `aidememo-nif` package version pins across the Rust workspace version and Elixir `mix.exs`. |

## Install And Hermes

| Script | Purpose |
|---|---|
| `install.sh` | One-line installer used by README. Downloads the matching macOS/Linux release archive, verifies `SHA256SUMS`, and installs the CLI without requiring Rust. |
| `prepare-continuity-demo.sh` | Recreate the deterministic Hermes to Codex to Claude Code project-memory handoff used by the cross-agent continuity recording guide. |
| `setup-hermes-test-env.sh` | Create an isolated Hermes profile for plugin smoke tests. |
| `test-hermes-e2e.sh` | Verify Hermes plugin registration, tools, hooks, and slash commands. |

## Bench And Analysis

These are research harnesses, not the first-stop user path.

| Family | Scripts |
|---|---|
| LongMemEval | `longmemeval_*.py`, `agent_eval_*.py`, `merge_retrievals.py`, `analyze_retrievals.py` |
| Multi-hop readers | `hotpotqa_reader.py`, `multihop_rag_reader.py`, `locomo_reader.py` |
| Query expansion | `expand_queries.py` |
| Consolidation analysis | `gac_analyze.py` |
| Overview eval | `overview_eval.py` |
| Privacy filtering | `privacy_filter_sidecar.py`, `privacy_filter_mlx_sidecar.py` |
| LFM / MLX experiments | `lfm_colbert_rerank.py`, `lfm_colbert_eval.py`, `lfm_dense_eval.py`, `lfm_mlx_dense_eval.py`, `lfm_mlx_docs_recall_eval.py`, `lfm_mlx_embedding_sidecar.py`, `lfm_mlx_colbert_eval.py`, `lfm_mlx_lm_eval.py`, `lfm_mlx_fact_type_eval.py`, `lfm_fact_type_sft_data.py`, `lfm_fact_type_sidecar.py`, `lfm_fact_type_threshold_eval.py`, `lfm_fact_type_log_fixture.py`, `lfm_fact_type_hf_probe.py`, `lfm_hf_agent_trace_retrieval_fixture.py` |

The Rust benchmark binaries live in `benchmarks/src/bin`. Scenario-style
multi-agent checks live in `bench/multi-agent`.
