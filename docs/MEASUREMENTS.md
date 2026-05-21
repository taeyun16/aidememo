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
| HTTP shared `wg mcp-serve`, 2 clients x 10 writes | 20/20 persisted; p50 18.4ms, p95 41.8ms, wall 251ms | Daemon mode is still the faster high-concurrency path, but it can stay optional. |
| Workflow trigger Scenario F, 4 sparse tickets | 13/13 invariants; p95 2.48s; max context 3,023 chars; forbidden leakage 0 | CLI, MCP, and Hermes paths create distinct sessions/ticket facts and keep `source_id`-scoped ticket context separated. |
| Hermes workflow binding Scenario G, 4 sparse tickets | 5/5 invariants; shape parity 4/4; leakage 0; p50 1,795.71ms CLI vs 13.14ms binding | When `wg-python` is installed, Hermes composes workflow packs in process: same context contract, about 137x lower p50 after the first model/index warmup. |
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

P3.5 adds a `workflow` block to `wg doctor --json` so sparse ticket automation
failures are visible without manually inspecting agent configs or fact lists.
The stable contract is:

| Field | Meaning |
|---|---|
| `workflow.ready` | At least one checked agent has `wg` registered as MCP. |
| `workflow.recent_ticket_count` | Current workflow-start ticket facts in the last 30 days. |
| `workflow.recent_tickets[]` | Up to five recent ticket summaries with `source` / `source_id`. |
| `workflow.hints[]` | Actionable setup or usage hints with a concrete command in `action`. |

Validation:

| Command | Result |
|---|---|
| `cargo test -p wg-cli doctor` | 22 passed; workflow unit tests cover ready/count/hints. |
| `cargo test -p wg-cli doctor_json_includes_workflow_readiness_hints` | fixture CLI smoke validates JSON `workflow.ready`, `recent_ticket_count`, and actionable hints. |
| `python3 bench/multi-agent/scenario_i_workflow_doctor.py` | 10/10 invariants; CLI/MCP/Hermes each created a workflow ticket; doctor reported `workflow.ready=true`, `recent_ticket_count=3`, and no false no-MCP/no-recent-ticket hints. |
| `scripts/workflow-release-smoke.sh` | Bundles Scenario F + I plus a fresh fixture `wg doctor --json` assert for release checks. Latest run: Scenario F 13/13, Scenario I 10/10, fixture doctor `workflow_ready=true`, `recent_ticket_count=1`, total 15.13s. |
| CI `workflow-release-smoke` job | Runs the same script on Ubuntu after lint with Python 3.13 and a 10-minute timeout. |

Latest local `workflow-release-smoke` timing:

| Step | Seconds |
|---|---:|
| `cargo build -p wg-cli` | 0.41 |
| `py_compile` | 0.04 |
| Scenario F | 7.53 |
| Scenario I | 5.21 |
| Fixture `wg workflow start` | 1.94 |
| Total | 15.13 |

Scenario I measurement, May 21 2026:

| Metric | Value |
|---|---:|
| Workflow tickets | 3 |
| Drivers | CLI, MCP, Hermes |
| Workflow p50 latency | 1887.69 ms |
| Workflow p95 latency | 1891.48 ms |
| Doctor recent ticket count | 3 |
| Doctor hint codes | `workflow_no_skill_prompt` |

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
| `scripts/wg-napi-pack-smoke.sh` | Builds release addon, runs `npm test`, runs `npm pack`, and verifies `index.js`, `index.d.ts`, and the platform `.node` binary are present. Local macOS arm64 tarball: `wg-napi-0.1.0.tgz`, 2.78 MB packed. |
| `.github/workflows/wg-napi-artifacts.yml` | Manual/tag workflow builds, tests, packs, and uploads `wg-napi` artifacts on Ubuntu, macOS, and Windows. |

## Historical Notes

The old scratch-note directory was intentionally removed to keep the repository
focused on durable documentation and executable benchmarks. When adding a new
finding:

1. Put reusable code under `benchmarks/`, `bench/`, or `scripts/`.
2. Store machine-readable outputs under `bench/**/results` or
   `benchmarks/results`.
3. Summarize the user-facing result in this file or in the relevant
   `RESULTS.md`.
