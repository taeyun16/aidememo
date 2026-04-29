# AGENTS.md — Wiki-Graph (wg)

Cross-tool agent guide for the `wg` Rust workspace. Loaded automatically by
Codex, Cursor, Aider, Jules, and any agent that follows the
[agents.md](https://agents.md) spec. Claude Code imports this via
`CLAUDE.md`.

> Working directory: `/Users/mixlink/dev/wg`. Edition 2024, Rust 1.85+.

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
cargo test -p wg-core --features semantic   # 35 tests
cargo test -p wg-cli --bin wg              # 7 tests
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
wg search <query> [-l N] [--current] [--hybrid] [--via URL]
                                                   BM25 by default (no model load — fast path).
                                                   --hybrid = also run semantic re-rank (loads
                                                   model). --via = dispatch via running
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
wg vector-rebuild [--json]                 rebuild HNSW from scratch (after model swap)
wg export [--scope all|...] / wg import
wg config get/set/list

# Speed/safety knob: drop per-commit fsync. Survives process crash
# (page cache outlives it), but power loss within ~30s of a write
# can lose recent commits. About 10× faster than the default.
wg config set store.durability eventual    # opt in
wg config set store.durability immediate   # default (recommended)

# HuggingFace text-embeddings-inference (TEI):
#   model.provider = "tei"         # native /embed + /info dimension auto-discover
#   model.endpoint = "http://host:8080"
#   rerank.provider = "tei"        # cross-encoder rerank of top-K (BGE-reranker, etc.)
#   rerank.endpoint = "http://host:8081"
#   rerank.model    = "BAAI/bge-reranker-base"
#   rerank.top_k    = 32
# Reranker failure is non-fatal — wg falls back to RRF order with a warning.
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
  cmd/mcp_tools.rs   shared MCP JSON-RPC types + 9-tool dispatch
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

**13 tools** (preferred order in agent prompts):

| Tool | When |
|---|---|
| `wg_query` | One-call context fetch — prefer over chaining other tools |
| `wg_search` | Pure hybrid search, no graph |
| `wg_recent` | Last N days of facts |
| `wg_entity_list` | Browse entities |
| `wg_traverse` | Forward walk from a known entity |
| `wg_backlinks` | Reverse walk — "what depends on X?" |
| `wg_doctor` | Health snapshot |
| `wg_lint` | Raw issues |
| `wg_entity_describe` | Set / clear an entity's prose summary |
| `wg_fact_add` | Append a new fact |
| `wg_fact_add_many` | Append N facts in one transaction (use for bulk imports) |
| `wg_fact_supersede` | Mark old fact replaced by a new one (validity-window invalidate) |
| `wg_fact_edit` | Patch a fact's content (append / prepend / find+replace / content) |

Tool schemas live in `cmd/mcp_tools.rs::list_tools()`.

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
