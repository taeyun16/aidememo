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
- `wg entity get|list|show` / `wg fact get|list` / `wg stats`
- `wg entity show <name>` — compiled view: summary + recent facts

### Write
- `wg fact add <content> --entities A,B [--type decision]` (auto-creates missing entities)
- `wg fact supersede <OLD_ID> <NEW_ID>` — set validity window
- `wg edit fact <ID> --append/--prepend/--find+--replace/--content`
- `wg entity add <name> [--type service]` (custom types accepted)
- `wg entity describe <name> "..."` (or `--from-stdin` / `--clear`) — set compiled-truth summary
- `wg relation add <source> <target> <rel_type>` / `wg relation remove`

### Maintenance
- `wg doctor [--json]` — health check (now also reports memory + disk footprint)
- `wg lint [--json]` — raw lint issues
- `wg bench <golden.jsonl> [--k 5]` — measure P@K, R@K, p50/p95 latency against a golden set
- `wg skill check <path>` — validate Claude Code SKILL.md frontmatter + tool refs
- `wg ingest <wiki_root> [-i]` — ingest markdown
- `wg watch <wiki_root> [--search QUERY]` — re-ingest on file changes (live search optional)
- `wg sync <wiki_root>` — alias for incremental ingest
- `wg vector-rebuild [--json]` — rebuild the HNSW index (after a model swap)
- `wg export [--scope all|entities|relations|facts]` / `wg import`
- `wg config get/set/list`
  - `wg config set store.durability eventual` — drop per-commit fsync (~13× faster writes; survives process crash, not power loss)
  - `wg config set store.lock_retry_ms 5000` — auto-retry briefly when another `wg` process is holding the redb lock (multi-agent setups). Default `0` keeps the fast-fail behaviour.

### Multi-project
- `wg project list/show/create/use/remove`
- Global `--project NAME` / `--store PATH` flags

### Servers
- `wg mcp` — stdio JSON-RPC MCP server (Claude Code, Codex)
- `wg mcp-serve --port 3000` — HTTP + SSE for browser/remote clients

## Use as an MCP server

Local agents (Claude Code, Codex CLI, …) can spawn `wg` as a stdio MCP
server. **22 tools** exposed:

| Read | Write |
|---|---|
| `wg_session_start` (one-call warmup) | `wg_fact_add` |
| `wg_query` (one-call context) | `wg_fact_add_many` (batched, one fsync) |
| `wg_search` | `wg_fact_supersede` |
| `wg_recent` / `wg_pinned_context` | `wg_fact_edit` |
| `wg_traverse` / `wg_backlinks` | `wg_fact_pin` (toggle "always loaded" tier) |
| `wg_path` (shortest entity path) | `wg_entity_describe` |
| `wg_entity_list` / `wg_entity_get` | `wg_feedback` (helpful/not on a search hit) |
| `wg_fact_list` / `wg_fact_get` | `wg_extract` (text → candidate facts, optional auto-apply) |
| `wg_doctor` / `wg_lint` | |

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

## How does `wg` compare?

[`COMPARE.md`](COMPARE.md) has the head-to-head against mem0, Letta,
Graphiti / Zep, and beads — what `wg` wins, where it lags, and when
to reach for something else.

## Architecture

| Crate | Purpose |
|---|---|
| `wg-core` | Embedded redb store, ingest, search, traverse, lint |
| `wg-cli` | The `wg` binary (CLI + stdio/HTTP MCP) |
| `wg-python` | Python bindings (PyO3 + maturin) |
| `wg-napi` | Node.js bindings (napi-rs) |
| `wg-nif` | Elixir/Erlang bindings (rustler) |
| `wg-ffi` | C-ABI bindings (cdylib + staticlib + header) |

All four bindings expose the full API: `current_only` filtering,
`fact_supersede` for validity-window workflows, and `fact_add_many`
for amortized-fsync bulk inserts. The Python constructor accepts
config kwargs (`model`, `semantic_index`, `durability`) so callers
can override any default without round-tripping through a config
file.

## Features

- **Hybrid search** — BM25 + Model2Vec semantic vectors, RRF fusion
- **4 query modes** — naive (search-only) / local (entity-centric) / hybrid (default) / global (broad)
- **Typed-relation extraction** — zero-LLM regex patterns for `works_at`, `depends_on`, `supersedes`, … (37 phrases)
- **Compiled-truth + timeline** — each entity has a manual `summary` ("what we currently believe") plus the fact list ("evidence trail")
- **`wg skill check`** — validate Claude Code SKILL.md frontmatter, tool references, body length
- **`wg bench`** — JSONL golden-set runner with P@K / R@K / latency reports
- **Knowledge graph** — entities, facts, relations with named traversal
- **Markdown ingest** — frontmatter, `[[wikilinks]]`, heading sections, dates
- **`wg query`** — one-call context: search + entity + traverse + recent
- **Validity windows** — `superseded_at` + `current_only` filter (Graphiti-style)
- **Custom entity types** — `service`, `rfc`, `incident` etc. without recompiling
- **Multi-project** — switch stores via `wg project use` or `--project`
- **Auto-create entities** — `wg fact add --entities A,B` creates missing entities
- **Mermaid / DOT graphs** — `wg graph --format mermaid`
- **MCP server** — stdio (preferred) and HTTP/SSE transports, 13 tools
- **Native bindings** — embed wg directly in Python / Node / Elixir / C
- **Adaptive ranking** — record search feedback, retrain ranker offline
- **TEI integration** — opt into HuggingFace text-embeddings-inference for embeddings (`model.provider = "tei"`) and/or cross-encoder reranking (`rerank.provider = "tei"`); reranker failure falls back to RRF

## Performance

Reference numbers from `benchmarks/src/bin/performance.rs` on a 10 000-fact
synthetic wiki, p95 latency, default config (HNSW + immediate durability):

| Operation | p95 |
|---|---|
| `traverse_d3` | ~0.01 ms |
| `search_bm25` (pure BM25) | ~0.5 ms |
| `search_hybrid` (HNSW path) | ~3.4 ms |
| `lint` (full health check) | ~34 ms |
| `fact_add_many` (per fact) | ~0.07 ms |
| `fact_add` (single) | ~5 ms (OS fsync floor; use `fact_add_many` or `store.durability = eventual` to amortize/skip) |
| `startup` (open + first traverse) | ~12 ms |

Profile yourself:

```bash
cargo run --release --bin performance      # full matrix → benchmarks/results/performance.json
WG_LINT_PROFILE=1 wg lint                  # per-phase lint timings
WG_SEARCH_PROFILE=1 wg search "…"          # per-phase hybrid_search timings
```

## License

MIT OR Apache-2.0.
