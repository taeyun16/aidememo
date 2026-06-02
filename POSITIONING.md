# wg Positioning

`wg` is best positioned as an **agent-friendly SDK memory system for coding
agents**.

Not a hosted memory product. Not a full agent runtime. Not a generic vector DB.

It sits underneath Claude Code, Codex, Cursor, Hermes, and similar tools as a
portable memory/retrieval substrate: one binary, one store, one agent SDK, and
one MCP surface. When the agent can execute Python, `wg-agent-sdk` is the
primary product path: fanout retrieval, dedupe, coverage, aggregation, and
batch fact writes stay in code instead of consuming model context. MCP remains
the model-visible tool surface for agents that cannot or should not run code.

## One-line Position

**`wg` gives coding agents an SDK-first local memory system: typed facts,
temporal history, graph traversal, and hybrid retrieval without a hosted memory
vendor or service stack.**

## Category

`wg` fits in:

- Agent SDK memory system
- Local MCP server
- Temporal knowledge graph
- Agent memory substrate

It does **not** fit cleanly in:

- Cloud memory SaaS
- Memory OS / full agent runtime
- General-purpose vector database

## Who It Is For

Best-fit users:

- Individual developers who want durable agent memory next to their codebase
- Small teams that want one shared local or self-hosted memory daemon
- Code-executing agents that need memory fanout, dedupe, aggregation, and batch
  capture without pushing every intermediate row through tokens
- Tool builders who need an embeddable Rust-backed memory layer with native bindings
- Agent workflows that need explicit facts, history, and graph traversal rather than only embedding recall

Poor-fit users:

- Teams wanting managed multi-tenant cloud memory across many customers
- Users who expect fully automatic LLM memory extraction by default
- Organizations that need true multi-writer merge or distributed conflict resolution
- Buyers looking for an opinionated end-to-end agent runtime that owns prompt, memory, and execution

## Core Wedge

The strongest wedge is not "best benchmark score." The strongest wedge is:

**SDK-first agent memory workflows on top of a lightweight temporal graph.**

Concretely:

- `wg-agent-sdk` gives code-executing agents a memory programming interface:
  `Memory.open`, `search_rows`, `coverage_by`, `aggregate_many`, and
  `remember`.
- MCP tools expose the same memory system when the model needs visible tool
  calls rather than hidden code execution.
- `wg` keeps temporal memory primitives like `supersede`, `current_only`, `as_of`, and archive/cold-tier behavior.
- It ships as a Rust binary with built-in stdio/HTTP MCP instead of a Python + DB + graph-service stack.
- It can be embedded directly via Python, Node, Elixir, and C bindings instead of forcing everything through one server runtime.

That makes `wg` compelling anywhere operational simplicity matters as much as raw memory quality.

## Why It Wins

### 1. Agent-friendly SDK workflow

`wg` is strongest when agents can treat memory as a programmable substrate:

- open a source-scoped memory client once
- fan out many searches without model-visible tool chatter
- dedupe and group retrieved rows in code
- use exact aggregation for counts, totals, and timelines
- write learned decisions, lessons, errors, and preferences in batches

That is the clearest product distinction from tools that only expose a chat
memory endpoint or a raw vector-search API.

### 2. Deployment simplicity

`wg` is unusually small for its category:

- single binary
- single `redb` file
- built-in MCP server
- no Postgres, Qdrant, Neo4j, or separate vector DB

This is the clearest advantage over Graphiti, Letta, and self-hosted mem0-style stacks.

### 3. Better memory model than "just vector search"

`wg` is stronger than lightweight memory tools that stop at embeddings because it has:

- facts with types
- entities and relations
- traversal
- temporal validity windows
- deterministic aggregation

That gives it a better answer to "what is true now?", "what was true then?", and
"how do these things connect?" than a plain retrieval cache.

### 4. Agent-native interface design

The MCP surface is a product asset, not just an integration detail. Tools like:

- `wg_context`
- `wg_query`
- `wg_aggregate`
- `wg_fact_add`

show a clear opinion about how agents should retrieve, count, and write memory.
The Python composition layer carries the same opinion into code-executing
agents: `Memory.open`, `search_rows`, `coverage_by`, `aggregate_many`, and
`remember` are for workflows where intermediate retrieval sets should stay in
program state rather than model context.

### 5. Good economics for high-ingest workflows

The default path does not force an LLM call on every insert. That matters when:

- ingest volume is high
- users want local-first operation
- memory should work offline or with minimal API dependency

## Where It Is Weak

### 1. Automatic memory capture is not the default superpower

