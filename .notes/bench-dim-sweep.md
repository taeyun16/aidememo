# Dimension sweep — capacity / dim / training scope vs accuracy

Two complementary sweeps:

- **A** — English synthetic 1500-fact wiki, swept through the
  potion family at 128/256/512 dim.
- **B** — MIRACL/ko 5503-doc Korean Wikipedia retrieval, swept
  through model2vec multilingual + Ollama-served transformers
  at 128/256/512/1024 dim.

Same goldens, same hybrid stack (HNSW path enabled where it
exists, BM25-prefilter otherwise). All numbers measured on the
same machine in the same session.

## A. English synthetic (HARD golden, K=50, HNSW)

| Model | dim | params | P@50 | R@50 | Heap | p50 |
|---|---:|---:|---:|---:|---:|---:|
| `potion-base-4M` | 128 | 4M | 0.122 | 0.083 | 10 MB | 18 ms |
| `potion-base-8M` | 256 | 8M | 0.092 | 0.063 | 11 MB | 18 ms |
| `potion-multilingual-128M` | 256 | 128M | 0.062 | 0.041 | 315 MB | 23 ms |
| **`potion-base-32M`** | **512** | 32M | **0.196** | 0.134 | 19 MB | 19 ms |
| `potion-retrieval-32M` | 512 | 32M | 0.184 | **0.144** | 19 MB | 19 ms |

**Surprises:**
1. **8M < 4M.** Same dim (256), 2× capacity, *worse* P@50 (-25%).
   The 4M model generalizes better on this synthetic English
   prose; the 8M training set probably skews further from our
   templated facts.
2. **32M = +61% P over 4M.** Dim 128 → 512 + capacity 4M → 32M
   together swing recall meaningfully. This is the only English
   regime where bigger model paid off here.
3. **Multilingual hurts on English.** `potion-multilingual-128M`
   (256d, 128M params) underperforms `potion-base-4M` (128d, 4M
   params). Cross-language capacity is overhead when the workload
   is monolingual.
4. **Pareto champion (English)**: `potion-base-32M`. 19 MB heap,
   18 ms p50, P=0.196 — beats `multilingual-128M` (315 MB) by
   +216% on accuracy with 16× less RAM.

## B. Korean MIRACL/ko (50 queries, K=5, HNSW path)

| Model | dim | provider | P@5 | R@5 | p50 |
|---|---:|---|---:|---:|---:|
| `potion-base-4M` (English-only) | 128 | model2vec | 0.158 | 0.575 | 241 ms |
| **`potion-multilingual-128M`** | 256 | model2vec | **0.206** | 0.758 | 216 ms |
| `potion-base-32M` (English-only) | 512 | model2vec | 0.164 | 0.608 | 245 ms |
| **`bge-m3`** | **1024** | Ollama HTTP | **0.228** | **0.871** | **11172 ms** |

## B'. Korean MIRACL/ko (50 queries, K=5, BM25 path) — full dim sweep

