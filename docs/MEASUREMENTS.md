# Measurements

This file is the public measurement ledger for `aidememo`. Historical scratch-note
files were removed; durable numbers should live here, in
`benchmarks/*/RESULTS.md`, or in JSON under `bench/**/results`.

## Re-run Commands

```bash
cargo check -p aidememo-core -p aidememo-cli
cargo test -p aidememo-core --features semantic
cargo test -p aidememo-cli --bin aidememo
python3 -m pytest plugins/hermes/tests -q

cargo run --release --bin performance
cargo run --release -p aidememo-benchmarks --bin storage_backend_probe
python3 bench/multi-agent/scenario_d_concurrent_writers.py
python3 bench/multi-agent/scenario_e_http_shared.py
python3 bench/multi-agent/scenario_f_workflow_triggers.py
python3 bench/multi-agent/scenario_g_hermes_binding.py
python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py
AIDEMEMO_E2E_SETUP_ONLY=1 python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py
python3 bench/multi-agent/scenario_j_lock_retry_sweep.py
python3 bench/multi-agent/scenario_k_sdk_workflow_parity.py
python3 bench/multi-agent/scenario_l_self_extraction.py
python3 bench/multi-agent/scenario_m_mcp_install_source_defaults.py
python3 bench/multi-agent/scenario_n_hermes_memory_as_code.py
scripts/aidememo-agent-sdk-pack-smoke.sh
scripts/hermes-aidememo-pack-smoke.sh
scripts/skillopt-lite-cycle.sh --max-cycles 1
scripts/skillopt-lite-check.sh
scripts/demo-workflow.sh
scripts/ci-local.sh demo
scripts/sdk-promotion-check.sh
scripts/ci-local.sh sdk
```

