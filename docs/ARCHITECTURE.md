---
title: Architecture
description: Visual system map for AideMemo's CLI, MCP, agent SDK, bindings, core, stores, and retrieval flows.
---

# Architecture

AideMemo is one Rust core with several access surfaces. The same typed facts,
entities, relations, validity windows, BM25 index, semantic HNSW sidecar, and
archive semantics are exposed through the CLI, MCP tools, Python agent SDK, and
native bindings.

## System map

```mermaid
flowchart LR
  codex["Coding agents<br/>Codex / Claude Code / Cursor / Hermes"]
  human["Human CLI user"]
  scripts["Scripts and tests"]
  plugins["Tool builders<br/>Python / Node / Elixir / C"]

  cli["aidememo-cli<br/>commands and daemon"]
  mcp["MCP transports<br/>stdio: aidememo mcp<br/>HTTP/SSE: aidememo mcp-serve"]
  sdk["aidememo-agent-sdk<br/>Memory.open / search_rows / remember"]
  bindings["Native bindings<br/>aidememo-python / aidememo-napi<br/>aidememo-nif / aidememo-ffi"]

  core["aidememo-core<br/>AideMemo API"]
  backend["StoreKind dispatch<br/>SQLite default / redb optional"]
  sqlite[("SQLite hot store<br/>entities / facts / relations")]
  redb[("redb hot store<br/>optional Cargo feature")]
  cold[("Cold tier<br/>*.cold.sqlite / *.cold.redb")]
  indexes["Retrieval sidecars<br/>BM25 + semantic HNSW"]

  human --> cli
  codex --> mcp
  codex --> sdk
  scripts --> sdk
  scripts --> cli
  plugins --> bindings
  cli --> core
  mcp --> core
  sdk --> bindings
  sdk --> cli
  bindings --> core
  core --> backend
  backend --> sqlite
  backend --> redb
  core --> cold
  core --> indexes
```

The public dispatch point is `AideMemo` in `aidememo-core`. Storage selection is
centralized behind `StoreKind`: SQLite / `libsqlite` is the default runtime
backend, while `redb` is selected only when the crate is built with the optional
Cargo feature and the config or CLI asks for it.

## Retrieval flow

```mermaid
flowchart TD
  request["User or agent asks a question"]
  entry{"Entry point"}
  search["aidememo_search<br/>aidememo search"]
  query["aidememo_query<br/>aidememo query"]
  context["aidememo_context"]
  filters["Apply filters<br/>source_id / current_only / include_archive / as_of"]
  bm25["BM25 lexical search"]
  hnsw["Optional semantic HNSW"]
  rerank["Optional TEI rerank<br/>non-fatal fallback"]
  graphCtx["Optional graph and recent context"]
  result["Ranked facts and context pack"]

  request --> entry
  entry --> search
  entry --> query
  entry --> context
  search --> filters
  query --> filters
  context --> filters
  filters --> bm25
  filters --> hnsw
  bm25 --> rerank
  hnsw --> rerank
  rerank --> graphCtx
  graphCtx --> result
```

Use `search` for direct ranked hits, `query` for a focused context pack, and
`context` for the broad opening-turn envelope. The CLI defaults to the
auto-hybrid policy: a BM25 probe stays lexical when confidence is good and
promotes weak or CJK queries to semantic retrieval when the semantic path is
ready. `--hybrid` forces semantic ranking for every query. MCP callers can pass
`bm25_only:true` when they need deterministic low-latency behavior.

## Write and lifecycle flow

```mermaid
flowchart TD
  fact["fact add / fact_add_many<br/>typed content + entities"]
  classify["Agent classifies fact_type<br/>decision / lesson / error / preference / note"]
  entities["Auto-create or resolve entities"]
  hot["Hot store write<br/>facts + entity links + relations"]
  session["Optional session link<br/>AIDEMEMO_SESSION_ID or session_id"]
  pin["Pinned context"]
  supersede["Validity window<br/>supersede old fact"]
  archive["Cold-tier archive<br/>preserve FactId"]
  consolidate["Consolidate / GAC / TTL"]
  rebuild["vector-rebuild --current-only"]
  read["Future search / query / context"]

  fact --> classify --> entities --> hot
  hot --> session
  hot --> pin
  hot --> supersede
  hot --> archive
  hot --> consolidate
  consolidate --> rebuild
  pin --> read
  supersede --> read
  archive --> read
  rebuild --> read
```

