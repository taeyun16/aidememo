# Changelog

## [Unreleased]

### Added

- **TEI integration** — first-class support for HuggingFace
  text-embeddings-inference, both as embedding source and as a
  cross-encoder reranker:
  - `model.provider = "tei"` uses TEI's native `/embed` endpoint
    and auto-discovers model id + dimension via `GET /info`
    (falls back to a one-shot probe if `/info` isn't available).
    The existing OpenAI-compat path (`model.provider = "openai"`)
    still works for TEI's `/v1/embeddings`.
  - `rerank.provider = "tei"` enables an optional cross-encoder
    rerank pass after RRF fusion. Top `rerank.top_k` (default 8;
    see `docs/MEASUREMENTS.md`) candidates are scored by
    `POST /rerank`; the rerank score replaces the per-row score,
    slots beyond top-K stay in RRF order. Reranker errors are
    non-fatal — wg logs once and serves RRF. Measured impact on
    MIRACL/ko (`docs/MEASUREMENTS.md`): MRR@10 +5.8 %,
    nDCG@10 +4.6 %, R@10 unchanged; p50 latency 9 ms → 765 ms (85×).
    Use only when the precision win is worth the latency hit.
  - On Apple Silicon, install TEI natively via `cargo install
    --git github.com/huggingface/text-embeddings-inference
    --features metal text-embeddings-router` — the `cpu-1.9`
    Docker image is amd64-only and runs ~5× slower under
    Rosetta/QEMU, with 2–10× more RAM. Linux x86_64 hasn't been
    measured but the Docker overhead should be much smaller there.
- **Bulk insert: `fact_add_many`** — single redb write transaction
  amortizes the per-commit fsync across the whole batch. ~70× faster
  per fact at typical batch sizes than sequential `fact_add`. Exposed
  on `WikiGraph` and surfaced in every binding (Python, Node, Elixir,
  C) plus the `wg_fact_add_many` MCP tool.
- **`wg vector-rebuild`** — explicit HNSW reindex command. Use after
  switching embedding models or recovering from a corrupted sidecar.
- **MCP write tools** — `wg_fact_supersede` and `wg_fact_edit`
  alongside the existing `wg_fact_add`, closing the validity-window
  CRUD cycle for MCP-only agents. Tool count: 9 → 13.
- **Fact-store semantics** — search ranking now weights by
  `source_confidence × relevance_score`, applies time-decay
  (configurable τ, default 90 days), and supports `--as-of <date>`
  historical queries. `wg lint` flags multiple current
  Decision/Convention/Pattern facts on the same entity as conflicts.
- **`store.durability` config** — `"immediate"` (default; per-commit
  fsync) or `"eventual"` (queued; ~13× faster commits, survives
  process crash but not power loss). Opt-in only; `Durability::None`
  is intentionally not exposed (redb's docs warn it grows the file
  rapidly).
- **wg-python ergonomics** — `WikiGraph(path, model=…,
  semantic_index=…, durability=…)` kwargs in the constructor route
  through `Config::set` so validation messages propagate to Python.
  Internal `dict_opt` / `fact_input_from_dict` helpers collapse
  `fact_add_many`'s per-item parsing.
- **`wg doctor` memory section** — disk + RAM-estimate breakdown
  (redb store, hnsw sidecar, bm25 index, fact embed cache, hnsw
  runtime, model load, total). Two new advisories: `model.quantize
  true` for large models, `wg vector-rebuild` for missing sidecar.
- **Hermes plugin: detector confidence forwarding** — the
  detector's per-match confidence (0.6–0.95) now reaches wg's
  `source_confidence` instead of collapsing to the 0.5 default.

### Performance

Bench: 10 000-fact synthetic wiki, p95 latency, before → after.

| Operation        | Before    | After     | Δ      | PLAN target |
|------------------|-----------|-----------|--------|-------------|
| `traverse_d3`    | 17.9 ms   | 0.01 ms   | 1700×  | 1 ms (OK)   |
| `search_bm25`    | 2 332 ms  | 0.55 ms   | 4 200× | 3 ms (OK)   |
| `search_hybrid_hnsw` | 9.6 ms | 3.4 ms   | 2.8×   | 5 ms (OK)   |
| `lint`           | 17 111 ms | 34 ms     | 506×   | 50 ms (OK)  |
| `fact_add_many` (per fact) | n/a | 0.07 ms | new   | 1 ms (OK)   |
| `startup`        | 95 ms     | 12 ms     | 8×     | 30 ms (OK)  |

What landed:

- **BM25 inverted-index caching on `WikiGraph`** — the
  `hybrid_search` path constructed a fresh `SearchEngine` (and a
  fresh BM25 build) on every call. Now cached + dirty-marked on
  fact / entity mutations, like the HNSW index already was.
- **Range-scan secondary indexes** — `count_entity_facts`,
  `relations_get`, and the new `Store::fact_get_many` walk only the
  `{entity_id}\0` prefix range in redb instead of full table
  iteration with prefix filtering.
- **Lint single-load + in-memory grouping** — entities, facts, and
  relations are loaded once at the start of `lint()` and passed by
  reference into each check; previously `check_conflicts` ran
  `fact_list(entity_id=…)` per entity (each a full scan).
- **Trigram blocking + common-trigram cutoff in
  `check_duplicates`** — adversarial shared-prefix corpora no
  longer collapse the candidate set; trigrams that appear in more
  than 25 % of names are dropped (they can't carry the 0.9 jaccard
  threshold anyway).
- **`fact_get_many` batch read** — search candidate hydration opens
  one redb read transaction instead of N (saves ~2 ms on a typical
  64-fact prefilter slate).

### Tooling

- **Profile env vars** — `WG_LINT_PROFILE` and `WG_SEARCH_PROFILE`
  emit per-phase elapsed times when set. No-op when unset.
- **`benchmarks/src/bin/`** — four perf runners now ship: the
  reproducible `performance` matrix that writes
  `benchmarks/results/performance.json`, plus focused
  `lint_profile`, `search_profile`, and a raw-redb `fsync_probe`
  that confirmed `fact_add`'s ~4 ms floor is the macOS APFS fsync
  cost, not algorithmic.

## [0.1.0] — 2026-04 (initial cut)

### Added

- Phase 1–6 complete:
  - BM25 + semantic hybrid search
  - MCP server (JSON-RPC + SSE)
  - Search feedback + DomainAdapter
  - Language bindings (napi, python, nif, ffi)
  - S3 multi-writer support (feature-gated)
