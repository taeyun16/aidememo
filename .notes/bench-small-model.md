# Bench: small-model comparison (potion-base-4M vs potion-multilingual-128M)

Companion to `bench-accuracy.md`. Same 1500-fact synthetic wiki,
same `bench_gold_easy.jsonl` / `bench_gold_hard.jsonl`. We swap
only `config.model.name` between runs.

## Results

### EASY (10 queries, K=20) — entity name in query

| Model | P@20 | R@20 |
|---|---:|---:|
| `minishlab/potion-multilingual-128M` (default) | 1.000 | 0.400 |
| `minishlab/potion-base-4M` | 1.000 | 0.400 |

Easy queries saturate; the floor is identical.

### HARD (10 queries, K=50) — semantic-only

| Model + config | P@50 | R@50 | p50 | CPU | Heap | RSS |
|---|---:|---:|---:|---:|---:|---:|
| 128M f32 mmap (default) | 0.062 | 0.041 | 23.5 ms | 1407 ms | 315 MB | 554 MB |
| 128M + i8 quantized | 0.054 | 0.036 | 23.4 ms | 1400 ms | 315 MB | 553 MB |
| **4M f32 mmap** | **0.122** | **0.083** | **20.2 ms** | **394 ms** | **10 MB** | **38 MB** |
| 4M + i8 quantized | 0.120 | 0.082 | 20.8 ms | 405 ms | 11 MB | 38 MB |

## What this proves

**The smaller, English-only model is better on every axis** for this
synthetic English-language workload.

| Metric | 4M vs 128M | Δ |
|---|---|---|
| HARD precision (P@50) | 0.122 vs 0.062 | **+97%** |
| HARD recall (R@50) | 0.083 vs 0.041 | **+102%** |
| Process heap peak | 10 MB vs 315 MB | **-97%** |
| Process RSS peak | 38 MB vs 554 MB | **-93%** |
| CPU user time | 394 ms vs 1407 ms | **-72%** |
| Latency (p50) | 20.2 ms vs 23.5 ms | -13% |
| i8 quantization accuracy loss | -2% vs -13% | **6× less** |

`potion-multilingual-128M` spreads the same parameter budget across
many languages, so for English-only retrieval its capacity is roughly
halved compared to a model trained exclusively on English. The 4M
build is also small enough that its embedding rows have lower
quantization variance — i8 conversion costs only ~2% precision
instead of 13%.

## Recommendations

- **Change the default model** for English-language wikis to
  `minishlab/potion-base-4M`. It's strictly Pareto-better here.
- Document `potion-multilingual-128M` as the opt-in for wikis
  containing non-English content.
- Stop quoting "i8 quantization loses <1% accuracy". The actual
  number depends heavily on the model:
  - 128M-multilingual: -13% on hard queries
  - 4M-base: -2% on hard queries
  Both are workload-dependent and should be measured per project.

## Caveats

- Synthetic data with template-y prose. Real wikis have richer
  semantic neighborhoods; absolute P@K will move, but the
  4M-vs-128M ratio likely holds for English content.
- We didn't try `potion-base-8M` or `potion-base-32M`. Those are
  intermediate and could land at a better English-only quality
  ceiling than 4M without paying the 128M memory cost. Worth a
  follow-up run.
