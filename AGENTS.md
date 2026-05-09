# AGENTS.md — Wiki-Graph (wg)

Cross-tool agent guide for the `wg` Rust workspace. Loaded automatically by
Codex, Cursor, Aider, Jules, and any agent that follows the
[agents.md](https://agents.md) spec. Claude Code imports this via
`CLAUDE.md`.

> Working directory: `/Users/mixlink/dev/wg`. Edition 2024, Rust 1.85+ (CI/tooling currently validated on Rust 1.95.0).

## What this project is

`wg` (Wiki-Graph) ingests a markdown wiki into a structured knowledge graph
(redb + BM25 + semantic vectors) and exposes it to LLM agents via CLI, MCP
server, and native bindings (Python / Node / Elixir / C).

| Crate | Purpose |
|---|---|
| `wg-core` | redb store, ingest, search, traverse, lint, validity windows |
| `wg-cli` | `wg` binary (CLI + stdio/HTTP MCP) |
| `wg-napi`, `wg-python`, `wg-nif`, `wg-ffi` | language bindings (full API) |

## Setup commands

```bash
cargo check -p wg-core -p wg-cli       # fast verify
cargo build -p wg-cli                   # debug binary at target/debug/wg
cargo build -p wg-cli --release         # release binary
cargo test -p wg-core --features semantic
cargo test -p wg-cli --bin wg
./scripts/ci-local.sh lint
./scripts/ci-local.sh test
# Tests that download a HuggingFace embedding model are #[ignore]'d
# (CI skips them — first run hits HF lock races). Run locally for the
# full surface once the model is cached:
cargo test -p wg-core --features semantic -- --ignored
cargo build 2>&1 | grep '^error'        # only errors
cargo install --path crates/wg-cli      # one-off install
```

### Logging

The CLI uses `tracing` (stderr). Default filter is
`wg=info,wg_core=warn` so `wg mcp-serve` startup, `wg watch` file
events, and core fallback warnings (HNSW sidecar missing, reranker
disabled) all appear without setup.

```bash
RUST_LOG=debug wg search redis              # standard env
WG_LOG="wg=debug,wg_core=trace" wg query …  # alias if you don't want
                                             # to scope RUST_LOG globally
RUST_LOG=error wg mcp-serve                 # silence everything except errors
```

Useful debug spans (turn on with `RUST_LOG=wg_core=debug`):

- `hybrid_search` — wraps every `wg search --hybrid` call. Inside it
  you'll see `embed_provider loaded ms=…` (model load on first call,
  near-zero after) and per-phase events `bm25` / `query_embed` /
  `hnsw_lookup`.
- `Store::open` — directory create + redb open + schema init.

The legacy `WG_SEARCH_PROFILE=1` and `WG_LINT_PROFILE=1` env vars
still work as a self-contained `eprintln` dump for users who'd
rather not configure tracing.

## CLI surface (post-Tier-3)

### Read / search
```
wg search <query> [-l N] [--current] [--hybrid] [--include-archive] [--via URL]
                                                   BM25 by default (no model load — fast path).
                                                   --hybrid = also run semantic re-rank (loads
                                                   model). --include-archive = also search the
                                                   cold-tier `<store>.cold.redb` and merge any
                                                   matches in. --via = dispatch via running
                                                   `wg mcp-serve` daemon (warm model, ~5-50 ms)
wg query <topic> [-l N] [-d N] [--recent-limit N] [-m naive|local|hybrid|global]
                                                   unified search+traverse+recent
wg recent [-n N] [--type T] [--last 30d]          last 7d facts (default)
wg traverse <entity> [-d N]                        forward graph walk
wg path <from> <to>                                shortest path
wg graph [--from E] [--depth N] [--format mermaid|dot]
wg entity get|list|show <NAME>                     show: compiled view (summary + recent)
wg fact get|list
wg stats                                           counts + size
```

### Write
```
wg fact add <content> --entities A,B [--type T]     auto-creates missing entities
wg fact supersede <OLD_ID> <NEW_ID>                 validity-window invalidate
wg fact archive --ids <ID,…>                        move to <store>.cold.redb (preserves FactId)
wg fact archive --older-than 30d [--type note]      bulk move by age (+ optional type filter)
wg edit fact <ID> --append/--prepend/--find+--replace/--content
wg entity add <name> [--type service]               custom types accepted
wg entity describe <name> "..." | --from-stdin | --clear   compiled-truth summary
wg relation add <src> <tgt> <rel_type>
```

