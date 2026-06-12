# How Does AideMemo Compare?

This page lays out where AideMemo sits in the 2026 agent-memory landscape
and what it's meaningfully better (or worse) at than its neighbours.
The category is crowded — pick the right tool.

> **Bottom line:** AideMemo is an SDK-first agent-memory system with a
> lightweight temporal knowledge graph underneath. One Rust binary, one redb
> file, `aidememo-agent-sdk` for code-executing agents, and MCP-native tools on
> stdio + HTTP. If you want a hosted service, vendor lock-in, or default LLM
> extraction from raw chat, look elsewhere.

## Category map

| Category | Representative | What you give up |
|---|---|---|
| Cloud-first SaaS | [mem0 managed](https://mem0.ai/), [Zep](https://www.getzep.com/) | data egress, vendor lock, network round-trip per call |
| Self-hosted server | [mem0 OSS](https://github.com/mem0ai/mem0), [Letta](https://github.com/letta-ai/letta), [Graphiti](https://github.com/getzep/graphiti) | Python + Postgres + Qdrant + Neo4j footprint |
| Local MCP server | mem0 OpenMemory MCP, [Supermemory](https://supermemory.ai/), OMEGA, **AideMemo** | (most don't have validity windows or graph traversal) |
| Memory OS | [Letta (MemGPT)](https://github.com/letta-ai/letta) | Python runtime, single-language ecosystem |
| Observational | [Mastra](https://mastra.ai/), MemPalace | requires a hosted LLM call per insert |
| Temporal KG | [Graphiti / Zep](https://github.com/getzep/graphiti) | Neo4j install, Python-only client |

AideMemo lives in **Local MCP server + Temporal KG hybrid**, and within
that slice carries the smallest footprint.

## Head-to-head

### vs **mem0** (the popular default)

|  | mem0 | aidememo |
|---|---|---|
| Default deployment | Managed cloud | Local single binary |
| Open-source local | mem0 OSS = Python + 3 services | 1 Rust binary + 1 file |
| MCP server | OpenMemory MCP (separate package) | Built-in (`aidememo mcp`) |
| Conversation auto-extract | Yes (LLM-driven) | Heuristic / agent-assisted (`aidememo_extract`, `fact_add_many`) |
| Validity windows | No (snapshot model) | Yes (`as_of` replay) |
| Polyglot bindings | Python first | Python · Node · Elixir · C, all in-process |
| Importance / pinning | Yes | Yes (`aidememo_fact_pin` + `aidememo_pinned_context`) |

**Pick mem0 if** you want managed cloud and LLM-grade extraction out of the box.
**Pick aidememo if** you want everything local, no service to run, and the wiki to live next to the code.

### vs **Letta** (memory-OS philosophy)

|  | Letta | aidememo |
|---|---|---|
| Architecture | Stateful agent runtime + memory tiers | Stateless library + MCP, agent stays where it is |
| Memory hierarchy | Core / archival baked in | `pinned: bool` tier on facts |
| Self-editing memory | Yes — agent rewrites its own context | No — agent calls `fact_add` / `fact_supersede` explicitly |
| Polyglot | Python only | 4 native bindings |
| Footprint | Python server + DB | Single binary |

**Pick Letta if** you want a full agent runtime that manages its own memory pages.
**Pick aidememo if** you bring your own agent (Claude Code, Cursor, Codex CLI) and want the memory layer to be pluggable.

### vs **Graphiti / Zep** (temporal knowledge graphs)

|  | Graphiti | aidememo |
|---|---|---|
| Validity windows | Yes — pioneered the model | Yes — same `superseded_at` semantics |
| Graph store | Neo4j | redb single-file |
| Community detection | Yes | No |
| Hybrid retrieval | Yes (vector + graph) | Yes (BM25 + HNSW + cross-encoder rerank + adapter) |
| Setup | Neo4j install + Python | `cargo install aidememo-cli` |
| Bindings | Python | 4 native (Py / Node / Elixir / C) |
| **Insert tokens / session** | ~12,000 (LLM extraction) | **0** (heuristic / explicit facts) |
| **Insert cost / 100K sessions** | $180 (gpt-4o-mini) – $3,000 (gpt-4o) | **$0** |
| LongMemEval E2E (gpt-4o reader, gpt-4o judge) | 71.2% | 67.6% (-3.6pt) |
| **LongMemEval E2E (best reader, gpt-4o judge)** | 71.2% (gpt-4o) | **74.0% (MiniMax-M2.7-highspeed) +2.8pt** ⭐ |
| Retrieval R@10 (LongMemEval-S) | not published | **0.992** |

**Pick Graphiti if** you need community / cluster detection, LLM-grade entity extraction at insert time, and the +3.6pt E2E margin justifies the per-insert LLM cost + Neo4j footprint.

**Pick aidememo if** you want the same temporal model without committing to a graph database — and especially if you ingest at scale (the insert-token gap is the single biggest cost lever; see [`docs/MEASUREMENTS.md`](docs/MEASUREMENTS.md) for the measurement ledger).

### vs **OMEGA** (current LongMemEval SOTA among local-first systems)

|  | OMEGA | aidememo |
|---|---|---|
| LongMemEval E2E (gpt-4o judge) | **95.4%** (gpt-4.1 reader) | **74.0%** (MiniMax-M2.7-highspeed reader) |
| Embedding | bge-small-en-v1.5 ONNX (384-dim) | **bge-small-en-v1.5 ONNX (384-dim)** ← same |
| Storage | SQLite + sqlite-vec | redb single-file |
| Bindings | Python only | **Py / Node / Elixir / C** in-process |
| Tools | 25 core + 29 coordination (omega-pro) | 25 MCP tools + CLI |
| Insert tokens / 100K sessions | LLM-aided default ≈ \$10-30 | **0** (heuristic / explicit default; opt-in LLM) |
| Encryption at rest | AES-256 | ❌ (relies on OS-level FS encryption) |
| Hook-based auto-capture | Built-in deeply integrated | aidememo-skill/hooks/ 3 scripts (manual install) |
| Forgetting | 5 mechanisms (SHA-256 dedup, 0.85 evolution, TTL, Jaccard compaction, conflict detection) | 4 (exact dedup, atomic conflict, semantic dedup, TTL — all via `aidememo consolidate`) |
| Multi-store | ❌ | ✅ `aidememo project create/use` |

**Pick OMEGA if** you want the highest LongMemEval score, are happy with Python+SQLite, and accept paying for hook-based LLM-aided ingestion at every conversation turn.

**Pick aidememo if** you want the same retrieval base (bge ONNX in-process) without Python, with 4 native bindings, multi-store native, zero insert-token cost by default, and the ability to slot any reader (gpt-4.1 / gpt-4o / MiniMax / Ollama). It still trails OMEGA by ~21pt on LongMemEval E2E today; the gap is mostly hook-depth, reader/prompting, and temporal reasoning. See [`docs/MEASUREMENTS.md`](docs/MEASUREMENTS.md) for the current benchmark ledger.

### vs **beads** (the closest neighbour)

|  | beads | aidememo |
|---|---|---|
| Primary unit | Issues + dependencies | Entities + facts (with typed relations) |
| Killer verb | `bd ready` (next unblocked issue) | `aidememo query` (search + traverse + recent in one) |
| Storage | Embedded Dolt (MySQL-compat) | redb (single-file) |
| Multi-writer merge | Yes — git-style cell merge | No — single-writer (use `aidememo mcp-serve` for shared) |
| Free-text search | SQL `LIKE` | BM25 + semantic + rerank |
| Bulk write throughput | ~5 s for 1k issues | **~339×** faster on 10k (see [`bench/beads-vs-aidememo/results`](bench/beads-vs-aidememo/results)) |
| Cold start to first query | ~50 ms (Dolt boot) | ~5 ms (redb open) |

**Pick beads if** you want a multi-agent dependency-aware issue tracker with git-style merge.
**Pick aidememo if** you want fast hybrid retrieval over a knowledge graph.

## Where aidememo wins on its own merits

1. **Single-binary local-first.** `cargo install aidememo-cli` and you're
   done — no Python, no Postgres, no Qdrant, no Neo4j. Most "local"
   alternatives in 2026 still want at least three services.

2. **SDK-first for code-executing agents.** `aidememo-agent-sdk` gives agents a
   programmable memory working set: `Memory.open`, `search_rows`,
   `coverage_by`, `aggregate_many`, and `remember`. MCP remains the
   model-visible tool path when the agent should call tools directly.

3. **MCP-native on both transports.** stdio (`aidememo mcp`) for in-editor
   agents, HTTP/SSE (`aidememo mcp-serve`) for shared / remote clients —
   same 25-tool surface served by the same dispatcher in-process Rust.

4. **Polyglot in-process bindings.** Python · Node · Elixir · C all
   call the same `AideMemo` API directly without IPC. Lets editor
   plugins, IDE extensions, and CLI tools share the wiki without a
   service intermediary.

5. **Hybrid retrieval without an external vector DB.** BM25 + HNSW,
   type-aware ranking, optional in-process cross-encoder rerank, and a
   per-fact adapter are all available locally. The TEI integration adds
   remote embedding / rerank services as an opt-in layer.

6. **Validity windows.** `aidememo fact supersede` + `--as-of <date>` reproduces past state. Graphiti has this; mem0 / Letta don't.

7. **Agent guardrails inline.** `aidememo_fact_add` returns `existing_similar`
   (BM25 dedup hint) and `entity_name_alternatives` (typo warning) in
   the same response — observable side effects that cloud SaaS
   provides only through a UI.

8. **Single-machine performance.** ~339× faster bulk write than beads
   on 10k inserts ([source](bench/beads-vs-aidememo/results)). Daemon
   hot-path search ~9 ms BM25 / ~45 ms HNSW.

## Where aidememo lags (be honest)

| Gap | What's missing | Status |
|---|---|---|
| LLM-grade auto-extraction | Mastra Observer/Reflector and Supermemory pre-curate chat at insert time with an LLM, hitting 84-95% on LongMemEval. AideMemo's default `aidememo_extract` path is heuristic. | Agent can call its own LLM and feed structured output to `aidememo_fact_add_many`; AideMemo keeps this explicit so the default path stays local-first and zero-token. |
| LongMemEval E2E SOTA | OMEGA reports 95.4% with gpt-4.1, deep hooks, rerank, and temporal prompting. | AideMemo @ bge + reranker K=20→10 measured 74.0% with MiniMax-M2.7-highspeed and 72.6% with gpt-4.1. Retrieval is high (R@10 = 0.992); remaining errors are mostly reader-side temporal / multi-session reasoning. |
| Multi-writer merge | beads has git-style cell merge; AideMemo is single-writer. | Use `lock_retry_ms` for small same-host teams, one `aidememo mcp-serve` daemon for shared writes, and pull-only sync for local read caches. Full distributed write merge isn't in scope. |
| Community / cluster detection | Graphiti groups related entities into communities. | Not implemented; out of scope for v0.x. |
| Cloud-managed multi-tenant ops | SaaS peers partition users and operate the service for you. | `source_id` scopes facts/search inside one store and projects provide hard local isolation, but AideMemo does not try to be hosted multi-tenant infrastructure. |
| Entity centrality | Zep / Graphiti boost facts on central nodes. | AideMemo ships `search.entity_centrality_weight` (commit `0d67e47`); default off but turning it on adds `1 + w * log10(1 + max_fact_count)` to the rrf weight. |

## When AideMemo Is The Right Call

- Your agent needs project memory that survives across IDEs / model
  vendors / agent runtimes — not memory tied to one SaaS account.
- You want the wiki to live in the repo (or alongside it) and be
  reproducible: a single redb file + git.
- You're embedding into an editor extension / IDE plugin / CLI tool
  and need the in-process API rather than a network call.
- You like Graphiti's temporal model but don't want to install Neo4j.
- You're allergic to vendor lock-in and Python service stacks.

## When AideMemo Is The Wrong Call

- You need cloud-managed multi-tenant memory across thousands of users.
- You need state-of-the-art chat extraction (use a hosted LLM there).
- You need distributed multi-writer merge.
- You need a memory-OS runtime that rewrites the agent's prompt for it.

## See also

- [`docs/MEASUREMENTS.md`](docs/MEASUREMENTS.md) — benchmark and agent-UX measurement ledger
- [`docs/SDK_POSITIONING.md`](docs/SDK_POSITIONING.md) — SDK promotion criteria and product positioning
- [`bench/multi-agent/README.md`](bench/multi-agent/README.md) — multi-agent scenario benchmarks
- [`AGENTS.md`](./AGENTS.md) — full agent guide (CLI + MCP surface)

## Sources cited

This page draws on the 2026 agent-memory landscape survey:

- [State of AI Agent Memory 2026 — Mem0](https://mem0.ai/blog/state-of-ai-agent-memory-2026)
- [Best AI Agent Memory Frameworks 2026 — Atlan](https://atlan.com/know/best-ai-agent-memory-frameworks-2026/)
- [Mem0 vs Letta (MemGPT) — Vectorize](https://vectorize.io/articles/mem0-vs-letta)
- [Mem0 vs Zep vs Letta vs Cognee — n1n.ai](https://explore.n1n.ai/blog/ai-agent-memory-comparison-2026-mem0-zep-letta-cognee-2026-04-23)
- [Letta forum: agent memory landscape](https://forum.letta.com/t/agent-memory-letta-vs-mem0-vs-zep-vs-cognee/88)
- [LongMemEval — arXiv](https://arxiv.org/abs/2410.10813)
- [LongMemEval-V2 — arXiv](https://arxiv.org/abs/2605.12493)
- [SkillOpt — arXiv](https://arxiv.org/abs/2605.23904)
- [SkillOpt project page](https://microsoft.github.io/SkillOpt/)
- [SkillOps — arXiv](https://arxiv.org/abs/2605.13716)
- [SkillMOO — arXiv](https://arxiv.org/abs/2604.09297)
- [Mastra Observational Memory](https://mastra.ai/research/observational-memory)
- [OMEGA benchmark report](https://omegamax.co/docs/benchmark-report)
- [beads documentation](https://gastownhall.github.io/beads/)
- [The Agent Memory Race of 2026 — OSS Insight](https://ossinsight.io/blog/agent-memory-race-2026)
