# Changelog

## [Unreleased]

### Added

- **Low-ceremony handoff UX** — `aidememo agent add` provides a friendly
  alias for credential-free installation profiles, while `aidememo handoff
  send ALIAS`, `handoff run ALIAS`, and actor-free `handoff show ID` reduce
  the common multi-account round trip to four commands. Sender outbox now
  includes completed results by default (`--pending-only` narrows it), and
  MCP, the Python SDK, and the Hermes plugin expose the same actor-free
  `show` lookup and completed-result default. Existing `installation`,
  `session handoff --dispatch`, `status`, and `run --installation ... --next`
  forms remain supported.
- **Cross-agent workflow handoff** — `aidememo session handoff`, the
  `aidememo_handoff` MCP/Hermes tool, and SDK `Memory.handoff(...)` export a
  bounded Markdown packet for another coding agent or Hermes profile. Packets
  preserve the tracked session, distinguish routing labels from `source_id`
  scope, group decisions/questions/risks, and retain fact ids for verification.
  Compact `--from AGENT[/PROFILE]` / `--to AGENT[/PROFILE]` routes reduce CLI
  ceremony, while `aidememo session resume` validates the receiver session and
  activates both `AIDEMEMO_SESSION_ID` and `AIDEMEMO_SOURCE_ID` in one eval-safe command.
  `--done-when` carries an observable completion condition, SDK
  `handoff_packet()` preserves the structured envelope, and Hermes now returns
  that envelope directly instead of forcing orchestrators to parse Markdown.
- **Cross-account assignment pull flow** — `--dispatch` and
  `aidememo handoff inbox|accept|return|outbox|status` /
  `aidememo_handoff_inbox` address
  an existing session to a user-assigned account or installation alias such as
  `codex-one`, `codex-two`, or `claude-main`. `mcp-install --actor-id` persists
  the alias as `AIDEMEMO_ACTOR_ID`; the Python SDK and Hermes plugin expose the
  same round-trip contract. A return links the result/error fact; successful
  results complete the acknowledgement while failures remain accepted for the
  caller's policy. The stored record is a session pointer plus
  route/focus/status/result metadata, not a broker message: there are no
  topics, offsets, consumer groups, retries, leases, or copied payloads.
- **Credential-free installation profiles** — `aidememo installation
  add|list|show|remove` records Codex/Claude adapter, config root, workspace,
  source, model, and named environment allowances. `aidememo handoff run
  --installation ALIAS --next` resolves the oldest pending assignment, maps
  config roots through `CODEX_HOME` / `CLAUDE_CONFIG_DIR`, and uses a minimal
  `core` child environment by default without storing credential values.
- **Orchestrator handoff demo** — `scripts/demo-agent-handoff.sh` exercises a
  Codex/coding to Hermes/reviewer route without an external LLM call.
- **Cross-agent handoff Scenario P** — separates evidence/route/isolation
  quality gates from context-efficiency gates. The scenario caught and fixed
  bounded session artifacts selecting the oldest fact window; continuation
  artifacts now retain the latest attached facts and render that window in
  chronological order without changing global `fact_list` pagination.
- **Multi-account Scenario Q** — three independent MCP processes acting as two
  Codex subscriptions and one Claude account pass `10/10` routing gates,
  including source/actor isolation, same-session continuation, zero fact
  copies, and zero broker/payload keys in the persisted assignment.
- **Hermes Kanban boundary Scenario R** — a real temporary Hermes Kanban board
  passes `12/12` zero-token gates: internal profile reassignment stays in
  Kanban with no AideMemo assignment, an external Codex boundary creates one
  pointer and returns evidence on the same session, and Hermes explicitly owns
  final card completion.
- **External Codex/Claude worker lane and Scenario S** — the installable
  `aidememo-worker-lane` command and Python `run_external_assignment(...)` API
  accept one addressed handoff, invoke Codex or Claude with shell-free argv,
  pass the packet/resume environment, and return success or error evidence to
  the same session. Codex `--output-schema` and Claude `--json-schema` converge
  on one result contract; `done_when_met=false` follows the failure path.
  Scenario S passes `14/14` zero-token fake-CLI gates: sender outbox/status link
  both success and failure facts, success completes the acknowledgement,
  failure remains accepted, and neither path mutates Hermes Kanban. This is not
  authentication, exactly-once execution, live-model task success, or Hermes
  `spawn_fn` registration.

## [0.1.0] - 2026-07-09

### Added

- **Public release safeguards** — runner-backed release preflight and
  fresh-checkout onboarding workflows, public registry post-release install
  smoke, canonical `v0.1.0` source-tag contract, and an offline portability
  gate that rejects developer-specific home paths from first-party files.
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
    non-fatal — aidememo logs once and serves RRF. Measured impact on
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
  on `AideMemo` and surfaced in every binding (Python, Node, Elixir,
  C) plus the `aidememo_fact_add_many` MCP tool.
- **`aidememo vector-rebuild`** — explicit HNSW reindex command. Use after
  switching embedding models or recovering from a corrupted sidecar.
- **MCP write tools** — `aidememo_fact_supersede` and `aidememo_fact_edit`
  alongside the existing `aidememo_fact_add`, closing the validity-window
  CRUD cycle for MCP-only agents. Tool count: 9 → 13.
- **Fact-store semantics** — search ranking now weights by
  `source_confidence × relevance_score`, applies time-decay
  (configurable τ, default 90 days), and supports `--as-of <date>`
  historical queries. `aidememo lint` flags multiple current
  Decision/Convention/Pattern facts on the same entity as conflicts.
- **`store.durability` config** — `"immediate"` (default; per-commit
  fsync) or `"eventual"` (queued; ~13× faster commits, survives
  process crash but not power loss). Opt-in only; `Durability::None`
  is intentionally not exposed (redb's docs warn it grows the file
  rapidly).
- **aidememo-python ergonomics** — `AideMemo(path, model=…,
  semantic_index=…, durability=…)` kwargs in the constructor route
  through `Config::set` so validation messages propagate to Python.
  Internal `dict_opt` / `fact_input_from_dict` helpers collapse
  `fact_add_many`'s per-item parsing.
- **`aidememo doctor` memory section** — disk + RAM-estimate breakdown
  (redb store, hnsw sidecar, bm25 index, fact embed cache, hnsw
  runtime, model load, total). Two new advisories: `model.quantize
  true` for large models, `aidememo vector-rebuild` for missing sidecar.
- **Hermes plugin: detector confidence forwarding** — the
  detector's per-match confidence (0.6–0.95) now reaches aidememo's
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

- **BM25 inverted-index caching on `AideMemo`** — the
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

- **Profile env vars** — `AIDEMEMO_LINT_PROFILE` and `AIDEMEMO_SEARCH_PROFILE`
  emit per-phase elapsed times when set. No-op when unset.
- **`benchmarks/src/bin/`** — four perf runners now ship: the
  `performance` matrix (its environment-sensitive local JSON is ignored unless
  promoted with provenance), plus focused
  `lint_profile`, `search_profile`, and a raw-redb `fsync_probe`
  that confirmed `fact_add`'s ~4 ms floor is the macOS APFS fsync
  cost, not algorithmic.

### Initial Release

- Phase 1–6 complete:
  - BM25 + semantic hybrid search
  - MCP server (JSON-RPC + SSE)
  - Search feedback + DomainAdapter
  - Language bindings (napi, python, nif, ffi)
  - S3 multi-writer support (feature-gated)
