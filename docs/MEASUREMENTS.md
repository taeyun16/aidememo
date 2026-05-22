# Measurements

This file is the public measurement ledger for `wg`. Historical scratch-note
files were removed; durable numbers should live here, in
`benchmarks/*/RESULTS.md`, or in JSON under `bench/**/results`.

## Re-run Commands

```bash
cargo check -p wg-core -p wg-cli
cargo test -p wg-core --features semantic
cargo test -p wg-cli --bin wg
python3 -m pytest plugins/hermes/tests -q

cargo run --release --bin performance
python3 bench/multi-agent/scenario_d_concurrent_writers.py
python3 bench/multi-agent/scenario_e_http_shared.py
python3 bench/multi-agent/scenario_f_workflow_triggers.py
python3 bench/multi-agent/scenario_g_hermes_binding.py
python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py
python3 bench/multi-agent/scenario_j_lock_retry_sweep.py
python3 bench/multi-agent/scenario_k_sdk_workflow_parity.py
scripts/demo-workflow.sh
scripts/ci-local.sh demo
scripts/sdk-promotion-check.sh
scripts/ci-local.sh sdk
```

The gbrain adapter path is documented in
[`benchmarks/gbrain-evals-adapter/README.md`](../benchmarks/gbrain-evals-adapter/README.md),
with current scorecards in
[`benchmarks/gbrain-evals-adapter/RESULTS.md`](../benchmarks/gbrain-evals-adapter/RESULTS.md).

## Agent UX

| Scenario | Result | Interpretation |
|---|---:|---|
| Hermes mixed-source prompt, unscoped | beta facts included 2/2 | Shared stores leak neighbouring source context unless scoped. |
| Hermes mixed-source prompt, `source_id=alpha` | alpha recall 2/2, beta leakage 0/2 | `source_id` gives clean per-agent/per-source reads. |
| Hermes serverless shared store, retry `0` | 10/20 writes persisted, 10 lock errors | redb's process lock is visible without smoothing. |
| Hermes serverless shared store, retry `5000` | 20/20 writes persisted, 0 errors; wall 2.16s, p50 98.1ms, max 1.22s | The default plugin path is smooth for ordinary two-agent local sharing. |
| Scenario J serverless lock-retry sweep, retry `5000` | Smooth until 4 concurrent writers; at 8 writers 79/80 persisted, p95 2.99s | Keep serverless sharing for small same-host teams; switch to daemon when high parallel write volume is normal. |
| HTTP shared `wg mcp-serve`, 2 clients x 10 writes | 20/20 persisted; p50 18.4ms, p95 41.8ms, wall 251ms | Daemon mode is still the faster high-concurrency path, but it can stay optional. |
| Workflow trigger Scenario F, 4 sparse tickets | 13/13 invariants; p95 2.48s; max context 3,023 chars; forbidden leakage 0 | CLI, MCP, and Hermes paths create distinct sessions/ticket facts and keep `source_id`-scoped ticket context separated. |
| Hermes workflow binding Scenario G, 4 sparse tickets | 5/5 invariants; shape parity 4/4; leakage 0; p50 1,795.71ms CLI vs 13.14ms binding | When `wg-python` is installed, Hermes composes workflow packs in process: same context contract, about 137x lower p50 after the first model/index warmup. |
| SDK workflow parity Scenario K, 4 sparse tickets | 8/8 invariants; Python and Node shape parity 4/4 each; leakage 0; p50 CLI 1,864.55ms, Python 16.19ms, Node 13.69ms | `wg-python` and `wg-napi` expose the same sparse-ticket context contract as CLI while avoiding per-command CLI spawn overhead. |
| Zero-token workflow demo | decision + lesson + error surfaced; search hits 4; workflow latency 128ms | `scripts/demo-workflow.sh` demonstrates the product position without an agent, model call, or persistent store. It uses CLI `workflow start --bm25-only` for deterministic first-run behaviour. `scripts/ci-local.sh demo` wraps the same smoke for daily local checks in about 0.91s warm. |
| Natural workflow adoption Scenario H, 3 agents | 4/4 invariants; 3/3 agents passed; each created 1 scoped workflow fact; prior reflection Claude 3/3, Codex 2/3, Hermes 3/3; forbidden leakage 0 | Sparse-ticket prompts can drive the workflow entry point across Claude, Codex, and Hermes when each runtime gets an isolated, deterministic MCP config. Hermes uses MCP-only here to avoid redb lock contention with the in-process plugin. |