### Maintenance
```
wg doctor [--json]                       friendly health check (wraps lint)
wg lint [--json]                          raw lint issues
wg bench <golden.jsonl> [--k 5] [--limit N]
                                          P@K / R@K / latency benchmark
wg skill check <path> [--json]            validate SKILL.md files
wg ingest <root> [-i]                     ingest markdown
wg watch <root> [--search Q]              live re-ingest + optional live search
wg vector-rebuild [--current-only] [--json]  rebuild HNSW from scratch (after model swap).
                                            --current-only skips superseded facts — pair with
                                            `wg consolidate --gac` to actually shrink the index
                                            to the representative set (default keeps all facts
                                            so `as_of` historical retrieval keeps working).
wg auto-relate [--top-k 3] [--threshold 0.0] [--dry-run] [--json]
                                            mine `related` edges from semantic similarity
                                            (one-shot; idempotent; semantic feature only)
wg overview [-n 10] [--recent-days 7] [--json]
                                            first-impression snapshot: entity-type buckets,
                                            fact-type distribution, top entities, recent
                                            activity. One call instead of stats + entity_list
                                            + fact_list.
wg consolidate [--semantic-threshold 0.85] [--ttl TYPE=DAYS] [--dry-run] [--json]
                                            periodic memory-lifecycle pass: pairwise cosine
                                            dedup (older fact superseded) + per-type TTL
                                            expiry. Idempotent. Mirrors OMEGA's compaction.
wg consolidate --gac [--gac-theta 0.85] [--gac-spread-budget N] [--gac-cold-tier]
                     [--gac-protect TYPE,TYPE,...]
                                            cluster-aware consolidation per the GAC
                                            (Geometry-Aware Consolidation, NeurIPS 2026)
                                            paper: tight clusters (d̄ < 1-θ) collapse to
                                            newest fact; spread clusters keep medoid +
                                            budget residuals. --gac-cold-tier moves losers
                                            to <store>.cold.redb (preserves FactId) instead
                                            of superseding. --gac-protect skips named fact
                                            types entirely (use `preference,lesson,error`
                                            on personalisation-tier stores — LongMemEval-S
                                            120q showed even θ=0.95 GAC drops 1 SS-pref q
                                            because near-paraphrases ARE the recall signal
                                            for that category). --dry-run for analysis-only.
                                            Pair with `wg vector-rebuild --current-only` to
                                            shrink the HNSW index proportionally — supersede
                                            alone leaves losers in the sidecar.
wg session new <topic>                      tracked session entity + shell-evaluable
                                            'export WG_SESSION_ID=…'. Auto-attaches to every
                                            subsequent fact_add while the env var is set.
wg session current / list                   current/recent tracked sessions.
wg export [--scope all|...] / wg import
wg config get/set/list

# Speed/safety knob: drop per-commit fsync. Survives process crash
# (page cache outlives it), but power loss within ~30s of a write
# can lose recent commits. About 10× faster than the default.
wg config set store.durability eventual    # opt in
wg config set store.durability immediate   # default (recommended)

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
wg config set model.provider fastembed
wg config set model.name bge-small-en-v1.5    # 133 MB, ONNX
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
# Detail: .notes/agent-ux-design-decisions.md "HotpotQA BGE =
# model2vec — cross-bench BGE validation finalized" section.

# HuggingFace text-embeddings-inference (TEI):
#   model.provider = "tei"         # native /embed + /info dimension auto-discover
#   model.endpoint = "http://host:8080"
#   rerank.provider = "tei"        # cross-encoder rerank of top-K (BGE-reranker, etc.)
#   rerank.endpoint = "http://host:8081"
#   rerank.model    = "BAAI/bge-reranker-base"
#   rerank.top_k    = 32
# Reranker failure is non-fatal — wg falls back to RRF order with a warning.
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
#     low-volume queries. Detail in `.notes/bench-rerank-miracl-ko.md`
#     and the LME finding in `.notes/agent-ux-design-decisions.md`.
```

### Multi-project
```
wg project list/show/create/use/remove
wg --project NAME ...                     one-off override
wg --store PATH ...                       absolute path override
```

