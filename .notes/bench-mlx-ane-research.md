# ANE / MLX vs TEI Metal — research only (no measurement)

Asked before spending time on additional reranker / multi-model
benchmarks: would Apple Neural Engine or MLX produce a meaningful
speedup over the existing TEI-with-Metal path?

## Verdict

**Stay on TEI native (Metal).** None of the alternatives produce
a >2× win on the encoder sizes wg cares about, and most cost
non-trivial integration time.

## Why ANE is wrong for our workload

- **No production server.** [ANEMLL](https://github.com/Anemll/Anemll)
  is the closest project — beta-quality, decoder-LLM focused, not
  encoders. There is no `apple/coreml-*` repo for
  `all-MiniLM-L6-v2` or BGE.
- **The dispatch overhead kills the win.** ANE pays ~1–3 ms just
  to schedule a forward pass on the engine, on top of the actual
  compute. For a 22 M-param encoder that already hits ~4 ms p50
  on Metal, that's >25 % overhead before you've even started.
  ANE wins on **power** (~2 W vs ~20 W) and on **convolution-heavy
  decoders** with 100s of tokens — single-shot encoder embeds
  aren't its strength.
- **macOS 26.3 silently reroutes `compute_units = ALL` to GPU**
  unless you use private batch APIs. Even getting the model onto
  ANE takes engineering.

## Why MLX is roughly tied with Metal for embeddings

- The arxiv paper [2510.18921](https://arxiv.org/html/2510.18921v1)
  benchmarks MLX-GPU within noise of Metal-via-MPS for BERT-class
  encoders. The 21–87 % MLX advantage published elsewhere
  ([LM Studio post](https://lmstudio.ai/blog/unified-mlx-engine))
  is a **decoder generation** number — long autoregressive
  sequences amortize MLX's unified-memory layout. Encoder embeds
  don't.
- 4 ms p50 means we are already **dispatch-bound, not
  compute-bound**. Swapping frameworks moves nothing at the
  bottom; you'd need a fundamentally different IO model
  (kernel-side batching, `keep-alive` HTTP, etc.) to push past it.

## Where MLX *would* be interesting: rerankers

TEI's cross-encoder coverage on macOS is sparser than its
embedding coverage. Two MLX-native servers actually do
production-style reranking today:

- **[MemTensor/mlx-memos](https://github.com/MemTensor/mlx-memos)** —
  ships `bge-m3` embeddings + `bge-reranker-v2-m3` rerank in one
  OpenAI-compatible server.
- **[jundot/omlx](https://github.com/jundot/omlx)** — menu-bar
  server, loads LLMs/VLMs/embeddings/**rerankers** in one process
  at `localhost:8000/v1`.

Worth testing if/when wg ships a recommendation around a specific
reranker model that's MLX-friendly. For now, the existing TEI
rerank path measured at 78 ms p50 (top_k=8) is acceptable.

## Where to spend the time instead

1. **Client-side batching.** TEI batches 1→32 inputs in one HTTP
   call drop per-vector cost ~5–10× — way bigger lever than any
   backend swap. wg's ingest already uses `provider.embed_batch`,
   but search-time queries are batch=1. Multi-query workloads
   (e.g. fact_add_many ingest of N items) should pre-batch.
2. **Document the Apple Silicon recipe** in the README: just
   `cargo install --git github.com/huggingface/text-embeddings-inference
   --features metal text-embeddings-router`, skip Docker.
3. Move on to the actually-pending work: rerank accuracy on
   MIRACL/ko, more embedding-model coverage, etc.

## Sources

- [Blaizzy/mlx-embeddings](https://github.com/Blaizzy/mlx-embeddings)
- [jakedahn/qwen3-embeddings-mlx](https://github.com/jakedahn/qwen3-embeddings-mlx)
- [cubist38/mlx-openai-server](https://github.com/cubist38/mlx-openai-server)
- [MemTensor/mlx-memos](https://github.com/MemTensor/mlx-memos)
- [jundot/omlx](https://github.com/jundot/omlx)
- [jina-ai/mlx-retrieval](https://github.com/jina-ai/mlx-retrieval)
- [Anemll](https://github.com/Anemll/Anemll)
- [InsiderLLM: ANE for LLM inference](https://insiderllm.com/guides/apple-neural-engine-llm-inference/)
- [arxiv 2510.18921 — On-device ML on Apple Silicon with MLX](https://arxiv.org/html/2510.18921v1)
- [LM Studio MLX engine](https://lmstudio.ai/blog/unified-mlx-engine)