## gbrain-evals Adapter

Fresh-checkout validation against `garrytan/gbrain-evals` commit `ef7794f`
on 2026-05-19:

| Adapter | Corpus Pages | Queries | P@5 | R@5 | Correct / Expected | Real Time |
|---|---:|---:|---:|---:|---:|---:|
| wg bm25 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 63.38s |
| wg bm25 daemon | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 11.04s |
| wg hybrid daemon | 240 | 145 | 16.7% | 62.5% | 121 / 261 | 45.64s |

Daemon BM25 preserves the score and cuts wall time by `5.7x`. Hybrid is
slower and slightly worse on this surface-form-heavy BrainBench slice; keep
semantic retrieval for paraphrase-heavy workloads.

## LongMemEval-S Retrieval

500 questions from the cleaned LongMemEval-S set, measured with the Rust
benchmark harness:

| Stack | R@1 | R@5 | R@10 | MRR |
|---|---:|---:|---:|---:|
| wg BM25-only | 0.866 | 0.952 | 0.974 | 0.902 |
| wg + time-decay soft bias | 0.858 | 0.958 | 0.978 | 0.898 |
| wg + bge-small-en-v1.5 | 0.914 | 0.976 | 0.986 | 0.941 |
| wg + bge + reranker K=10 | 0.938 | 0.984 | 0.986 | 0.957 |
| wg + bge + two-stage reranker K=20 -> 10 | 0.940 | 0.984 | 0.992 | 0.958 |

The retrieval ceiling is high: answer evidence lands in the top-10 set for
496/500 questions. Remaining E2E errors are mostly reader-side temporal or
multi-session reasoning, not missing evidence.

## LongMemEval-S E2E

LLM-graded with a `gpt-4o` judge:

| Stack | Reader | Overall |
|---|---|---:|
| Mem0 published baseline | gpt-4o | 49.0% |
| wg @ model2vec + decay | gpt-4o-mini | 60.0% |
| wg @ model2vec + decay | gpt-4o | 60.4% |
| wg @ bge + reranker K=20 -> 10 | gpt-4o-mini | 65.6% |
| wg @ bge + reranker K=20 -> 10 | gpt-4o | 67.6% |
| wg @ bge + reranker K=20 -> 10 | gpt-4.1 | 72.6% |
| wg @ bge + reranker K=20 -> 10 | MiniMax-M2.7-highspeed | 74.0% |
| Zep / Graphiti 2026 published | gpt-4o | 71.2% |
| Mastra published | gpt-4o | 84.2% |
| OMEGA published local | gpt-4.1 | 95.4% |

Use these numbers carefully. `wg` should lead with deployment and temporal
memory ergonomics, not a SOTA claim.

## Model And Rerank Trade-offs

| Workload | Recommended path | Measured reason |
|---|---|---|
| Code/docs/news where query terms overlap answer text | default `model2vec` / BM25-hybrid | HotpotQA and MultiHop-RAG were saturated: model2vec and BGE both landed R@5 around 94-96%. |
| English paraphrase-heavy personal memory | `fastembed` + `bge-small-en-v1.5` | LongMemEval-S R@5 improved from 96.2% to 98.0%; SS-pref went 93.3 -> 100. |
| Korean / multilingual repositories | default multilingual model2vec | `bge-small-en-v1.5` is English-tuned. |
| Retrieval-bound search where top-K recall is low | enable rerank | MIRACL/ko improved MRR@10 by 5.8% and nDCG@10 by 4.6%. |
| Reader-bound agent loops where top-K already overlaps | keep rerank off | Cross-encoder rerank can add roughly 85x latency at top_k=8 with no E2E lift. |

HNSW is the default semantic index because it closed a Korean MIRACL candidate
drop caused by BM25 prefiltering while keeping query latency below brute-force
cosine on larger corpora.

## Performance

Reference numbers from `benchmarks/src/bin/performance.rs` on a 10,000-fact
synthetic wiki, p95 latency, default config:

| Operation | p95 |
|---|---:|
| `traverse_d3` | ~0.01 ms |
| `search_bm25` | ~0.5 ms |
| `search_hybrid` | ~3.4 ms |
| `lint` | ~34 ms |
| `fact_add_many` per fact | ~0.07 ms |
| `fact_add` single | ~5 ms |
| `startup` open + first traverse | ~12 ms |

`fact_add` is limited by the OS fsync floor under immediate durability. Use
`fact_add_many` or `store.durability = eventual` when ingest throughput matters.

## Workflow Doctor Readiness

