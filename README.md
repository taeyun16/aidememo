# WikiGraph (`wg`)

Local knowledge-graph wiki for LLM agents. Single Rust binary, redb-backed,
hybrid BM25 + semantic search, native bindings for Python / Node / Elixir /
C, and a built-in MCP server (stdio + HTTP).

## Install

```bash
# One-line installer (builds via cargo)
curl -fsSL https://raw.githubusercontent.com/aspect-build/wg/main/scripts/install.sh | bash

# Or directly with cargo
cargo install --git https://github.com/aspect-build/wg wg-cli

# Or from a local checkout
cargo install --path crates/wg-cli
```

The binary is named `wg`. Add `~/.cargo/bin` to your `PATH` if it isn't already.

## Quick start

```bash
wg init ./my-wiki                       # create a store + ingest markdown
wg query "Redis"                        # one-shot context (search + entity + traverse + recent)
wg search "high availability" -l 5      # hybrid search
wg recent -n 10                         # what changed in the last 7 days
wg doctor                               # health check (orphans, broken refs, …)
wg fact add "Decided to use Redis Cluster" --type decision --entities Redis
wg edit fact <ID> --append "Updated 2026-04-26"
wg fact supersede <OLD_ID> <NEW_ID>     # validity-window: mark superseded
wg graph --from Redis --depth 2 --format mermaid
```

The default store lives at `~/.wg/wiki.redb` (override with `--store` or
`--project`). Multi-project mode:

```bash
wg project create work --path ~/work-wiki.redb
wg project use work
wg --project personal stats             # one-off override
```

## CLI commands

### Read & search
- `wg search <query> [-l N] [--last 30d] [--current]` — hybrid search
- `wg query <topic> [--mode naive|local|hybrid|global]` — unified context (search + traverse + recent)
- `wg recent [-n N] [--type T] [--last 30d]` — recent activity
- `wg traverse <entity> [-d N]` — graph traversal
- `wg path <from> <to>` — find a path between entities
- `wg graph [--from E] [--format mermaid|dot]` — visualize subgraph
- `wg entity get|list` / `wg fact get|list` / `wg stats`

### Write
- `wg fact add <content> --entities A,B [--type decision]` (auto-creates missing entities)
- `wg fact supersede <OLD_ID> <NEW_ID>` — set validity window
- `wg edit fact <ID> --append/--prepend/--find+--replace/--content`
- `wg entity add <name> [--type service]` (custom types accepted)
- `wg relation add <source> <target> <rel_type>` / `wg relation remove`

### Maintenance
- `wg doctor [--json]` — health check
- `wg lint [--json]` — raw lint issues
- `wg bench <golden.jsonl> [--k 5]` — measure P@K, R@K, p50/p95 latency against a golden set
- `wg ingest <wiki_root> [-i]` — ingest markdown
- `wg watch <wiki_root> [--search QUERY]` — re-ingest on file changes (live search optional)
- `wg sync <wiki_root>` — alias for incremental ingest
- `wg export [--scope all|entities|relations|facts]` / `wg import`
- `wg config get/set/list`

### Multi-project
- `wg project list/show/create/use/remove`
- Global `--project NAME` / `--store PATH` flags

### Servers
- `wg mcp` — stdio JSON-RPC MCP server (Claude Code, Codex)
- `wg mcp-serve --port 3000` — HTTP + SSE for browser/remote clients

## Use as an MCP server

Local agents (Claude Code, Codex CLI, …) can spawn `wg` as a stdio MCP
server. **9 tools** exposed: `wg_query`, `wg_search`, `wg_entity_list`,
`wg_traverse`, `wg_lint`, `wg_fact_add`, `wg_doctor`, `wg_recent`,
`wg_backlinks`.

```bash
# Claude Code
claude mcp add wg -- wg mcp

# Codex CLI — append to ~/.codex/config.toml
[mcp_servers.wg]
command = "wg"
args = ["mcp"]
```

See [`AGENTS.md`](AGENTS.md) and
[`wg-skill/setup-claude-code.md`](wg-skill/setup-claude-code.md)
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

All four bindings expose the full API including `current_only` filtering
and `fact_supersede` for validity-window workflows.

## Features

- **Hybrid search** — BM25 + Model2Vec semantic vectors, RRF fusion
- **4 query modes** — naive (search-only) / local (entity-centric) / hybrid (default) / global (broad)
- **Typed-relation extraction** — zero-LLM regex patterns for `works_at`, `depends_on`, `supersedes`, … (37 phrases)
- **`wg bench`** — JSONL golden-set runner with P@K / R@K / latency reports
- **Knowledge graph** — entities, facts, relations with named traversal
- **Markdown ingest** — frontmatter, `[[wikilinks]]`, heading sections, dates
- **`wg query`** — one-call context: search + entity + traverse + recent
- **Validity windows** — `superseded_at` + `current_only` filter (Graphiti-style)
- **Custom entity types** — `service`, `rfc`, `incident` etc. without recompiling
- **Multi-project** — switch stores via `wg project use` or `--project`
- **Auto-create entities** — `wg fact add --entities A,B` creates missing entities
- **Mermaid / DOT graphs** — `wg graph --format mermaid`
- **MCP server** — stdio (preferred) and HTTP/SSE transports, 9 tools
- **Native bindings** — embed wg directly in Python / Node / Elixir / C
- **Adaptive ranking** — record search feedback, retrain ranker offline

## License

MIT OR Apache-2.0.
