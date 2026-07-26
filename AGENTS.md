# AGENTS.md — AideMemo (aidememo)

Cross-tool agent guide for the `aidememo` Rust workspace. Loaded automatically by
Codex, Cursor, Aider, Jules, and any agent that follows the
[agents.md](https://agents.md) spec. Claude Code imports this via
`CLAUDE.md`.

> Working directory: the AideMemo repository root. Edition 2024, Rust 1.95+ (CI/tooling currently validated on Rust 1.96.0).

## What this project is

AideMemo (`aidememo`) ingests a markdown wiki into a structured knowledge graph
(SQLite by default, redb as an optional Cargo feature, plus BM25 and semantic
vectors) and exposes it to LLM agents via CLI, MCP server, and native bindings
(Python / Node / Elixir / C).

| Crate | Purpose |
|---|---|
| `aidememo-core` | SQLite default store, optional redb store, ingest, search, traverse, lint, validity windows |
| `aidememo-cli` | `aidememo` binary (CLI + stdio/HTTP MCP) |
| `aidememo-napi`, `aidememo-python`, `aidememo-nif`, `aidememo-ffi` | language bindings (full API; SQLite default, optional `redb` Cargo feature) |

## Setup commands

```bash
mise install                                      # sync local tool versions with CI
cargo check -p aidememo-core -p aidememo-cli       # fast verify
cargo build -p aidememo-cli                   # debug binary at target/debug/aidememo
cargo build -p aidememo-cli --release         # release binary
cargo check -p aidememo-python
cargo check -p aidememo-napi
cargo check -p aidememo-nif
cargo check -p aidememo-ffi
cargo test -p aidememo-core --no-default-features --features sqlite
cargo check -p aidememo-core --no-default-features --features redb
cargo check -p aidememo-cli --features s3
cargo test -p aidememo-core --features semantic
cargo test -p aidememo-cli --bin aidememo
./scripts/ci-local.sh lint
./scripts/ci-local.sh demo               # first-run workflow memory smoke
./scripts/ci-local.sh test
mise run changelog-release-check         # verify CHANGELOG.md is cut for current workspace version
./scripts/release-preflight.sh           # full local release gate; includes changelog + registry checks
# Tests that download a HuggingFace embedding model are #[ignore]'d
# (CI skips them — first run hits HF lock races). Run locally for the
# full surface once the model is cached:
cargo test -p aidememo-core --features semantic -- --ignored
cargo build 2>&1 | grep '^error'        # only errors
cargo install --path crates/aidememo-cli      # one-off install
```

### Logging

The CLI uses `tracing` (stderr). Default filter is
`aidememo=info,aidememo_core=warn` so `aidememo mcp-serve` startup, `aidememo watch` file
events, and core fallback warnings (HNSW sidecar missing, reranker
disabled) all appear without setup.

```bash
RUST_LOG=debug aidememo search redis              # standard env
AIDEMEMO_LOG="aidememo=debug,aidememo_core=trace" aidememo query …  # alias if you don't want
                                             # to scope RUST_LOG globally
RUST_LOG=error aidememo mcp-serve                 # silence everything except errors
```

Useful debug spans (turn on with `RUST_LOG=aidememo_core=debug`):

- `hybrid_search` — wraps every `aidememo search --hybrid` call. Inside it
  you'll see `embed_provider loaded ms=…` (model load on first call,
  near-zero after) and per-phase events `bm25` / `query_embed` /
  `hnsw_lookup`.
- `StoreKind::open` / backend `open` — directory create + backend schema init.

The legacy `AIDEMEMO_SEARCH_PROFILE=1` and `AIDEMEMO_LINT_PROFILE=1` env vars
still work as a self-contained `eprintln` dump for users who'd
rather not configure tracing.

## CLI surface (post-Tier-3)

### Setup / init
```
aidememo init <wiki-root> [--no-ingest]                  create store + ingest markdown
aidememo init --agent codex <wiki-root>                  init + register aidememo MCP for an agent
aidememo init --agent claude --agent-force <wiki-root>   overwrite existing agent MCP config
aidememo --store <PATH> mcp-install --target codex       pin the resolved store in Codex MCP config
         [--source-id ID] [--codex-home PATH --actor-id ID]...
                                                          install isolated Codex profiles with source scope + writer provenance
```

### Read / search
```
aidememo search <query> [-l N] [--current] [--bm25-only] [--hybrid] [--source-id ID] [--include-archive] [--via URL]
                                                   auto-hybrid by default: probe BM25 first, then
                                                   promote only when lexical evidence is weak.
                                                   CJK queries can promote more eagerly, but stay
                                                   lexical when BM25 evidence is strong.
                                                   --bm25-only = deterministic
                                                   no-model fast path. --hybrid = force semantic
                                                   retrieval on every query. --include-archive = also search the
                                                   backend-specific cold tier (`<store>.cold.redb`
                                                   or `<store>.cold.sqlite`) and merge any
                                                   matches in. --via = dispatch via running
                                                   `aidememo mcp-serve` daemon (warm model, ~5-50 ms)
                                                   --source-id scopes retrieval to one source /
                                                   tenant / upstream namespace.
aidememo query <topic> [-l N] [-d N] [--recent-limit N] [--bm25-only] [-m naive|local|hybrid|global] [--source-id ID]
                                                   unified search+traverse+recent.
                                                   --bm25-only skips semantic
                                                   embedding lookup for
                                                   deterministic demos/hooks.
aidememo recent [-n N] [--type T] [--last 30d]          last 7d facts (default)
aidememo traverse <entity> [-d N] [--source-id ID]       forward graph walk
aidememo path <from> <to> [--source-id ID]               shortest path
aidememo graph [--from E] [--depth N] [--format mermaid|dot] [--source-id ID]
aidememo entity get <NAME> [--source-id ID]
aidememo entity list [--type T] [--limit N] [--source-id ID]
aidememo entity show <NAME> [--recent N] [--source-id ID]
aidememo fact get <ID> [--source-id ID]
aidememo fact list [--type T] [--entity E] [--source-id ID]
aidememo fact pinned [--limit N] [--source-id ID]
aidememo stats                                           counts + size
```

