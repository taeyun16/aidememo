# wg Product Roadmap

This roadmap tracks product gaps against agent-memory peers (GBrain,
Graphiti/Zep, Mem0, Hindsight, Letta) and ties each item to a measurable
acceptance metric. Keep durable measurement write-ups in `docs/MEASUREMENTS.md`
or benchmark-specific `RESULTS.md` files; keep user-facing product work here.

## Measurement Rules

- Every shipped item needs a command that can be re-run locally.
- Prefer counts, latency, recall, pass rates, or bytes over prose claims.
- Record before/after numbers in the PR or changelog entry.
- Do not count a feature as complete if only the internal API exists; the CLI,
  MCP, docs, and tests must match the intended user path.

## Milestones

| ID | Status | Product Gap | Target Metric | Measurement Command |
|---|---|---|---|---|
| P0.1 | done | Capture inbox is TUI-only, hard to automate | `wg pending list/approve/reject` work non-interactively; JSON includes `count`, `selected`, `committed`, `discarded`, `failed`, `remaining` | `cargo test -p wg-cli pending::` plus a CLI smoke with a temp pending log |
| P0.2 | done | Cross-system validation is not packaged | A `gbrain-evals` adapter exists, matches the current Adapter interface, and has fresh-checkout scorecards for direct and daemon modes | direct bm25: P@5 17.4%, R@5 64.1%, 125/261, real 63.38s; daemon bm25: same score, real 11.04s (5.7x faster); daemon hybrid: R@5 62.5%, real 45.64s |
| P0.2a | done | `gbrain-evals` adapter still pays per-query CLI spawn even after daemon work | Adapter supports `WG_ADAPTER_BACKEND=auto|cli|napi`; `napi` uses `wg-napi` in process while preserving CLI/daemon baselines | Temp Bun harness, 30 BM25 queries: CLI p50 124.55 ms / p95 132.08 ms; NAPI p50 0.02 ms / p95 0.03 ms; both returned top=`redis` |
| P0.2b | done | Native adapter speedup was only fixture-level, not public-runner validated | Fresh `gbrain-evals` BrainBench scorecard runs with `WG_ADAPTER_BACKEND=napi` and daemon baseline on the same checkout | `gbrain-evals@89445dd`, `BRAINBENCH_N=1`: NAPI bm25 P@5 17.4%, R@5 64.1%, 125/261, real 6.48s; daemon bm25 same score, real 10.77s; NAPI 1.66x faster |
| P0.2c | done | External users still need a local `wg-napi` build for the native adapter | `wg-napi` artifact workflow builds/test/packs platform packages and uploads tarballs; local pack smoke verifies package contents | `scripts/wg-napi-pack-smoke.sh`: `npm run build`, `npm test`, root `npm pack`, current-platform `npm pack`, and install smoke passed in 2.81s; `.github/workflows/wg-napi-artifacts.yml` covers Ubuntu/macOS/Windows |
| P0.2d | done | `wg-napi` had artifacts but no publish-readiness gate | Local script and CI workflow run `npm publish --dry-run --access public`, verify publish payload files, and keep real publish separate until npm ownership/trusted-publisher policy is set | `scripts/wg-napi-publish-dry-run.sh`: root dry-run payload `wg-napi@0.1.0`, 4 files, 4.18 KB packed, no `.node`; platform payload `wg-napi-darwin-arm64@0.1.0`, 2 files, 2.79 MB packed |
| P0.2e | done | Cross-platform npm release would break if the root package shipped only one platform `.node` | Root `wg-napi` now declares platform optionalDependencies and platform package scaffolds exist for macOS arm64/x64, Linux arm64/x64 glibc, and Windows x64 MSVC | `scripts/wg-napi-pack-smoke.sh`: root package excludes `.node`, `wg-napi-darwin-arm64` includes `wg-napi.darwin-arm64.node`, temp install of both tarballs resolves `require("wg-napi").version() = 0.1.0` |
| P0.2f | done | Real npm release still required hand-written commands | Trusted-publisher workflow skeleton publishes platform packages first and root wrapper second; default is dry-run, real publish requires npm trusted publisher setup plus explicit `dry_run=false` and version match | `.github/workflows/wg-napi-publish.yml`; local `WG_NAPI_EXPECT_VERSION=0.1.0 scripts/wg-napi-publish-dry-run.sh`, `WG_NAPI_PUBLISH_SCOPE=platform ...`, and `WG_NAPI_PUBLISH_SCOPE=root ...` all passed |
| P0.2g | done | npm release can drift if root/platform versions or optionalDependency pins are edited by hand | `scripts/wg-napi-version.sh [VERSION]` verifies or updates root + platform package versions and root optionalDependency pins; pack/publish scripts run this gate automatically | `scripts/wg-napi-version.sh`: 0.1.0 pinned across 5 platform packages; temp-copy bump to 0.1.1 updated root, platform packages, and optionalDependencies; root publish dry-run with `WG_NAPI_EXPECT_VERSION=0.1.0` passed |
| P0.2h | done | Binding release readiness was language-specific and hard to compare | One cross-binding smoke checks Rust binding crates, runs npm's full version/pack/install smoke, and can run Python/Elixir/C package smokes. The first run caught and fixed `source_id` type drift in `wg-ffi` / `wg-nif` before release work moved on. | `scripts/bindings-release-smoke.sh`: default warm path passed cargo check + npm version/pack/install smoke in 3.37s; `WG_BINDINGS_SMOKE_NPM=0 WG_BINDINGS_SMOKE_OPTIONAL=1` built the Python wheel, ran `mix compile.cargo --force && mix test`, and passed the C FFI smoke |
| P0.2i | done | Python wheel releases could drift between `pyproject.toml`, Cargo metadata, and runtime `__version__` | `scripts/wg-python-version.sh [VERSION]` gates the Rust workspace + Python package version, and `scripts/wg-python-pack-smoke.sh` builds a wheel, installs it into a temp venv, runs the binding smoke, and checks installed metadata against `wg_python.__version__` | `scripts/wg-python-version.sh`: 0.1.0 pinned; `scripts/wg-python-pack-smoke.sh`: wheel build + temp install + `tests/smoke.py` + metadata/runtime version check passed in 48.38s |
| P0.2j | done | Python publish readiness had no CI-safe dry-run because `maturin publish` uploads directly | `scripts/wg-python-publish-dry-run.sh` builds wheel + sdist payloads, validates metadata/file contents without uploading, and CI runs the same check on `wg-python-v*` tags/manual dispatch. The dry-run caught that the sdist could not rebuild with the workspace-only `tokenizers` patch, so the vendored patch is now included in the Python sdist. | `scripts/wg-python-publish-dry-run.sh`: built `wg_python-0.1.0-cp313-cp313-macosx_11_0_arm64.whl` + `wg_python-0.1.0.tar.gz` and validated payload metadata/files in 60.94s; `.github/workflows/wg-python-publish-dry-run.yml` |
| P0.2k | done | Real Python release still required hand-written trusted-publisher wiring | `.github/workflows/wg-python-publish.yml` builds/validates distributions in a non-publishing job, uploads the checked artifacts, and only the `dry_run=false` publish job gets PyPI OIDC permissions via `pypa/gh-action-pypi-publish@release/v1` | `WG_PYTHON_DIST_DIR=/tmp/wg-python-dist scripts/wg-python-publish-dry-run.sh`: checked reusable artifact output; workflow defaults to `dry_run=true` and requires exact version input; latest local wheel 2.8M + sdist 349K |
| P0.2l | done | Multi-language release versions could still drift when bumping packages by hand | `scripts/wg-release-version.sh [VERSION]` composes the Python, npm, and NIF version gates so Cargo/Python/npm/Elixir stay pinned; FFI follows Cargo package metadata | `scripts/wg-release-version.sh`: 0.1.0 pinned; temp-copy bump to 0.1.1 updated Cargo workspace, Python `pyproject.toml`, npm root/platform packages, npm optionalDependencies, and `wg-nif` `mix.exs` |
| P0.3 | done | Capture quality is not measured | Pending approval rate and extraction precision can be computed from one JSONL log | `wg pending stats --from LOG --json` returns total/count-by-type/confidence histogram |
| P1.1 | done | First-run setup requires several commands | One command prints or applies init + MCP install + skill install for a target agent | `wg init --agent codex --no-ingest PATH --json` reports steps and elapsed ms |
| P1.2 | done | Shared daemon is operational but opaque | HTTP MCP exposes health/sync/admin status without exposing secrets | `curl /health` and `curl /admin/status` return request count, store path, auth mode, sync cursors |
| P1.3 | done | Feedback loop exists but is manual | `wg feedback` count and `wg adapt train` status are visible in doctor/overview | `wg doctor --json` includes `adaptation.feedback_count`, `has_adapter`, `generation`, `ready`; smoke: before train 1/false/0/false + `wg adapt train` fix, after train 1/true/1/true |
| P2.1 | done | Per-user/source scoping is project-level only | Facts carry optional `source_id`; `wg fact add/list`, `wg search`, `wg query`, MCP equivalents, and Hermes plugin tools filter by source | Unit: `cargo test -p wg-core source_id --features semantic` 2 passed; `cargo test -p wg-cli source_id` 1 passed. Hermes agent mixed-source eval: unscoped beta inclusion 2/2 → scoped beta leakage 0/2 while alpha recall stayed 2/2 |
| P2.2 | done (non-goal) | Distributed multi-writer merge | No hidden multi-master writes; docs steer users to canonical daemon + pull cache | `AGENTS.md` documents single shared `wg mcp-serve`, no multi-stdio writers, and pull-only delta sync |
| P2.3 | done | Two local Hermes agents sharing one store need a daemon or hit redb lock errors | Hermes plugin retries short CLI fallback lock collisions by default (`lock_retry_ms=5000`), so ordinary same-host sharing works without a user-visible server step | Serverless Hermes `WgClient` smoke, 2 processes x 10 writes: retry `0` persisted 10/20 with 10 lock errors; retry `5000` persisted 20/20 with 0 errors, wall 2.16s, p50 98.1ms, max 1.22s |
| P2.4 | done | Serverless retry had no measured smoothness ceiling | `store.lock_retry_ms=5000` is recommended for small same-host teams; shared daemon remains the guidance beyond the smooth serverless envelope | `python3 bench/multi-agent/scenario_j_lock_retry_sweep.py`: retry `5000` smooth until 4 concurrent writers (40/40 persisted, p95 1282.56 ms); at 8 writers 79/80 persisted, p95 2988.4 ms; retry `0` at 8 writers persisted 10/80 |
| P2.5 | done | Shared-store guidance was buried in benchmark docs | `wg doctor --json` emits a dedicated `sharing` block with retry setting, daemon state, writer thresholds, and actionable hints/fixes | `cargo test -p wg-cli doctor`: 25 passed; `doctor_json_includes_shared_store_guidance` validates `sharing.lock_retry_ms`, `serverless_recommended_writers=4`, daemon state, mode, and hint actions |
| P2.6 | done | Agent-facing `wg_doctor` lacked the shared-store guidance now visible in CLI doctor | MCP `wg_doctor` reuses the CLI sharing report, so agents see retry/daemon guidance without shelling out to `wg doctor --json` | `cargo test -p wg-cli doctor_groups_by_code_with_action_hints` validates `sharing.serverless_recommended_writers=4`, `recommended_mode=serverless_fail_fast`, and `sharing_retry_disabled` in MCP doctor output |
| P3.1 | done | Sparse issues/tickets require agents to manually chain session + search + write | `wg workflow start` and MCP `wg_workflow_start` create a tracked session, store the trigger as a question fact, and return a project context pack | `cargo test -p wg-cli workflow_start`; `python3 bench/multi-agent/scenario_f_workflow_triggers.py` validates 4 distinct tickets across CLI/MCP/Hermes with 10/10 invariants |
| P3.2 | done | Workflow trigger quality claims are pass/fail only | Scenario F reports latency, context size, prior type distribution, and forbidden context leakage for each ticket | `python3 bench/multi-agent/scenario_f_workflow_triggers.py` validates 4 tickets with 13/13 invariants; p95 workflow latency < 5s; max context < 12k chars; leakage total = 0 |
| P3.3 | done | Hermes workflow start still shells out even when `wg-python` is installed | `wg-python` exposes `source_id` and Hermes composes workflow packs in process when the binding is available, falling back to CLI otherwise | `python3 bench/multi-agent/scenario_g_hermes_binding.py`: 5/5 invariants; shape parity 4/4; leakage 0; p50 1795.71ms CLI vs 13.14ms binding (136.66x) |
| P3.4 | done | Natural-language workflow adoption differs by agent | Claude / Codex / Hermes sparse-ticket prompts naturally call `wg_workflow_start` or produce its store side effect | `python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py`: 4/4 invariants; 3/3 agents passed; each created 1 scoped workflow fact; prior reflection Claude 3/3, Codex 2/3, Hermes 3/3; forbidden leakage 0 |
| P3.5 | done | Workflow setup failures are hard to diagnose | `wg doctor --json` reports workflow readiness, recent workflow tickets, and agent integration hints | `cargo test -p wg-cli doctor_json_includes_workflow_readiness_hints`; unit smoke covers `workflow.ready`, `recent_ticket_count`, recent ticket summaries, and actionable `hints[]` |
| P3.6 | done | Doctor readiness exists but is not validated against real workflow traces | Scenario I creates workflow tickets through CLI, MCP, and Hermes, then validates `wg doctor --json` from an isolated agent config | `python3 bench/multi-agent/scenario_i_workflow_doctor.py`: 10/10 invariants; `workflow.ready=true`; `recent_ticket_count=3`; drivers CLI/MCP/Hermes; p95 workflow latency 1891.48 ms |
| P3.7 | done | Release workflow checks require remembering several commands | One script builds `wg`, runs the first-run demo + Scenario F + I, and asserts `wg doctor --json` workflow readiness on a fixture store | `scripts/workflow-release-smoke.sh`: demo recovered decision/lesson/error, Scenario F 13/13, Scenario I 10/10, fixture doctor `workflow_ready=true`, `recent_ticket_count=1` |
| P3.8 | done | Workflow release smoke is local-only | CI runs the zero-token workflow release smoke as a named check after lint | `.github/workflows/ci.yml` job `workflow-release-smoke`; local command remains `scripts/workflow-release-smoke.sh` |
| P3.9 | done | Workflow release smoke runtime is opaque in CI | Smoke script prints per-step timing and writes the same markdown table to `$GITHUB_STEP_SUMMARY` | `scripts/workflow-release-smoke.sh`: latest local total 13.40s; demo 0.65s, Scenario F 7.17s, Scenario I 5.10s, fixture workflow start 0.20s |
| P3.10 | done | Release workflows can drift syntactically without a cheap CI signal | CI now runs `actionlint` over every GitHub Actions workflow before heavier Rust jobs; local-only `.codex/` agent config is ignored so status stays clean | `actionlint .github/workflows/*.yml`: 0 issues; `.github/workflows/ci.yml` job `workflow-lint` installs `actionlint@v1.7.1` |
| P3.11 | done | Release readiness still required stitching several gates together by hand | `scripts/release-preflight.sh` gives local/full profiles with timed status rows; full profile adds optional binding smokes and Python/npm publish dry-runs | `scripts/release-preflight.sh`: local profile can run version, actionlint, binding smoke, workflow smoke, and SDK promotion; binding-only measured path passed in 3.79s; fast path with bindings/workflow/sdk/publish disabled passed version + actionlint in 0.29s |
| P3.12 | done | Python/Node bindings were too low-level to support SDK positioning for sparse-ticket agents | `WikiGraph::workflow_start` is now shared by CLI, MCP, Python, and Node; Python/Node package docs include workflow-level examples with `source_id` scoping | `cargo check -p wg-core -p wg-cli -p wg-python -p wg-napi`; `cargo test -p wg-cli workflow_start`; `scripts/wg-python-pack-smoke.sh`; `scripts/wg-napi-pack-smoke.sh` |
| P3.13 | done | SDK candidates still surfaced Rust errors as undifferentiated runtime failures | `WgError::code()` gives stable machine codes; Python maps core failures to typed exceptions; Node throws JS errors with N-API `code` plus `[wg_code]` message prefixes | `cargo check -p wg-core -p wg-python -p wg-napi`; `cargo test -p wg-core entity_not_found_display_includes_suggestions`; `scripts/wg-python-pack-smoke.sh`; `cd crates/wg-napi && npm test` |
| P3.14 | done | SDK workflow APIs had no cross-language parity measurement | Scenario K compares CLI, `wg-python`, and `wg-napi` workflow-start packs across four sparse tickets and checks session/ticket side effects, prior counts, `source_id`, and leakage | `python3 bench/multi-agent/scenario_k_sdk_workflow_parity.py`: 8/8 invariants; Python/Node shape parity 4/4 each; leakage 0; p50 CLI 1864.55ms, Python 16.19ms, Node 13.69ms |
| P3.15 | done | SDK promotion criteria existed only as prose, so "binding" vs "SDK" could drift | `scripts/sdk-promotion-check.sh` reports local readiness, optional package smokes / Scenario K, and explicit public-registry blockers before README wording changes | Default: ok=6, ready=3, blocked=2, fail=0, `local_ready=true`, `sdk_promotable=false`; with `WG_SDK_PROMOTION_RUN_SCENARIO_K=1`: ok=7, ready=2, blocked=2, fail=0 |
| P3.16 | done | Release preflight still did not enforce the SDK wording gate | `scripts/release-preflight.sh` runs `sdk promotion check` by default, can skip it explicitly, and can fail on public install blockers when `WG_RELEASE_PREFLIGHT_SDK_REQUIRE_PUBLIC=1` | Fast profile with bindings/workflow/actionlint/publish disabled: sdk promotion step ok in 0.13s, total 0.28s; require-public mode fails the step with exit 1 and records `fail | sdk promotion check` |
| P3.17 | done | SDK wording gate was local/preflight-only and could be skipped in PRs | CI now has a dedicated `SDK promotion check` job that runs the fast gate after lint with Python 3.13 and Node 22, separate from heavier package smokes | `actionlint .github/workflows/*.yml`: 0 issues; `scripts/sdk-promotion-check.sh`: ok=6, ready=3, blocked=2, fail=0 |
| P3.18 | done | Local CI parity did not include the new SDK wording check | `scripts/ci-local.sh sdk` runs the SDK promotion gate directly, and `scripts/ci-local.sh all` now includes it between lint and tests | `bash -n scripts/ci-local.sh`; `scripts/ci-local.sh sdk`: ok=6, ready=3, blocked=2, fail=0 |
| P3.19 | done | SDK promotion CI details required opening raw logs | `scripts/sdk-promotion-check.sh` writes a Markdown table to `$GITHUB_STEP_SUMMARY` while preserving text and JSON stdout modes | Summary smoke writes 11 check rows plus metric rows; `WG_SDK_PROMOTION_JSON=1` remains valid JSON; `bash -n scripts/sdk-promotion-check.sh`; `git diff --check` |
| P3.20 | done | First-run README did not immediately demonstrate sparse-ticket workflow memory | `scripts/demo-workflow.sh` seeds a temp store and verifies `wg workflow start --bm25-only` recovers decision, lesson, and error context from a sparse ticket | `scripts/demo-workflow.sh`: decision=1, lesson=1, error=1, search_hits=4, latency 128ms; `cargo check -p wg-cli` |
| P3.21 | done | First-run demo was not protected by release smoke | `scripts/workflow-release-smoke.sh` now runs `scripts/demo-workflow.sh` after build and uses `--bm25-only` for the fixture workflow start to avoid semantic cold-start noise | `scripts/workflow-release-smoke.sh`: demo step 0.65s; fixture workflow start 0.20s; total 13.40s; Scenario F 13/13; Scenario I 10/10 |
| P3.22 | done | First-run workflow demo was not part of daily local CI | `scripts/ci-local.sh demo` runs the zero-token workflow smoke directly, and `scripts/ci-local.sh all` now runs it between lint and SDK promotion | `scripts/ci-local.sh demo`: decision=1, lesson=1, error=1, search_hits=4, workflow latency 128ms, wall 0.91s; `bash -n scripts/ci-local.sh` |
| P3.23 | done | Local CI failures had no per-step timing context | `scripts/ci-local.sh` now records each command status/seconds and prints a Markdown timing table, also appending it to `$GITHUB_STEP_SUMMARY` when available | `scripts/ci-local.sh demo`: timing table total 0.91s; `bash -n scripts/ci-local.sh`; `git diff --check` |
| P3.24 | done | Workflow release smoke only printed timing after full success | `scripts/workflow-release-smoke.sh` now records `ok`/`fail` status and detail for each step, prints the timing table from an EXIT trap, and appends the same table to `$GITHUB_STEP_SUMMARY` | `GITHUB_STEP_SUMMARY=$(mktemp) scripts/workflow-release-smoke.sh`: total 13.40s; status table written to stdout and summary file; forced `WG_BIN=/bin/false` failure records `fail ... exit 1` |
| P3.25 | done | Full release preflight could exit on missing required actionlint without a failed row | `scripts/release-preflight.sh` now supports `WG_RELEASE_PREFLIGHT_ACTIONLINT_BIN` and records missing required actionlint as `fail` in the summary table before exiting | Forced missing actionlint: `fail | workflow syntax lint | 0.00 | /nonexistent/actionlint not installed`; fast path with actionlint present total 0.29s |
| P3.26 | done | Binding release smoke had package status but no timing or summary artifact | `scripts/bindings-release-smoke.sh` now records timed command rows plus `ok`/`ready`/`todo`/`skip` package status rows, prints a Markdown summary on exit, and appends it to `$GITHUB_STEP_SUMMARY` | `GITHUB_STEP_SUMMARY=$(mktemp) scripts/bindings-release-smoke.sh`: total 3.37s warm; cargo check 0.22s; npm pack/install smoke 3.05s; Python/NIF/FFI optional checks ready |
| P3.27 | done | npm publish dry-run logs did not show where release time went | `scripts/wg-napi-publish.sh` now records timed rows for install, build, test, platform/root publish dry-runs, and payload validators, then writes the Markdown summary to stdout and `$GITHUB_STEP_SUMMARY` | `GITHUB_STEP_SUMMARY=$(mktemp) WG_NAPI_PUBLISH_SCOPE=both scripts/wg-napi-publish-dry-run.sh`: total 3.86s; build 0.71s; test 1.13s; platform publish 0.63s; root publish 0.75s |
| P3.28 | done | Python publish dry-run hid the expensive sdist rebuild behind raw logs | `scripts/wg-python-publish-dry-run.sh` now records timed rows for version gate, venv creation, `maturin build --release --sdist`, and payload validation, writes the Markdown summary to stdout and `$GITHUB_STEP_SUMMARY`, and records fast failure rows for missing maturin / version mismatch | `GITHUB_STEP_SUMMARY=$(mktemp) WG_PYTHON_DIST_DIR=$(mktemp -d) scripts/wg-python-publish-dry-run.sh`: total 60.94s; maturin build 59.48s; payload check 0.05s; forced version mismatch records `fail | version expectation` |
| P3.29 | done | NAPI pack/install smoke had no standalone timing artifact | `scripts/wg-napi-pack-smoke.sh` now records timed rows for version gate, build, test, root/platform `npm pack`, packed tarball install, and installed package verification, then writes the summary to stdout and `$GITHUB_STEP_SUMMARY` | `GITHUB_STEP_SUMMARY=$(mktemp) scripts/wg-napi-pack-smoke.sh`: total 2.81s; build 0.67s; test 1.00s; root pack 0.22s; platform pack 0.53s; install 0.30s |
| P3.30 | done | Python wheel pack smoke had no standalone timing artifact | `scripts/wg-python-pack-smoke.sh` now records timed rows for version gate, venv creation, `maturin build --release`, wheel install, binding smoke, and installed version verification, then writes the summary to stdout and `$GITHUB_STEP_SUMMARY` | `GITHUB_STEP_SUMMARY=$(mktemp) scripts/wg-python-pack-smoke.sh`: total 48.38s; venv 1.43s; maturin build 45.51s; install 0.30s; smoke 1.03s |
| P3.31 | done | Composite binding smoke duplicated child package summary tables in `$GITHUB_STEP_SUMMARY` | `bindings-release-smoke.sh` suppresses child pack-smoke summary-file writes while preserving their stdout summaries, so the GitHub summary contains one top-level binding table | `summary_file=$(mktemp) && GITHUB_STEP_SUMMARY=$summary_file scripts/bindings-release-smoke.sh`: `rg -n '^## ' "$summary_file"` returns one heading; top-level total 3.37s; nested NAPI summary remains in stdout |
| P3.32 | done | Release preflight could duplicate child smoke/check summary tables in `$GITHUB_STEP_SUMMARY` | `release-preflight.sh` suppresses child summary-file writes for binding, workflow, SDK, and publish subchecks while preserving child stdout summaries, so CI shows one preflight table | `summary_file=$(mktemp) && GITHUB_STEP_SUMMARY=$summary_file WG_RELEASE_PREFLIGHT_WORKFLOW=0 WG_RELEASE_PREFLIGHT_SDK_PROMOTION=0 WG_RELEASE_PREFLIGHT_PUBLISH=0 scripts/release-preflight.sh`: `rg -n '^## ' "$summary_file"` returns one heading; total 3.79s; binding smoke 3.45s |
| P3.33 | done | Local CI SDK mode duplicated the SDK promotion table in `$GITHUB_STEP_SUMMARY` | `ci-local.sh` suppresses child summary-file writes for `sdk-promotion-check.sh` while preserving its stdout details, so local CI summary remains one top-level timing table | `summary_file=$(mktemp) && GITHUB_STEP_SUMMARY=$summary_file scripts/ci-local.sh sdk`: `rg -n '^## ' "$summary_file"` returns one heading; total 0.14s; SDK gate reports ok=6, ready=3, blocked=2 |
| P3.34 | done | Public comparison page had stale gap claims after rerank/type-weighted ranking shipped | `COMPARE.md` now describes rerank, type-aware ranking, MCP tool count, explicit extraction, and shared-store limits using the current implementation state | `rg -n "24-tool|17 \\(after|doesn't use|Roadmap: add|Roadmap: enable|we haven't measured|TEI rerank wired but" COMPARE.md README.md docs/MEASUREMENTS.md` returns no matches |
| P3.35 | done | Self-extraction positioning lacked a zero-token contract test | Scenario L simulates an agent-classified `wg_fact_add_many` batch and verifies typed facts feed sparse-ticket workflow context without cross-source leakage | `python3 bench/multi-agent/scenario_l_self_extraction.py`: 10/10 invariants; 7 facts inserted via MCP in 295.44ms; alpha workflow recovered decision=1, lesson=1, error=1; beta leakage 0 |
| P3.36 | done | MCP workflow sessions returned a session id but follow-up fact writes had no first-class way to attach to that thread | `wg_fact_add` accepts `session_id`; `wg_fact_add_many` accepts top-level or per-item `session_id`; tool descriptions now tell agents to pass the id from `wg_workflow_start` | `cargo test -p wg-cli --bin wg workflow_start_creates_session_ticket_and_scoped_context` passed; `cargo test -p wg-cli --bin wg fact_add_many_attaches_top_level_session_id_to_each_item` passed |
| P3.37 | done | Agents had to repeat `source_id` on every MCP read/write to keep shared-store isolation smooth | MCP tools now fall back to `WG_SOURCE_ID` for source scoping when the tool call omits `source_id`, while explicit arguments still override the environment default | `cargo test -p wg-cli --bin wg`: 114 passed, 1 ignored; `python3 bench/multi-agent/scenario_l_self_extraction.py`: 10/10 invariants including env-default write + search |
| P3.38 | done | `WG_SOURCE_ID` default scoping still required manual agent config edits after install | `wg mcp-install --source-id <namespace>` injects `WG_SOURCE_ID` into Claude / Hermes / OpenClaw shell registrations and Codex / Cursor / OpenCode config files | `cargo test -p wg-cli --bin wg mcp_install`: 10/10 tests passed; `wg mcp-install --target codex --source-id agent-alpha --print --json` reports `source_id=agent-alpha` and `env.WG_SOURCE_ID` detail |
| P3.39 | done | Setup diagnostics still suggested an unscoped MCP install even when recent workflow tickets already showed the project namespace | `wg doctor` reuses the most recent workflow ticket `source_id` in its no-MCP action, and `wg skill install` follow-up text points to `wg mcp-install --source-id <namespace>` for shared stores | `cargo test -p wg-cli --bin wg`: 120 passed, 1 ignored; temp `wg skill install --target opencode --dest ...` output includes `wg mcp-install --target opencode` and `--source-id <namespace>` |
| P3.40 | done | MCP source-default install had unit coverage but no product-boundary regression showing generated configs feed actual MCP calls | Scenario M installs Codex / Cursor / OpenCode configs into an isolated HOME, checks Claude / Hermes / OpenClaw print commands, then runs MCP write/search with the installed `WG_SOURCE_ID` env and no explicit `source_id` args | `scripts/bench-agent-ux.sh`: Scenario M 12/12 invariants; 323.97ms; scoped MCP write/search returned `agent-alpha` |
| P3.41 | done | Scenario H still instructed agents to pass `source_id` on each workflow tool call, so natural-prompt validation did not exercise the smooth source-default path | Scenario H now configures source defaults per runtime, Codex via isolated `wg mcp-install --source-id`; Hermes plugin honors `plugins.wg.source_id` / `WG_SOURCE_ID` for omitted tool args | `WG_E2E_SETUP_ONLY=1 python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py`: 5/5 setup invariants; `python3 -m pytest plugins/hermes/tests -q`: 62 passed |
| P3.42 | done | Hermes positioning was workflow-only even though real Hermes sessions need broader context, aggregation, batch capture, and diagnostics | Hermes native plugin now exposes `wg_context`, `wg_aggregate`, `wg_fact_add_many`, and `wg_doctor` alongside the existing workflow/query/search tools; slash commands add `/wg-context`, `/wg-aggregate`, and `/wg-doctor` | `python3 -m pytest plugins/hermes/tests -q`; `cargo test -p wg-cli --bin wg mcp_tools::` |
| P3.43 | done | `wg_aggregate` did not respect shared-store source scoping, weakening Hermes team/multi-agent profiles | MCP aggregate now falls back to `WG_SOURCE_ID` / explicit `source_id` and Hermes forwards source scope through native tools and slash commands | `python3 -m pytest plugins/hermes/tests/test_client.py plugins/hermes/tests/test_plugin.py -q`; `cargo test -p wg-cli --bin wg mcp_tools::` |
| P3.44 | done | Hermes docs listed plugin surfaces but did not map them to concrete Hermes usage modes | `plugins/hermes/README.md` now documents coding, long-session, research, team, and safe-capture profiles with the matching wg surfaces and example slash commands | `rg -n "Hermes-fit usage profiles|wg_context|wg_aggregate|wg_doctor" plugins/hermes/README.md` |
| P3.45 | done | Hermes research workflows still had to chain tool calls through model-visible turns instead of composing memory primitives in code | `hermes_wg.WgMemorySDK` exposes `open`, `search_rows`, `search_many`, `query_many`, `aggregate_many`, `coverage_by`, `group_by_entity`, and `remember` for code-first memory orchestration | `python3 -m pytest plugins/hermes/tests/test_sdk.py -q` |
| P3.46 | done | The bundled wg skill taught tool availability but not profile-specific Hermes composition patterns | `wg-skill/SKILL.md` now includes Hermes coding, long-session, research, team, and safe-capture recipes plus a Memory-as-Code SDK example under 2k-token-style guidance | `rg -n "Hermes composition recipes|Memory-as-Code|WgMemorySDK" wg-skill/SKILL.md` |
| P3.47 | done | Hermes Memory-as-Code had no measured product-boundary regression | Scenario N seeds scoped research memory, fans out SDK searches, dedupes hits, computes coverage, writes derived observations in a batch, aggregates scoped facts, and checks beta-source exclusion | `python3 bench/multi-agent/scenario_n_hermes_memory_as_code.py`; `python3 -m pytest plugins/hermes/tests -q` |
| P3.48 | done | Hermes engine/SDK positioning still relied on editable checkout assumptions | `scripts/hermes-wg-pack-smoke.sh` builds real `wg-agent-sdk` + `hermes-wg` wheels, installs them into a temp venv, and verifies SDK re-export, Hermes plugin entry point, `plugin.yaml`, and bundled skill payload | `scripts/hermes-wg-pack-smoke.sh`: installed `wg-agent-sdk 0.1.0` + `hermes-wg 1.0.0`, verified `hermes_wg.WgMemorySDK` re-exports `wg_agent.Memory`, `hermes.plugins`, `plugin.yaml`, and `SKILL.md`; total 4.36s |
| P3.49 | done | The Hermes SDK first-use path still exposed too many helper steps for routine research tasks | `Memory.open`, `search_rows`, and `remember` now cover the natural install-to-first-script path while keeping lower-level helpers available for custom pipelines | `python3 -m pytest plugins/hermes/tests/test_sdk.py -q`; `python3 bench/multi-agent/scenario_n_hermes_memory_as_code.py` |
| P3.50 | done | Code-first memory composition was still packaged as Hermes-specific even though Codex and Claude Code can use the same Python path | `packages/wg-agent-sdk` now owns the shared `wg_agent.Memory` / `WgMemorySDK` client+composition layer; `hermes_wg` re-exports it for compatibility, and pack smokes verify both packages | `python3 -m pytest packages/wg-agent-sdk/tests -q`: 7 passed; `python3 -m pytest plugins/hermes/tests -q`: 75 passed; `scripts/wg-agent-sdk-pack-smoke.sh`: 3.20s; `scripts/hermes-wg-pack-smoke.sh`: 4.36s |
| P3.51 | done | SDK-memory positioning needs a safe way to evolve agent memory skills without runtime self-modification | SkillOpt-lite defines the trainable memory profile artifact, bounded edit discipline, rejected-edit buffer, and validation gate; `scripts/skillopt-lite-check.sh` accepts a candidate only after skill/profile token checks plus local workflow / SDK gates pass | `scripts/skillopt-lite-check.sh`; optional `WG_SKILLOPT_RUN_SCENARIOS=1 scripts/skillopt-lite-check.sh` for Scenario L/M/N |