Compared with Mem0, Mastra, or richer observational-memory systems, `wg` is weaker at:

- auto-extracting useful memory from arbitrary chats
- curating or rewriting memory with an LLM at insert time
- turning passive conversation streams into high-quality long-term memory without user/agent discipline

Today `wg` is strongest when the calling agent is explicit and deliberate.

### 2. Multi-agent shared write is constrained

`redb`'s single-writer model keeps the local architecture simple, but it means:

- one-process write ownership
- shared-write setups want a daemon pattern
- no beads-style merge story

That is an acceptable tradeoff for local-first operation, but still a real limit.

### 3. Product surface is broader than its current product story

The codebase already contains many serious capabilities:

- archive
- lifecycle/consolidation
- rerank
- adapt
- multi-project
- bindings
- hooks

The risk is diffusion. If messaging stays too broad, `wg` can read as "many memory features"
instead of one sharp product.

### 4. It should not claim SOTA memory performance as its lead message

The benchmark story is respectable, and in some comparisons very good, but the cleanest
message is still:

- simpler deployment
- stronger structure than basic vector memory
- better temporal semantics
- better portability across agents

That is more defensible than leading with "highest score."

## Competitive Position

### Versus Mem0

`wg` should position against Mem0 as:

**more local, more explicit, more structural.**

Mem0 wins on:

- managed experience
- stronger out-of-the-box auto-extraction
- easier cloud adoption

`wg` wins on:

- local-first deployment
- explicit fact/history model
- no default vendor dependence
- graph + temporal semantics

### Versus Letta

`wg` should position against Letta as:

**SDK memory substrate, not memory OS.**

Letta wins when buyers want one system to own the entire agent runtime.

`wg` wins when buyers already have agents and need a portable SDK/tool memory
layer underneath them.

### Versus Graphiti / Zep

This is the most important comparison.

`wg` should position as:

**the lightweight temporal-graph alternative.**

Graphiti/Zep win on:

- richer graph-centric platform story
- community / clustering features
- stronger server-centric architecture

`wg` wins on:

- far lighter deployment
- lower insertion cost
- built-in MCP and local embedding model options
- polyglot in-process embedding into tools

### Versus OMEGA / high-end local memory systems

`wg` should not try to out-OMEGA OMEGA in messaging.

Instead:

**OMEGA is the "highest-performance local memory workflow."**

**`wg` is the "agent-friendly SDK memory system with the lightest serious
temporal graph underneath."**

That is a cleaner, more durable split.

## Message Hierarchy

Recommended order:

1. Agent-friendly SDK memory for coding agents
2. Code-first composition when the agent can run Python
3. MCP for model-visible memory tools
4. One binary, one store, local-first by default
5. Facts + graph + history, not just embeddings
6. Native bindings for tool builders

Avoid leading with:

- benchmark percentages
- vague "AI memory platform" language
- feature-count messaging

## Claims To Make

- "Local-first"
- "SDK-first agent memory"
- "Temporal memory graph"
- "Built for coding agents"
- "One binary, built-in MCP"
- "Graph traversal + hybrid retrieval"
- "Portable across agent environments"

## Claims To Avoid

- "Best overall memory system"
- "Fully automatic memory capture"
- "Multi-writer collaborative memory database"
- "End-to-end agent operating system"
- "Managed enterprise memory platform"

## Suggested Taglines

- `wg`: SDK-first memory for coding agents
- Agent-friendly memory SDK with a local temporal graph underneath
- A repo-adjacent memory graph for Claude Code, Codex, and beyond
- Temporal agent memory without the service stack
- Bring-your-own-agent memory substrate with graph + history

## Short Pitch

`wg` is an SDK-first memory system for coding agents. It stores facts,
entities, relations, and temporal history in a single embedded store, exposes
them through `wg-agent-sdk`, MCP, CLI, and native bindings, and gives agents a
better memory model than plain vector retrieval without forcing users into a
hosted service or a heavy Python infrastructure stack.

## Strategic Focus

If the project wants a sharper market position, the best path is:

1. Double down on the "agent-friendly SDK memory system" identity
2. Keep code-first memory composition as the primary workflow
3. Keep temporal/history semantics as the structural differentiator
4. Treat deployment simplicity as a first-class product feature
5. Improve ingestion quality without giving up the local-first default
6. Avoid drifting into "generic memory platform for everything"

## Bottom Line

`wg` is strongest when presented as:

**the agent-friendly SDK memory system with a serious local temporal graph
underneath.**

That is sharper than "local RAG," more practical than "memory OS," and more
defensible than trying to win a general AI-memory category on benchmark score
alone.
