# WikiGraph (`wg`)

Local knowledge-graph wiki for LLM agents. Single Rust binary, redb-backed,
hybrid BM25 + semantic search, native bindings for Python / Node / Elixir /
C, and a built-in MCP server (stdio + HTTP).

## Install

```bash
# Recommended: install latest from main
cargo install --git https://github.com/aspect-build/wg wg-cli

# Or from a local checkout
cargo install --path crates/wg-cli
```

The binary is named `wg`. Add `~/.cargo/bin` to your `PATH` if it isn't
already (cargo's default).

> Release-binary install (`curl ... | sh`) and Homebrew tap are on the
> roadmap; for now the cargo path is the supported flow.

## Quick start

```bash
wg init ./my-wiki                       # create a store + ingest markdown
wg query "Redis"                        # one-shot context (search + entity + traverse + recent)
wg search "high availability" -l 5      # hybrid search
wg recent -n 10                         # what changed in the last 7 days
wg doctor                               # health check
wg fact add "Decided to use Redis Cluster" --type decision --entities Redis
wg edit fact <ID> --append "Updated 2026-04-26"
```

The default store lives at `~/.wg/wiki.redb` (override with `--store`).

## Use as an MCP server

Local agents (Claude Code, Codex CLI, …) can spawn `wg` as a stdio MCP
server. Six tools are exposed: `wg_query`, `wg_search`, `wg_entity_list`,
`wg_traverse`, `wg_lint`, `wg_fact_add`, plus `wg_recent`, `wg_backlinks`,
`wg_doctor`.

```bash
# Claude Code
claude mcp add wg -- wg mcp

# Codex CLI — append to ~/.codex/config.toml
[mcp_servers.wg]
command = "wg"
args = ["mcp"]
```

See [`AGENTS.md`](AGENTS.md) and [`wg-skill/setup-claude-code.md`](wg-skill/setup-claude-code.md)
for full integration details.

## Architecture

| Crate | Purpose |
|---|---|
| `wg-core` | Embedded redb store, ingest, search, traverse, lint |
| `wg-cli` | The `wg` binary (CLI + stdio/HTTP MCP) |
| `wg-python` | Python bindings (PyO3 + maturin) |
| `wg-napi` | Node.js bindings (napi-rs) |
| `wg-nif` | Elixir/Erlang bindings (rustler) |
| `wg-ffi` | C-ABI bindings (cdylib + staticlib + header) |

## Features

- **Hybrid search** — BM25 + Model2Vec semantic vectors, RRF fusion
- **Knowledge graph** — entities, facts, relations with named traversal
- **Markdown ingest** — frontmatter, `[[wikilinks]]`, heading sections, dates
- **`wg query`** — one-call context: search + entity + traverse + recent
- **MCP server** — stdio (preferred) and HTTP/SSE transports
- **Native bindings** — embed wg directly in Python / Node / Elixir / C
- **Adaptive ranking** — record search feedback, retrain ranker offline

## License

MIT OR Apache-2.0.
