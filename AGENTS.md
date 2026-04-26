# AGENTS.md — Wiki-Graph (wg)

Cross-tool agent guide for the `wg` Rust workspace. Loaded automatically by
Codex, Cursor, Aider, Jules, and any agent that follows the
[agents.md](https://agents.md) spec. Claude Code imports this via
`CLAUDE.md`.

> Working directory: `/Users/mixlink/dev/wg`. Edition 2024, Rust 1.85+.

## What this project is

`wg` (Wiki-Graph) ingests a markdown wiki into a structured knowledge graph
(redb + BM25 + semantic vectors) and exposes it to LLM agents via CLI, MCP
server, NAPI/Python/NIF/FFI bindings.

| Crate | Purpose |
|---|---|
| `wg-core` | redb store, ingest, search, traverse, lint |
| `wg-cli` | `wg` binary (CLI + stdio/HTTP MCP) |
| `wg-napi`, `wg-python`, `wg-nif`, `wg-ffi` | language bindings |

## Setup commands

```bash
cargo check -p wg-core -p wg-cli       # fast verify
cargo build -p wg-cli                   # debug binary at target/debug/wg
cargo build -p wg-cli --release         # release binary
cargo test -p wg-core -p wg-cli         # tests
cargo build 2>&1 | grep '^error'        # only errors
```

## Code map

```
crates/wg-core/src/
  lib.rs        WikiGraph public API (re-exports)
  store.rs      redb CRUD (entity/fact/relation, alias index)
  graph.rs      traverse/path_find
  search.rs     BM25 + semantic hybrid search (feature-gated `semantic`)
  ingest.rs     markdown → entity/fact/relation parser
  types.rs      EntityInput, FactInput, ListOpts, SearchOpts, …
  config.rs     Config { store, model, search, lint }
  error.rs      WgError + Result<T>
  lint.rs       graph health checks
  migrate.rs    schema migrations

crates/wg-cli/src/
  main.rs            command dispatch
  output.rs          Format::{Table, Json} renderers
  cmd/mod.rs         bpaf top-level + per-command parsers
  cmd/{init,watch,model,feedback,adapt}.rs  subcommand handlers
  cmd/mcp_tools.rs   shared MCP JSON-RPC types + dispatch
  cmd/mcp_stdio.rs   `wg mcp` (stdio JSON-RPC, for Claude Code/Codex)
  cmd/mcp_serve.rs   `wg mcp-serve` (HTTP + SSE, for browser clients)
```

## House rules

### bpaf 0.9

- **Positional/command items must be the rightmost fields** in the struct, and
  `construct!` argument order must match field order. Violation panics at
  runtime with `bpaf usage BUG`.
- Don't put `module::function()` calls inside `construct!([...])`. Bind a local
  variable first (`let init_cmd = init::init_command();`) then reference the
  variable.
- Parser return type is `impl Parser<Command>`, not the concrete type.

### redb

- `WikiGraph` wraps `Arc<RwLock<Store>>`; `Store` itself owns `Arc<Database>`.
- Only `Store::open(path, config)` exists — no `new` / `get_or_create`.
- Read txn and write txn cannot be nested. Drop the txn before opening another.
- Drop table handles (`drop(meta);`) before `write_txn.commit()`.

### Errors / lints

- Workspace lints forbid `unsafe_code` and deny `unwrap_used`, `panic`,
  `dbg_macro`. Use `?` with `WgError` variants instead.
- `Config.store.path` is a `String` — convert with `PathBuf::from(&…)`.
- Time helpers: `parse_iso_to_epoch_ms` (YYYY-MM-DD or RFC3339),
  `parse_duration_to_ms` (`30d`, `12h`, `4w`, `1y`).

## MCP integration

`wg` ships two MCP transports backed by the same tool dispatch
(`cmd/mcp_tools.rs`):

| Subcommand | Transport | When to use |
|---|---|---|
| `wg mcp` | stdio (newline-delimited JSON-RPC) | local agents (Claude Code, Codex CLI) |
| `wg mcp-serve --port 3000` | HTTP POST `/mcp` + SSE `/sse` | browser/remote clients |

Tools exposed: `wg_query` (preferred for context fetch), `wg_search`,
`wg_entity_list`, `wg_traverse`, `wg_lint`, `wg_fact_add`. Tool schemas live
in `cmd/mcp_tools.rs::list_tools()`.

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

## Common errors → fixes

| Error | Fix |
|---|---|
| `bpaf usage BUG: all positional and command items…` | Move positional fields rightmost; sync `construct!` order |
| `Cannot start a read transaction inside a read transaction` | Drop the outer txn before starting the next |
| `cannot find type WgError / WikiGraph` | Use `wg_core::WgError` etc. |
| `method not found: normalized_similarity` | Use `strsim::jaro_winkler(a, b)` |
| `borrow of moved value` on `config.search.…` | Extract scalar fields into locals before the closure |

## Testing

- Unit tests live next to source (`#[cfg(test)] mod tests`).
- Integration tests under `tests/` (workspace) and `crates/*/tests/`.
- Run `cargo test -p wg-core -p wg-cli` before opening a PR.

## Reference

- `PLAN.md` — Phase 1–6 roadmap
- `wg-skill/SKILL.md` — full API reference for skill consumers
- `README.md` — user-facing quick start