Facts are intentionally explicit. AideMemo does not need a built-in hosted
extractor for normal agent loops because the calling agent already has the
stronger model and should classify durable facts before writing them. The
`extract` and `pending` commands exist for opt-in capture and review workflows.

## Cloud and branch-log flow

```mermaid
sequenceDiagram
  participant C as Coordinator store
  participant B as Baseline backup
  participant A as Agent candidate store
  participant L as Branch log

  C->>B: backup create
  B->>A: backup restore --force
  A->>A: local fact writes during experiment
  A->>L: branch push --base backup
  C->>L: inspect selected branch
  L->>C: branch merge --branch winner
```

Branch logs are append-only artifacts for cloud agents and speculative memory
experiments. They are not full multi-master conflict resolution: duplicate
records are skipped through `sync_import`, independent facts are appended, and
semantic conflicts between competing decisions remain application policy.

## Source map

| System area | Primary implementation | Public docs |
|---|---|---|
| CLI commands and parsers | `crates/aidememo-cli/src/cmd/mod.rs`, `crates/aidememo-cli/src/main.rs` | [`CLI Usage`](CLI.md), [`Feature Inventory`](FEATURES.md) |
| MCP tools and schemas | `crates/aidememo-cli/src/cmd/mcp_tools.rs` | [`MCP Setup`](MCP.md), [`Agent Workflows`](AGENT_WORKFLOWS.md) |
| Core API and retrieval | `crates/aidememo-core/src/lib.rs`, `search.rs`, `graph.rs` | [`Architecture`](ARCHITECTURE.md), [`Operations`](OPERATIONS.md) |
| Storage dispatch | `crates/aidememo-core/src/backend.rs`, `sqlite_store.rs`, `store.rs` | [`Operations`](OPERATIONS.md), [`Feature Inventory`](FEATURES.md) |
| Python agent SDK | `packages/aidememo-agent-sdk/src/aidememo_agent/sdk.py` | [`Python SDK`](SDK.md), [`Agent Workflows`](AGENT_WORKFLOWS.md) |
| Native bindings | `crates/aidememo-python`, `crates/aidememo-napi`, `crates/aidememo-nif`, `crates/aidememo-ffi` | [`Python SDK`](SDK.md), package READMEs |
| Validation and release gates | `scripts/changelog-release-check.py`, `scripts/registry-readiness-check.py`, `scripts/cargo-package-readiness.sh`, `scripts/docs-feature-gate.py`, `scripts/docs-site-e2e.py`, `scripts/*smoke*.sh`, `scripts/ci-local.sh` | [`Measurements`](MEASUREMENTS.md), [`Release Checklist`](RELEASE.md) |

## Documentation contract

Documentation validation has two layers:

`scripts/docs-feature-gate.py` is the source-level public-docs drift gate. It
currently checks:

- every top-level CLI command and subcommand listed by `aidememo --help` appears
  in [`Feature Inventory`](FEATURES.md);
- every MCP tool declared in `cmd/mcp_tools.rs::list_tools()` appears in
  [`Feature Inventory`](FEATURES.md);
- public numeric claims such as MCP tool counts, CLI command counts, architecture
  diagram counts, and AGENTS core-tool counts match implementation-derived
  values; the count-claim detector self-tests this rejection path on every run;
- core explanatory docs such as this page, [`Agent Workflows`](AGENT_WORKFLOWS.md),
  and [`Measurements`](MEASUREMENTS.md) are exposed through Docusaurus;
- Mermaid is enabled so system diagrams render as diagrams, not inert code;
- public wording keeps SQLite as the default backend and redb as the optional
  Cargo-feature backend.

`scripts/docs-site-e2e.py` is the rendered-site gate. It builds Docusaurus and
checks that the sitemap, sidebar, homepage cards, page H1s, baseUrl-scoped
links, static assets, anchors, and architecture-doc implementation paths still
match the current repo.

Run it before publishing docs:

```bash
python3 scripts/docs-feature-gate.py
python3 scripts/docs-site-e2e.py
```