### Write
```
aidememo fact add <content> --entities A,B [--type T] [--source-id ID] [--actor-id ID]
                                                   auto-creates missing entities; optional
                                                   source_id owns dedup + scoped retrieval.
aidememo fact delete <ID> [--source-id ID]                source-checked destructive delete
aidememo fact feedback <ID> [--helpful] [--source-id ID]  source-checked direct feedback
aidememo fact supersede <OLD_ID> <NEW_ID> [--source-id ID] validity-window invalidate
aidememo fact pin|unpin <ID> [--source-id ID]             source-checked always-loaded tier
aidememo fact archive --ids <ID,…> [--source-id ID]       move to backend-specific cold tier
                                                   (`<store>.cold.redb` or `<store>.cold.sqlite`;
                                                   preserves FactId)
aidememo fact archive --older-than 30d [--type note] [--source-id ID]
                                                   bulk move by age (+ optional type/source filter)
aidememo edit fact <ID> --append/--prepend/--find+--replace/--content
aidememo entity add <name> [--type service]               custom types accepted
aidememo entity describe <name> "..." | --from-stdin | --clear   compiled-truth summary
```

### Maintenance
```
aidememo doctor [--json]                       friendly health check (lint + memory +
                                          agent setup + feedback-adapter status)
aidememo lint [--json]                          raw lint issues
aidememo bench <golden.jsonl> [--k 5] [--limit N]
                                          P@K / R@K / latency benchmark
aidememo skill check <path> [--json]            validate SKILL.md files
aidememo backup create <DIR|s3://bucket/prefix> [--json]
                                          consistent hot + existing cold-tier
                                          SQLite snapshots with byte counts and
                                          SHA-256 in one manifest.
                                          S3 targets require a CLI built with
                                          `--features s3`.
aidememo backup restore <DIR|s3://bucket/prefix> --force [--json]
                                          verify all manifested payloads + SQLite
                                          integrity_check, replace selected hot/
                                          cold stores, and remove stale WAL/SHM/
                                          HNSW sidecars. SQLite backend only.
aidememo branch push --branch ID [--base DIR|s3://bucket/prefix] <DIR|s3://bucket/prefix> [--json]
                                          export this store's append-only branch
                                          segment. With --base, emits changes
                                          after that backup manifest's sync cursor;
                                          without --base, emits a full sync segment.
                                          Use for cloud agents or what-if memory
                                          experiments, then merge only the winner.
aidememo branch merge [--branch ID] <DIR|s3://bucket/prefix> [--json]
                                          verify branch segment manifests and
                                          import them idempotently via sync_import.
                                          S3 targets require `--features s3`.
aidememo ingest <root> [-i]                     ingest markdown
aidememo watch <root> [--search Q]              live re-ingest + optional live search
aidememo vector-rebuild [--current-only] [--json]  rebuild HNSW from scratch (after model swap).
                                            --current-only skips superseded facts — pair with
                                            `aidememo consolidate --gac` to actually shrink the index
                                            to the representative set (default keeps all facts
                                            so `as_of` historical retrieval keeps working).
aidememo auto-relate [--top-k 3] [--threshold 0.0] [--dry-run] [--json]
                                            mine `related` edges from semantic similarity
                                            (one-shot; idempotent; semantic feature only)
aidememo overview [-n 10] [--recent-days 7] [--json]
                                            first-impression snapshot: entity-type buckets,
                                            fact-type distribution, top entities, recent
                                            activity. One call instead of stats + entity_list
                                            + fact_list.
aidememo consolidate [--semantic-threshold 0.85] [--ttl TYPE=DAYS] [--dry-run] [--json]
                                            periodic memory-lifecycle pass: pairwise cosine
                                            dedup (older fact superseded) + per-type TTL
                                            expiry. Idempotent. Mirrors OMEGA's compaction.
aidememo consolidate --gac [--gac-theta 0.85] [--gac-spread-budget N] [--gac-cold-tier]
                     [--gac-protect TYPE,TYPE,...]
                                            cluster-aware consolidation per the GAC
                                            (Geometry-Aware Consolidation, NeurIPS 2026)
                                            paper: tight clusters (d̄ < 1-θ) collapse to
                                            newest fact; spread clusters keep medoid +
                                            budget residuals. --gac-cold-tier moves losers
                                            to the backend-specific cold tier (preserves FactId) instead
                                            of superseding. --gac-protect skips named fact
                                            types entirely (use `preference,lesson,error`
                                            on personalisation-tier stores — LongMemEval-S
                                            120q showed even θ=0.95 GAC drops 1 SS-pref q
                                            because near-paraphrases ARE the recall signal
                                            for that category). --dry-run for analysis-only.
                                            Pair with `aidememo vector-rebuild --current-only` to
                                            shrink the HNSW index proportionally — supersede
                                            alone leaves losers in the sidecar.
aidememo session new <topic> [--source-id ID]     tracked session entity + shell-evaluable
                                            'export AIDEMEMO_SESSION_ID=…'. Auto-attaches to every
                                            subsequent fact_add while the env var is set.
aidememo session start [--source-id ID]           scoped warmup envelope.
aidememo session current|list [--source-id ID]    current/recent tracked sessions.
aidememo session canvas [SESSION] [--source-id ID]
                                            auditable Markdown + Mermaid session artifact.
aidememo session resume SESSION [--source-id ID] validate an existing session + print shell exports
                                            for both session continuity and source scope.
aidememo session handoff SESSION [--dispatch --from-actor ID --to-actor ID]
                                            preview a bounded packet; optional dispatch stores
                                            only a pull-based session assignment pointer.
aidememo agent add|list|show|remove                friendly account/runtime profile surface.
aidememo installation add|list|show|remove         credential-free Codex/Claude runtime profiles.
aidememo handoff send AGENT [SESSION]              infer destination metadata and dispatch the session.
aidememo handoff inbox|accept|return|outbox|show|status
                                            receiver/sender round trip with result fact link;
aidememo handoff run AGENT [HANDOFF_ID]             execute oldest pending external-worker assignment;
                                            not a message queue, auto-retry, or task-success proof.
aidememo workflow start <TITLE> [--body-file issue.md] [--source github:org/repo#123]
                                            [--source-id ID] [--actor-id ID]
                                            [--parent-session SESSION]
                                            sparse issue/ticket entry point: create session,
                                            store trigger, return project context pack.
                                            Use --bm25-only for deterministic demos/hooks
                                            that should skip embedding-model load.
aidememo profile export [--source-id ID]          read-only typed-fact profile artifact.
aidememo export [--scope all|...] / aidememo import
aidememo config get/set/list

# Speed/safety knob: drop per-commit fsync. Survives process crash
# (page cache outlives it), but power loss within ~30s of a write
# can lose recent commits. About 10× faster than the default.
aidememo config set store.durability eventual    # opt in
aidememo config set store.durability immediate   # default (recommended)

# Privacy guard — disabled by default. It does not call an external
# LLM; when enabled it calls a local OpenAI Privacy Filter-compatible
# sidecar before fact persistence. CPU OPF is too heavy for default-on
# (~261 ms p50 writes, ~3.8 GB RSS). On Apple Silicon, prefer the
# measured MLX mxfp4 sidecar for opt-in shared/project stores
# (~51 ms p50 fact_add vs ~22.5 ms baseline, ~1.28 GB RSS).
#
python3 scripts/privacy_filter_mlx_sidecar.py \
  --model-dir /private/tmp/openai-privacy-filter-mlx-mxfp4 \
  --port 8091
aidememo config set privacy.provider openai-privacy-filter
aidememo config set privacy.endpoint http://127.0.0.1:8091
aidememo config set privacy.mode redact
#
# Local deterministic secret-prefix detection runs before sidecar policy, so
# common bare key prefixes such as sk-proj-, sk-, github_pat_, ghp_, gho_,
# xoxb-, AKIA, and AIza still hit the default secret block policy.
# Detail: docs/OPERATIONS.md, "Use write-time privacy filtering".

# Embedding model — when to switch off the model2vec default:
#
# Default is `model2vec` / `potion-multilingual-128M` (28 MB,
# HashMap lookup, ~3 ms/query warm). Robust across languages,
# adequate for most workloads.
#
# Switch to fastembed `bge-small-en-v1.5` when the workload is
# **paraphrase-bridge dominant**: the user's question abstracts
# away from the surface form of the answer ("What's my favorite
# camera setup?" → answer in turns naming "Sony A7R IV" by
# model number; the question never repeats the model name).
# That's the LongMemEval / DMR / personal-memory shape — and
# it's where BGE's English-tuned semantics close the gap that
# multilingual potion's lookup-based vectors miss.
#
aidememo config set model.provider fastembed
aidememo config set model.name bge-small-en-v1.5    # 133 MB, ONNX
#
# Stay on the default when the workload is **surface-form-match
# dominant**: code / docs / news RAG where the question shares
# tokens with the answer (HotpotQA bridge, MultiHop-RAG).
# BM25 already lands those at R@5 ≥ 95%; embedding semantics
# are transparent and you save the latency.
#
# Measured (LongMemEval-S 500q R@5):
#   model2vec + HNSW            96.2%
#   bge-small-en + HNSW         98.0%   (+1.8pt; SS-pref 93.3 → 100)
#                                       beats gbrain-hybrid 97.6%
# Cross-bench validation:
#   MultiHop-RAG 2,556q   model2vec 93.7% = BGE 93.7% (saturated)
#   HotpotQA 7,405q       model2vec 95.8% = BGE 95.8% (saturated)
#
# Trade-off: ~30 ms/query warm vs ~3 ms for model2vec (10×, still
# well under reader latency but accumulates if the agent loops
# hundreds of queries per turn). Multilingual repos (Korean,
# Japanese) keep model2vec — bge-small-en is English-only.
#
# Detail: docs/MEASUREMENTS.md, "Model And Rerank Trade-offs".

# HuggingFace text-embeddings-inference (TEI):
#   model.provider = "tei"         # native /embed + /info dimension auto-discover
#   model.endpoint = "http://host:8080"
#   rerank.provider = "tei"        # cross-encoder rerank of top-K (BGE-reranker, etc.)
#   rerank.endpoint = "http://host:8081"
#   rerank.model    = "BAAI/bge-reranker-base"
#   rerank.top_k    = 32
# Reranker failure is non-fatal — AideMemo falls back to RRF order with a warning.
#
# When to enable rerank — measured trade-offs (default: off):
#   * Retrieval-bound workload: corpus where top-K recall is < 95%
#     (e.g. Wikipedia-scale BM25-only on MIRACL/ko: R@10 ~0.81).
#     Rerank lifts MRR @+5-6%, R@1 @+10-30%. KEEP ON.
#   * Reader-bound workload: corpus where top-K recall is already
#     saturated (e.g. LongMemEval per-question haystack: R@10 1.00
#     after dates default). Rerank reorders the head but the
#     reader doesn't see the difference (top-K context overlaps);
#     measured 60q drop -2pt and 5× latency. KEEP OFF.
#   * Latency budget: cross-encoder is ~85× slower than HNSW alone
#     (Apple Metal: 9 ms → 765 ms p50 at top_k=8). Most agent hot
#     paths can't afford it. Reserve for offline batch / high-stakes
#     low-volume queries. Detail in `docs/MEASUREMENTS.md`.
```