### Servers
```
wg mcp                                    stdio JSON-RPC (preferred)
wg mcp-serve --port 3000                  HTTP + SSE
```

### Daemon (background mcp-serve, opportunistic discovery)

`wg daemon` wraps `wg mcp-serve` so manual CLI calls (`wg search`,
…) auto-pick up the warm path without `--via`. Pattern follows
docker / pg_ctl: one shell, one daemon, the rest of the machine
just uses it.

```
wg daemon start                          spawn mcp-serve in background;
                                         ~/.wg/daemon.json holds port/pid
wg daemon status                         show registry + /health probe
wg daemon stop                           SIGTERM the recorded pid
```

After `wg daemon start`, `wg search redis` automatically dispatches
to the daemon over HTTP (no `--via` needed). Measured warm:

| call | latency |
|---|---|
| `wg search redis` (BM25 via daemon)        | ~9 ms |
| `wg search redis --hybrid` (HNSW via daemon) | ~45 ms |
| `wg search redis` (no daemon, fresh CLI)   | ~70-300 ms |

Set `WG_NO_DAEMON=1` to bypass discovery for one invocation. Note:
because redb is single-writer, an in-process `wg` cannot open the
store while the daemon holds it — the bypass mode is only useful
when no daemon is running (e.g. running CI scripts on the same
store path).

## Code map

```
crates/wg-core/src/
  lib.rs        WikiGraph public API (re-exports)
  store.rs      redb CRUD (entity/fact/relation/alias index, current_only filter)
  graph.rs      traverse / path_find
  search.rs     BM25 + semantic hybrid (current_only filter, time windows)
  ingest.rs     markdown → entity/fact/relation parser (frontmatter dates)
  types.rs      EntityType (Custom variant), FactRecord (superseded_at/by),
                QueryOpts (current_only), …
  config.rs     Config { store, model, search, lint, projects, default_project }
  error.rs      WgError + Result<T>
  lint.rs       graph health checks
  migrate.rs    schema migrations

crates/wg-cli/src/
  main.rs            command dispatch
  output.rs          Format::{Table, Json} renderers + format_query_result
  cmd/mod.rs         bpaf top-level + per-command parsers (--project / --json)
  cmd/{init,watch,model,feedback,adapt,doctor,recent,edit,graph,project}.rs
  cmd/mcp_tools.rs   shared MCP JSON-RPC types + 24-tool dispatch
  cmd/mcp_stdio.rs   `wg mcp` (stdio)
  cmd/mcp_serve.rs   `wg mcp-serve` (HTTP + SSE)
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

### redb / wg-core

- `WikiGraph` wraps `Arc<RwLock<Store>>`; `Store` itself owns `Arc<Database>`.
- `WikiGraph::ingest` is `&self` (uses interior mutability via the RwLock).
  This makes `Arc<WikiGraph>` callable from bindings.
- Only `Store::open(path, config)` exists — no `new` / `get_or_create`.
- Read txn and write txn cannot be nested. Drop the txn before opening another.
- All persisted records are JSON (not bincode), so adding `#[serde(default)]`
  fields to types is fully backward-compatible — no migration needed.
- `EntityType::Custom(String)` and `FactRecord.superseded_at/by` rely on this.

### Errors / lints

- Workspace lints forbid `unsafe_code` and deny `unwrap_used`, `panic`,
  `dbg_macro`. Use `?` with `WgError` variants instead.
- `Config.store.path` is a `String` — convert with `PathBuf::from(&…)`.
  `Config::default_store_path()` and `Config::project_path()` already
  expand `~/`.
- Time helpers: `parse_iso_to_epoch_ms` (YYYY-MM-DD or RFC3339),
  `parse_duration_to_ms` (`30d`, `12h`, `4w`, `1y`).

## MCP integration

`wg` ships two MCP transports backed by the same tool dispatch
(`cmd/mcp_tools.rs`):

| Subcommand | Transport | When to use |
|---|---|---|
| `wg mcp` | stdio (newline-delimited JSON-RPC) | local agents (Claude Code, Codex CLI) |
| `wg mcp-serve --port 3000` | HTTP POST `/mcp` + SSE `/sse` | browser/remote clients |

**Tool surface — 4 core, then standard, then advanced.** Agent prompts
should lead with the core 4; the rest are there when needed.

**Core (90% of agent traffic)**:

