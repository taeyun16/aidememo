# How does `wg` compare?

This page lays out where `wg` sits in the 2026 agent-memory landscape
and what it's meaningfully better (or worse) at than its neighbours.
The category is crowded — pick the right tool.

> **Bottom line:** `wg` is the lightest agent-memory backend that
> still gives you a temporal knowledge graph + hybrid retrieval. One
> Rust binary, one redb file, MCP-native on stdio + HTTP. If you want
> a service to run, vendor lock-in, or auto-extraction from raw chat,
> look elsewhere.

## Category map

| Category | Representative | What you give up |
|---|---|---|
| Cloud-first SaaS | [mem0 managed](https://mem0.ai/), [Zep](https://www.getzep.com/) | data egress, vendor lock, network round-trip per call |
| Self-hosted server | [mem0 OSS](https://github.com/mem0ai/mem0), [Letta](https://github.com/letta-ai/letta), [Graphiti](https://github.com/getzep/graphiti) | Python + Postgres + Qdrant + Neo4j footprint |
| Local MCP server | mem0 OpenMemory MCP, [Supermemory](https://supermemory.ai/), OMEGA, **wg** | (most don't have validity windows or graph traversal) |
| Memory OS | [Letta (MemGPT)](https://github.com/letta-ai/letta) | Python runtime, single-language ecosystem |
| Observational | [Mastra](https://mastra.ai/), MemPalace | requires a hosted LLM call per insert |
| Temporal KG | [Graphiti / Zep](https://github.com/getzep/graphiti) | Neo4j install, Python-only client |

`wg` lives in **Local MCP server + Temporal KG hybrid**, and within
that slice carries the smallest footprint.

## Head-to-head

### vs **mem0** (the popular default)

|  | mem0 | wg |
|---|---|---|
| Default deployment | Managed cloud | Local single binary |
| Open-source local | mem0 OSS = Python + 3 services | 1 Rust binary + 1 file |
| MCP server | OpenMemory MCP (separate package) | Built-in (`wg mcp`) |
| Conversation auto-extract | Yes (LLM-driven) | Yes — heuristic only (`wg_extract`); LLM via agent |
| Validity windows | No (snapshot model) | Yes (`as_of` replay) |
| Polyglot bindings | Python first | Python · Node · Elixir · C, all in-process |
| Importance / pinning | Yes | Yes (`wg_fact_pin` + `wg_pinned_context`) |

**Pick mem0 if** you want managed cloud and LLM-grade extraction out of the box.
**Pick wg if** you want everything local, no service to run, and the wiki to live next to the code.

### vs **Letta** (memory-OS philosophy)

|  | Letta | wg |
|---|---|---|
| Architecture | Stateful agent runtime + memory tiers | Stateless library + MCP, agent stays where it is |
| Memory hierarchy | Core / archival baked in | `pinned: bool` tier on facts |
| Self-editing memory | Yes — agent rewrites its own context | No — agent calls `fact_add` / `fact_supersede` explicitly |
| Polyglot | Python only | 4 native bindings |
| Footprint | Python server + DB | Single binary |

**Pick Letta if** you want a full agent runtime that manages its own memory pages.
**Pick wg if** you bring your own agent (Claude Code, Cursor, Codex CLI) and want the memory layer to be pluggable.

### vs **Graphiti / Zep** (temporal knowledge graphs)

|  | Graphiti | wg |
|---|---|---|
| Validity windows | Yes — pioneered the model | Yes — same `superseded_at` semantics |
| Graph store | Neo4j | redb single-file |
| Community detection | Yes | No |
| Hybrid retrieval | Yes (vector + graph) | Yes (BM25 + HNSW + cross-encoder rerank + adapter) |
| Setup | Neo4j install + Python | `cargo install wg-cli` |
| Bindings | Python | 4 native (Py / Node / Elixir / C) |

**Pick Graphiti if** you need community / cluster detection and already run Neo4j.
**Pick wg if** you want the same temporal model without committing to a graph database.

### vs **beads** (the closest neighbour)

|  | beads | wg |
|---|---|---|
| Primary unit | Issues + dependencies | Entities + facts (with typed relations) |
| Killer verb | `bd ready` (next unblocked issue) | `wg query` (search + traverse + recent in one) |
| Storage | Embedded Dolt (MySQL-compat) | redb (single-file) |
| Multi-writer merge | Yes — git-style cell merge | No — single-writer (use `wg mcp-serve` for shared) |
| Free-text search | SQL `LIKE` | BM25 + semantic + rerank |
| Bulk write throughput | ~5 s for 1k issues | **~339×** faster on 10k (see [`.notes/bench-beads-results.md`](./.notes/bench-beads-results.md)) |
| Cold start to first query | ~50 ms (Dolt boot) | ~5 ms (redb open) |

**Pick beads if** you want a multi-agent dependency-aware issue tracker with git-style merge.
**Pick wg if** you want fast hybrid retrieval over a knowledge graph.

## Where wg wins on its own merits

1. **Single-binary local-first.** `cargo install wg-cli` and you're
   done — no Python, no Postgres, no Qdrant, no Neo4j. Most "local"
   alternatives in 2026 still want at least three services.

2. **MCP-native on both transports.** stdio (`wg mcp`) for in-editor
   agents, HTTP/SSE (`wg mcp-serve`) for shared / remote clients —
   same 22-tool surface served by the same dispatcher in-process Rust.

3. **Polyglot in-process bindings.** Python · Node · Elixir · C all
   call the same `WikiGraph` API directly without IPC. Lets editor
   plugins, IDE extensions, and CLI tools share the wiki without a
   service intermediary.

4. **Hybrid retrieval out of the box.** BM25 + HNSW + cross-encoder
   rerank + per-fact adapter — all in-process, no external vector DB.
   The TEI integration adds remote rerankers as an opt-in layer.

5. **Validity windows.** `wg fact supersede` + `--as-of <date>` reproduces past state. Graphiti has this; mem0 / Letta don't.

6. **Agent guardrails inline.** `wg_fact_add` returns `existing_similar`
   (BM25 dedup hint) and `entity_name_alternatives` (typo warning) in
   the same response — observable side effects that cloud SaaS
   provides only through a UI.

7. **Single-machine performance.** ~339× faster bulk write than beads
   on 10k inserts ([source](.notes/bench-beads-results.md)). Daemon
   hot-path search ~9 ms BM25 / ~45 ms HNSW.

## Where wg lags (be honest)

| Gap | What's missing | Status |
|---|---|---|
| LLM-grade auto-extraction | Mastra / Supermemory hit 95-99% on LongMemEval by feeding raw chat to an LLM. wg's `wg_extract` is heuristic-only. | Agent can still call its own LLM and feed structured output to `wg_fact_add_many`. Built-in LLM extraction is a future feature. |
| Multi-writer merge | beads has git-style cell merge; wg is single-writer. | Use `wg mcp-serve` to share one daemon; full distributed merge isn't in scope. |
| Community / cluster detection | Graphiti groups related entities into communities. | Not implemented; out of scope for v0.x. |
| LongMemEval public score | mem0 / Zep have published numbers. | Harness lives at `benchmarks/src/bin/longmemeval.rs`; [run instructions](.notes/bench-longmemeval.md). Numbers TBD. |
| Per-user / multi-tenant | Cloud peers partition by `user_id`. | Workaround: separate stores via `wg --project`. Native multi-tenant is a future feature. |

## When `wg` is the right call

- Your agent needs project memory that survives across IDEs / model
  vendors / agent runtimes — not memory tied to one SaaS account.
- You want the wiki to live in the repo (or alongside it) and be
  reproducible: a single redb file + git.
- You're embedding into an editor extension / IDE plugin / CLI tool
  and need the in-process API rather than a network call.
- You like Graphiti's temporal model but don't want to install Neo4j.
- You're allergic to vendor lock-in and Python service stacks.

## When `wg` is the wrong call

- You need cloud-managed multi-tenant memory across thousands of users.
- You need state-of-the-art chat extraction (use a hosted LLM there).
- You need distributed multi-writer merge.
- You need a memory-OS runtime that rewrites the agent's prompt for it.

## See also

- [`.notes/compare-beads.md`](.notes/compare-beads.md) — beads-specific deep dive
- [`.notes/bench-longmemeval.md`](.notes/bench-longmemeval.md) — retrieval-only LongMemEval-S baseline
- [`.notes/project-completeness.md`](.notes/project-completeness.md) — internal completeness scorecard
- [`AGENTS.md`](./AGENTS.md) — full agent guide (CLI + MCP surface)

## Sources cited

This page draws on the 2026 agent-memory landscape survey:

- [State of AI Agent Memory 2026 — Mem0](https://mem0.ai/blog/state-of-ai-agent-memory-2026)
- [Best AI Agent Memory Frameworks 2026 — Atlan](https://atlan.com/know/best-ai-agent-memory-frameworks-2026/)
- [Mem0 vs Letta (MemGPT) — Vectorize](https://vectorize.io/articles/mem0-vs-letta)
- [Mem0 vs Zep vs Letta vs Cognee — n1n.ai](https://explore.n1n.ai/blog/ai-agent-memory-comparison-2026-mem0-zep-letta-cognee-2026-04-23)
- [Letta forum: agent memory landscape](https://forum.letta.com/t/agent-memory-letta-vs-mem0-vs-zep-vs-cognee/88)
- [LongMemEval — arXiv](https://arxiv.org/abs/2410.10813)
- [Mastra Observational Memory](https://mastra.ai/research/observational-memory)
- [The Agent Memory Race of 2026 — OSS Insight](https://ossinsight.io/blog/agent-memory-race-2026)
