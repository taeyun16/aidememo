# HNSW integration (Tier 8) — measured in-tree

The standalone prototype (`bench-hnsw.md`) showed HNSW could match
brute-force recall at HNSW latency. This commits the integrated
version into wg-core and re-measures end-to-end against the same
MIRACL/ko goldens that exposed the BM25-prefilter recall gap.

## Path through the code

```
WikiGraph::hybrid_search
└── if config.search.semantic_index == "hnsw"
     and sidecar exists and matches provider:
       hybrid_search_with_hnsw
         ├── BM25 search (still feeds RRF)
         ├── query embed → L2 normalize
         ├── HnswIndex::search(top = prefilter * 2)
         ├── semantic_search(candidates = HNSW result)
         └── rrf_fusion(BM25, semantic)
     else:
       hybrid_search_with_ctx  (existing BM25-prefilter path)
```

If the sidecar is missing or built against a different model, we
log to stderr and silently fall back. No search ever fails because
the index isn't ready.

## End-to-end measurements

Same setup as the prototype: 213 MIRACL/ko dev queries, 5 503-doc
corpus, `potion-multilingual-128M`. wg release build via
`wg bench --json`.

| `search.semantic_index` | P@10 | R@10 | p50 | CPU 213q |
|---|---:|---:|---:|---:|
| `bm25` (default) | 0.175 | 0.706 | 187 ms | 47.5 s |
| **`hnsw`** | **0.192** | **0.791** | 215 ms | 76.9 s |
| Δ | **+9.7%** | **+12.0%** | +28 ms | +29 s |

The integrated path beats the standalone prototype's 0.176 / 0.724
because it keeps the BM25 ranking in RRF fusion alongside the HNSW
candidates — so we get the union of both signals, not just the
HNSW set.

## Regression check on the 1500-fact synthetic English wiki

| Golden | `bm25` P/R | `hnsw` P/R |
|---|---|---|
| EASY (K=20, entity name in query) | 1.000 / 0.400 | 1.000 / 0.400 |
| HARD (K=50, semantic only) | 0.062 / 0.041 | 0.060 / 0.040 |

Tied within noise. Synthetic English data was the regime where the
BM25 prefilter was already optimal, so HNSW adds nothing — but
critically, it doesn't regress either. The +28 ms p50 latency cost
shows up here too (`bm25` 25 ms vs `hnsw` ~30 ms).

## Sidecar shape

Persisted next to the redb store as `wiki.hnsw.bin`. For the
5 503-fact MIRACL corpus with potion-128M (256-dim):

```
/tmp/wg-bench-miracl/_meta/
  wiki.redb       3.7 MB
  wiki.hnsw.bin   7.6 MB   ← new
```

Size scales linearly with `(facts × dim × 4 bytes)` plus HNSW
graph overhead (~2× the raw vector matrix in our experience). The
header (model name + dim + count) is checked at load time; mismatch
triggers a silent fallback and an stderr warning.

## Build cost

Auto-rebuilt at the end of `wg ingest` whenever
`search.semantic_index = "hnsw"`. Cost is dominated by embedding
inference: ~1 ms per fact for `model2vec`, ~100 ms per fact for
HTTP-backed providers like Ollama. For 5 503 facts on the
multilingual model: 5–6 seconds end to end.

### Incremental embedding cache (Tier 8 follow-up)

`vector_index_rebuild` reuses cached vectors from the existing
sidecar (or in-memory index) when the model + dimension still
match. Embedding inference is the long pole — skipping it on
unchanged facts collapses a no-op rebuild from 3.7s to ~2.2s on
the 5503-fact MIRACL/ko corpus.

Measured by `cargo run --release --bin hnsw_rebuild_cache`:

```
  cold (no cache):        3.65s  n=5503
  warm (in-mem cache):    1.75s  n=5503   -52%
  warm (disk cache):      2.20s  n=5503   -40%
```

In-memory cache hits avoid the bincode deserialize, so they're
the fastest path. Disk-cache hits still pay the deserialize cost
but skip the embed_batch — the dominant win. For HTTP providers
(Ollama, OpenAI), the speedup multiplier is much larger because
the embed step there is 50–100× more expensive than model2vec.

There's still no `wg vector-rebuild` standalone command — for
now operators trigger a rebuild by re-running `wg ingest`. That's
the next obvious follow-up.

## Trade-offs to keep in mind

- **+15% query latency** at this corpus size. The win is recall,
  not speed; if you're latency-bound and your data is keyword-
  matchable (synthetic English, technical docs with consistent
  terminology), `bm25` is still the right default.
- **Sidecar drift.** Adding/updating facts marks the index stale
  but doesn't auto-update — you have to re-ingest. Acceptable for
  the typical `wg ingest` workflow but a footgun for `wg fact add`
  loops. Incremental insert is a natural follow-up.
- **F32 only.** The i8-quantized fact_embed_cache from Tier 7-C
  isn't reused here — HNSW gets a separate copy. A future
  `Point<i8>` impl with `simsimd::i8::dot` would shave 4× memory
  off the sidecar.

## Recommendation for ops

Default `search.semantic_index = "bm25"` ships unchanged. Document
turning it on for:
- Korean / Japanese / Chinese / other morphologically-rich
  languages where whitespace BM25 underperforms.
- Wikis with paraphrase-heavy queries (where keywords don't match
  exactly).
- Corpora over ~5 k facts where brute-force semantic was getting
  too slow.

Else stick with BM25. The +28 ms isn't free.