| Tool | When |
|---|---|
| **`wg_context`** | **🟢 Top-of-turn entry point** — pinned + personalisation + recent + (with topic) search/traverse/lessons. One round-trip. |
| **`wg_query`** | Topic dive after wg_context. `format:"text"` for compact prompt injection; `max_chars:N` for hard caps; `level:"entity"`/`"session"` to group by entity / by tracked session. |
| **`wg_aggregate`** | Deterministic counting / sum / timeline across facts. Pulls agent out of in-head arithmetic for "how much / how many / between when" questions. ops: `count`/`enumerate`/`by_entity`/`sum_currency`/`sum_duration`/`count_distinct_dates`/`timeline`. |
| **`wg_fact_add`** | Append a fact (auto-creates entities, `preference`/`lesson`/`error` types). Auto-attaches to `WG_SESSION_ID` when the env var is set. |

**Standard (frequent, but optional in any single turn)**:

| Tool | When |
|---|---|
| `wg_search` | Pure hybrid search, no graph wrapper — fastest pinpoint lookup. |
| `wg_recent` | Last N days of facts. |
| `wg_overview` | First-impression snapshot for an unfamiliar wiki — entity-type buckets, top centrals, recent activity. |
| `wg_traverse` | Walk the graph from a known entity. `direction:"reverse"` for "what depends on X". |
| `wg_doctor` | Health snapshot — counts + lint issues + per-code action hints. |
| `wg_entity_get` | Fetch one entity (name / alias). |
| `wg_fact_get` | Fetch one fact by ULID. |
| `wg_entity_list` | Browse entities. |

**Advanced (write-side power tools, batch ops, niche)**:

| Tool | When |
|---|---|
| `wg_fact_add_many` | Bulk fact insert in one transaction. |
| `wg_fact_supersede` | Mark an old fact replaced by a new one (validity-window invalidate). |
| `wg_fact_edit` | Patch fact content (append/prepend/find+replace/content). |
| `wg_fact_archive` | Move facts to cold-tier (`<store>.cold.redb`) — hot store shrinks, FactId preserved for `wg_fact_get`. Pass `include_archive:true` on `wg_search` / `wg_query` to merge cold matches back in. |
| `wg_fact_pin` | Pin / unpin a fact for the always-on tier. |
| `wg_fact_list` | Paginated fact list with filters. |
| `wg_pinned_context` | Just the pinned tier (subset of wg_context). |
| `wg_session_start` | Pinned + recent only (subset of wg_context). |
| `wg_path` | Shortest-path between two entities. |
| `wg_entity_describe` | Set / clear an entity's prose summary. |
| `wg_extract` | Run heuristic / optional LLM fact extraction over raw text. |
| `wg_feedback` | Record positive / negative feedback on a search hit (training signal). |

Tool schemas live in `cmd/mcp_tools.rs::list_tools()`. The deprecated
`wg_lint` and `wg_backlinks` tools were removed in favour of
`wg_doctor` and `wg_traverse(direction:"reverse")`.

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

**Tracked sessions** — every `wg fact add` auto-attaches a session entity while `WG_SESSION_ID` is in the env:
```bash
eval "$(wg session new 'auth migration')"
wg fact add 'Decided to use Keycloak for SSO' --type decision --entities Auth
wg fact add 'Tried bare-metal Postgres, hit IO limits at 5k qps' --type lesson --entities Postgres
wg session current     # show topic + fact count attached so far
wg session list        # all tracked sessions
wg fact list --entity $WG_SESSION_ID  # pull one session's full thread
```

**Lifecycle / consolidation** — periodic batch dedup + TTL pass:
```bash
wg consolidate --semantic-threshold 0.85          # OMEGA-style dedup
wg consolidate --ttl note=30 --ttl question=14    # per-type expiry
wg consolidate --semantic-threshold 0.85 --ttl note=30 --dry-run   # preview
```
Plus: `wg config set lifecycle.auto_supersede_atomic_types true` makes `wg fact add` of a `decision`/`convention` auto-supersede the existing same-type fact on the same entity (off by default — preserves the historical "every fact_add creates a new fact" contract).

**LLM-aided extractor (opt-in)** — when `extract.provider = "openai"`:
```bash
wg config set extract.provider openai            # uses OPENAI_API_KEY
wg config set extract.model gpt-4o-mini          # default
wg extract --llm 'long chat transcript here…'    # CLI
# MCP: wg_extract { llm: true, text: "…" }
```

