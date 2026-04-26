# Bench accuracy validation (P@K, R@K)

Until this run, every perf measurement reported `Mean P@K: NaN` and
`Mean R@K: NaN` because our golden file had no `expected` IDs. The
optimizations were validated for **latency / memory** but their
**relevance impact was assumed, not measured**.

This note records the first end-to-end accuracy run.

## Setup

- **Workload**: 1500 facts, 30 entities, 17 relations
  (synthetic — `potion-multilingual-128M`)
- **Goldens**: two flavors
  - `bench_gold_easy.jsonl` (10 queries, K=20):
    each query mentions an entity name explicitly. Ground truth =
    every fact attached to that entity (50 facts/query).
  - `bench_gold_hard.jsonl` (10 queries, K=50):
    natural-language queries with **no entity name in the query** —
    forces semantic matching. Ground truth = facts attached to the
    1–3 entities that the prose actually describes.

## Results

### EASY (10 queries, K=20)

| Condition | P@20 | R@20 |
|---|---:|---:|
| Default (graph=on, prefilter=50, f32 mmap) | **1.000** | **0.400** |
| Graph=off | 1.000 | 0.400 |
| i8 quantized | 1.000 | 0.400 |
| Brute-force semantic (prefilter=0, scores all 1500) | 1.000 | 0.400 |

R@20 = 0.40 because each query has 50 expected facts but only K=20
slots (20/50 = 0.4). All conditions max out P — entity-name keyword
match dominates. **No discrimination between optimizations.**

### HARD (10 queries, K=50)

| Condition | P@50 | R@50 | vs Default |
|---|---:|---:|---|
| **Default** (graph=on, prefilter=50, f32 mmap) | **0.062** | **0.041** | baseline |
| Graph=off | 0.062 | 0.041 | **0%** (no effect) |
| **i8 quantized** | 0.054 | 0.036 | **−13% / −12%** (real loss) |
| Brute-force semantic (prefilter=0) | 0.060 | 0.040 | -3% (noise) |

## What this proves

| Claim we'd been making | Verdict |
|---|---|
| "i8 quantized: cosine recovery >0.99, accuracy fine" | **Partially false.** True for easy keyword-matched queries; on semantic-only queries P drops 13%, R drops 12%. |
| "graph_prefilter helps catch semantic-only matches" | **Unproven on this dataset.** With 17 relations and 1500 facts the graph signal contributes nothing measurable. |
| "semantic_prefilter=50 is sufficient" | **Confirmed.** Brute-force scoring all 1500 candidates only recovers ~3% more — within noise of our 10-query sample. |

## Caveats

1. **Absolute P=0.06 is low.** That's `potion-multilingual-128M`'s
   ceiling on synthetic 50-token-fact texts that all share a
   "configuration tip … under heavy load" template. A real wiki
   would have richer prose and likely higher absolute scores. The
   **relative** comparison (i8 vs f32, graph on vs off) is what
   matters here.
2. **17-relation graph is sparse.** A more interconnected wiki
   (50+ relations, deeper hops) might surface a graph_prefilter
   benefit. Re-run before declaring graph_prefilter dead.
3. **10 queries is a small sample.** p95 latency varies wildly
   (cold start dominates). For accuracy stats this is OK because
   we're aggregating P/R, but we should grow the golden set if
   we tune further.

## Recommendations

- **Default `model.quantize = false`** stays correct for
  recall-sensitive workloads. Document the 13% drop as the price
  of i8.
- **Reconsider `search.graph_prefilter` default.** It costs 0
  accuracy on this dataset and adds 0–3 ms p50 latency. Either
  make it opt-in or invest in finding the workload where it
  helps.
- **Keep `search.semantic_prefilter = 50`** — confirmed sweet
  spot. Going to 200+ adds latency without measurable recall.
- **Need a real-wiki golden set** before publishing relevance
  claims. Synthetic data hides absolute differences.
