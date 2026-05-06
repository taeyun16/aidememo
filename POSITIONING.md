# wg Positioning

`wg` is best positioned as a **local-first temporal memory graph for coding agents**.

Not a hosted memory product. Not a full agent runtime. Not a generic vector DB.

It sits underneath Claude Code, Codex, Cursor, Hermes, and similar tools as a
portable memory/retrieval substrate: one binary, one store, one MCP surface.

## One-line Position

**`wg` gives coding agents repo-adjacent memory with temporal facts, graph
traversal, and hybrid retrieval, without requiring a Python service stack or a
hosted memory vendor.**

## Category

`wg` fits in:

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
- Tool builders who need an embeddable Rust-backed memory layer with native bindings
- Agent workflows that need explicit facts, history, and graph traversal rather than only embedding recall

Poor-fit users:

- Teams wanting managed multi-tenant cloud memory across many customers
- Users who expect fully automatic LLM memory extraction by default
- Organizations that need true multi-writer merge or distributed conflict resolution
- Buyers looking for an opinionated end-to-end agent runtime that owns prompt, memory, and execution

## Core Wedge

The strongest wedge is not "best benchmark score." The strongest wedge is:

**Graphiti-like temporal memory semantics in a much lighter deployment model.**

Concretely:

- `wg` keeps temporal memory primitives like `supersede`, `current_only`, `as_of`, and archive/cold-tier behavior.
- It ships as a Rust binary with built-in stdio/HTTP MCP instead of a Python + DB + graph-service stack.
- It can be embedded directly via Python, Node, Elixir, and C bindings instead of forcing everything through one server runtime.

That makes `wg` compelling anywhere operational simplicity matters as much as raw memory quality.

## Why It Wins

### 1. Deployment simplicity

`wg` is unusually small for its category:

- single binary
- single `redb` file
- built-in MCP server
- no Postgres, Qdrant, Neo4j, or separate vector DB

This is the clearest advantage over Graphiti, Letta, and self-hosted mem0-style stacks.

### 2. Better memory model than "just vector search"

`wg` is stronger than lightweight memory tools that stop at embeddings because it has:

- facts with types
- entities and relations
- traversal
- temporal validity windows
- deterministic aggregation

That gives it a better answer to "what is true now?", "what was true then?", and
"how do these things connect?" than a plain retrieval cache.

### 3. Agent-native interface design

The MCP surface is a product asset, not just an integration detail. Tools like:

- `wg_context`
- `wg_query`
- `wg_aggregate`
- `wg_fact_add`

show a clear opinion about how agents should retrieve, count, and write memory.

### 4. Good economics for high-ingest workflows

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

**memory substrate, not memory OS.**

Letta wins when buyers want one system to own the entire agent runtime.

`wg` wins when buyers already have agents and only need a portable memory layer.

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

**`wg` is the "most portable and operationally light temporal memory layer."**

That is a cleaner, more durable split.

## Message Hierarchy

Recommended order:

1. Local-first temporal memory for coding agents
2. One binary, one store, built-in MCP
3. Facts + graph + history, not just embeddings
4. Bring your own agent; `wg` plugs underneath
5. Native bindings for tool builders

Avoid leading with:

- benchmark percentages
- vague "AI memory platform" language
- feature-count messaging

## Claims To Make

- "Local-first"
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

- `wg`: local-first temporal memory for coding agents
- A repo-adjacent memory graph for Claude Code, Codex, and beyond
- Temporal agent memory without the service stack
- Bring-your-own-agent memory substrate with graph + history

## Short Pitch

`wg` is a local-first memory layer for coding agents. It stores facts,
entities, relations, and temporal history in a single embedded store, exposes
them through CLI, MCP, and native bindings, and gives agents a better memory
model than plain vector retrieval without forcing users into a hosted service
or a heavy Python infrastructure stack.

## Strategic Focus

If the project wants a sharper market position, the best path is:

1. Double down on the "coding-agent memory substrate" identity
2. Keep temporal/history semantics as the differentiator
3. Treat deployment simplicity as a first-class product feature
4. Improve ingestion quality without giving up the local-first default
5. Avoid drifting into "generic memory platform for everything"

## Bottom Line

`wg` is strongest when presented as:

**the lightest serious temporal memory graph you can drop under a coding
agent.**

That is sharper than "local RAG," more practical than "memory OS," and more
defensible than trying to win a general AI-memory category on benchmark score
alone.
