# Bench: 3-way embedding model comparison

`potion-base-4M` (English-only, 4 MB) vs `potion-multilingual-128M`
(multilingual, 489 MB) vs `qwen3-embedding:0.6b` via Ollama
(general-purpose, ~600 MB external).

Same 1500-fact wiki, same goldens, same hybrid (BM25 + semantic +
graph) and prefilter settings.

## English wiki (synthetic, 30 entities)

| Condition | 4M base | 128M multi | **qwen 0.6b** |
|---|---:|---:|---:|
| EASY P@20 | 1.000 | 1.000 | 1.000 |
| EASY R@20 | 0.400 | 0.400 | 0.400 |
| **HARD P@50** | 0.122 | 0.062 | **0.272** |
| **HARD R@50** | 0.083 | 0.041 | **0.181** |
| p50 latency | 20 ms | 23 ms | **93 ms** |

On the hard semantic-only set, qwen 0.6b lifts P@50 by +123% vs 4M
and +338% vs 128M. The multilingual 128M underperforms 4M because
its parameter budget is split across many languages and English
retrieval gets only a fraction of capacity.

## Korean wiki (synthetic, 30 entities, Korean prose facts)

| Condition | 4M base | 128M multi | **qwen 0.6b** |
|---|---:|---:|---:|
| EASY P@20 | 1.000 | 1.000 | 1.000 |
| HARD P@50 (keywords match facts) | 0.400 | 0.400 | 0.400 |
| **PARAPHRASE P@50** (synonyms) | 0.110 | 0.130 | **0.272** |
| **PARAPHRASE R@50** | 0.110 | 0.117 | **0.199** |
| p50 latency | 25 ms | 23 ms | ~115 ms |

When fact and query share keywords, BM25 carries the search and
all three models tie. Paraphrase queries reveal the semantic
difference — qwen 0.6b gains +147% precision vs 4M.

## Cost-of-quality comparison

| | wg-side heap | external RAM | Network | Offline | Latency p50 |
|---|---:|---:|---|:---:|---:|
| **4M base** | **10 MB** | 0 | none | ✓ | **20 ms** |
| 128M multilingual | 315 MB | 0 | none | ✓ | 23 ms |
| qwen 0.6b (Ollama) | 5 MB | ~600 MB | localhost | ✓ (with Ollama) | 90–120 ms |
| qwen 8b (Ollama, untested) | 5 MB | ~5 GB | localhost | ✓ | several × |

`qwen 0.6b` puts the model in a separate process (Ollama), so wg's
own heap shrinks to ~5 MB (HTTP client only). The 600 MB of RAM
moves *out of wg* but is still on your machine — call it
"distributed across processes" rather than "saved".

## Decision matrix

| Workload | Recommendation |
|---|---|
| English-only docs, predictable phrasing | **4M base** — fastest, smallest, ties on accuracy |
| English-only with paraphrase / synonym search | **qwen 0.6b** if 600 MB external RAM + Ollama daemon is acceptable; otherwise 4M |
| Korean docs, term-stable phrasing | **4M base** ties with multilingual. Pick smaller. |
| Korean docs, heavy paraphrase / cross-language | **qwen 0.6b** is +147% better than the model2vec options |
| Mixed languages, no Ollama allowed | **128M multilingual** (only true offline + multi-language path) |
| Latency-critical (≤30 ms hard cap) | model2vec only. qwen's 90+ ms p50 won't fit. |

## Caveats

- All tests use synthetic 50-fact-per-entity prose. Real wikis
  have richer text and may shift these numbers in either direction.
- 10 paraphrase queries / set is small; treat ratios with grain
  of salt, but the **2× gap** between qwen and the model2vec
  family is large enough that noise won't flip it.
- qwen latency includes Ollama HTTP roundtrip + GGUF inference.
  First query (cold model load) is ~30 s; subsequent calls are
  ~100 ms. We took p50 to hide cold-start.
- We didn't test `qwen 3-embedding:8b`. Likely better accuracy at
  ~5 GB external RAM and ~10× the latency. Worth a follow-up if
  the use case justifies it.
- BM25 + hybrid RRF is doing a lot of heavy lifting in our setup.
  A pure-semantic config (BM25 disabled) would amplify the qwen
  advantage further but is rarely the right deployment choice.
