# TEI Docker (arm64 emulation) vs Native (Metal) — overhead

Apple Silicon (M-series) arm64 macOS, OrbStack Docker (11.72 GiB
allocated), TEI v1.9.3. Same models, same machine, same probe
script. Single-threaded HTTP loop, 50 runs after 5-warmup, p50/p95
reported.

The HuggingFace TEI registry only ships an **amd64** image, so on
arm64 macOS the Docker path runs the binary under Rosetta/QEMU
emulation. The native install is `cargo install --git
github.com/huggingface/text-embeddings-inference --features metal
text-embeddings-router`, which builds the arm64 binary with
Metal GPU acceleration enabled.

## Embedding — `sentence-transformers/all-MiniLM-L6-v2` (22 M params, 384 d)

| metric | Docker (amd64 emul, fp32) | Native (Metal, fp16) | speedup |
|---|---:|---:|---:|
| `/embed` batch=1 p50 | 7.1 ms | **4.3 ms** | 1.6× |
| `/embed` batch=1 p95 | 10.2 ms | 6.5 ms | 1.6× |
| `/embed` batch=8 p50 | 29.3 ms | **17.0 ms** | 1.7× |
| `/embed` batch=32 p50 | 104.5 ms | **36.7 ms** | **2.8×** |
| RSS (steady) | 273 MiB | **117 MiB** | 2.3× less |
| dtype | fp32 | fp16 | — |

## Rerank — `BAAI/bge-reranker-base` (278 M params, sequence classification)

| top_k | Docker (amd64 emul, fp32) | Native (Metal, fp16) | speedup |
|---|---:|---:|---:|
| 4 | 232.7 ms p50 | **39.8 ms p50** | 5.8× |
| 8 | 394.2 ms p50 | **78.5 ms p50** | 5.0× |
| 16 | 794.3 ms p50 | **158.9 ms p50** | 5.0× |
| 32 | 1511.8 ms p50 | **363.7 ms p50** | **4.2×** |
| RSS | 2.97 GiB | **20 MiB process + GPU mem** | ~10× less¹ |

¹ Native uses Metal for the model weights, so the bulk of the
278 M-param model lives in unified GPU memory (~280 MiB) instead
of the process RSS. The Docker container has to keep everything
in CPU RAM under fp32, which is what produces the 2.97 GiB number.
Either way the practical footprint difference is ~10×.

## Findings

1. **Docker on arm64 macOS isn't viable for production search.**
   Docker is 1.6–2.8× slower for embeddings and **4–6× slower** for
   rerank, with 2–10× more memory. The `cpu-1.9` image runs under
   amd64 emulation; HuggingFace doesn't publish an arm64 build.

2. **The default `rerank.top_k = 32` is too aggressive.** Even on
   native Metal, top_k=32 adds **364 ms p50** to every hybrid_search
   call — far above the 5 ms target the rest of the pipeline holds.
   Better defaults:

   | top_k | added latency (native) | use case |
   |---|---:|---|
   | 4 | 40 ms | latency-sensitive (still ~10× the BM25/HNSW baseline) |
   | 8 | 78 ms | reasonable for interactive search |
   | 16 | 159 ms | recall-heavy retrieval |
   | 32 | 364 ms | offline / batch only |

   The rerank cost is roughly linear at **~10 ms / pair on Metal**
   (and ~50 ms / pair on amd64 emulation), so users can budget by
   counting candidates.

3. **`max_client_batch_size = 32` is the upstream cap.** TEI rejects
   `texts.length > 32` with HTTP 422. wg's `rerank.top_k` must be
   ≤ 32 (or the user must redeploy TEI with `--max-client-batch-size`
   raised).

## Recommendations for wg defaults

* Lower `default_rerank_top_k` from **32 → 8**. The 78 ms cost on
  native is acceptable for interactive search; users explicitly
  opt up if they want more thorough rerank.
* Document the Docker overhead and recommend `cargo install` for
  any arm64 macOS user. On Linux x86_64 the Docker path probably
  matches native, but we haven't measured that yet.
* `text-embeddings-router` 1.9.3 on this machine compiles cleanly
  with `--features metal` in ~10 min and produces a 26 MiB binary
  that Just Works.

## Reproducer scripts (in `/tmp/wg-tei-bench/`)

* `probe.py` — `/embed` p50/p95 sweep at multiple batch sizes
* `rerank_probe.py` — `/rerank` p50/p95 sweep at multiple top_k

Both are ~80-line stdlib-only Python; no Rust changes shipped from
this round of measurement.
