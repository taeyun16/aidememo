# How Does AideMemo Compare?

This page lays out where AideMemo sits in the 2026 agent-memory landscape
and what it's meaningfully better (or worse) at than its neighbours.
The category is crowded — pick the right tool.

> **Bottom line:** AideMemo is an SDK-first agent-memory system with a
> lightweight temporal knowledge graph underneath. It ships as one Rust binary
> with one embedded SQLite file by default; redb remains an optional Cargo
> feature. Code-executing agents can use `aidememo-agent-sdk`, while model-visible
> tools are available over stdio and HTTP MCP. The default memory loop is local
> and does not call an external LLM.

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
| Graph store | Neo4j | Embedded SQLite by default; optional redb Cargo feature |
| Community detection | Yes | No |
| Hybrid retrieval | Yes (vector + graph) | Yes (BM25 + HNSW + cross-encoder rerank + adapter) |
| Setup | Neo4j install + Python | `scripts/install.sh` or `cargo install --git ...` |
| Bindings | Python | 4 native (Py / Node / Elixir / C) |
| Default write path | LLM-aided entity extraction | Deterministic / explicit local capture; optional extractor |
| Benchmark evidence | Follow the project's current published reports | Versioned AideMemo evidence and caveats live in `docs/EVIDENCE.md` and `docs/MEASUREMENTS.md` |

**Pick Graphiti if** you need community / cluster detection and LLM-aided entity
extraction tightly integrated with a graph service.

**Pick AideMemo if** you want temporal validity without committing to a graph
database and prefer an explicit, zero-external-LLM default write path. See
[`docs/EVIDENCE.md`](docs/EVIDENCE.md) for the supported claim boundary.

### vs **OMEGA** (LongMemEval-oriented local memory)

|  | OMEGA | aidememo |
|---|---|---|
| Product emphasis | LongMemEval-oriented memory pipeline | Portable SDK memory for coding agents |
| Embedding | BGE-based retrieval in the published OMEGA report | Multilingual model2vec default; opt-in `bge-small-en-v1.5` for English paraphrase-heavy memory |
| Storage | SQLite + sqlite-vec | SQLite default; optional redb Cargo feature |
| Bindings | Python only | **Py / Node / Elixir / C** in-process |
| Model-visible tools | OMEGA-specific tool surface | 29 MCP tools + CLI |
| Default capture | LLM-aided | Deterministic / explicit; opt-in LLM |
| Encryption at rest | AES-256 | ❌ (relies on OS-level FS encryption) |
| Hook-based auto-capture | Built-in deeply integrated | aidememo-skill/hooks/ 3 scripts (manual install) |
| Forgetting | 5 mechanisms (SHA-256 dedup, 0.85 evolution, TTL, Jaccard compaction, conflict detection) | 4 (exact dedup, atomic conflict, semantic dedup, TTL — all via `aidememo consolidate`) |
| Multi-store | ❌ | ✅ `aidememo project create/use` |

**Pick OMEGA if** your main objective is a specialized LongMemEval-style memory
pipeline and its capture/runtime trade-offs fit your deployment.

**Pick AideMemo if** you want a multilingual local default, four native
bindings, multi-store support, zero external-LLM cost on the default write path,
and the ability to bring your own reader. See
[`docs/MEASUREMENTS.md`](docs/MEASUREMENTS.md) for versioned measurements rather
than treating this comparison page as a benchmark ledger.

### vs **beads** (the closest neighbour)

|  | beads | aidememo |
|---|---|---|
| Primary unit | Issues + dependencies | Entities + facts (with typed relations) |
| Killer verb | `bd ready` (next unblocked issue) | `aidememo query` (search + traverse + recent in one) |
| Storage | Embedded Dolt (MySQL-compat) | SQLite default; optional redb Cargo feature |
| Concurrent / branch writes | Git-style cell merge | SQLite same-host writers; daemon for a warm shared path; append-only branch segment merge |
| Free-text search | SQL `LIKE` | BM25 + semantic + rerank |
| Performance evidence | Follow the current beads benchmark | AideMemo measurements are reported with backend and execution-envelope caveats |

**Pick beads if** you want a multi-agent dependency-aware issue tracker with git-style merge.
**Pick aidememo if** you want fast hybrid retrieval over a knowledge graph.

## Where aidememo wins on its own merits

1. **Single-binary local-first.** Install from Git or the one-line installer;
   no Python, Postgres, Qdrant, or Neo4j service is required for the default
   path.

2. **SDK-first for code-executing agents.** `aidememo-agent-sdk` gives agents a
   programmable memory working set: `Memory.open`, `search_rows`,
   `coverage_by`, `aggregate_many`, and `remember`. MCP remains the
   model-visible tool path when the agent should call tools directly.

3. **MCP-native on both transports.** stdio (`aidememo mcp`) for in-editor
   agents, HTTP/SSE (`aidememo mcp-serve`) for shared / remote clients —
   the same 29-tool surface served by the same dispatcher in-process Rust.

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

8. **Measured performance with explicit envelopes.** Backend, daemon warmth,
   model readiness, and dataset shape are recorded with the result. The public
   scorecard links to the reproducible command or artifact instead of carrying
   an unversioned cross-product multiplier here.

## Where aidememo lags (be honest)

| Gap | What's missing | Status |
|---|---|---|
| LLM-grade auto-extraction | Some peers pre-curate chat at insert time with an LLM; AideMemo's default `aidememo_extract` path is heuristic. | An agent can feed reviewed structured output to `aidememo_fact_add_many`; AideMemo keeps this explicit so the default path stays local-first and zero-token. |
| LongMemEval specialization | Systems with deeper capture hooks and benchmark-specific prompting can optimize harder for LongMemEval. | AideMemo publishes its dated retrieval and reader results in `docs/MEASUREMENTS.md`; this page does not duplicate volatile scores. |
| Distributed conflict resolution | beads has git-style cell merge; AideMemo does not merge arbitrary concurrent edits. | SQLite supports small same-host writer groups, `aidememo mcp-serve` provides one warm shared path, and branch logs import append-only segments idempotently. General distributed conflict resolution remains out of scope. |
| Community / cluster detection | Graphiti groups related entities into communities. | Not implemented; out of scope for v0.x. |
| Cloud-managed multi-tenant ops | SaaS peers partition users and operate the service for you. | `source_id` scopes facts/search inside one store and projects provide hard local isolation, but AideMemo does not try to be hosted multi-tenant infrastructure. |
| Entity centrality | Zep / Graphiti boost facts on central nodes. | AideMemo ships `search.entity_centrality_weight` (commit `0d67e47`); default off but turning it on adds `1 + w * log10(1 + max_fact_count)` to the rrf weight. |

## When AideMemo Is The Right Call

- Your agent needs project memory that survives across IDEs / model
  vendors / agent runtimes — not memory tied to one SaaS account.
- You want the wiki to live in the repo (or alongside it) and be reproducible:
  one SQLite file by default, with optional redb when its trade-offs fit.
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