**Auto-context hooks (Claude Code)** — see `wg-skill/hooks/README.md` for installation:
| Hook | Effect |
|---|---|
| `SessionStart` | injects `wg overview` + `wg recent` + pinned facts as `additionalContext` |
| `PostToolUse` (Edit/Write) | surfaces wg facts related to the just-edited file |
| `UserPromptSubmit` | extracts candidate facts from the prompt (preview only; opt into LLM via `WG_EXTRACT_LLM=1`) |

### Agentic-loop pattern — when to call `wg_aggregate` mid-turn

For most user questions, a single `wg_context` (opening turn) or `wg_query`
(follow-up) round-trip yields enough context for the reader to answer
directly from snippets. Don't reach for `wg_aggregate` unless the question
shape demands deterministic arithmetic across facts:

| Question shape | Tool | Rationale |
|---|---|---|
| "What did I say about X?" | `wg_query` | Simple recall — read the snippet |
| "When did I last do Y?" | `wg_query` (level=fact) | Single-fact lookup |
| "What's my preference for Z?" | `wg_context` (personalisation tier) | Pre-surfaced |
| **"How much $ total did I spend on X?"** | `wg_aggregate(op=sum_currency)` | Arithmetic across N facts |
| **"How many hours of Y total?"** | `wg_aggregate(op=sum_duration)` | Time accumulation |
| **"How many distinct days had event Z?"** | `wg_aggregate(op=count_distinct_dates)` | Set-cardinality across facts |
| **"Timeline of all X events"** | `wg_aggregate(op=timeline)` | Chronological ordering |
| **"How many times did I decide X?"** | `wg_aggregate(op=count, fact_type=decision)` | Bounded enumeration |

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
above baseline when the agent uses `wg_aggregate(op=count|enumerate)`.

Auto-dispatch via single-shot LLM classifier ("does this question
need aggregation? YES/NO") nets to baseline at 240q (+0pt mean,
classifier precision ≈40%; KU/temporal "false positives" are
genuinely counting-shaped, not classifier mistakes). Don't bake
auto-dispatch into wg — pay no extra round-trip for zero lift.

The trigger table above remains the right pattern, but treat
`wg_aggregate` as **insurance** for cross-fact arithmetic, not a
guaranteed accuracy lever. Most lift in our measurements comes from
`wg_query` granularity / `level` and ingest hygiene, not from
mid-turn tool dispatch.

Implementation pattern in your reader prompt (recommended):

```
You have access to wg_aggregate(op=sum_currency|sum_duration|count_distinct_dates|timeline|count).
Call it when the user's question requires summing or counting across
facts (e.g., 'how much total', 'how many distinct days'). Otherwise
answer directly from the snippets you already have.
```

### Self-extraction pattern — agent classifies, wg stores

`wg` deliberately does **not** ship a built-in LLM-aided ingest
pipeline (the kind Mem0 / Letta have a dedicated extractor for).
The reason is structural: every agent that calls `wg_fact_add`
already has its own LLM, and that LLM is almost always a stronger
model than wg could ever embed. Asking the calling agent to do the
classification keeps wg local-first and free of API-key /
extractor-quality coupling, while still benefiting from the
agent's reasoning.

What this means for the calling agent:

* Before `wg_fact_add`, decide which `fact_type` fits — the trigger
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

* Pass the entity hints in the same call (`entities: [...]`) — wg
  auto-creates and aliases.

* `wg_fact_add_many` gets the same self-extracted classification —
  the batch round-trip is for fsync amortisation, not classification.

This is the pattern you should follow rather than reaching for a
heavier ingest framework. wg's bench measurements show the in-pipeline
weighting (decay-exempt + 2× boost on personalisation tiers like
preference / lesson / error) materially shifts ranking — but only
when those types are populated correctly. Ship-default `note`
classification leaves the boost dormant.

Historical caveats from our measurements:

* Routing extraction through a same-class extractor (MiniMax →
  MiniMax via `wg_extract --llm`) regressed LongMemEval 60q by
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
that consumes the result". In practice production agents call wg
from Claude Opus / GPT-5-class models, well above the bench reader
class we measured against, so the lift hypothesis remains
plausible — just unmeasured at this quota window.