### Multi-project
```
aidememo project list/show/create/use/remove
aidememo --project NAME ...                     one-off override
aidememo --store PATH ...                       absolute path override
aidememo --backend libsqlite --store PATH ...   one-off storage backend override
```

### Servers
```
aidememo mcp                                    stdio JSON-RPC (preferred)
aidememo mcp-serve --port 3000                  HTTP + SSE; /health + /admin/status
         [--auth-bindings-file PATH]            bind tokens to source_id + actor_id
```

### Daemon (background mcp-serve, opportunistic discovery)

`aidememo daemon` wraps `aidememo mcp-serve` so manual CLI calls (`aidememo search`,
…) auto-pick up the warm path without `--via`. Pattern follows
docker / pg_ctl: one shell, one daemon, the rest of the machine
just uses it.

```
aidememo daemon start                          spawn mcp-serve in background;
                                         ~/.aidememo/daemon.json holds port/pid
aidememo daemon status                         show registry + /health probe
aidememo daemon stop                           SIGTERM the recorded pid
```

After `aidememo daemon start`, `aidememo search redis` automatically dispatches
to the daemon over HTTP (no `--via` needed). Measured warm:

| call | latency |
|---|---|
| `aidememo search redis` (BM25 via daemon)        | ~9 ms |
| `aidememo search redis --hybrid` (HNSW via daemon) | ~45 ms |
| `aidememo search redis` (no daemon, fresh CLI)   | ~70-300 ms |

