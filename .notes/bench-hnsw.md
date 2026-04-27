# HNSW ROI prototype on MIRACL/ko

In-tree wg bench reported a 0.706 → 0.730 recall ceiling gap between
the BM25-prefiltered hybrid and brute-force semantic on MIRACL/ko.
This document is the standalone prototype that confirms HNSW can
match brute-force quality without paying brute-force latency.

## Setup

- **Corpus**: 5 503 Korean Wikipedia passages (the same `wg-bench-miracl`
  store used by the in-tree bench).
- **Queries**: 213 dev queries with qrels (`bench_miracl_ko.jsonl`).
- **Model**: `potion-multilingual-128M`, 256-dim, L2-normalized.
- **Index**: `instant-distance` 0.6.1, `Builder::default()` +
  `ef_construction=200`, varying `ef_search`.
- **Brute force**: scan all 5 503 dot products per query (vectors are
  already unit-normalized so dot ≡ cosine sim).

Prototype binary lives in `benchmarks/src/bin/hnsw_miracl.rs`.

## Results

| Method | P@10 | R@10 | p50 query | p95 query | total 213q |
|---|---:|---:|---:|---:|---:|
| brute force (5 503 cos) | **0.176** | **0.724** | 294 µs | 312 µs | 72.5 ms |
| HNSW `ef=20` | 0.176 | 0.724 | **217 µs** | 281 µs | 53.5 ms |
| HNSW `ef=50` | 0.176 | 0.724 | 220 µs | 273 µs | 53.9 ms |
| HNSW `ef=100` | 0.176 | 0.724 | 209 µs | 243 µs | **50.3 ms** |
| HNSW `ef=200` | 0.176 | 0.724 | 217 µs | 274 µs | 53.3 ms |

For comparison, the in-tree wg bench (`wg bench /tmp/bench_miracl_ko.jsonl`)
on the same store reported:

| Pipeline | P@10 | R@10 | p50 |
|---|---:|---:|---:|
| Hybrid w/ `semantic_prefilter=50` (today's default) | 0.175 | 0.706 | 196 ms |
| Hybrid w/ `semantic_prefilter=0` (brute force) | 0.177 | 0.730 | 381 ms |

The difference between the prototype's brute-force (72 ms total) and
wg's `prefilter=0` brute-force (381 ms p50, ~80 s total) is wg's BM25
index rebuild + redb fact-by-fact hydration + per-query model inference.
The prototype amortizes embedding once and skips BM25 entirely; this
matches what an HNSW-backed wg path would look like.

## What this proves

1. **HNSW matches brute-force quality at every `ef_search`** tested
   (20–200) on this 5 503-doc corpus. ANN drift = 0.
2. **HNSW is faster than brute force even at this small corpus size**
   (~210 µs vs 294 µs p50). The advantage will widen with corpus size
   — brute force is `O(N)`, HNSW is `O(log N)` query-side.
3. **Switching wg's semantic candidate path from BM25-prefilter to
   HNSW would lift R@10 from 0.706 → 0.724** on Korean retrieval —
   the +2.5 pp recall is exactly the gap BM25 prefilter is silently
   dropping (see `bench-miracl-ko.md`).

## Costs

| | Estimate (5 503 docs, 256 dim) | At 50 k docs |
|---|---:|---:|
| Index build time | 2.5 s | ~25 s |
| Index memory (in-process) | ~90 MB | ~800 MB |
| Per-query latency | ~210 µs | ~400 µs (estimate) |
| Sidecar disk size (if persisted) | similar to in-mem | similar |

Build time + memory grow ~linearly with corpus size; query time grows
logarithmically. The 90 MB in-memory cost is comparable to today's
fact_embed_cache when fully populated, and would replace it.

## wg integration plan (1–2 day estimate)

1. **`wg-core/src/index/vector.rs`** — HnswIndex over `instant-distance`,
   key = `FactId`, value = matrix row. Build from the existing
   `fact_embed_cache` so quantization choice (f32 vs i8) doesn't change.
2. **`WikiGraph::ingest`** — embed every new/changed fact and insert
   into the index. Persist the index as a sidecar
   (`model.hnsw.bin` or similar) using `bincode`/`postcard` next to
   the existing `model.q8.safetensors` cache.
3. **`search::semantic_search`** — when `config.search.semantic_index = "hnsw"`,
   query the HNSW for top-K candidates instead of the BM25-prefilter
   pool. Falls back to today's path when the toggle is off.
4. **`wg doctor`** — surface "HNSW index is X facts behind store" so
   users notice if the sidecar drifts.

Out of scope for the first PR: deletes (rebuild on demand), graph
prefilter integration (already neutral on this dataset).

## Open questions

- Does `instant-distance` support incremental insert without
  rebuild? Glancing at the API: yes, but expensive. Acceptable
  pattern: rebuild on every `wg ingest` (matches BM25 today).
- Should HNSW consume f32 or i8 quantized embeddings? The
  prototype used f32 because instant-distance only takes a Point
  trait; an i8-aware Point impl with simsimd::i8::dot would
  shrink memory 4× without measurable accuracy loss (see
  `bench-accuracy.md`).
- For 1500-fact synthetic-style workloads, BM25 prefilter still
  wins on simplicity. Keep both paths; default = HNSW only when
  corpus exceeds some threshold (e.g. 5 000 facts).
