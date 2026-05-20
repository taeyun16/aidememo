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

## Historical Notes

The old scratch-note directory was intentionally removed to keep the repository
focused on durable documentation and executable benchmarks. When adding a new
finding:

1. Put reusable code under `benchmarks/`, `bench/`, or `scripts/`.
2. Store machine-readable outputs under `bench/**/results` or
   `benchmarks/results`.
3. Summarize the user-facing result in this file or in the relevant
   `RESULTS.md`.