Set `AIDEMEMO_NO_DAEMON=1` to bypass discovery for one invocation. Note:
for optional redb stores, an in-process `aidememo` cannot open the store while
the daemon holds it because redb is single-writer. For SQLite stores, bypassing
is mainly useful when you explicitly want a fresh process instead of the warm
daemon path.

## Code map

```
crates/aidememo-core/src/
  lib.rs        AideMemo public API (re-exports)
  sqlite_store.rs SQLite CRUD (default backend)
  store.rs      redb CRUD (optional backend; entity/fact/relation/alias index)
  graph.rs      traverse / path_find
  search.rs     BM25 + semantic hybrid (current_only filter, time windows)
  ingest.rs     markdown → entity/fact/relation parser (frontmatter dates)
  privacy.rs    optional write-time privacy guard (local OPF sidecar + secret prefilter)
  types.rs      EntityType (Custom variant), FactRecord (superseded_at/by),
                QueryOpts (current_only), …
  config.rs     Config { store, model, search, privacy, lint, projects, default_project }
  error.rs      AideMemoError + Result<T>
  lint.rs       graph health checks
  migrate.rs    schema migrations

crates/aidememo-cli/src/
  main.rs            command dispatch
  output.rs          Format::{Table, Json} renderers + format_query_result
  cmd/mod.rs         bpaf top-level + per-command parsers (--project / --json)
  cmd/{init,watch,model,feedback,adapt,doctor,recent,edit,graph,project}.rs
  cmd/mcp_tools.rs   shared MCP JSON-RPC types + 29-tool dispatch
  cmd/mcp_stdio.rs   `aidememo mcp` (stdio)
  cmd/mcp_serve.rs   `aidememo mcp-serve` (HTTP + SSE)
```

## House rules

### bpaf 0.9

- **Positional/command items must be the rightmost fields** in the struct, and
  `construct!` argument order must match field order. Violation panics at
  runtime with `bpaf usage BUG`.
- Don't put `module::function()` calls inside `construct!([...])`. Bind a local
  variable first (`let init_cmd = init::init_command();`) then reference the
  variable.
- `construct!` field-rename syntax (`{ field: var }`) is **not** supported —
  use a variable named the same as the field.
- Parser return type is `impl Parser<Command>`, not the concrete type.

### Storage backends / aidememo-core

- `AideMemo` wraps `Arc<RwLock<StoreKind>>`; `StoreKind` dispatches to SQLite
  by default or redb when the `redb` Cargo feature and `store.backend = "redb"`
  are selected.
- `AideMemo::ingest` is `&self` (uses interior mutability via the RwLock).
  This makes `Arc<AideMemo>` callable from bindings.
- Only `StoreKind::open(path, config)` exists at the public dispatch layer — no
  `new` / `get_or_create`.
- In the redb implementation, read txn and write txn cannot be nested. Drop the
  txn before opening another.
- All persisted records are JSON (not bincode), so adding `#[serde(default)]`
  fields to types is fully backward-compatible — no migration needed.
- `EntityType::Custom(String)` and `FactRecord.superseded_at/by` rely on this.

### Errors / lints

- Workspace lints forbid `unsafe_code` and deny `unwrap_used`, `panic`,
  `dbg_macro`. Use `?` with `AideMemoError` variants instead.
- `Config.store.path` is a `String` — convert with `PathBuf::from(&…)`.
  `Config::default_store_path()` and `Config::project_path()` already
  expand `~/`.
- Keep store suffixes aligned with the selected backend (`.sqlite` for
  SQLite / `libsqlite`, `.redb` for redb). The engine does not require it, but
  `aidememo doctor` warns on mismatches because they hide which persistence
  layer owns the file.
- Time helpers: `parse_iso_to_epoch_ms` (YYYY-MM-DD or RFC3339),
  `parse_duration_to_ms` (`30d`, `12h`, `4w`, `1y`).

## MCP integration

`aidememo` ships two MCP transports backed by the same tool dispatch
(`cmd/mcp_tools.rs`):

| Subcommand | Transport | When to use |
|---|---|---|
| `aidememo mcp` | stdio (newline-delimited JSON-RPC) | local agents (Claude Code, Codex CLI) |
| `aidememo mcp-serve --port 3000` | HTTP POST `/mcp` + SSE `/sse` | browser/remote clients |

**Tool surface — 7 core, then standard, then advanced.** Agent prompts
should lead with the core 7; the rest are there when needed.

**Core (90% of agent traffic)**:

| Tool | When |
|---|---|
| **`aidememo_workflow_start`** | **🟢 Ticket/issue/PR automation entry point** — create tracked session + store trigger text + return decisions/lessons/errors/search context. |
| **`aidememo_handoff`** | **🟢 Orchestrator routing point** — preview a tracked-session packet or dispatch a pointer to another account/agent/profile. `session_id` is continuity, `source_id` is scope, and actor/agent/profile fields are routing metadata. |
| **`aidememo_handoff_inbox`** | **🟢 Round-trip routing point** — receiver list/accept/return and sender outbox/status for `actor_id` / `AIDEMEMO_ACTOR_ID`. Results link to facts; there are no topics, offsets, retries, or copied payloads. |
| **`aidememo_context`** | **🟢 Top-of-turn entry point** — pinned + personalisation + recent + (with topic) search/traverse/lessons. One round-trip. |
| **`aidememo_query`** | Topic dive after aidememo_context. `format:"text"` for compact prompt injection; `max_chars:N` for hard caps; `level:"entity"`/`"session"` to group by entity / by tracked session. |
| **`aidememo_aggregate`** | Deterministic counting / sum / timeline across facts. Pulls agent out of in-head arithmetic for "how much / how many / between when" questions. ops: `count`/`enumerate`/`by_entity`/`sum_currency`/`sum_duration`/`count_distinct_dates`/`timeline`. |
| **`aidememo_fact_add`** | Append a fact (auto-creates entities, `preference`/`lesson`/`error` types). Auto-attaches to `AIDEMEMO_SESSION_ID` when the env var is set. |

**Standard (frequent, but optional in any single turn)**:

| Tool | When |
|---|---|
| `aidememo_search` | Pure hybrid search, no graph wrapper — fastest pinpoint lookup. |
| `aidememo_recent` | Last N days of facts. |
| `aidememo_overview` | First-impression snapshot for an unfamiliar wiki — entity-type buckets, top centrals, recent activity. |
| `aidememo_traverse` | Walk the graph from a known entity. `direction:"reverse"` for "what depends on X". |
| `aidememo_doctor` | Health snapshot — counts + lint issues + per-code action hints + shared-store `sharing` guidance (`lock_retry_ms`, daemon state, serverless writer envelope). |
| `aidememo_entity_get` | Fetch one entity (name / alias). |
| `aidememo_fact_get` | Fetch one fact by ULID. |
| `aidememo_entity_list` | Browse entities. |

**Advanced (write-side power tools, batch ops, niche)**:

| Tool | When |
|---|---|
| `aidememo_fact_add_many` | Bulk fact insert in one transaction. |
| `aidememo_fact_supersede` | Mark an old fact replaced by a new one (validity-window invalidate). |
| `aidememo_fact_edit` | Patch fact content (append/prepend/find+replace/content). |
| `aidememo_fact_archive` | Move facts to cold-tier (`<store>.cold.redb` for redb, `<store>.cold.sqlite` for SQLite) — hot store shrinks, FactId preserved for `aidememo_fact_get`. Pass `include_archive:true` on `aidememo_search` / `aidememo_query` to merge cold matches back in. |
| `aidememo_fact_pin` | Pin / unpin a fact for the always-on tier. |
| `aidememo_fact_list` | Paginated fact list with filters. |
| `aidememo_pinned_context` | Just the pinned tier (subset of aidememo_context). |
| `aidememo_session_start` | Pinned + recent only (subset of aidememo_context). |
| `aidememo_path` | Shortest-path between two entities. |
| `aidememo_entity_describe` | Set / clear an entity's prose summary. |
| `aidememo_extract` | Run heuristic / optional LLM fact extraction over raw text. |
| `aidememo_feedback` | Record positive / negative feedback on a search hit (training signal). |

Tool schemas live in `cmd/mcp_tools.rs::list_tools()`. The deprecated
`aidememo_lint` and `aidememo_backlinks` tools were removed in favour of
`aidememo_doctor` and `aidememo_traverse(direction:"reverse")`.

### Agent-UX cheatsheet (Tier A+B additions)

**Fact types — pick the right one** (auto-boosts + decay-exempt):
| Type | Use when | Behaviour |
|---|---|---|
| `decision` | "we'll use X for Y" | atomic per entity (with `lifecycle.auto_supersede_atomic_types=true`); 2× boost; decay-exempt |
| `convention` | "always X" / "format Y as Z" | atomic per entity; 2× boost; decay-exempt |
| `pattern` | architectural pattern, "X uses Y for Z" | 1.5× boost; decay-exempt |
| **`preference`** | first-person preference ("I prefer dark mode") | **2× boost; decay-exempt; surfaced in personalisation** |
| **`lesson`** | tried-X-hit-Y, learned pattern | **2× boost; decay-exempt; surfaced in personalisation + topic_lessons** |
| **`error`** | recurring failure mode to avoid | **2× boost; decay-exempt; surfaced in personalisation + topic_errors** |
| `claim` | factual assertion | 1× weight |
| `note` | observational | 1× weight (default fallback) |
| `question` | open investigation | 0.5× (deprioritised) |

**Tracked sessions** — every `aidememo fact add` auto-attaches a session entity while `AIDEMEMO_SESSION_ID` is in the env:
```bash
eval "$(aidememo session new 'auth migration')"
aidememo fact add 'Decided to use Keycloak for SSO' --type decision --entities Auth
aidememo fact add 'Tried bare-metal Postgres, hit IO limits at 5k qps' --type lesson --entities Postgres
aidememo session current     # show topic + fact count attached so far
aidememo session list        # all tracked sessions
aidememo fact list --entity $AIDEMEMO_SESSION_ID  # pull one session's full thread
```

**Lifecycle / consolidation** — periodic batch dedup + TTL pass:
```bash
aidememo consolidate --semantic-threshold 0.85          # OMEGA-style dedup
aidememo consolidate --ttl note=30 --ttl question=14    # per-type expiry
aidememo consolidate --semantic-threshold 0.85 --ttl note=30 --dry-run   # preview
```
Plus: `aidememo config set lifecycle.auto_supersede_atomic_types true` makes `aidememo fact add` of a `decision`/`convention` auto-supersede the existing same-type fact on the same entity (off by default — preserves the historical "every fact_add creates a new fact" contract).

**LLM-aided extractor (opt-in)** — when `extract.provider = "openai"`:
```bash
aidememo config set extract.provider openai            # uses OPENAI_API_KEY
aidememo config set extract.model gpt-4o-mini          # default
aidememo extract --llm 'long chat transcript here…'    # CLI
# MCP: aidememo_extract { llm: true, text: "…" }

# Non-interactive review queue for hooks / auto-capture:
aidememo pending list --json
aidememo pending stats --json
aidememo pending approve --indices 1,3-5 --json
aidememo pending reject --all --json
```

**Auto-context hooks (Claude Code)** — see `aidememo-skill/hooks/README.md` for installation:
| Hook | Effect |
|---|---|
| `SessionStart` | injects `aidememo overview` + `aidememo recent` + pinned facts as `additionalContext` |
| `PostToolUse` (Edit/Write) | surfaces AideMemo facts related to the just-edited file |
| `UserPromptSubmit` | extracts candidate facts from the prompt (preview only; opt into LLM via `AIDEMEMO_EXTRACT_LLM=1`) |

### Agentic-loop pattern — when to call `aidememo_aggregate` mid-turn

For most user questions, a single `aidememo_context` (opening turn) or `aidememo_query`
(follow-up) round-trip yields enough context for the reader to answer
directly from snippets. Don't reach for `aidememo_aggregate` unless the question
shape demands deterministic arithmetic across facts:

| Question shape | Tool | Rationale |
|---|---|---|
| "What did I say about X?" | `aidememo_query` | Simple recall — read the snippet |
| "When did I last do Y?" | `aidememo_query` (level=fact) | Single-fact lookup |
| "What's my preference for Z?" | `aidememo_context` (personalisation tier) | Pre-surfaced |
| **"How much $ total did I spend on X?"** | `aidememo_aggregate(op=sum_currency)` | Arithmetic across N facts |
| **"How many hours of Y total?"** | `aidememo_aggregate(op=sum_duration)` | Time accumulation |
| **"How many distinct days had event Z?"** | `aidememo_aggregate(op=count_distinct_dates)` | Set-cardinality across facts |
| **"Timeline of all X events"** | `aidememo_aggregate(op=timeline)` | Chronological ordering |
| **"How many times did I decide X?"** | `aidememo_aggregate(op=count, fact_type=decision)` | Bounded enumeration |

LongMemEval measurement (240q balanced, MiniMax temp=0, 3-run mean,
reproducibility-fixed bench): the agentic-loop variant is **within
reader noise** of the omega-style single-call baseline (-1.9pt vs
mean, σ=±1.1pt). The +30pt multi-session lift seen in our earlier
60q run was **lucky-sample variance** — at 240q with stable retrievals
the multi-session category sits at baseline (20-22/40 either way).
What does survive across scales: SS-pref / temporal regressions when
agentic-loop is forced everywhere (JSON tool-call overhead measurably
scrambles single-fact reasoning). And one suggestive within-noise
positive: KU "How many X?" counting questions tend to land 1-2pt
above baseline when the agent uses `aidememo_aggregate(op=count|enumerate)`.

Auto-dispatch via single-shot LLM classifier ("does this question
need aggregation? YES/NO") nets to baseline at 240q (+0pt mean,
classifier precision ≈40%; KU/temporal "false positives" are
genuinely counting-shaped, not classifier mistakes). Don't bake
auto-dispatch into aidememo — pay no extra round-trip for zero lift.

The trigger table above remains the right pattern, but treat
`aidememo_aggregate` as **insurance** for cross-fact arithmetic, not a
guaranteed accuracy lever. Most lift in our measurements comes from
`aidememo_query` granularity / `level` and ingest hygiene, not from
mid-turn tool dispatch.

Implementation pattern in your reader prompt (recommended):

```
You have access to aidememo_aggregate(op=sum_currency|sum_duration|count_distinct_dates|timeline|count).
Call it when the user's question requires summing or counting across
facts (e.g., 'how much total', 'how many distinct days'). Otherwise
answer directly from the snippets you already have.
```

### Self-extraction pattern — agent classifies, aidememo stores

`aidememo` deliberately does **not** ship a built-in LLM-aided ingest
pipeline (the kind Mem0 / Letta have a dedicated extractor for).
The reason is structural: every agent that calls `aidememo_fact_add`
already has its own LLM, and that LLM is almost always a stronger
model than AideMemo could ever embed. Asking the calling agent to do the
classification keeps aidememo local-first and free of API-key /
extractor-quality coupling, while still benefiting from the
agent's reasoning.

What this means for the calling agent:

* Before `aidememo_fact_add`, decide which `fact_type` fits — the trigger
  cues are baked into the tool description, but here's the short
  form for prompt injection:

  | Cue in user / assistant text | fact_type |
  |---|---|
  | "I prefer X" / "my favorite is Y" / "I like Z" | preference |
  | "I decided to X" / "I chose Y" / "going with Z" | decision |
  | "tried X but Y" / "turns out" / "wish I had" | lesson |
  | "avoid X" / "never again" / "was a mistake" | error |
  | "always X" / "every time" / "I never X" | convention |
  | "X uses Y for Z" (architectural) | pattern |
  | factual without opinion | claim |
  | catch-all | note |

* Pass the entity hints in the same call (`entities: [...]`) — aidememo
  auto-creates and aliases.

* `aidememo_fact_add_many` gets the same self-extracted classification —
  the batch round-trip is for fsync amortisation, not classification.

This is the pattern you should follow rather than reaching for a
heavier ingest framework. aidememo's bench measurements show the in-pipeline
weighting (decay-exempt + 2× boost on personalisation tiers like
preference / lesson / error) materially shifts ranking — but only
when those types are populated correctly. Ship-default `note`
classification leaves the boost dormant.

Historical caveats from our measurements:

* Routing extraction through a same-class extractor (MiniMax →
  MiniMax via `aidememo_extract --llm`) regressed LongMemEval 60q by
  ~13pt because the extractor *rewrites* facts (paraphrases /
  summarises) and the reader can no longer match rewritten
  extracts to the raw turns.
* Same model, classification-only (label fact_type without
  rewriting content): also regressed −6pt on 60q. The boost on
  correctly-labelled `preference` / `lesson` / etc. didn't
  outweigh the noise from false-positive labels at MiniMax's
  classification quality.

The takeaway isn't "self-extraction is bad" — it's "the calling
agent's classifier needs to be at least as strong as the reader
that consumes the result". In practice production agents call aidememo
from Claude Opus / GPT-5-class models, well above the bench reader
class we measured against, so the lift hypothesis remains
plausible — just unmeasured at this quota window.

### Register with Claude Code
```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target claude \
  --source-id project:my-app \
  --actor-id claude:local
# Or install the repository's plugins/claude bundle for MCP + skills + hooks.
```

See `docs/CODING_AGENTS.md` for Claude Code, Codex, Hermes, pi, Cursor,
OpenClaw, and OpenCode setup. pi uses its native skill and local CLI rather
than MCP.

### Register with Codex CLI
Add to `~/.codex/config.toml`:
```toml
[mcp_servers.aidememo]
command = "aidememo"
args = ["--backend", "libsqlite", "mcp"]
```

### Multi-agent shared store

SQLite is the default backend and uses `store.lock_retry_ms` as its busy timeout
for short write collisions. The optional redb backend holds an **exclusive file
lock per process** and uses the same setting to retry opening the store while
another process holds that lock. Two ways to handle shared writes:

1. **Single shared `aidememo mcp-serve` instance** (recommended for shared
   writes). Run one HTTP/SSE server, point every agent at the same
   URL. Each agent's MCP client sees the same tools, the same store,
   no lock contention.
   ```bash
   aidememo --store ~/.aidememo/team.sqlite mcp-serve --port 3000 &
   # In each agent's MCP config: type=http, url=http://localhost:3000/mcp
   ```