### Register with Claude Code
```bash
claude mcp add wg -- wg mcp
# or commit .mcp.json at repo root (already provided)
```

### Register with Codex CLI
Add to `~/.codex/config.toml`:
```toml
[mcp_servers.wg]
command = "wg"
args = ["mcp"]
```

### Multi-agent shared store

`wg` uses redb, which holds an **exclusive file lock per process**.
Two agents that each spawn their own `wg mcp` against the same store
will fight over the lock — one wins, the other gets `Database
already open. Cannot acquire lock.` Two ways to handle this:

1. **Single shared `wg mcp-serve` instance** (recommended for shared
   writes). Run one HTTP/SSE server, point every agent at the same
   URL. Each agent's MCP client sees the same tools, the same store,
   no lock contention.
   ```bash
   wg mcp-serve --port 3000 --store ~/.wg/team.redb &
   # In each agent's MCP config: type=http, url=http://localhost:3000/mcp
   ```
2. **Brief opportunistic contention** is fine: set
   `store.lock_retry_ms` so transient locks (one agent's `wg mcp`
   briefly holds while you run a one-off `wg fact add`) auto-resolve.
   ```bash
   wg config set store.lock_retry_ms 5000   # 5s budget, polls every 100ms
   ```
   `lock_retry_ms = 0` (default) preserves the old fail-fast behaviour.

Don't try to give multiple agents their own stdio `wg mcp` against
the same redb path — it will work for whichever started first, then
silently lose writes from the others.

### Hardened mcp-serve (Phase 1)

`wg mcp-serve` defaults to `127.0.0.1` since the network-hardening
pass landed (commit 8aa3f68). Three flags govern exposure:

| Flag | Default | Effect |
|---|---|---|
| `--bind ADDR` | `127.0.0.1` | Loopback only. Pass `0.0.0.0` to expose on the LAN |
| `--auth-token TOKEN` | unset | Bearer token literal — exposed in shell history / `ps aux`, only use for ad-hoc tests |
| `--auth-token-file PATH` | unset | Read the token from a file (mode 0600 recommended). Production-friendly |
| `WG_MCP_AUTH_TOKEN` env | unset | Final fallback when neither flag is set |

Non-loopback bind without an auth token is **refused at startup** —
the server won't expose an unauthenticated store on the network.
Loopback + no token is fine for single-host multi-agent. TLS / rate
limiting / per-method auth are deliberately out of scope; put a
reverse proxy (caddy / nginx) in front if you need them.

```bash
# Same-host: agents go through loopback, no token needed
wg mcp-serve --port 3000

# Multi-host team server: token in a file, never on the command line
wg auth generate > /etc/wg/team-token   # one-time: emit 64-char hex
chmod 600 /etc/wg/team-token
wg mcp-serve --port 3000 --bind 0.0.0.0 --auth-token-file /etc/wg/team-token
```

### Token UX — `wg auth` (commit will land alongside this section)

Operators rarely want to type bearer tokens. `wg auth` ships four
small commands so the secret never has to live in shell history:

```bash
wg auth generate                        # → 64-char hex (32 random bytes)
wg auth login http://team-host:3000 \
        --token-file /etc/wg/team-token  # store in ~/.wg/auth.json (0600)
wg auth list                            # redacted preview of stored URLs
wg auth logout http://team-host:3000    # forget one
```

After `wg auth login`, every `wg sync pull <URL>` reads the token
transparently — no `--token` / env var on the call:

```bash
wg sync pull http://team-host:3000      # uses stored token
```

Token resolution chain in `wg sync pull` (highest precedence first):

1. `--token TOKEN` (literal, exposed in `ps aux` — discouraged)
2. `--token-file PATH` (read + trim)
3. `WG_MCP_AUTH_TOKEN` env var
4. Stored entry in `~/.wg/auth.json` (populated by `wg auth login`)
5. None — request goes out without an `Authorization` header
   (works only against loopback, no-auth servers)

### Pull-only delta sync (Phase 2)

For "local working set + team-canonical store" topologies (Hermes
first-party shape), each agent runs its own local `wg` for fast
offline reads and pulls deltas from the shared `mcp-serve`
periodically. Writes still go through the shared store via mcp tool
calls so there's a single writer (no conflict resolution needed
yet — that's Phase 3).

