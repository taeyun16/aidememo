# Scripts

Scripts are grouped by purpose. Prefer keeping durable benchmark outputs in
`bench/**/results`, `benchmarks/results`, or `docs/MEASUREMENTS.md`; avoid
adding scratch note files.

## Daily Checks

| Script | Purpose |
|---|---|
| `ci-local.sh` | Local CI parity with one top-level timed summary: fmt, clippy, docs, first-run workflow demo, SDK promotion check, and tests. Use `ci-local.sh demo` for just the workflow onboarding smoke or `ci-local.sh sdk` for just the SDK wording/parity gate. |
| `demo-workflow.sh` | Zero-token first-run demo: seeds a temp store, starts a sparse ticket, and verifies decision + lesson + error context. |
| `openai_check.sh` | Quick OpenAI-compatible API smoke. |
| `bench-agent-ux.sh` | Small agent-memory UX regression: Rust check, Hermes tests, and zero-token multi-agent scenarios. |
| `bindings-release-smoke.sh` | Cross-binding release readiness with one top-level timed summary: Rust checks for Python/Node/Elixir/C bindings, npm version/pack/install smoke, and optional Python/Elixir/C package smokes. Python wheel smokes run `maturin` through pinned `uvx`. |
| `release-preflight.sh` | One-command release gate with one top-level timed summary. Local profile runs version, workflow lint, docs feature gate, docs build, storage backend preflight, binding smoke, workflow smoke, and SDK promotion check; full profile adds optional binding smokes plus Python/npm publish dry-runs. Set `AIDEMEMO_RELEASE_PREFLIGHT_STORAGE_BACKEND=0` only for narrow non-storage release checks. Set `AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT_BIN` to pin or test the actionlint executable. |
| `docs-feature-gate.py` | Public documentation drift gate: verifies CLI/MCP feature inventory, Docusaurus sidebar wiring, product wording, and the storage positioning that SQLite is default while redb is optional. |
| `sdk-promotion-check.sh` | SDK wording/parity gate for `aidememo-python`, `aidememo-napi`, and `aidememo-agent-sdk`: checks local criteria, session/pinned surface parity, optional package smokes / Scenario K, and public-registry blockers. Also runs as the CI `SDK promotion check` job and writes `$GITHUB_STEP_SUMMARY` when available. |
| `skillopt-lite-cycle.sh` | Periodic SkillOpt-lite runner: checks the current memory profile or queued candidates, writes accepted/rejected JSONL records under `target/skillopt-lite`, and applies a passing candidate only with `--apply` / `AIDEMEMO_SKILLOPT_APPLY=1`. |
| `skillopt-lite-check.sh` | Offline SkillOpt-style gate for agent memory skill/profile edits: validates required `aidememo` memory workflow tokens, runs `aidememo skill check`, whitespace check, `cargo check -p aidememo-cli`, the first-run demo, and the SDK promotion gate. Set `AIDEMEMO_SKILLOPT_CANDIDATE=/path/to/SKILL.md` for a candidate and `AIDEMEMO_SKILLOPT_RUN_SCENARIOS=1` for Scenario L/M/N. |
| `workflow-release-smoke.sh` | Release-oriented workflow memory smoke with an always-printed timing summary: builds `aidememo`, runs the first-run demo plus Scenario F + I, then asserts `aidememo doctor --json` workflow readiness on a fixture store. |
| `storage-backend-feature-gate.sh` | Storage feature boundary gate: checks default/SQLite-only core, CLI, and SDK builds plus S3-only core builds omit `redb`, while redb still builds only through the explicit `redb` feature. Also smoke-tests a redb-only CLI default config opens `./_meta/wiki.redb` rather than a SQLite store. |
| `storage-backend-parity.sh` | Storage compatibility gate: verifies redb/SQLite mutation, direct feedback, relation removal, sync compatibility including the `libsqlite` alias, redb export/import into SQLite and `libsqlite`, relation preservation, SQLite archive/include-archive, and concurrent SQLite MCP writes. |
| `storage-backend-sqlite-full-surface.sh` | SQLite-only storage smoke: builds the CLI with `--no-default-features --features sqlite`, then verifies init, ingest, entity/fact writes, entity rename/delete, fact delete, search/query, graph traversal, sessions, workflow start, archive, export, and import against a single SQLite store. Defaults to the `libsqlite` backend alias; set `AIDEMEMO_SQLITE_FULL_SURFACE_BACKEND=sqlite` to exercise the canonical spelling. |
| `storage-backend-sqlite-advanced-surface.sh` | SQLite-only advanced smoke: builds the CLI with `--no-default-features --features sqlite`, then verifies CLI busy-timeout behaviour, feedback/adapt, heuristic extract preview/apply, pending approve/reject, and TTL consolidate against a single SQLite store without requiring model downloads. Defaults to the `libsqlite` backend alias; set `AIDEMEMO_SQLITE_ADVANCED_SURFACE_BACKEND=sqlite` for canonical spelling. |
| `storage-backend-sqlite-mcp-soak.sh` | SQLite-only MCP gate: builds the CLI with `--no-default-features --features sqlite`, defaults `store.backend` to the `libsqlite` alias, runs representative MCP reads/writes (`fact_add_many`, `search`, `query`, `context`, `aggregate`, `workflow_start`, archive/include-archive, graph reads, feedback, extract preview), then verifies concurrent HTTP writes through one SQLite-backed `mcp-serve`. Set `AIDEMEMO_SQLITE_MCP_SOAK_BACKEND=sqlite` to exercise the canonical spelling. |
| `storage-backend-sdk-bindings-check.sh` | SDK/native binding backend gate: verifies default SQLite builds, explicit SQLite-only builds, omitted/empty backend args inherit the compiled default, `libsqlite` alias opens, redb feature builds, and package-level NIF smoke coverage across Python, Node, Elixir, and C bindings. |
| `aidememo-release-version.sh` | Verify or update the release version across Cargo, Python, npm, and Elixir/NIF packages. |
| `aidememo-python-version.sh` | Verify or update `aidememo-python` package version pins across the Rust workspace version and Python `pyproject.toml`. |
| `uv.sh` / `uvx.sh` | Repo-pinned `uv` runners: execute `uvx --from "$AIDEMEMO_UV_SPEC" uv|uvx`, defaulting to `uv==0.11.21`. |
| `maturin.sh` | Repo-pinned `maturin` runner: executes `scripts/uvx.sh --from "$AIDEMEMO_MATURIN_SPEC" maturin`, defaulting to `maturin==1.14.0`. |
| `aidememo-python-pack-smoke.sh` | Build a `aidememo-python` wheel with pinned `uvx`/`maturin`, install it into a temp venv, run the Python binding smoke, verify wheel metadata matches `aidememo_python.__version__`, and write a timed summary. `AIDEMEMO_PYTHON_SMOKE_BACKEND` accepts `sqlite`, `libsqlite`, or `redb`; set `AIDEMEMO_PYTHON_PACK_SMOKE_FEATURES=redb AIDEMEMO_PYTHON_PACK_SMOKE_NO_DEFAULT_FEATURES=1 AIDEMEMO_PYTHON_SMOKE_BACKEND=redb` to exercise a redb-only wheel. |
| `aidememo-agent-sdk-pack-smoke.sh` | Build a `aidememo-agent-sdk` wheel, install it into a temp venv, verify `Memory`, `AideMemoClient`, `AideMemoMemorySDK`, and first-use methods, and write a timed summary. |
| `hermes-aidememo-pack-smoke.sh` | Build `aidememo-agent-sdk` and `hermes-aidememo` wheels, install both into a temp venv, verify SDK re-export, the Hermes plugin entry point, `plugin.yaml`, and bundled `SKILL.md`, and write a timed summary. |
| `aidememo-python-publish-dry-run.sh` | Build `aidememo-python` wheel + sdist publish payloads with pinned `uvx`/`maturin`, validate their metadata/file contents without uploading to PyPI, and write a timed summary. |
| `.github/workflows/aidememo-python-publish.yml` | Trusted-publisher release path: build/validate distributions first, then publish through PyPA's OIDC action only when `dry_run=false`. |
| `aidememo-napi-version.sh` | Verify or update every `aidememo-napi` npm package version: root package, platform packages, and root `optionalDependencies`. |
| `aidememo-napi-pack-smoke.sh` | Build, test, pack root `aidememo-napi`, pack the current platform package, then install both tarballs, verify `require("aidememo-napi")` resolves through the platform optional dependency, and write a timed summary. |
| `aidememo-napi-publish.sh` | Shared npm publish engine for root/platform `aidememo-napi` packages with a timed summary. Defaults to dry-run; set `AIDEMEMO_NAPI_PUBLISH_MODE=publish` only from the trusted-publisher workflow. |
| `aidememo-napi-publish-dry-run.sh` | Build, test, and `npm publish --dry-run --access public` root + current platform packages, verifying root excludes `.node` and the platform payload includes exactly one `.node`; uses the shared publish summary. |
| `aidememo-nif-version.sh` | Verify or update `aidememo-nif` package version pins across the Rust workspace version and Elixir `mix.exs`. |

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