2. **Brief opportunistic contention** is fine: set
   `store.lock_retry_ms` so transient local-store write contention can wait
   instead of failing immediately.
   ```bash
   aidememo config set store.lock_retry_ms 5000   # 5s budget
   ```
   `lock_retry_ms = 0` (default) preserves the old fail-fast behaviour.
   Scenario J (`python3 bench/multi-agent/scenario_j_lock_retry_sweep.py`)
   measures the optional redb backend's serverless lock-retry path as smooth
   through **4 concurrent local writers** (40/40 persisted, p95 1.28s). At 8
   writers it still recovered 79/80 writes but p95 rose to 2.99s and one write
   exhausted the 5s budget, so use the shared daemon when that level of
   parallelism is normal. `aidememo doctor --json` exposes the same threshold
   in its `sharing` block. The default SQLite backend's concurrent HTTP write
   path is covered by `scripts/storage-backend-sqlite-mcp-soak.sh`.

Don't try to give multiple agents their own stdio `aidememo mcp` against the
same optional-redb path — one process owns the redb file lock and the others
will fail to open or return explicit lock errors. For default SQLite stores,
HTTP `mcp-serve` is still the simplest shared warm process.

### Hardened mcp-serve

`aidememo mcp-serve` defaults to `127.0.0.1`. The following options govern
exposure and caller identity:

| Flag | Default | Effect |
|---|---|---|
| `--bind ADDR` | `127.0.0.1` | Loopback only. Pass `0.0.0.0` to expose on the LAN |
| `--auth-token TOKEN` | unset | Bearer token literal — exposed in shell history / `ps aux`, only use for ad-hoc tests |
| `--auth-token-file PATH` | unset | Read the token from a file (mode 0600 recommended). Production-friendly |
| `--auth-bindings-file PATH` | unset | JSON token bindings with fixed `source_id` + `actor_id`; caller overrides are rejected |
| `AIDEMEMO_MCP_AUTH_TOKEN` env | unset | Single unscoped-token fallback |
| `AIDEMEMO_MCP_AUTH_BINDINGS_FILE` env | unset | Token-bindings file fallback when no explicit single-token option is set |

Non-loopback bind without an auth token is **refused at startup** —
the server won't expose an unauthenticated store on the network.
Loopback + no token is fine for single-host multi-agent. TLS / rate limiting
are deliberately out of scope; put a reverse proxy (caddy / nginx) in front if
you need them.

```bash
# Same-host: agents go through loopback, no token needed
aidememo mcp-serve --port 3000
curl http://127.0.0.1:3000/admin/status   # request counts, auth mode,
                                           # store path, sync cursor status

# Multi-host team server: token in a file, never on the command line
aidememo auth generate > /etc/aidememo/team-token   # one-time: emit 64-char hex
chmod 600 /etc/aidememo/team-token
aidememo mcp-serve --port 3000 --bind 0.0.0.0 --auth-token-file /etc/aidememo/team-token

# Multi-source server: each token is pinned to one retrieval/writer identity.
# File shape: {"tokens":[{"token":"...","source_id":"team-a","actor_id":"codex:a"}]}
chmod 600 /etc/aidememo/token-bindings.json
aidememo mcp-serve --port 3000 --bind 0.0.0.0 \
  --auth-bindings-file /etc/aidememo/token-bindings.json
```

Bound credentials inject their configured identity into every MCP tool call,
including each `aidememo_fact_add_many` item, and reject mismatches. They cannot
call the unscoped `/sync/since` or `/admin/status` routes; `/health` returns only
health and semantic-prewarm state. Use a single-token credential only for a
trusted unscoped administrator. Semantic prewarm starts in a blocking-worker
task after the listener binds, so `/health` does not wait for model cold load;
`/admin/status.semantic_prewarm` reports `warming|ready|failed|disabled`.

### Token UX — `aidememo auth`

Operators rarely want to type bearer tokens. `aidememo auth` ships four
small commands so the secret never has to live in shell history:

```bash
aidememo auth generate                        # → 64-char hex (32 random bytes)
aidememo auth login http://team-host:3000 \
        --token-file /etc/aidememo/team-token  # store in ~/.aidememo/auth.json (0600)
aidememo auth list                            # redacted preview of stored URLs
aidememo auth logout http://team-host:3000    # forget one
```

After `aidememo auth login`, every `aidememo sync pull <URL>` reads the token
transparently — no `--token` / env var on the call:

```bash
aidememo sync pull http://team-host:3000      # uses stored token
```

Token resolution chain in `aidememo sync pull` (highest precedence first):

1. `--token TOKEN` (literal, exposed in `ps aux` — discouraged)
2. `--token-file PATH` (read + trim)
3. `AIDEMEMO_MCP_AUTH_TOKEN` env var
4. Stored entry in `~/.aidememo/auth.json` (populated by `aidememo auth login`)
5. None — request goes out without an `Authorization` header
   (works only against loopback, no-auth servers)

### Pull-only delta sync (Phase 2)

For "local working set + team-canonical store" topologies (Hermes
first-party shape), each agent runs its own local `aidememo` for fast
offline reads and pulls deltas from the shared `mcp-serve`
periodically. Writes still go through the shared store via mcp tool
calls so there's a single writer (no conflict resolution needed
yet — that's Phase 3).

```bash
# On each agent host (any machine that needs a local read cache)
aidememo sync pull http://team-host:3000 --token "$AIDEMEMO_TEAM_TOKEN"
# → "pulled from http://team-host:3000: +12 entities, +47 facts,
#    +3 relations (0 skipped, 0 errors); cursor → entity=… fact=…"

# Idempotent — re-running with no upstream changes is a no-op
aidememo sync pull http://team-host:3000 --token "$AIDEMEMO_TEAM_TOKEN"
# → "+0 entities, +0 facts, +0 relations"
```

Cursor state lives at `<store>.sync.json` next to the store file,
keyed by upstream URL. Pull is incremental from the cursor; the
upstream emits a trailing `cursor` line on every batch so the
downstream knows where to resume.

What's transferred: entities, facts, relations — all with original
ULIDs preserved. As of Phase 2.5, **in-place mutations** also
propagate: `fact_supersede`, `fact_pin/unpin`, `entity_describe`,
and any other write that bumps `updated_at`. Each update watermark is a stable
`(updated_at, *_updated_id)` pair, so same-millisecond mutations paginate
without omission. Pull runs a 2-pass export (new ULIDs + records whose update
tuple moved); insert-pass records do not advance update watermarks. The
receiving `*_upsert_record` paths LWW by `updated_at`. Existing Phase 2 cursor
files keep loading and timestamp-only cursors replay their boundary once
(`#[serde(default)]` on the new fields).

