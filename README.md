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
- `wg auto-relate [--top-k N] [--threshold F] [--dry-run] [--json]` — add `related` edges between entities whose facts cluster semantically (one-shot; idempotent)
- `wg overview [-n N] [--recent-days D] [--json]` — first-impression snapshot of a wiki (entity-type buckets + fact-type distribution + top entities + recent activity)
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
server. **23 tools** exposed:

| Read | Write |
|---|---|
| `wg_session_start` (one-call warmup) | `wg_fact_add` |
| `wg_overview` (first-impression snapshot) | `wg_fact_add_many` (batched, one fsync) |
| `wg_query` (one-call context) | `wg_fact_supersede` |
| `wg_search` | `wg_fact_edit` |
| `wg_recent` / `wg_pinned_context` | `wg_fact_pin` (toggle "always loaded" tier) |
| `wg_traverse` / `wg_backlinks` | `wg_entity_describe` |
| `wg_path` (shortest entity path) | `wg_feedback` (helpful/not on a search hit) |
| `wg_entity_list` / `wg_entity_get` | `wg_extract` (text → candidate facts, optional auto-apply) |
| `wg_fact_list` / `wg_fact_get` | |
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

### LongMemEval-S retrieval baseline

500 questions from the published [LongMemEval-S cleaned](https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned)
dataset, BM25 via `wg_search`, ~350 ms/question end-to-end
(per-question store build + 50-200 turn ingest + one search):

| | R@1 | R@5 | **R@10** | MRR |
|---|---|---|---|---|
| wg (BM25-only, M-series) | 0.866 | 0.952 | **0.974** | 0.902 |
| wg + time-decay soft-bias (τ=90d) | 0.858 | 0.958 | **0.978** | 0.898 |
| wg + decay + bge-small-en-v1.5 | 0.808 | 0.952 | 0.984 | 0.868 |
| wg + bge-small-en-v1.5 (no decay) | 0.914 | 0.976 | 0.986 | 0.941 |
| wg + bge + bge-reranker-base (narrow K=10) | 0.938 | 0.984 | 0.986 | 0.957 |
| **wg + bge + reranker (two-stage K=20→10)** ★ | **0.940** | **0.984** | **0.992** | **0.958** |

End-to-end with an LLM reader (`scripts/longmemeval_e2e.py`, judge =
`gpt-4o` to match the published-baseline calibration):

| Stack | Reader | Overall |
|---|---|---|
| Mem0 (published) | gpt-4o | 49.0% |
| wg @ model2vec + decay τ=90 | gpt-4o-mini | 60.0% |
| wg @ model2vec + decay τ=90 | gpt-4o | 60.4% |
| wg @ model2vec + decay τ=90 | gpt-5.4-mini | 62.6% |
| wg @ bge + reranker wide K=20→10 | gpt-4o-mini | 65.6% |
| wg @ bge + reranker wide K=20→10 | gpt-5.4-mini | 66.0% |
| wg @ bge + reranker wide K=20→10 | gpt-4o | 67.6% |
| **wg @ bge + reranker wide K=20→10** | **gpt-4.1** | **72.6%** |
| **wg @ bge + reranker wide K=20→10** ★ | **MiniMax-M2.7-highspeed** | **74.0%** |
| Zep / Graphiti 2026 (published) | gpt-4o | 71.2% |
| Supermemory (published) | — | 85.4% |
| Mastra (published) | gpt-4o | 84.2% |
| Mastra (published) | gpt-5-mini | 94.9% |
| OMEGA (published, local) | gpt-4.1 | 95.4% |

`wg + MiniMax-M2.7-highspeed` lands at **74.0%** — beating Mem0
(49%) by **+25.0pt** and edging past Zep/Graphiti (71.2%) by
**+2.8pt** while keeping the entire stack local: a single Rust
binary + ONNX bge embedder + cross-encoder reranker, no Python
services, no vector DB, no graph store, the only network call is
the reader LLM itself (and that can be any OpenAI-compatible
endpoint — MiniMax / OpenAI / Kimi / Ollama / vLLM). Below Mastra
(84.2%) and SOTA local systems (OMEGA 95.4% with 25 lifecycle
tools) — see [`.notes/compare-graphiti.md`](.notes/compare-graphiti.md)
for the wg vs Graphiti head-to-head, [`.notes/omega-pipeline-analysis.md`](.notes/omega-pipeline-analysis.md)
for the OMEGA gap analysis, and [`COMPARE.md`](COMPARE.md) for the
broader architecture comparison. Of wg's remaining errors,
**99% had the right context in the top-10 retrievals**; the failures
are reader-side reasoning (multi-session synthesis, temporal
comparison) — and the worst category (`temporal-reasoning` 39.8%) is
bottlenecked by LongMemEval-S's known timestamp noise rather than wg
retrieval (R@10 there is 0.977). Full per-category breakdown,
judge-calibration analysis, and methodology in
[`.notes/bench-longmemeval.md`](.notes/bench-longmemeval.md).

The R@10 = 99.2% number says the answer-evidence session lands in the
top-10 retrieval set 496 / 500 times — i.e., the LLM has the right
context at hand for nearly every question. End-to-end answer
correctness is left to the LLM the agent uses; this is the slice wg
controls. See [`.notes/bench-longmemeval.md`](.notes/bench-longmemeval.md)
for per-`question_type` breakdown and the reproduction recipe.

## License

MIT OR Apache-2.0.
