# Bench: MIRACL/ko — real Korean Wikipedia retrieval

First measurement against a real, public IR benchmark. Prior runs
all used synthetic 1500-fact wikis with template-y prose. MIRACL
(WSDM 2023 Cup) gives us 213 dev queries against actual
Wikipedia passages with human-judged qrels.

## Setup

- **Source**: `miracl/miracl` v1.0 (HuggingFace), Korean subset.
- **Corpus**: 503 relevance-judged Wikipedia passages + 5 000 random
  passages from the same Wikipedia dump (5 503 total). All
  503 ground-truth answers are present.
- **Queries**: 213 dev queries with qrels (median 2 relevant docs
  per query, range 1–12).
- **K**: 10 (matches typical IR benchmarks).
- **Hybrid stack**: BM25 + semantic via `model2vec-native`,
  hybrid_search RRF fusion, semantic_prefilter cap 50.

## Pipeline correctness gotcha (worth documenting)

First-pass measurement returned **P@10 = 0.000 across all three
models**. Cause: `wg import` of the corpus assigned its own ULIDs
to facts, ignoring the `id` field in the import JSONL. Our
`docid_to_fid` map was built from the IDs we *wrote* into the JSONL,
not the IDs wg actually stored. Every expected ID in the golden
set was therefore wrong.

Fix: rebuild the map from `wg fact list --json` after import,
keying off the `source` field (which `wg import` *does* preserve).

Lesson: round-trip every external mapping through wg's actual
output before trusting it.

## Results — model comparison (hybrid default, prefilter=50)

| Model | P@10 | R@10 | p50 | CPU total | wg heap |
|---|---:|---:|---:|---:|---:|
| `potion-base-4M` (English-only) | 0.157 | 0.633 | 191 ms | 54.3 s | 65 MB |
| **`potion-multilingual-128M`** | **0.175** | **0.706** | 196 ms | 47.6 s | 315 MB |
| `qwen3-embedding:0.6b` (Ollama) | *measuring* | *measuring* | — | — | 5 MB + 1.2 GB Ollama |

Multilingual 128M lifts P@10 by +11% and R@10 by +12% over the
English-only 4M on real Korean text — the first time in this
project that the multilingual penalty pays off. The earlier
*synthetic* Korean bench tied the two models because BM25 caught
the keyword match; on real Wikipedia prose with paraphrased
queries the semantic model has to actually understand Korean.

## Signal decomposition (potion-128M, hybrid family)

| Condition | P@10 | R@10 | p50 |
|---|---:|---:|---:|
| BM25 only (`semantic_weight=0`) | 0.154 | 0.639 | 198 ms |
| Semantic only (`bm25_weight=0`) | 0.174 | 0.705 | 192 ms |
| **Semantic brute-force** (`semantic_prefilter=0`) | **0.177** | **0.730** | 381 ms |
| Hybrid default (BM25 + semantic) | 0.175 | 0.706 | 205 ms |
| Hybrid `semantic_prefilter=200` | 0.178 | 0.717 | 232 ms |

**Interpretation:**

1. **BM25 underperforms semantic on Korean** (R@10 0.639 vs 0.705).
   `bm25-0.2` uses whitespace tokenization, which can't decompose
   Korean morphology. A char-n-gram or KOREAN morpheme analyzer
   would close the gap.

2. **Hybrid ≈ semantic-only.** BM25 is contributing little to
   the fused result on Korean — RRF mostly weights the same
   semantic ranking either way.

3. **Brute-force semantic is the recall ceiling.** Scoring all
   5 503 candidates lifts R@10 from 0.706 to 0.730 (+3.5%) at
   2× the latency. The BM25 prefilter is silently dropping a
   real fraction of correct answers from the candidate pool.

## What this proves about HNSW

This is the **first workload where HNSW would be ROI-positive**:

- Brute-force semantic gives the best recall (0.730) but pays
  381 ms p50 (almost 2× the prefiltered hybrid).
- HNSW would deliver brute-force-quality candidates in ≪50 cosine
  comparisons → close to prefilter-50 latency.
- Synthetic benches showed near-zero HNSW ROI because BM25 prefilter
  already caught everything it needed to. Real Korean retrieval is
  the regime where prefilter loses recall.

Estimated effort to add HNSW backed by `instant-distance` or
`hnsw_rs`: ~1–2 days including persistence (sidecar like the i8
quantization cache), warm-up, and config plumbing.

## Other observations

- **CPU**: `model2vec` 47–55 s for 213 queries × ~50 candidates.
  Embedding inference dominates; cosine itself is sub-ms via simsimd.
- **Latency**: 4M and 128M tie within noise (~190 ms p50).
  Bigger model = bigger query embedding compute, but model load
  + Korean tokenization are the heavy steps regardless of model
  size — the matrix lookup is cheap.
- **Heap**: 4M still wins by 5× on memory (65 MB vs 315 MB).
  If you can tolerate the 12% recall hit, 4M is the better
  Pareto pick on Korean Wikipedia too.

## Caveats

- 5 503-doc corpus is a 0.4% subsample of MIRACL/ko's full
  1.5 M passage corpus. Recall metrics will move on the full
  corpus — likely lower for both BM25 and semantic, with
  semantic widening its lead because the candidate pool grows.
- We compared at K=10 because that matches MIRACL's
  evaluation. nDCG@10 was not computed (wg bench reports P/R only).
- Hybrid graph_prefilter has zero effect on this corpus (no
  relations between facts), as expected. The earlier synthetic
  result that flagged graph as "marginal" still holds.

## Next steps queued

- HNSW backend behind `search.semantic_index = "hnsw"` config
  toggle (sidecar persistence; build once on `wg ingest`).
- Korean BM25 — try `lindera` or char-n-gram tokenization to
  see if BM25 R@10 closes vs semantic.
- Larger MIRACL subset (50k or full 1.5M) once HNSW is in.