Relations use a digest of the complete sorted snapshot
(`relation_generation`) plus an in-snapshot `relation_scan_key`. Additions at
old timestamps and weight/evidence/scope changes alter the generation and
restart an idempotent full scan. Legacy `(created_at, relation_key)` clients
retain their fallback. The complete envelope is validated before writes;
malformed or unsupported batches apply zero records. Store-level failures
withhold the cursor and idempotent upserts make retry safe. CLI pull holds a
same-directory file lock across the full operation and atomically persists a
unique temporary cursor file.

This is canonical-writer, pull-only replication. Relation records replicate as
append/upsert; relation deletions do not propagate. Send writes and deletions to
the canonical shared server and treat pulled stores as read caches.

What's intentionally not in this pipeline: push from agent → shared.
For that, use the regular MCP tool calls against `mcp-serve` (every
write happens at the canonical writer, no merge problem). Multi-
master push with conflict resolution is Phase 3.

### Sync operations (Phase 2.6)

`aidememo sync pull` auto-paginates — a single invocation keeps issuing
`/sync/since` requests with the advancing cursor until the upstream
returns less than a full batch. So `aidememo sync pull <URL>` always
catches up fully, no manual loop needed. Default per-batch limit
is 5,000 records; tune with `--limit N`.

| Flag | Effect |
|---|---|
| `--limit N` | Per-batch cap; total pull is unbounded (paginates until drained) |
| `--watch SEC` | Long-running mode: drain, sleep N seconds, repeat. SIGINT exits cleanly |
| `--json` | One JSON object per pull cycle (stable shape for scripts) |

Two recipes:

```bash
# Cron / one-shot (e.g. `*/5 * * * * aidememo sync pull http://team-host:3000`)
aidememo sync pull http://team-host:3000

# Long-running daemon (systemd / supervisord)
aidememo sync pull http://team-host:3000 --watch 60
```

`aidememo sync status` prints per-remote cursor + age:

```bash
aidememo sync status                          # all remotes in <store>.sync.json
aidememo sync status http://team-host:3000    # one remote
aidememo sync status --json                   # script-friendly
```

`aidememo sync status --json` shape (stable):

```json
{
  "store": "...",
  "cursor_file": "<store>.sync.json",
  "remotes": [
    {
      "url": "http://team-host:3000",
      "entity": "01ABC...", "fact": "01DEF...",
      "entity_updated_at": 1778..., "entity_updated_id": "01ABC...",
      "fact_updated_at": 1778..., "fact_updated_id": "01DEF...",
      "relation_generation": "ab12...", "relation_scan_key": null,
      "last_pulled_at": 1778..., "age_ms": 432
    }
  ]
}
```

The original `aidememo sync <DIR>` markdown-ingest alias moved under
`aidememo sync ingest <DIR>` to make room for `aidememo sync pull <URL>`.

## Bindings (all four full coverage)

Each binding exposes ~22 methods including `current_only` filtering and
`fact_supersede`:

```python
# Python
import aidememo_python as aidememo
g = aidememo.AideMemo("./_meta/wiki.sqlite", backend="libsqlite")
ctx = g.query("Redis", current_only=True)
g.fact_supersede(old_id, new_id)
```

```javascript
// Node
const { AideMemoStore } = require('aidememo-napi');
const g = new AideMemoStore('./_meta/wiki.sqlite');
const hits = JSON.parse(g.search('redis', { limit: 5, currentOnly: true }));
g.factSupersede(oldId, newId);
```

```elixir
# Elixir
g = AideMemoNif.open!("./_meta/wiki.sqlite")
ctx = AideMemoNif.query(g, "Redis", current_only: true)
:ok = AideMemoNif.fact_supersede(g, old_id, new_id)
AideMemoNif.branch_push(g, "candidate-b", "./shared")
AideMemoNif.branch_merge(g, "./shared", branch: "candidate-b")
```

```c
/* C */
char* json = aidememo_query(g, "Redis", 5, 2, 5, /* current_only */ true);
aidememo_fact_supersede(g, old_id, new_id);
aidememo_free_string(json);
```

## Common errors → fixes

| Error | Fix |
|---|---|
| `bpaf usage BUG: all positional and command items…` | Move positional fields rightmost; sync `construct!` order |
| `bpaf: no rules expected ':' in macro` | `construct!` doesn't support `field: var`; rename var to match field |
| `Cannot start a read transaction inside a read transaction` | Drop the outer txn before starting the next |
| `cannot find type AideMemoError / AideMemo` | Use `aidememo_core::AideMemoError` etc. |
| `method not found: normalized_similarity` | Use `strsim::jaro_winkler(a, b)` |
| `borrow of moved value` on `config.search.…` | Extract scalar fields into locals before the closure |
| `missing field 'current_only' in initializer` | Add `current_only: false` (default) when constructing FactListOpts/SearchOpts/QueryOpts |

## Testing

- Unit tests live next to source (`#[cfg(test)] mod tests`).
- Integration tests under `tests/` (workspace) and `crates/*/tests/`.
- Each binding has a smoke test:
  - `crates/aidememo-python/tests/smoke.py` (run after `maturin build` + `pip install`)
  - `crates/aidememo-napi/tests/smoke.js` (run after `npm run build`)
  - `crates/aidememo-nif/test/aidememo_nif_test.exs` (run via `mix test`)
  - `crates/aidememo-ffi/example/smoke.c` (compile against `libaidememo_ffi.a`)
- Run `cargo test -p aidememo-core --features semantic && cargo test -p aidememo-cli --bin aidememo`
  before opening a PR.

## Reference

- `aidememo-skill/SKILL.md` — Claude Code skill format
- `aidememo-skill/REFERENCE.md` — full API reference
- `README.md` — user-facing quick start

### Inline TODO markers

Roadmap gaps that touch live code carry a `TODO(phaseN):` marker so they
surface where the gap actually lives. List them with:

```bash
grep -rn "TODO(phase" crates/
```

Currently flagged: phase6 (S3 transport is a local-fs mirror — see
`aidememo-core/src/s3.rs`).
