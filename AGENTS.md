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
cargo build 2>&1 | grep '^error'        # only errors
cargo install --path crates/wg-cli      # one-off install
```

## CLI surface (post-Tier-3)

### Read / search
```
wg search <query> [-l N] [--current]              hybrid BM25+semantic
wg query <topic> [-l N] [-d N] [--recent-limit N] unified search+traverse+recent
wg recent [-n N] [--type T] [--last 30d]          last 7d facts (default)
wg traverse <entity> [-d N]                        forward graph walk
wg path <from> <to>                                shortest path
wg graph [--from E] [--depth N] [--format mermaid|dot]
wg entity get|list / wg fact get|list
wg stats                                           counts + size
```

### Write
```
wg fact add <content> --entities A,B [--type T]     auto-creates missing entities
wg fact supersede <OLD_ID> <NEW_ID>                 validity-window invalidate
wg edit fact <ID> --append/--prepend/--find+--replace/--content
wg entity add <name> [--type service]               custom types accepted
wg relation add <src> <tgt> <rel_type>
```

### Maintenance
```
wg doctor [--json]                       friendly health check (wraps lint)
wg lint [--json]                          raw lint issues
wg ingest <root> [-i]                     ingest markdown
wg watch <root> [--search Q]              live re-ingest + optional live search
wg export [--scope all|...] / wg import
wg config get/set/list
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

**9 tools** (preferred order in agent prompts):

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
| `wg_fact_add` | Append a new fact |

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