## Current Sprint

All planned P0-P3.50 roadmap items are closed. Scenario H now isolates each
agent's integration path: Claude project MCP, Codex temp `CODEX_HOME` MCP, and
Hermes MCP-only profile to avoid redb lock contention with the in-process
plugin. Hermes plugin work now follows usage profiles rather than a generic
integration checklist: coding workflows use `wg_workflow_start`, long sessions
use `wg_context`, research loops use `wg_fact_add_many` + `wg_aggregate`, and
team setups use `source_id` + `/wg-doctor`. The research profile now also has a
Memory-as-Code SDK and Scenario N, so Hermes can keep fanout/dedupe/coverage
state in Python instead of model tokens. That SDK is now protected by a real
wheel install smoke, so the Hermes engine/SDK path is package-installable
rather than checkout-only. The first-use SDK path now starts with
`wg_agent.Memory.open(...)`, collects evidence with `search_rows(...)`, and stores
observations with `remember(...)`, so common Hermes, Codex, and Claude Code
research scripts do not need to manually chain setup, flattening, dedupe, and
batch conversion. SkillOpt-lite now gives that agent-facing memory profile an
offline improvement loop: bounded skill edits are validated by
`scripts/skillopt-lite-check.sh` before they are accepted, and optional Scenario
L/M/N gates cover typed self-extraction, source-default install, and
Memory-as-Code composition.
`wg doctor --json` now exposes workflow readiness,
recent workflow tickets, and actionable setup hints for sparse ticket
automation. Scenario I now validates that doctor view against actual
CLI/MCP/Hermes workflow traces.
Scenario J defines the serverless shared-store envelope: `lock_retry_ms=5000`
is smooth through four concurrent local writers, while eight writers should use
the shared daemon path if every write matters. `wg doctor --json` now surfaces
that as a first-class `sharing` report instead of burying it in workflow hints.
MCP `wg_doctor` now carries the same sharing summary so coding agents can react
to retry/daemon guidance through the tool surface they already call. Release
version bumps are now one measured gate: `scripts/wg-release-version.sh
[VERSION]` verifies Cargo, Python, npm, and NIF package versions together while
leaving C FFI on Cargo metadata. `scripts/release-preflight.sh` now wraps the
release gates into a timed local/full preflight. SDK naming now distinguishes
the product layer from the low-level package layer: `wg-agent-sdk` is the
agent-facing SDK path, while `wg-python` and `wg-napi` remain SDK candidates
until public registry releases succeed. Python and Node now have
workflow-level sparse-ticket APIs, package docs, and stable error handling.
Scenario K validates their workflow contract against the CLI.
`scripts/sdk-promotion-check.sh` keeps the low-level package promotion rule as
a local gate, `ci-local.sh all` runs it, release preflight runs it by default,
and CI exposes it as a dedicated fast check with a GitHub summary table. Elixir
and C remain low-level bindings. First-run onboarding now starts with a
zero-token workflow demo that shows the sparse-ticket memory loop directly, and
release smoke plus local CI protect that demo. Local CI now prints the same
kind of timed summary used by release smoke and preflight scripts, and workflow
release smoke keeps that timing context even when a later step fails. Full
release preflight now also records missing required `actionlint` as a failed
summary row instead of exiting without a step record. Binding release smoke now
emits the same summary style, so package-level readiness and slow binding
steps are visible both standalone and inside release preflight logs. npm
publish dry-run now uses the same timing style for install/build/test and
root/platform payload verification. Python publish dry-run now does the same
for its expensive sdist rebuild and payload validation path. NAPI pack/install
smoke now also exposes build/test/pack/install timing as a reusable summary.
Python wheel pack smoke now exposes the same summary style for the release
wheel build/install/test path, making its 45s maturin build cost visible before
the heavier publish dry-run is needed. Binding release smoke now keeps child
pack-smoke tables in stdout but writes only one top-level table to
`$GITHUB_STEP_SUMMARY`, avoiding duplicated CI summary sections. Release
preflight now follows the same rule for binding, workflow, SDK, and publish
subchecks, so a full preflight keeps detailed logs without flooding the GitHub
summary page. Local CI now applies the same rule to its SDK promotion subcheck.
The public comparison page now matches the current implementation: rerank and
type-aware ranking are no longer described as roadmap gaps, tool count is
current, and remaining trade-offs are explicit extraction, E2E SOTA gap,
single-writer semantics, community detection, and hosted multi-tenant ops.
Scenario L now makes the explicit self-extraction contract measurable: an
agent-classified `wg_fact_add_many` batch persists decision / lesson / error /
preference / convention facts and drives sparse-ticket workflow recall without
requiring a built-in LLM extraction pipeline.
MCP workflow follow-up writes now close the loop by accepting the
`wg_workflow_start` `session_id` in `wg_fact_add` and `wg_fact_add_many`, so
agent chats can keep task-local decisions and lessons on the same workflow
thread without relying on CLI-only environment variables.
MCP source scoping now has the same low-friction shape: set `WG_SOURCE_ID`
once in the MCP server environment and the common read/write tools stay scoped
without requiring every agent turn to repeat `source_id`.
The install path now wires that default directly: `wg mcp-install --source-id`
adds `WG_SOURCE_ID` to the agent MCP server registration, so the recommended
workflow no longer requires a manual config edit.
Diagnostics now keep that path visible: if `wg doctor` sees recent scoped
workflow tickets but no MCP agent registration, it suggests the scoped
`wg mcp-install --source-id` command instead of a generic install.
Scenario M now locks the full source-default install contract at the product
boundary: generated agent configs carry `WG_SOURCE_ID`, shell targets print the
right env-bearing command, and MCP calls consume that env without per-call
`source_id`.
Scenario H now follows the same smooth path for natural prompts: its prompt no
longer asks agents to pass `source_id` per call, setup injects the namespace
through MCP/plugin config, and Hermes's native plugin consumes the same default
source namespace as MCP.

Next measurement candidates:
1. Reserve/configure the PyPI `wg-python` trusted publisher, then run `.github/workflows/wg-python-publish.yml` with `dry_run=false`.
2. Configure npm trusted publishers for `wg-napi*` package names, then run `.github/workflows/wg-napi-publish.yml` with `dry_run=false`.
3. Promote `wg-python` from binding / SDK candidate to package SDK only after the PyPI release succeeds.
4. Prototype daemon auto-discovery only if a future scenario needs more than four concurrent local writers without user-visible setup.

## Positioning Guardrails

- Preserve `wg`'s default zero-LLM, local-first path.
- Make LLM extraction opt-in and measurable, not implicit.
- Optimize for coding-agent memory next to a repo, not hosted consumer memory.
- Prefer explicit approval queues over silent memory rewrites.