The gbrain adapter path is documented in
[`benchmarks/gbrain-evals-adapter/README.md`](https://github.com/taeyun16/aidememo/blob/main/benchmarks/gbrain-evals-adapter/README.md),
with current scorecards in
[`benchmarks/gbrain-evals-adapter/RESULTS.md`](https://github.com/taeyun16/aidememo/blob/main/benchmarks/gbrain-evals-adapter/RESULTS.md).

## Agent UX

| Scenario | Result | Interpretation |
|---|---:|---|
| Hermes mixed-source prompt, unscoped | beta facts included 2/2 | Shared stores leak neighbouring source context unless scoped. |
| Hermes mixed-source prompt, `source_id=alpha` | alpha recall 2/2, beta leakage 0/2 | `source_id` gives clean per-agent/per-source reads. |
| Hermes serverless shared store, retry `0` | 10/20 writes persisted, 10 lock errors | redb's process lock is visible without smoothing. |
| Hermes serverless shared store, retry `5000` | 20/20 writes persisted, 0 errors; wall 2.16s, p50 98.1ms, max 1.22s | The default plugin path is smooth for ordinary two-agent local sharing. |
| Scenario J serverless lock-retry sweep, retry `5000` | Smooth until 4 concurrent writers; at 8 writers 79/80 persisted, p95 2.99s | Keep serverless sharing for small same-host teams; switch to daemon when high parallel write volume is normal. |
| HTTP shared `aidememo mcp-serve`, 2 clients x 10 writes | 20/20 persisted; p50 18.4ms, p95 41.8ms, wall 251ms | Daemon mode is still the faster high-concurrency path, but it can stay optional. |
| Workflow trigger Scenario F, 4 sparse tickets | 13/13 invariants; p95 2.48s; max context 3,023 chars; forbidden leakage 0 | CLI, MCP, and Hermes paths create distinct sessions/ticket facts and keep `source_id`-scoped ticket context separated. |
| Hermes workflow binding Scenario G, 4 sparse tickets | 5/5 invariants; shape parity 4/4; leakage 0; p50 1,795.71ms CLI vs 13.14ms binding | When `aidememo-python` is installed, Hermes composes workflow packs in process: same context contract, about 137x lower p50 after the first model/index warmup. |
| SDK workflow parity Scenario K, 4 sparse tickets | 8/8 invariants; Python and Node shape parity 4/4 each; leakage 0; p50 CLI 1,864.55ms, Python 16.19ms, Node 13.69ms | `aidememo-python` and `aidememo-napi` expose the same sparse-ticket context contract as CLI while avoiding per-command CLI spawn overhead. |
| Self-extraction Scenario L, MCP batch + default source | 10/10 invariants; `aidememo_fact_add_many` inserted 7 classified facts in 295.44ms; alpha sparse-ticket workflow recovered decision=1, lesson=1, error=1 with beta leakage 0; `AIDEMEMO_SOURCE_ID` scoped an env-default MCP write and search | If the calling agent classifies facts before `aidememo_fact_add_many`, aidememo preserves typed memory and returns it in the workflow shape agents consume without a built-in LLM extraction pipeline. MCP agents can also set `AIDEMEMO_SOURCE_ID` once instead of repeating `source_id` on every call. |
| MCP install source defaults | `cargo test -p aidememo-cli --bin aidememo mcp_install`: 10/10 tests passed; `aidememo mcp-install --target codex --source-id agent-alpha --print --json` reports `source_id=agent-alpha` and env detail | `aidememo mcp-install --source-id` makes the smooth path installable: the MCP server starts with `AIDEMEMO_SOURCE_ID` already set instead of asking users to hand-edit agent config. |
| Doctor scoped setup hint | `cargo test -p aidememo-cli --bin aidememo`: 120 passed, 1 ignored; temp `aidememo skill install --target opencode --dest ...` output includes `aidememo mcp-install --target opencode` and `--source-id <namespace>` | Setup diagnostics now preserve project namespace context: a scoped recent workflow ticket turns the no-MCP doctor hint into `aidememo mcp-install --target codex --source-id <namespace>`, and skill install output points shared-store users at the same flag. |
| MCP install source defaults Scenario M | 12/12 invariants; elapsed 323.97ms; Codex / Cursor / OpenCode configs all contain `AIDEMEMO_SOURCE_ID=agent-alpha`; Claude / Hermes / OpenClaw print-mode commands include env injection; MCP write/search without explicit `source_id` returned only `agent-alpha` facts | The install story is now end-to-end testable without real agent CLIs: generated configs provide the env value that MCP tools consume for scoped defaults. |
| Scenario H source-default setup | 5/5 setup invariants; Codex config created through `aidememo mcp-install --source-id workflow-alpha`; Claude project MCP and Hermes plugin config both carry `AIDEMEMO_SOURCE_ID` / `source_id`; no model calls | The token-burning natural prompt scenario can now run without asking agents to pass `source_id` per tool call; setup itself is zero-cost and regression-testable. |
| MCP workflow session attachment | `cargo test -p aidememo-cli --bin aidememo workflow_start_creates_session_ticket_and_scoped_context` and `cargo test -p aidememo-cli --bin aidememo fact_add_many_attaches_top_level_session_id_to_each_item` passed | MCP agents can pass `session_id` from `aidememo_workflow_start` into `aidememo_fact_add` / `aidememo_fact_add_many`, keeping follow-up facts on the workflow thread without relying on CLI-only `AIDEMEMO_SESSION_ID`. |
| Zero-token workflow demo | decision + lesson + error surfaced; search hits 4; workflow latency 128ms | `scripts/demo-workflow.sh` demonstrates the product position without an agent, model call, or persistent store. It uses CLI `workflow start --bm25-only` for deterministic first-run behaviour. `scripts/ci-local.sh demo` wraps the same smoke for daily local checks in about 0.91s warm. |
| Natural workflow adoption Scenario H, 3 agents | 4/4 invariants; 3/3 agents passed; each created 1 scoped workflow fact; prior reflection Claude 3/3, Codex 2/3, Hermes 3/3; forbidden leakage 0 | Sparse-ticket prompts can drive the workflow entry point across Claude, Codex, and Hermes when each runtime gets an isolated, deterministic MCP config. Hermes uses MCP-only here to avoid redb lock contention with the in-process plugin. |
| Hermes native core parity | `python3 -m pytest plugins/hermes/tests -q`; `cargo test -p aidememo-cli --bin aidememo mcp_tools::` | Hermes now exposes `aidememo_context`, `aidememo_aggregate`, `aidememo_fact_add_many`, and `aidememo_doctor` through native tools/slash commands, and source-scoped aggregate calls honor explicit `source_id` / `AIDEMEMO_SOURCE_ID` instead of mixing shared-store facts. |
| Hermes Memory-as-Code Scenario N | `python3 bench/multi-agent/scenario_n_hermes_memory_as_code.py`: 9/9 invariants; fanout search + dedupe + coverage + derived batch + aggregate completed with beta source excluded from scoped rows | The Hermes research profile now has a zero-token, code-first regression through the shared `aidememo_agent.Memory` API: intermediate candidate sets stay in Python and only compact coverage/aggregate artifacts need to reach model context. |
| aidememo-agent-sdk wheel install smoke | `scripts/aidememo-agent-sdk-pack-smoke.sh`: built `aidememo_agent_sdk-0.1.0-py3-none-any.whl`, installed it into a temp venv, verified `Memory`, `AideMemoClient`, `AideMemoMemorySDK`, and first-use methods; total 3.20s | The code-first SDK path is installable independently of Hermes, so Codex / Claude Code / CI scripts do not need the Hermes plugin package. |
| hermes-aidememo wheel install smoke | `scripts/hermes-aidememo-pack-smoke.sh`: built `aidememo_agent_sdk-0.1.0-py3-none-any.whl` + `hermes_aidememo-1.0.0-py3-none-any.whl`, installed both into a temp venv, verified `hermes_aidememo.AideMemoMemorySDK` re-exports `aidememo_agent.Memory`, `hermes.plugins` entry point, `plugin.yaml`, and bundled `SKILL.md`; total 4.36s | The Hermes plugin remains pip-installable while delegating the code-first SDK layer to the shared package. |
| SkillOpt-lite profile gate | `scripts/skillopt-lite-check.sh` validates the bundled `aidememo-skill/SKILL.md` as the current trainable memory profile candidate, then runs `aidememo skill check`, `git diff --check`, `cargo check -p aidememo-cli`, `scripts/demo-workflow.sh`, and `scripts/sdk-promotion-check.sh`; optional Scenario L/M/N gates run with `AIDEMEMO_SKILLOPT_RUN_SCENARIOS=1` | This turns SkillOpt's useful discipline into a local `aidememo` product boundary: memory skill edits are bounded, auditable, and accepted only after zero-token workflow / SDK gates pass. |
| SkillOpt-lite periodic cycle | `scripts/skillopt-lite-cycle.sh --max-cycles 1` checks the current profile when no candidate is queued, records the accepted dry-run under `target/skillopt-lite/runs.jsonl`, and stores the full gate output in `target/skillopt-lite/logs/`; passing candidates are applied only with `--apply` | Periodic skill/profile improvement can run without dirtying the repo or turning rejected candidates into failures by default; rejected edits are preserved for optimizer feedback. |

## Agentic Loop Calibration

`aidememo_aggregate` should be described as a deterministic arithmetic primitive, not
as a general accuracy lever. The stable product rule is:

| Question shape | Recommended path |
|---|---|
| "What did I say about X?" / "When did I last do Y?" / "What's my preference for Z?" | Answer from `aidememo_context`, `aidememo_query`, or `aidememo_search` snippets. |
| "How much total did I spend on X?" | `aidememo_aggregate(op=sum_currency)` |
| "How many hours/days total?" | `aidememo_aggregate(op=sum_duration)` |
| "How many distinct days had event X?" | `aidememo_aggregate(op=count_distinct_dates)` |
| "Timeline of all X events" | `aidememo_aggregate(op=timeline)` |
| "How many times did I decide/try X?" | `aidememo_aggregate(op=count)` or `op=enumerate` |

The earlier 60-question focused run showed a large multi-session gain when the
agent used aggregation. A later balanced LongMemEval-style run with 240
questions, MiniMax temperature 0, and a 3-run mean put the agentic-loop variant
within reader noise of the single-call baseline: roughly `-1.9pt` versus the
mean with `sigma ~= 1.1pt`. Forced agentic-loop dispatch also caused
single-fact SS-pref / temporal regressions because extra JSON tool-call
structure can disturb simple recall.

A single-shot "does this need aggregation?" classifier netted to baseline at
240 questions (`+0pt` mean, around 40% precision). The practical conclusion is
to expose `aidememo_aggregate` in reader prompts as insurance for counting, summing,
and timelines, while keeping normal recall on `aidememo_context` / `aidememo_query`.

## gbrain-evals Adapter

Fresh-checkout validation against `garrytan/gbrain-evals` commit `ef7794f`
on 2026-05-19:

| Adapter | Corpus Pages | Queries | P@5 | R@5 | Correct / Expected | Real Time |
|---|---:|---:|---:|---:|---:|---:|
| aidememo bm25 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 63.38s |
| aidememo bm25 daemon | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 11.04s |
| aidememo hybrid daemon | 240 | 145 | 16.7% | 62.5% | 121 / 261 | 45.64s |

Daemon BM25 preserves the score and cuts wall time by `5.7x`. Hybrid is
slower and slightly worse on this surface-form-heavy BrainBench slice; keep
semantic retrieval for paraphrase-heavy workloads.

## LongMemEval-S Retrieval

500 questions from the cleaned LongMemEval-S set, measured with the Rust
benchmark harness:

| Stack | R@1 | R@5 | R@10 | MRR |
|---|---:|---:|---:|---:|
| aidememo BM25-only | 0.866 | 0.952 | 0.974 | 0.902 |
| aidememo + time-decay soft bias | 0.858 | 0.958 | 0.978 | 0.898 |
| aidememo + bge-small-en-v1.5 | 0.914 | 0.976 | 0.986 | 0.941 |
| aidememo + bge + reranker K=10 | 0.938 | 0.984 | 0.986 | 0.957 |
| aidememo + bge + two-stage reranker K=20 -> 10 | 0.940 | 0.984 | 0.992 | 0.958 |

The retrieval ceiling is high: answer evidence lands in the top-10 set for
496/500 questions. Remaining E2E errors are mostly reader-side temporal or
multi-session reasoning, not missing evidence.

## LongMemEval-S E2E

LLM-graded with a `gpt-4o` judge:

| Stack | Reader | Overall |
|---|---|---:|
| Mem0 published baseline | gpt-4o | 49.0% |
| aidememo @ model2vec + decay | gpt-4o-mini | 60.0% |
| aidememo @ model2vec + decay | gpt-4o | 60.4% |
| aidememo @ bge + reranker K=20 -> 10 | gpt-4o-mini | 65.6% |
| aidememo @ bge + reranker K=20 -> 10 | gpt-4o | 67.6% |
| aidememo @ bge + reranker K=20 -> 10 | gpt-4.1 | 72.6% |
| aidememo @ bge + reranker K=20 -> 10 | MiniMax-M2.7-highspeed | 74.0% |
| Zep / Graphiti 2026 published | gpt-4o | 71.2% |
| Mastra published | gpt-4o | 84.2% |
| OMEGA published local | gpt-4.1 | 95.4% |

Use these numbers carefully. AideMemo should lead with deployment and temporal
memory ergonomics, not a SOTA claim.

## Model And Rerank Trade-offs

| Workload | Recommended path | Measured reason |
|---|---|---|
| Code/docs/news where query terms overlap answer text | default `model2vec` / BM25-hybrid | HotpotQA and MultiHop-RAG were saturated: model2vec and BGE both landed R@5 around 94-96%. |
| English paraphrase-heavy personal memory | `fastembed` + `bge-small-en-v1.5` | LongMemEval-S R@5 improved from 96.2% to 98.0%; SS-pref went 93.3 -> 100. |
| Korean / multilingual repositories | default multilingual model2vec | `bge-small-en-v1.5` is English-tuned. |
| Retrieval-bound search where top-K recall is low | enable rerank | MIRACL/ko improved MRR@10 by 5.8% and nDCG@10 by 4.6%. |
| Reader-bound agent loops where top-K already overlaps | keep rerank off | Cross-encoder rerank can add roughly 85x latency at top_k=8 with no E2E lift. |

HNSW is the default semantic index because it closed a Korean MIRACL candidate
drop caused by BM25 prefiltering while keeping query latency below brute-force
cosine on larger corpora.

## Performance

Reference numbers from `benchmarks/src/bin/performance.rs` on a 10,000-fact
synthetic wiki, p95 latency, default config:

| Operation | p95 |
|---|---:|
| `traverse_d3` | ~0.01 ms |
| `search_bm25` | ~0.5 ms |
| `search_hybrid` | ~3.4 ms |
| `lint` | ~34 ms |
| `fact_add_many` per fact | ~0.07 ms |
| `fact_add` single | ~5 ms |
| `startup` open + first traverse | ~12 ms |

`fact_add` is limited by the OS fsync floor under immediate durability. Use
`fact_add_many` or `store.durability = eventual` when ingest throughput matters.

### Storage Backend Probe

`benchmarks/src/bin/storage_backend_probe.rs` compares the optional redb-backed
AideMemo API path with the default SQLite schema. SQLite is now the default
runtime backend (`store.backend = "sqlite"`, default Cargo features) with
normalized `entities`, `facts`, `fact_entities`, `relations`, feedback tables,
secondary indexes, JSON record payloads, and FTS5. Results are written to
`benchmarks/results/storage_backend_probe.json`.

Local macOS arm64 run on 2026-06-13:

```bash
cargo run --release -p aidememo-benchmarks --bin storage_backend_probe
```

10,000-fact synthetic store, p95 latency after the redb
`fact_list(entity_id=...)` fast path started using the existing
`fact_by_entity` prefix index:

| Backend | Build | Single `fact_add` | Batch per fact | All facts scan | Entity facts | Search | Open existing |
|---|---:|---:|---:|---:|---:|---:|---:|
| redb immediate | 3086.79 ms | 6.38 ms | 0.122 ms | 30.00 ms | 6.12 ms | BM25 0.92 ms | 38.23 ms |
| redb eventual | 425.35 ms | 1.23 ms | 0.050 ms | 30.04 ms | 6.04 ms | BM25 0.96 ms | 32.90 ms |
| SQLite WAL FULL | 95.23 ms | 0.25 ms | 0.022 ms | 1.66 ms | 0.022 ms | FTS5 0.12 ms | 1.53 ms |
| SQLite WAL NORMAL | 89.24 ms | 0.19 ms | 0.023 ms | 1.59 ms | 0.017 ms | FTS5 0.13 ms | 0.77 ms |

Interpretation:

* SQLite is now the default backend. It naturally fits entity/fact joins,
  indexed filtered lists, migrations, introspection, and persistent FTS.
* The redb `fact_list_entity` path improved from the earlier all-scan result
  (`~29 ms`) to `~6 ms` after using `fact_by_entity`. The remaining gap is no
  longer from total-store scanning; it is the cost of point-hydrating JSON facts
  through the redb table path versus SQLite's join/index cursor.
* redb immediate write latency is dominated by per-transaction fsync. The
  existing `store.durability = eventual` knob narrows single-write cost, but
  SQLite still wins this synthetic write path while also maintaining FTS rows.
* SQLite FTS5 is not directly equivalent to the current in-memory BM25 index,
  but the persistent-index result supports a deeper local SQLite spike.

Runtime promotion status:

* `crates/aidememo-core/src/backend.rs` defines `StoreBackend` plus `StoreKind`,
  so `AideMemo::open` can select redb or SQLite from `config.store.backend`.
  The trait also owns the shared archive-transfer contract
  (`existing_fact_ids`, `fact_archive_to`): cold-tier moves must preserve
  `FactId`, skip already-archived hot misses, and delete from hot only after the
  cold store can address the archived id.
* `crates/aidememo-core/src/sqlite_store.rs` implements the same public store
  surface used by entity/fact CRUD, relation graph traversal, lint, ingest,
  BM25 search, query, archive/cold-tier moves, JSONL import/export, pull sync,
  feedback, and semantic-adapt adapter state.
* JSONL import now uses the same ID-preserving upsert path as sync import.
  This makes `aidememo export` from a redb store followed by `aidememo import`
  into a SQLite store a usable migration primitive instead of allocating fresh
  ULIDs.
* `aidememo-cli` defaults to SQLite and exposes a `redb` Cargo feature that
  forwards `aidememo-core/redb`; build with
  `cargo build -p aidememo-cli --features redb` before setting
  `aidememo config set store.backend redb`.
* The Python, Node, Elixir, and C native binding crates share the same default
  SQLite backend and optional `redb` feature. This keeps the SDK replacement
  path aligned with CLI/MCP instead of making backend choice a CLI-only spike.
* `sqlite_matches_redb_for_core_public_api_fixture` seeds the same fixture into
  redb and SQLite, then compares stats, fact contents, traversal output, and
  BM25 search results.
  `archive_contract_matches_redb_sqlite_and_libsqlite_public_api` exercises
  the same hot-to-cold archive contract through the public `AideMemo` API on
  redb, canonical SQLite, and the `libsqlite` alias. Together these are the
  current semantic parity gates for backend promotion.
* `sqlite_import_preserves_redb_export_ids_for_migration_gate` exports a redb
  fixture, imports it into SQLite, verifies entity/fact IDs and graph/search
  parity, then replays the same JSONL to prove the migration path is
  idempotent.
* `sync_export_import_is_backend_compatible` verifies the pull-sync wire format
  in both directions (redb → SQLite and SQLite → redb). It checks full sync
  entity/fact/relation ID preservation and then applies an incremental delta
  with `entity_describe` plus `fact_supersede`, proving in-place updates cross
  the backend boundary as well as fresh inserts.
* `scripts/storage-backend-parity.sh` is the CLI/MCP gate: redb export/import
  into SQLite, redb/SQLite sync compatibility, relation preservation, SQLite
  cold-tier archive/search, and 24 concurrent MCP writes through `mcp-serve`.
* `scripts/storage-backend-sqlite-full-surface.sh` is the SQLite-only
  full-surface smoke: it builds the CLI with `--features sqlite` and exercises
  init, ingest, entity/fact writes, BM25 search/query, graph traversal,
  sessions, workflow start, archive, export, and import without redb compiled
  in. It defaults `store.backend` to the `libsqlite` alias, so the full public
  surface proves both the canonical SQLite backend and the user-facing
  libsqlite spelling resolve to the same implementation.
* `scripts/storage-backend-sqlite-advanced-surface.sh` is the SQLite-only
  advanced-surface smoke: it builds the same SQLite CLI and verifies
  CLI-level `store.lock_retry_ms` busy-timeout behaviour under a held SQLite
  writer lock, fact-level feedback, search feedback, adapter train/status/eval,
  heuristic extract preview/apply, pending approve/reject, and TTL-only
  consolidate without model downloads. The TTL gate explicitly runs with
  `--semantic-threshold 0`, proving expiry is independent from semantic dedup.
* `fact_archive_preserves_mcp_fact_get_for_cold_tier` is the MCP archive
  invariant gate: archived facts leave the hot store, `aidememo_fact_get` still
  resolves them from the backend-specific cold tier, default search hides them,
  and `include_archive:true` search returns them.
* `scripts/storage-backend-real-corpus-diff.sh` ingests the repo's real docs
  corpus into redb and SQLite independently, normalizes away backend-specific
  ULIDs/timestamps, compares entity/fact/relation exports, then compares
  BM25 search results across representative queries.
* `scripts/storage-backend-sqlite-mcp-soak.sh` runs the SQLite `mcp-serve`
  write path under concurrent load. The default local gate writes 200 facts
  through 16 parallel HTTP MCP callers, verifies unique fact IDs, final stats,
  and BM25 visibility for a tail write.
* `scripts/storage-backend-sdk-bindings-check.sh` verifies the SDK/binding
  surface: default SQLite builds, `libsqlite` alias opens in native binding
  tests, and redb-only Cargo feature builds across Python, Node, Elixir, and C.
* `s3` no longer enables the `redb` feature. Its local WAL staging path uses
  SQLite (`wal.sqlite`), so `cargo check -p aidememo-core --no-default-features
  --features s3` proves the S3/manifest code can build without compiling the
  optional redb backend.
* `scripts/storage-backend-feature-gate.sh` locks the Cargo feature boundary:
  default and SQLite-only core/CLI/SDK builds plus S3-only core builds must
  omit the `redb` crate from `cargo tree`, while redb still appears only when
  the explicit `redb` feature is selected.

Validation added in the runtime spike:

```bash
cargo test -p aidememo-core
cargo test -p aidememo-core --no-default-features --features sqlite
cargo test -p aidememo-core --no-default-features --features redb
cargo test -p aidememo-core --features sqlite,redb archive_contract_matches_redb_sqlite_and_libsqlite_public_api
cargo test -p aidememo-core --features sqlite,redb sync_export_import_is_backend_compatible
cargo test -p aidememo-core --features sqlite,semantic,semantic-adapt
cargo check -p aidememo-core --no-default-features --features s3
./scripts/storage-backend-feature-gate.sh
cargo check -p aidememo-cli
cargo test -p aidememo-cli --no-default-features --features redb --bin aidememo
./scripts/storage-backend-sqlite-full-surface.sh
./scripts/storage-backend-sqlite-advanced-surface.sh
./scripts/storage-backend-parity.sh
./scripts/storage-backend-real-corpus-diff.sh
./scripts/storage-backend-sqlite-mcp-soak.sh
./scripts/storage-backend-sdk-bindings-check.sh
```

Current replacement read:

* SQLite is now the default backend and runs through the full public
  `AideMemo` API surface in tests. The main replacement blockers are no longer
  graph/search/lint/ingest wiring.
* The redb path remains available as an explicit Cargo feature and runtime
  backend selection. Archive siblings use backend-specific suffixes
  (`.cold.redb` for redb, `.cold.sqlite` for SQLite), the docs corpus parity
  gate covers a representative real markdown corpus, and the local MCP soak
  covers concurrent SQLite write traffic.
* libSQL/Turso remote operation is still a separate decision. The current
  implementation uses bundled SQLite through rusqlite and the S3 manifest/WAL
  path uses local SQLite staging; this proves relational schema fit and local
  runtime replaceability, not managed remote replication semantics.

## Workflow Doctor Readiness

P3.5 adds a `workflow` block and P2.5 adds a separate `sharing` block to
`aidememo doctor --json` so sparse ticket automation and shared-store ergonomics are
visible without manually inspecting agent configs, fact lists, or benchmark
notes. P2.6 threads the same `sharing` contract through MCP `aidememo_doctor`, so
agents can see it without shelling out. The stable workflow contract is:

| Field | Meaning |
|---|---|
| `workflow.ready` | At least one checked agent has `aidememo` registered as MCP. |
| `workflow.recent_ticket_count` | Current workflow-start ticket facts in the last 30 days. |
| `workflow.recent_tickets[]` | Up to five recent ticket summaries with `source` / `source_id`. |
| `workflow.hints[]` | Actionable setup or usage hints with a concrete command in `action`. |

The sharing contract is:

| Field | Meaning |
|---|---|
| `sharing.lock_retry_ms` | Current local-store contention wait budget: SQLite busy timeout and redb open retry. |
| `sharing.serverless_recommended_writers` | Measured smooth same-host writer envelope, currently 4. |
| `sharing.high_concurrency_writers` | Stress point used by Scenario J, currently 8. |
| `sharing.daemon.state` | `healthy`, `stale_registry`, or `none`. |
| `sharing.recommended_mode` | `daemon`, `serverless_retry`, or `serverless_fail_fast`. |
| `sharing.hints[]` | Actionable retry / daemon guidance with a concrete command in `action`. |

Validation:

| Command | Result |
|---|---|
| `cargo test -p aidememo-cli doctor` | 25 passed; workflow unit tests cover ready/count/hints, sharing unit tests cover retry advisory behaviour, and integration tests cover the JSON `sharing` contract. |
| `cargo test -p aidememo-cli doctor_json_includes_workflow_readiness_hints` | fixture CLI smoke validates JSON `workflow.ready`, `recent_ticket_count`, and actionable hints. |
| `cargo test -p aidememo-cli doctor_json_includes_shared_store_guidance` | fixture CLI smoke validates JSON `sharing.lock_retry_ms`, `serverless_recommended_writers=4`, `daemon.state`, `recommended_mode`, and actionable hints. |
| `cargo test -p aidememo-cli doctor_groups_by_code_with_action_hints` | MCP `aidememo_doctor` unit validates lint grouping plus `sharing.serverless_recommended_writers=4`, `recommended_mode=serverless_fail_fast`, and `sharing_retry_disabled`. |
| `python3 bench/multi-agent/scenario_i_workflow_doctor.py` | 10/10 invariants; CLI/MCP/Hermes each created a workflow ticket; doctor reported `workflow.ready=true`, `recent_ticket_count=3`, and no false no-MCP/no-recent-ticket hints. |
| `python3 bench/multi-agent/scenario_j_lock_retry_sweep.py` | 7/7 invariants; `store.lock_retry_ms=5000` stayed smooth through 4 concurrent serverless writers and mostly recovered 8-writer contention. |
| `scripts/workflow-release-smoke.sh` | Bundles the first-run workflow demo, Scenario F + I, and a fresh fixture `aidememo doctor --json` assert for release checks. Latest run: demo recovered decision/lesson/error, Scenario F 13/13, Scenario I 10/10, fixture doctor `workflow_ready=true`, `recent_ticket_count=1`, total 13.40s. The timing table includes `ok`/`fail` status and is printed from an EXIT trap so partial failures still leave context; forced `AIDEMEMO_BIN=/bin/false` failure records `fail ... exit 1`. |
| CI `workflow-release-smoke` job | Runs the same script on Ubuntu after lint with Python 3.13 and a 10-minute timeout. |
| CI `workflow-lint` job | Runs `actionlint@v1.7.1` across `.github/workflows/*.yml` before heavier Rust checks. Local `actionlint .github/workflows/*.yml`: 0 issues. |

Latest local `workflow-release-smoke` timing:

| Status | Step | Seconds |
|---|---|---:|
| ok | `cargo build -p aidememo-cli` | 0.22 |
| ok | `bash -n scripts/demo-workflow.sh` | 0.02 |
| ok | `py_compile` | 0.04 |
| ok | `scripts/demo-workflow.sh` | 0.65 |
| ok | Scenario F | 7.17 |
| ok | Scenario I | 5.10 |
| ok | Fixture `aidememo workflow start --bm25-only` | 0.20 |
| total | | 13.40 |

Scenario I measurement, May 21 2026:

| Metric | Value |
|---|---:|
| Workflow tickets | 3 |
| Drivers | CLI, MCP, Hermes |
| Workflow p50 latency | 1887.69 ms |
| Workflow p95 latency | 1891.48 ms |
| Doctor recent ticket count | 3 |
| Doctor hint codes | `workflow_no_skill_prompt` |

Scenario J lock-retry sweep, May 21 2026:

| Writers | Retry ms | Persisted | Success | p50 ms | p95 ms | Max ms | Wall ms |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 0 | 10/10 | 100.0% | 94.80 | 185.26 | 185.26 | 1113.06 |
| 2 | 0 | 10/20 | 50.0% | 92.80 | 124.25 | 179.96 | 1150.86 |
| 4 | 0 | 10/40 | 25.0% | 8.72 | 106.01 | 192.92 | 1171.85 |
| 8 | 0 | 10/80 | 12.5% | 9.52 | 107.85 | 200.79 | 1242.34 |
| 1 | 5000 | 10/10 | 100.0% | 106.71 | 186.70 | 186.70 | 1207.83 |
| 2 | 5000 | 20/20 | 100.0% | 103.61 | 196.69 | 1274.25 | 2255.45 |
| 4 | 5000 | 40/40 | 100.0% | 101.86 | 1282.56 | 3452.56 | 4431.45 |
| 8 | 5000 | 79/80 | 98.8% | 103.02 | 2988.40 | 5049.66 | 8529.39 |

Product read: serverless CLI retry is appropriate for one to four same-host
writers. At eight concurrent writers it is still much better than fail-fast
mode, but p95 approaches three seconds and one write can still exhaust the
5s retry budget; use a shared `aidememo mcp-serve` daemon when that level of
parallelism is normal.

## GBrain Adapter Native Backend

The `gbrain-evals` scaffold now supports `AIDEMEMO_ADAPTER_BACKEND=auto|cli|napi`.
The native path imports `aidememo-napi` in process and keeps the existing CLI and
daemon paths as baselines.

Local Bun-shaped fixture, 3 pages, 30 repeated BM25 queries:

| Backend | Top Hit | p50 | p95 |
|---|---|---:|---:|
| CLI | `redis` | 124.55 ms | 132.08 ms |
| NAPI | `redis` | 0.02 ms | 0.03 ms |

This is a scaffold-level latency check, not the full public `gbrain-evals`
scorecard. Record full runner wall time separately when re-running BrainBench.

Fresh-checkout BrainBench scorecard, `garrytan/gbrain-evals@89445dd`,
`BRAINBENCH_N=1`, 240 pages, 145 queries:

| Backend | P@5 | R@5 | Correct / Expected | Real Time |
|---|---:|---:|---:|---:|
| CLI daemon bm25 | 17.4% | 64.1% | 125 / 261 | 10.77 s |
| NAPI bm25 | 17.4% | 64.1% | 125 / 261 | 6.48 s |

NAPI preserves score parity and is `1.66x` faster than the daemon baseline
(`9.78x` faster than the historical direct CLI `63.38 s` run).

Packaging readiness:

| Command / workflow | Result |
|---|---|
| `scripts/release-preflight.sh` | One-command release gate with timed summary rows. `local` profile runs version gate, workflow syntax lint when `actionlint` is available, docs feature gate/build, binding release smoke, workflow release smoke, and SDK promotion check; `full` profile adds optional Python/Elixir/C binding smokes plus Python/npm publish dry-runs. Child scripts still print their own summaries to stdout, but preflight clears child `$GITHUB_STEP_SUMMARY` so CI gets one top-level `release-preflight` table. Latest local profile passed in 10.75s: version 0.14s, workflow lint 0.11s, docs feature gate 0.83s, docs build 1.51s, binding smoke 4.12s, workflow smoke 3.92s, SDK promotion 0.12s. Binding-only measured path (`AIDEMEMO_RELEASE_PREFLIGHT_WORKFLOW=0 AIDEMEMO_RELEASE_PREFLIGHT_SDK_PROMOTION=0 AIDEMEMO_RELEASE_PREFLIGHT_PUBLISH=0`): total 3.79s. `AIDEMEMO_RELEASE_PREFLIGHT_SDK_REQUIRE_PUBLIC=1` fails the SDK step with exit 1 while recording the failure row. Forced `AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT_BIN=/nonexistent/actionlint` records `fail | workflow syntax lint | 0.00 | /nonexistent/actionlint not installed`. |
| CI `sdk-promotion-check` job | Runs `scripts/sdk-promotion-check.sh` on Ubuntu after lint with Python 3.13 and Node 22, keeping SDK wording and binding surface drift visible in PR checks without running package smokes. Local workflow lint: `actionlint .github/workflows/*.yml` 0 issues; local gate: ok=13, ready=3, blocked=2, fail=0. |
| `scripts/ci-local.sh demo` | Local first-run workflow smoke with a timed Markdown summary. `bash -n scripts/ci-local.sh` passes; `scripts/ci-local.sh demo` recovered decision=1, lesson=1, error=1, search_hits=4, workflow latency 128ms, wall 0.91s. `scripts/ci-local.sh all` now runs this check between lint and SDK promotion. |
| `scripts/ci-local.sh sdk` | Local CI parity hook for the same SDK wording/parity gate. `bash -n scripts/ci-local.sh` passes; direct `scripts/sdk-promotion-check.sh` reports ok=13, ready=3, blocked=2, fail=0. Child SDK details remain in stdout, but `$GITHUB_STEP_SUMMARY` contains only the top-level `ci-local timings` table (`rg -n '^## ' "$summary_file"` returns one heading). Latest local SDK mode total: 0.12s. `scripts/ci-local.sh all` now runs this check between the workflow demo and tests. |
| SDK promotion GitHub summary | `GITHUB_STEP_SUMMARY=$(mktemp) scripts/sdk-promotion-check.sh` writes a Markdown table with 18 check rows plus metric rows while keeping stdout unchanged. `GITHUB_STEP_SUMMARY=$(mktemp) AIDEMEMO_SDK_PROMOTION_JSON=1 scripts/sdk-promotion-check.sh` still emits valid JSON to stdout. |
| `scripts/aidememo-release-version.sh` | Unified release version gate. With no args, verifies Cargo, Python, npm, and NIF package versions together; with a semver arg, updates every managed package version. Latest run: `0.1.0` pinned; temp-copy bump to `0.1.1` updated Cargo workspace, Python `pyproject.toml`, npm root/platform packages plus optionalDependency pins, and `aidememo-nif` `mix.exs`. |
| `scripts/aidememo-python-version.sh` | Version gate for the Python wheel. With no args, verifies `Cargo.toml` workspace version equals `crates/aidememo-python/pyproject.toml` `project.version`; with a semver arg, updates both. Latest run: `0.1.0` pinned. |
| `scripts/aidememo-python-pack-smoke.sh` | Builds a `aidememo-python` wheel with maturin for a temp venv interpreter, installs that wheel into the venv, runs `crates/aidememo-python/tests/smoke.py`, verifies installed wheel metadata equals `aidememo_python.__version__`, and writes timed rows to stdout and `$GITHUB_STEP_SUMMARY`. The smoke accepts `AIDEMEMO_PYTHON_SMOKE_BACKEND=sqlite\|libsqlite\|redb` and checks the created store file header so an ignored backend argument fails visibly. Local macOS arm64 default SQLite wheel: total 44.93s; version gate 0.04s; `maturin build --release` 42.03s; install 0.72s; smoke 1.41s; version check 0.06s. libsqlite alias smoke (`AIDEMEMO_PYTHON_SMOKE_BACKEND=libsqlite`): total 33.12s; `maturin build --release` 30.59s; install 0.64s; smoke 1.33s; version check 0.05s. redb-only wheel (`AIDEMEMO_PYTHON_PACK_SMOKE_NO_DEFAULT_FEATURES=1 AIDEMEMO_PYTHON_PACK_SMOKE_FEATURES=redb AIDEMEMO_PYTHON_SMOKE_BACKEND=redb`): total 37.14s; `maturin build --release --no-default-features --features redb` 34.58s; install 0.65s; smoke 1.48s; version check 0.06s. All built `aidememo_python-0.1.0-cp313-cp313-macosx_11_0_arm64.whl`; smoke passed including `workflow_start(..., bm25_only=True)` and `AideMemoNotFoundError` typed exception handling, installed version `0.1.0`. |
| `scripts/aidememo-python-publish-dry-run.sh` | Builds PyPI publish payloads without uploading and writes a timed Markdown summary. Local macOS arm64: version gate 0.05s, venv 1.36s, `maturin build --release --sdist` 59.48s, payload validation 0.05s, total 60.94s; built `aidememo_python-0.1.0-cp313-cp313-macosx_11_0_arm64.whl` (2.8M) + `aidememo_python-0.1.0.tar.gz` (349K). The dry-run rebuilds the wheel from the sdist, proving the vendored `tokenizers` patch is present; forced `AIDEMEMO_PYTHON_EXPECT_VERSION=9.9.9` records a `fail | version expectation` summary row. |
| `scripts/aidememo-napi-version.sh` | Version gate for root + platform package graph. With no args, verifies all versions and root `optionalDependencies`; with a semver arg, updates every package. Latest run: `0.1.0` pinned across 5 platform packages; temp-copy bump to `0.1.1` updated root, platform packages, and optionalDependency pins. |
| `scripts/aidememo-napi-pack-smoke.sh` | Builds release addon, runs `npm test`, packs root `aidememo-napi`, packs the current platform package, then installs both tarballs into a temp project with offline/no-audit npm flags and verifies `require("aidememo-napi").version()`. Writes timed rows to stdout and `$GITHUB_STEP_SUMMARY`. Local macOS arm64: total 2.81s; build 0.67s; test 1.00s; root pack 0.22s; platform pack 0.53s; install 0.30s. Payloads: root `aidememo-napi-0.1.0.tgz` is 4.18 KB / 4 files / includes README + no `.node`; platform `aidememo-napi-darwin-arm64-0.1.0.tgz` is 2.79 MB / 2 files / includes `aidememo-napi.darwin-arm64.node`; smoke includes `workflowStart(..., bm25Only:true)` and JS Error `code=InvalidArg` with `[entity_not_found]` prefix. |
| `scripts/aidememo-napi-publish.sh` | Shared publish engine with a timed Markdown summary. `AIDEMEMO_NAPI_PUBLISH_MODE=dry-run|publish`, `AIDEMEMO_NAPI_PUBLISH_SCOPE=platform|root|both`, and optional `AIDEMEMO_NAPI_EXPECT_VERSION` gate both local and CI release flows. Local dry-run passed both scopes and wrote stdout + `$GITHUB_STEP_SUMMARY`: total 3.86s, build 0.71s, test 1.13s, platform publish 0.63s, root publish 0.75s. |
| `scripts/aidememo-napi-publish-dry-run.sh` | Wrapper around the publish engine with `AIDEMEMO_NAPI_PUBLISH_MODE=dry-run`. Local macOS arm64: root payload `aidememo-napi@0.1.0`, 4 files, 4.18 KB packed, README included, no `.node`; platform payload `aidememo-napi-darwin-arm64@0.1.0`, 2 files, 2.79 MB packed; payload validators passed for both. |
| `scripts/aidememo-nif-version.sh` | Version gate for the Elixir package. With no args, verifies `Cargo.toml` workspace version equals `crates/aidememo-nif/mix.exs`; with a semver arg, updates both. Latest run: `0.1.0` pinned. |
| `scripts/bindings-release-smoke.sh` | Cross-binding readiness smoke with a timed Markdown summary. Runs `cargo check -p aidememo-python -p aidememo-napi -p aidememo-nif -p aidememo-ffi`, npm version/pack/install smoke, and reports Python/Elixir/C optional package smokes based on local tools. Local macOS arm64 warm default path: cargo check 0.22s, npm version gate 0.05s, npm pack/install smoke 3.05s, total 3.37s; child pack-smoke summaries remain in stdout, but `$GITHUB_STEP_SUMMARY` contains only the one top-level `bindings-release-smoke` table (`rg -n '^## ' "$summary_file"` returns one heading). With `AIDEMEMO_BINDINGS_SMOKE_NPM=0 AIDEMEMO_BINDINGS_SMOKE_OPTIONAL=1`, the Python wheel build, Elixir `mix compile.cargo --force && mix test`, and C FFI smoke all passed. |
| `scripts/sdk-promotion-check.sh` | Package-SDK wording and parity gate for `aidememo-python`, `aidememo-napi`, and `aidememo-agent-sdk`. Default local run: ok=13, ready=3, blocked=2, fail=0, `local_ready=true`, `sdk_promotable=false` because public PyPI/npm installs are not verified. The gate now explicitly checks session-aware writes and pinned context API/docs for Python, Node, and the agent SDK. This does not block positioning `aidememo-agent-sdk` as the agent-facing SDK path. With `AIDEMEMO_SDK_PROMOTION_RUN_SCENARIO_K=1`, Scenario K still covers end-to-end workflow parity. Release preflight runs this gate by default; CI gets the same table in `$GITHUB_STEP_SUMMARY`; set `AIDEMEMO_RELEASE_PREFLIGHT_SDK_PROMOTION=0` only for focused debugging. |
| `.github/workflows/aidememo-napi-artifacts.yml` | Manual/tag workflow builds, tests, packs, and uploads root + platform `aidememo-napi` artifacts on Ubuntu, macOS, and Windows. |
| `.github/workflows/aidememo-python-publish-dry-run.yml` | Manual/tag workflow builds and validates `aidememo-python` PyPI payloads on Ubuntu without uploading. |
| `.github/workflows/aidememo-python-publish.yml` | Manual trusted-publisher workflow. It builds and validates distributions without PyPI permissions, uploads them as artifacts, then publishes via `pypa/gh-action-pypi-publish@release/v1` only when `dry_run=false`. Default `dry_run=true`; real publish requires a PyPI trusted publisher for this workflow and the `pypi-publish` environment. Local artifact-mode check: `AIDEMEMO_PYTHON_DIST_DIR=$(mktemp -d) scripts/aidememo-python-publish-dry-run.sh` produced wheel 2.8M + sdist 349K. |
| `.github/workflows/aidememo-napi-publish-dry-run.yml` | Manual/tag workflow runs the publish dry-run on Ubuntu with `id-token: write` reserved for the later trusted-publisher publish path. |
| `.github/workflows/aidememo-napi-publish.yml` | Manual trusted-publisher workflow. It publishes current-platform packages first, then the root wrapper. Default `dry_run=true`; real publish requires npm trusted-publisher setup for the exact workflow filename, `dry_run=false`, and `version` matching `package.json`. |

Package split: root `aidememo-napi` now ships the generated JS loader, types, README,
and optional dependencies. Platform packages ship exactly one native binary each:
`aidememo-napi-darwin-arm64`, `aidememo-napi-darwin-x64`,
`aidememo-napi-linux-arm64-gnu`, `aidememo-napi-linux-x64-gnu`, and
`aidememo-napi-win32-x64-msvc`. This matches the generated NAPI loader fallback names
and avoids publishing one platform's `.node` binary as the whole package.

Trusted-publisher notes: npm's current guidance requires Node 22.14+ and npm
11.5.1+ for trusted publishing, `id-token: write` in GitHub Actions, a
cloud-hosted runner, and an exact trusted-publisher registration for the
workflow filename. npm also notes trusted publishing automatically generates
provenance for public packages from public repositories, so the release
workflow uses OIDC rather than a long-lived `NPM_TOKEN`.

## Historical Notes

The old scratch-note directory was intentionally removed to keep the repository
focused on durable documentation and executable benchmarks. When adding a new
finding:

1. Put reusable code under `benchmarks/`, `bench/`, or `scripts/`.
2. Store machine-readable outputs under `bench/**/results` or
   `benchmarks/results`.
3. Summarize the user-facing result in this file or in the relevant
   `RESULTS.md`.