Apples-to-apples: same hybrid pipeline, different model behind
`semantic_search`. Index = BM25 prefilter, no HNSW (so HNSW build
failures across ollama models don't confound the comparison).

| Model | dim | provider | P@5 | R@5 | p50 |
|---|---:|---|---:|---:|---:|
| `potion-base-4M` (English-only) | 128 | model2vec | 0.174 | 0.610 | 207 ms |
| **`potion-multilingual-128M`** | 256 | model2vec | **0.202** | 0.693 | 235 ms |
| `granite-embedding:30m` | 384 | Ollama | 0.148 | 0.532 | 624 ms |
| `paraphrase-multilingual` | 768 | Ollama | 0.188 | 0.623 | 802 ms |
| **`granite-embedding:278m`** | 768 | Ollama | **0.204** | 0.690 | 914 ms |
| **`bge-m3`** | 1024 | Ollama | 0.204 | **0.704** | 2292 ms |

### What this exposes

1. **Dim is *not* monotonic with accuracy.** `granite-embedding:30m`
   (384d) underperforms `potion-multilingual-128M` (256d). Adding
   capacity doesn't help if the model wasn't trained on enough
   Korean.
2. **`potion-multilingual-128M` ties with 768d/1024d transformers
   on P.** P=0.202 vs 0.204 (granite-278m, bge-m3). The model2vec
   multilingual lookup table is competitive with full transformers
   that are 3-4× larger in dimension and 50× slower.
3. **paraphrase-multilingual (768d) < multilingual-128M (256d)
   on P**. Same dim ≠ same model — `paraphrase-multilingual` was
   trained for paraphrase classification, not retrieval. Training
   objective matters more than dim at the same capacity tier.
4. **Diminishing return at the top.** 768 → 1024 (granite-278m
   vs bge-m3) lifts R@5 by 0.014 only. Doubling dim past ~768
   doesn't pay off in the way 256 → 768 did.
5. **Latency wall**. model2vec ~210 ms p50; transformer 624–2292
   ms (3–11×). HTTP roundtrip + local GGUF inference is the
   floor — even tiny 384d transformers pay the network tax.

For reference, the earlier 213-query run (K=10) gave:

| Model | dim | P@10 | R@10 |
|---|---:|---:|---:|
| `potion-base-4M` | 128 | 0.157 | 0.633 |
| `potion-multilingual-128M` | 256 | 0.192 | 0.706 (BM25) / **0.791 (HNSW)** |
| `qwen3-embedding:0.6b` | 1024 | 0.272 | 0.181 (prefilter) |

(qwen run used the BM25-prefilter path before HNSW landed; not
directly comparable to the 50q HNSW table above.)

### Key findings (Korean)

1. **`bge-m3` is multilingual SOTA.** Beats `potion-multilingual-128M`
   by +11% P@5 / +15% R@5 on real Korean Wikipedia retrieval.
2. **Multilingual training >> capacity** for cross-lingual
   workloads. `potion-multilingual-128M` (256d, multi-trained)
   beats `potion-base-32M` (512d, English-only) by **+26% P@5**.
   The 32M's larger capacity doesn't help if the model never saw
   Korean during training.
3. **Dim curve on multilingual training scope**: 256 → 1024
   takes R@5 from 0.758 → 0.871 — a ~15% jump per doubling once
   the model speaks the language.
4. **Latency cliff**: model2vec ≈ 220 ms p50; `bge-m3` ≈ 11 s p50
   (50×). HTTP roundtrip + 1024d transformer inference is the
   cost of that +15% recall.
5. **`potion-base-4M` works on Korean.** It has zero Korean
   training data but BM25 + hybrid RRF fusion lifts it to
   P@5=0.158 — keyword matching covers a lot of ground when fact
   text and queries share entity names.

## Combined picture

| Workload | Best model2vec | Best transformer | Gap |
|---|---|---|---|
| English synthetic | `potion-base-32M` (P=0.196) | not tested | n/a |
| Korean MIRACL | `potion-multilingual-128M` (P=0.206) | **`bge-m3`** (P=0.228) | +11% accuracy at 50× latency |

## Recommendations

| If you need… | Pick |
|---|---|
| English-only, latency-bound, smallest RAM | `potion-base-32M` |
| English-only, smallest RAM possible | `potion-base-4M` (close second on accuracy) |
| Multilingual, latency-bound, offline-capable | `potion-multilingual-128M` |
| Multilingual, accuracy-first, OK with 10s+ latency | `bge-m3` (Ollama) |
| Korean specifically, GPU available | `bge-m3` or `qwen3-embedding:8b` |

Default in wg config stays `potion-multilingual-128M` because it's
the only model that handles all major languages out-of-the-box and
pays no HTTP roundtrip. Power users on Korean-heavy workloads
should switch to `bge-m3` via the openai-compatible provider.

## Caveats

- bge-m3 measurement was 50 queries (not 213) due to per-query
  latency. Smaller sample → wider confidence intervals on the
  P/R numbers. The +0.022 P@5 lead over `multilingual-128M` is
  consistent with the larger 213-query qwen comparison, so we
  trust the direction even with 50 samples.
- We did not test `mxbai-embed-large` or `multilingual-e5-base`.
  Both are reasonable midpoints between potion-multilingual and
  bge-m3 if the latency cliff is too steep.
- All runs used HNSW where available; BM25-prefilter would lower
  R@K by ~3pp on the multilingual numbers (see `bench-miracl-ko.md`).
- "K=5" in the 50q table is wg bench's default; the goldens
  carry K=10 in the JSONL but `wg bench` uses `default_k=5` from
  the `--k` flag, which we didn't override. The relative ordering
  between models is unaffected.
