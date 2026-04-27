# Bench: cross-encoder rerank on MIRACL/ko

First empirical pass over the TEI rerank integration that landed
in commit `0377928`. Question: does adding a `BAAI/bge-reranker-base`
post-RRF rerank improve search quality on real Korean Wikipedia
retrieval, and at what latency cost?

## Setup

* **Wiki**: 5 503 passages from MIRACL/ko (503 dev-relevant +
  5 000 random filler). Same recipe as `bench-miracl-ko.md`,
  re-prepared via `tmp/wg-tei-bench/prep_miracl.py`. Ingest into
  wg via `fact_add_many` — 5 503 facts in **0.1 s** (~47 K
  facts/s; the per-row `fact_add` path would have taken ~30 s).
* **Embedding**: `model2vec` / `potion-multilingual-128M`
  (default). `semantic_index = "hnsw"` so the baseline matches
  the production-default code path.
* **Rerank**: `BAAI/bge-reranker-base` (278 M params, fp16) on
  TEI native (`text-embeddings-router 1.9.3`, Metal). Port 8082.
  Started in ~10 s after model download.
* **Queries**: 213 dev queries from MIRACL/ko, all with at least
  one in-corpus relevant doc. Single-threaded HTTP loop.
* **Metrics**: Recall@10, MRR@10, nDCG@10 — golden set keyed on
  the original miracl docid via `source` field.

## Results

| condition | R@10 | MRR@10 | nDCG@10 | mean | p50 | p95 |
|---|---:|---:|---:|---:|---:|---:|
| baseline (HNSW, no rerank) | 0.816 | 0.759 | 0.725 | 10 ms | **9 ms** | 11 ms |
| rerank top_k=8 | **0.820** | 0.803 | 0.758 | 847 ms | 765 ms | 1 484 ms |
| rerank top_k=16 | 0.817 | 0.805 | **0.761** | 1 350 ms | 1 255 ms | 2 309 ms |
| rerank top_k=32 | 0.816 | **0.808** | 0.761 | 1 422 ms | 1 341 ms | 2 552 ms |

Δ vs baseline:

| metric | top_k=8 | top_k=16 | top_k=32 |
|---|---:|---:|---:|
| R@10 | +0.5 % | +0.1 % | +0.0 % |
| **MRR@10** | **+5.8 %** | +6.1 % | +6.5 % |
| **nDCG@10** | +4.6 % | +5.0 % | +5.0 % |
| latency | **85 ×** slower | 139 × | 149 × |

## Findings

1. **Rerank improves precision at the top of the list, not
   recall.** R@10 sits at ~0.816 in every condition because
   reranking only reorders the top K — it can't recover documents
   that retrieval already missed. The metric that actually moves
   is MRR (where the *first* relevant doc lands): +5.8 %–6.5 %
   across top_k variants. nDCG@10 follows.

2. **Diminishing returns past top_k=8.** MRR climbs from 0.803
   (top_k=8) to 0.808 (top_k=32) — a 0.6 % marginal gain — but
   latency nearly doubles. The 85 × slowdown at top_k=8 is
   already painful; going further is rarely worth it.

3. **Latency cost is brutal in absolute terms.** Baseline HNSW
   p50 = 9 ms. Add rerank top_k=8 → p50 = 765 ms. That's an 85 ×
   multiplier on every search. At top_k=32 the p95 hits 2.5 s.
   The standalone TEI `/rerank` probe earlier reported 78 ms p50
   at top_k=8 — the gap is real Wikipedia passages being much
   longer than the synthetic samples (cross-encoder cost is
   linear in pair-text length).

4. **Caveat: `bge-reranker-base` is English-leaning.** A
   multilingual cross-encoder (`bge-reranker-v2-m3`, 568 M
   params) would likely produce a bigger MRR/nDCG bump on
   Korean text. We didn't test it because of model-download
   size; the qualitative direction would not change.

## Implications for wg defaults

* The current default `rerank.provider = ""` (off) is correct.
  Rerank is a precision/cost trade-off most users won't want
  invisibly enabled.
* The newly-lowered `default_rerank_top_k = 8` is well-placed:
  it's the only point on the curve with reasonable cost/benefit
  (peak relative MRR gain per latency dollar). Bumping to 16 or
  32 is for offline batch use.
* **Document this trade-off** in the rerank guidance:
  "Cross-encoder rerank lifts MRR/nDCG ~5–6 % on real-corpus
  retrieval but costs ~80 × in p50 search latency. Enable for
  precision-sensitive workloads (long-form answers, citation
  recall) and accept the cost. Recall@K barely changes."

## Reproduce

```bash
# 1) Prepare 5503-doc corpus + golden set
python3 /tmp/wg-tei-bench/prep_miracl.py

# 2) Bulk-ingest into wg
cargo run --release --bin miracl_ingest

# 3) Spin up TEI native (if not already running)
text-embeddings-router \
  --model-id BAAI/bge-reranker-base --port 8082 \
  --huggingface-hub-cache /tmp/wg-tei-bench/native

# 4) Run the A/B
cargo run --release --bin miracl_rerank_bench -- --top-ks=8,16,32
```

## What we still don't know

* `bge-reranker-v2-m3` (multilingual) on the same corpus — would
  it widen the MRR gap, or is the issue retrieval recall (which
  rerank can't fix)?
* Whether rerank shines on a corpus where baseline HNSW recall
  is *worse* than 0.816. The headroom here is small.
* Throughput under concurrent load — TEI's `max_batch_requests`
  caps at 8 by default; under contention rerank latency would
  get worse.