P3.5 adds a `workflow` block and P2.5 adds a separate `sharing` block to
`wg doctor --json` so sparse ticket automation and shared-store ergonomics are
visible without manually inspecting agent configs, fact lists, or benchmark
notes. P2.6 threads the same `sharing` contract through MCP `wg_doctor`, so
agents can see it without shelling out. The stable workflow contract is:

| Field | Meaning |
|---|---|
| `workflow.ready` | At least one checked agent has `wg` registered as MCP. |
| `workflow.recent_ticket_count` | Current workflow-start ticket facts in the last 30 days. |
| `workflow.recent_tickets[]` | Up to five recent ticket summaries with `source` / `source_id`. |
| `workflow.hints[]` | Actionable setup or usage hints with a concrete command in `action`. |

The sharing contract is:

| Field | Meaning |
|---|---|
| `sharing.lock_retry_ms` | Current serverless redb lock retry budget. |
| `sharing.serverless_recommended_writers` | Measured smooth same-host writer envelope, currently 4. |
| `sharing.high_concurrency_writers` | Stress point used by Scenario J, currently 8. |
| `sharing.daemon.state` | `healthy`, `stale_registry`, or `none`. |
| `sharing.recommended_mode` | `daemon`, `serverless_retry`, or `serverless_fail_fast`. |
| `sharing.hints[]` | Actionable retry / daemon guidance with a concrete command in `action`. |

Validation:

| Command | Result |
|---|---|
| `cargo test -p wg-cli doctor` | 25 passed; workflow unit tests cover ready/count/hints, sharing unit tests cover retry advisory behaviour, and integration tests cover the JSON `sharing` contract. |
| `cargo test -p wg-cli doctor_json_includes_workflow_readiness_hints` | fixture CLI smoke validates JSON `workflow.ready`, `recent_ticket_count`, and actionable hints. |
| `cargo test -p wg-cli doctor_json_includes_shared_store_guidance` | fixture CLI smoke validates JSON `sharing.lock_retry_ms`, `serverless_recommended_writers=4`, `daemon.state`, `recommended_mode`, and actionable hints. |
| `cargo test -p wg-cli doctor_groups_by_code_with_action_hints` | MCP `wg_doctor` unit validates lint grouping plus `sharing.serverless_recommended_writers=4`, `recommended_mode=serverless_fail_fast`, and `sharing_retry_disabled`. |
| `python3 bench/multi-agent/scenario_i_workflow_doctor.py` | 10/10 invariants; CLI/MCP/Hermes each created a workflow ticket; doctor reported `workflow.ready=true`, `recent_ticket_count=3`, and no false no-MCP/no-recent-ticket hints. |
| `python3 bench/multi-agent/scenario_j_lock_retry_sweep.py` | 7/7 invariants; `store.lock_retry_ms=5000` stayed smooth through 4 concurrent serverless writers and mostly recovered 8-writer contention. |
| `scripts/workflow-release-smoke.sh` | Bundles the first-run workflow demo, Scenario F + I, and a fresh fixture `wg doctor --json` assert for release checks. Latest run: demo recovered decision/lesson/error, Scenario F 13/13, Scenario I 10/10, fixture doctor `workflow_ready=true`, `recent_ticket_count=1`, total 13.40s. The timing table includes `ok`/`fail` status and is printed from an EXIT trap so partial failures still leave context; forced `WG_BIN=/bin/false` failure records `fail ... exit 1`. |
| CI `workflow-release-smoke` job | Runs the same script on Ubuntu after lint with Python 3.13 and a 10-minute timeout. |
| CI `workflow-lint` job | Runs `actionlint@v1.7.1` across `.github/workflows/*.yml` before heavier Rust checks. Local `actionlint .github/workflows/*.yml`: 0 issues. |

Latest local `workflow-release-smoke` timing:

| Status | Step | Seconds |
|---|---|---:|
| ok | `cargo build -p wg-cli` | 0.22 |
| ok | `bash -n scripts/demo-workflow.sh` | 0.02 |
| ok | `py_compile` | 0.04 |
| ok | `scripts/demo-workflow.sh` | 0.65 |
| ok | Scenario F | 7.17 |
| ok | Scenario I | 5.10 |
| ok | Fixture `wg workflow start --bm25-only` | 0.20 |
| total | | 13.40 |

Scenario I measurement, May 21 2026:

| Metric | Value |
|---|---:|
| Workflow tickets | 3 |
| Drivers | CLI, MCP, Hermes |
| Workflow p50 latency | 1887.69 ms |
| Workflow p95 latency | 1891.48 ms |
| Doctor recent ticket count | 3 |
| Doctor hint codes | `workflow_no_skill_prompt` |

Scenario J lock-retry sweep, May 21 2026:

| Writers | Retry ms | Persisted | Success | p50 ms | p95 ms | Max ms | Wall ms |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 0 | 10/10 | 100.0% | 94.80 | 185.26 | 185.26 | 1113.06 |
| 2 | 0 | 10/20 | 50.0% | 92.80 | 124.25 | 179.96 | 1150.86 |
| 4 | 0 | 10/40 | 25.0% | 8.72 | 106.01 | 192.92 | 1171.85 |
| 8 | 0 | 10/80 | 12.5% | 9.52 | 107.85 | 200.79 | 1242.34 |
| 1 | 5000 | 10/10 | 100.0% | 106.71 | 186.70 | 186.70 | 1207.83 |
| 2 | 5000 | 20/20 | 100.0% | 103.61 | 196.69 | 1274.25 | 2255.45 |
| 4 | 5000 | 40/40 | 100.0% | 101.86 | 1282.56 | 3452.56 | 4431.45 |
| 8 | 5000 | 79/80 | 98.8% | 103.02 | 2988.40 | 5049.66 | 8529.39 |

Product read: serverless CLI retry is appropriate for one to four same-host
writers. At eight concurrent writers it is still much better than fail-fast
mode, but p95 approaches three seconds and one write can still exhaust the
5s retry budget; use a shared `wg mcp-serve` daemon when that level of
parallelism is normal.

## GBrain Adapter Native Backend

The `gbrain-evals` scaffold now supports `WG_ADAPTER_BACKEND=auto|cli|napi`.
The native path imports `wg-napi` in process and keeps the existing CLI and
daemon paths as baselines.

Local Bun-shaped fixture, 3 pages, 30 repeated BM25 queries:

| Backend | Top Hit | p50 | p95 |
|---|---|---:|---:|
| CLI | `redis` | 124.55 ms | 132.08 ms |
| NAPI | `redis` | 0.02 ms | 0.03 ms |

This is a scaffold-level latency check, not the full public `gbrain-evals`
scorecard. Record full runner wall time separately when re-running BrainBench.

Fresh-checkout BrainBench scorecard, `garrytan/gbrain-evals@89445dd`,
`BRAINBENCH_N=1`, 240 pages, 145 queries:

| Backend | P@5 | R@5 | Correct / Expected | Real Time |
|---|---:|---:|---:|---:|
| CLI daemon bm25 | 17.4% | 64.1% | 125 / 261 | 10.77 s |
| NAPI bm25 | 17.4% | 64.1% | 125 / 261 | 6.48 s |

NAPI preserves score parity and is `1.66x` faster than the daemon baseline
(`9.78x` faster than the historical direct CLI `63.38 s` run).

Packaging readiness:

| Command / workflow | Result |
|---|---|
| `scripts/release-preflight.sh` | One-command release gate with timed summary rows. `local` profile runs version gate, workflow syntax lint when `actionlint` is available, binding release smoke, workflow release smoke, and SDK promotion check; `full` profile adds optional Python/Elixir/C binding smokes plus Python/npm publish dry-runs. Latest fast path with bindings/workflow/sdk/publish disabled passed version + actionlint in `0.29s`. `WG_RELEASE_PREFLIGHT_SDK_REQUIRE_PUBLIC=1` fails the SDK step with exit 1 while recording the failure row. Forced `WG_RELEASE_PREFLIGHT_PROFILE=full WG_RELEASE_PREFLIGHT_ACTIONLINT_BIN=/nonexistent/actionlint` records `fail | workflow syntax lint | 0.00 | /nonexistent/actionlint not installed`. |
| CI `sdk-promotion-check` job | Runs `scripts/sdk-promotion-check.sh` on Ubuntu after lint with Python 3.13 and Node 22, keeping SDK wording drift visible in PR checks without running package smokes. Local workflow lint: `actionlint .github/workflows/*.yml` 0 issues; local gate: ok=6, ready=3, blocked=2, fail=0. |
| `scripts/ci-local.sh demo` | Local first-run workflow smoke with a timed Markdown summary. `bash -n scripts/ci-local.sh` passes; `scripts/ci-local.sh demo` recovered decision=1, lesson=1, error=1, search_hits=4, workflow latency 128ms, wall 0.91s. `scripts/ci-local.sh all` now runs this check between lint and SDK promotion. |
| `scripts/ci-local.sh sdk` | Local CI parity hook for the same SDK wording gate. `bash -n scripts/ci-local.sh` passes; `scripts/ci-local.sh sdk` reports ok=6, ready=3, blocked=2, fail=0. `scripts/ci-local.sh all` now runs this check between the workflow demo and tests. |
| SDK promotion GitHub summary | `GITHUB_STEP_SUMMARY=$(mktemp) scripts/sdk-promotion-check.sh` writes a Markdown table with 11 check rows plus metric rows while keeping stdout unchanged. `GITHUB_STEP_SUMMARY=$(mktemp) WG_SDK_PROMOTION_JSON=1 scripts/sdk-promotion-check.sh` still emits valid JSON to stdout. |
| `scripts/wg-release-version.sh` | Unified release version gate. With no args, verifies Cargo, Python, npm, and NIF package versions together; with a semver arg, updates every managed package version. Latest run: `0.1.0` pinned; temp-copy bump to `0.1.1` updated Cargo workspace, Python `pyproject.toml`, npm root/platform packages plus optionalDependency pins, and `wg-nif` `mix.exs`. |
| `scripts/wg-python-version.sh` | Version gate for the Python wheel. With no args, verifies `Cargo.toml` workspace version equals `crates/wg-python/pyproject.toml` `project.version`; with a semver arg, updates both. Latest run: `0.1.0` pinned. |
| `scripts/wg-python-pack-smoke.sh` | Builds a `wg-python` wheel with maturin for a temp venv interpreter, installs that wheel into the venv, runs `crates/wg-python/tests/smoke.py`, and verifies installed wheel metadata equals `wg_python.__version__`. Local macOS arm64: built `wg_python-0.1.0-cp313-cp313-macosx_11_0_arm64.whl`, smoke passed including `workflow_start(..., bm25_only=True)` and `WgNotFoundError` typed exception handling, installed version `0.1.0`. |
| `scripts/wg-python-publish-dry-run.sh` | Builds PyPI publish payloads without uploading and writes a timed Markdown summary. Local macOS arm64: version gate 0.05s, venv 1.36s, `maturin build --release --sdist` 59.48s, payload validation 0.05s, total 60.94s; built `wg_python-0.1.0-cp313-cp313-macosx_11_0_arm64.whl` (2.8M) + `wg_python-0.1.0.tar.gz` (349K). The dry-run rebuilds the wheel from the sdist, proving the vendored `tokenizers` patch is present; forced `WG_PYTHON_EXPECT_VERSION=9.9.9` records a `fail | version expectation` summary row. |
| `scripts/wg-napi-version.sh` | Version gate for root + platform package graph. With no args, verifies all versions and root `optionalDependencies`; with a semver arg, updates every package. Latest run: `0.1.0` pinned across 5 platform packages; temp-copy bump to `0.1.1` updated root, platform packages, and optionalDependency pins. |
| `scripts/wg-napi-pack-smoke.sh` | Builds release addon, runs `npm test`, packs root `wg-napi`, packs the current platform package, then installs both tarballs into a temp project with offline/no-audit npm flags and verifies `require("wg-napi").version()`. Local macOS arm64: root `wg-napi-0.1.0.tgz` is 4.18 KB / 4 files / includes README + no `.node`; platform `wg-napi-darwin-arm64-0.1.0.tgz` is 2.79 MB / 2 files / includes `wg-napi.darwin-arm64.node`; smoke includes `workflowStart(..., bm25Only:true)` and JS Error `code=InvalidArg` with `[entity_not_found]` prefix. |
| `scripts/wg-napi-publish.sh` | Shared publish engine with a timed Markdown summary. `WG_NAPI_PUBLISH_MODE=dry-run|publish`, `WG_NAPI_PUBLISH_SCOPE=platform|root|both`, and optional `WG_NAPI_EXPECT_VERSION` gate both local and CI release flows. Local dry-run passed both scopes and wrote stdout + `$GITHUB_STEP_SUMMARY`: total 3.86s, build 0.71s, test 1.13s, platform publish 0.63s, root publish 0.75s. |
| `scripts/wg-napi-publish-dry-run.sh` | Wrapper around the publish engine with `WG_NAPI_PUBLISH_MODE=dry-run`. Local macOS arm64: root payload `wg-napi@0.1.0`, 4 files, 4.18 KB packed, README included, no `.node`; platform payload `wg-napi-darwin-arm64@0.1.0`, 2 files, 2.79 MB packed; payload validators passed for both. |
| `scripts/wg-nif-version.sh` | Version gate for the Elixir package. With no args, verifies `Cargo.toml` workspace version equals `crates/wg-nif/mix.exs`; with a semver arg, updates both. Latest run: `0.1.0` pinned. |
| `scripts/bindings-release-smoke.sh` | Cross-binding readiness smoke with a timed Markdown summary. Runs `cargo check -p wg-python -p wg-napi -p wg-nif -p wg-ffi`, npm version/pack/install smoke, and reports Python/Elixir/C optional package smokes based on local tools. Local macOS arm64 default path: cargo check 0.21s, npm version gate 0.05s, npm pack/install smoke 45.00s, total 45.31s; summary written to stdout and `$GITHUB_STEP_SUMMARY`. With `WG_BINDINGS_SMOKE_NPM=0 WG_BINDINGS_SMOKE_OPTIONAL=1`, the Python wheel build, Elixir `mix compile.cargo --force && mix test`, and C FFI smoke all passed. |
| `scripts/sdk-promotion-check.sh` | SDK wording gate for `wg-python` and `wg-napi`. Default local run: ok=6, ready=3, blocked=2, fail=0, `local_ready=true`, `sdk_promotable=false` because public PyPI/npm installs are not verified. With `WG_SDK_PROMOTION_RUN_SCENARIO_K=1`: ok=7, ready=2, blocked=2, fail=0 and Scenario K reports 8/8 invariants with p50 CLI 1882.33ms, Python 19.97ms, Node 20.0ms. Release preflight runs this gate by default; CI gets the same table in `$GITHUB_STEP_SUMMARY`; set `WG_RELEASE_PREFLIGHT_SDK_PROMOTION=0` only for focused debugging. |
| `.github/workflows/wg-napi-artifacts.yml` | Manual/tag workflow builds, tests, packs, and uploads root + platform `wg-napi` artifacts on Ubuntu, macOS, and Windows. |
| `.github/workflows/wg-python-publish-dry-run.yml` | Manual/tag workflow builds and validates `wg-python` PyPI payloads on Ubuntu without uploading. |
| `.github/workflows/wg-python-publish.yml` | Manual trusted-publisher workflow. It builds and validates distributions without PyPI permissions, uploads them as artifacts, then publishes via `pypa/gh-action-pypi-publish@release/v1` only when `dry_run=false`. Default `dry_run=true`; real publish requires a PyPI trusted publisher for this workflow and the `pypi-publish` environment. Local artifact-mode check: `WG_PYTHON_DIST_DIR=$(mktemp -d) scripts/wg-python-publish-dry-run.sh` produced wheel 2.8M + sdist 349K. |
| `.github/workflows/wg-napi-publish-dry-run.yml` | Manual/tag workflow runs the publish dry-run on Ubuntu with `id-token: write` reserved for the later trusted-publisher publish path. |
| `.github/workflows/wg-napi-publish.yml` | Manual trusted-publisher workflow. It publishes current-platform packages first, then the root wrapper. Default `dry_run=true`; real publish requires npm trusted-publisher setup for the exact workflow filename, `dry_run=false`, and `version` matching `package.json`. |

Package split: root `wg-napi` now ships the generated JS loader, types, README,
and optional dependencies. Platform packages ship exactly one native binary each:
`wg-napi-darwin-arm64`, `wg-napi-darwin-x64`,
`wg-napi-linux-arm64-gnu`, `wg-napi-linux-x64-gnu`, and
`wg-napi-win32-x64-msvc`. This matches the generated NAPI loader fallback names
and avoids publishing one platform's `.node` binary as the whole package.

Trusted-publisher notes: npm's current guidance requires Node 22.14+ and npm
11.5.1+ for trusted publishing, `id-token: write` in GitHub Actions, a
cloud-hosted runner, and an exact trusted-publisher registration for the
workflow filename. npm also notes trusted publishing automatically generates
provenance for public packages from public repositories, so the release
workflow uses OIDC rather than a long-lived `NPM_TOKEN`.

## Historical Notes

The old scratch-note directory was intentionally removed to keep the repository
focused on durable documentation and executable benchmarks. When adding a new
finding:

1. Put reusable code under `benchmarks/`, `bench/`, or `scripts/`.
2. Store machine-readable outputs under `bench/**/results` or
   `benchmarks/results`.
3. Summarize the user-facing result in this file or in the relevant
   `RESULTS.md`.