```bash
# On each agent host (any machine that needs a local read cache)
wg sync pull http://team-host:3000 --token "$WG_TEAM_TOKEN"
# → "pulled from http://team-host:3000: +12 entities, +47 facts,
#    +3 relations (0 skipped, 0 errors); cursor → entity=… fact=…"

# Idempotent — re-running with no upstream changes is a no-op
wg sync pull http://team-host:3000 --token "$WG_TEAM_TOKEN"
# → "+0 entities, +0 facts, +0 relations"
```

Cursor state lives at `<store>.sync.json` next to the redb file,
keyed by upstream URL. Pull is incremental from the cursor; the
upstream emits a trailing `cursor` line on every batch so the
downstream knows where to resume.

What's transferred: entities, facts, relations — all with original
ULIDs preserved. What's NOT yet transferred: supersede-state changes
on already-synced facts (LWW merge), pin/unpin updates, search
feedback. Those land in Phase 2.5.

What's intentionally not in this pipeline: push from agent → shared.
For that, use the regular MCP tool calls against `mcp-serve` (every
write happens at the canonical writer, no merge problem). Multi-
master push with conflict resolution is Phase 3.

The original `wg sync <DIR>` markdown-ingest alias moved under
`wg sync ingest <DIR>` to make room for `wg sync pull <URL>`.

## Bindings (all four full coverage)

Each binding exposes ~22 methods including `current_only` filtering and
`fact_supersede`:

```python
# Python
import wg_python as wg
g = wg.WikiGraph("./_meta/wiki.redb")
ctx = g.query("Redis", current_only=True)
g.fact_supersede(old_id, new_id)
```

```javascript
// Node
const { WgStore } = require('wg-napi');
const g = new WgStore('./_meta/wiki.redb');
const hits = JSON.parse(g.search('redis', { limit: 5, currentOnly: true }));
g.factSupersede(oldId, newId);
```

```elixir
# Elixir
g = WgNif.open!("./_meta/wiki.redb")
ctx = WgNif.query(g, "Redis", current_only: true)
:ok = WgNif.fact_supersede(g, old_id, new_id)
```

```c
/* C */
char* json = wg_query(g, "Redis", 5, 2, 5, /* current_only */ true);
wg_fact_supersede(g, old_id, new_id);
wg_free_string(json);
```

## Common errors → fixes

| Error | Fix |
|---|---|
| `bpaf usage BUG: all positional and command items…` | Move positional fields rightmost; sync `construct!` order |
| `bpaf: no rules expected ':' in macro` | `construct!` doesn't support `field: var`; rename var to match field |
| `Cannot start a read transaction inside a read transaction` | Drop the outer txn before starting the next |
| `cannot find type WgError / WikiGraph` | Use `wg_core::WgError` etc. |
| `method not found: normalized_similarity` | Use `strsim::jaro_winkler(a, b)` |
| `borrow of moved value` on `config.search.…` | Extract scalar fields into locals before the closure |
| `missing field 'current_only' in initializer` | Add `current_only: false` (default) when constructing FactListOpts/SearchOpts/QueryOpts |

## Testing

- Unit tests live next to source (`#[cfg(test)] mod tests`).
- Integration tests under `tests/` (workspace) and `crates/*/tests/`.
- Each binding has a smoke test:
  - `crates/wg-python/tests/smoke.py` (run after `maturin build` + `pip install`)
  - `crates/wg-napi/tests/smoke.js` (run after `npm run build`)
  - `crates/wg-nif/test/wg_nif_test.exs` (run via `mix test`)
  - `crates/wg-ffi/example/smoke.c` (compile against `libwg_ffi.a`)
- Run `cargo test -p wg-core --features semantic && cargo test -p wg-cli --bin wg`
  before opening a PR.

## Reference

- `PLAN.md` — Phase 1–6 roadmap
- `wg-skill/SKILL.md` — Claude Code skill format
- `wg-skill/REFERENCE.md` — full API reference
- `README.md` — user-facing quick start

### Inline TODO markers

Roadmap gaps that touch live code carry a `TODO(phaseN):` marker so they
surface where the gap actually lives, not just in `PLAN.md`. List them with:

```bash
grep -rn "TODO(phase" crates/
```

Currently flagged: phase6 (S3 transport is a local-fs mirror — see
`wg-core/src/s3.rs`).
