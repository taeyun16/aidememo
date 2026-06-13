---
name: aidememo
description: Local knowledge-graph wiki tool. Use to search, traverse, and append facts to a private markdown wiki indexed with BM25 + semantic vectors. Ideal when the user asks "what do we know about X", wants to record decisions/conventions, or needs context from prior project notes.
license: MIT OR Apache-2.0
compatibility: Requires the `aidememo` CLI binary on PATH (cargo install aidememo-cli, or build from https://github.com/taeyun16/aidememo). Optionally registers as an MCP server (`aidememo mcp` for stdio, `aidememo mcp-serve` for HTTP).
allowed-tools: Bash(aidememo:*)
metadata:
  homepage: https://github.com/taeyun16/aidememo
  version: "1.0"
  claude:
    when_to_use:
      - 'User asks "what do we know about ...", "do we have notes on ...", "search the wiki"'
      - 'User states a decision/convention worth recording ("we decided X", "always do Y")'
      - 'You need persistent context across conversations beyond CLAUDE.md'
---

# AideMemo (`aidememo`)

Structured local wiki: SQLite store by default, optional redb backend, BM25 +
semantic vectors, and entity graph.
All operations are offline and private.

## Quick reference

```bash
# Read
aidememo --json query "<topic>"                       # one-shot context (search + traverse + recent)
aidememo --json search "<query>" --limit 10           # ranked facts (BM25 + semantic)
aidememo --json search "<query>" --as-of 2026-01-01   # what we knew on a given date
aidememo --json entity list --limit 50                # all entities
aidememo --json entity show <name>                    # compiled summary + recent facts
aidememo --json fact list --entity <name> --last 30d --current
aidememo --json traverse <entity> --depth 2           # related entities
aidememo --json recent --last 7d                      # what changed recently

# Write
aidememo fact add "<content>" --type <type> --entities <a>,<b>
aidememo fact supersede <OLD_ID> <NEW_ID>             # validity-window: old becomes "no longer current"
aidememo edit fact <ID> --append/--prepend/--find+--replace/--content
aidememo entity describe <name> "<prose>"             # set / clear compiled-truth summary
aidememo relation add <source> <target> <rel_type>

# Maintenance
aidememo lint --json                                  # graph health (orphan / duplicate / conflict / stale)
aidememo doctor [--json] [--fix]                      # health + memory/disk + agent integration
aidememo vector-rebuild                               # rebuild HNSW after a model swap
aidememo stats --json

# Optional: HuggingFace text-embeddings-inference (TEI)
aidememo config set model.provider tei                 # native /embed + auto /info dimension
aidememo config set model.endpoint http://localhost:8080
aidememo config set rerank.provider tei                # cross-encoder rerank of top-K results
aidememo config set rerank.endpoint http://localhost:8081
aidememo config set rerank.model BAAI/bge-reranker-base
aidememo config set rerank.top_k 32
```

`aidememo query <topic>` collapses the "what do we know about X" workflow into one
call — prefer it when an LLM needs context. Returns
`{topic, entity, search, related, recent_facts}` so the model gets the
resolved entity, top search hits, related entities (graph), and recent facts
in a single response.

`fact_type`: `decision | pattern | convention | claim | note | question | preference | lesson | error`

| Type | Use when | Auto-behaviour |
|---|---|---|
| `decision` | "we'll use X for Y" | atomic per entity (when `lifecycle.auto_supersede_atomic_types=true`); 2× retrieval boost; never decays |
| `convention` | "always X" / "format Y as Z" | atomic per entity; 2× boost; never decays |
| `pattern` | "X uses Y for Z" / architectural pattern | 1.5× boost; never decays |
| **`preference`** ⭐ | first-person preference ("I prefer dark mode") | 2× boost; never decays; surfaced in `aidememo_context.personalisation` |
| **`lesson`** ⭐ | "tried X, hit Y" / learned-the-hard-way | 2× boost; never decays; surfaced in `aidememo_context.personalisation` AND `aidememo_context.topic.topic_lessons` |
| **`error`** ⭐ | recurring failure mode to avoid | 2× boost; never decays; surfaced in `aidememo_context.personalisation` AND `aidememo_context.topic.topic_errors` |
| `claim` | factual assertion | 1× weight |
| `note` | observational, default fallback | 1× weight, decays |
| `question` | open investigation | 0.5× (deprioritised) |

**Atomic types** (`decision`/`convention`) — only one current per entity
when `lifecycle.auto_supersede_atomic_types` is on. `aidememo lint` flags
multiple current ones as conflicts; resolve with `aidememo fact supersede`.

`entity_type`: `technology | concept | comparison | query | person | team`
or any custom string (e.g. `service`, `rfc`, `incident`, `session`).

## When to add facts

- Always link facts to existing entities (run `aidememo entity list` first). Don't
  invent entity names — if no match, ask the user before creating.
- For decisions / conventions / patterns, use `aidememo fact supersede` rather
  than editing in place when the *meaning* changes — the validity window
  preserves the timeline (`--as-of` queries can replay past state).
- Use `aidememo edit fact` only for typo / clarification fixes that don't
  alter what the fact asserts.

## MCP — preferred for tool use

If `aidememo` is registered as an MCP server (`.mcp.json` at the repo root, or
`aidememo mcp-install --target <agent>`), use the MCP tools instead of shelling
out. They return structured JSON.

When a shared store needs per-agent / per-project isolation, install with
`aidememo mcp-install --target <agent> --source-id <namespace>`. That sets
`AIDEMEMO_SOURCE_ID` in the MCP server environment. `aidememo_search`, `aidememo_query`,
`aidememo_context`, `aidememo_workflow_start`, `aidememo_fact_add`, `aidememo_fact_add_many`, and
`aidememo_fact_list` use it as the default source namespace unless the tool call
passes an explicit `source_id`.

| Tool | Use for |
|---|---|
| **`aidememo_workflow_start`** ⭐ | **Issue/PR/ticket automation entry point** — tracked session + ticket fact + context pack with relevant decisions, lessons, errors, and hits |
| **`aidememo_context`** ⭐ | **Top-of-turn entry point** — pinned + personalisation + recent + (with topic) search/traverse/lessons. Replaces session_start → query → search chain |
| `aidememo_query` | Topic-only retrieval (search + entity + traverse + recent). Lighter than aidememo_context — use for follow-up topic dives |
| `aidememo_overview` | First-impression snapshot of an unfamiliar wiki — call once at session start |
| `aidememo_search` | Pure hybrid search, no graph |
| `aidememo_aggregate` | Exact counts, sums, distinct dates, and timelines across facts. Do not use for simple recall or simple retrieval; answer those from `aidememo_context`, `aidememo_query`, or `aidememo_search` snippets |
| `aidememo_recent` | Last N days of facts |
| `aidememo_entity_list` / `aidememo_entity_get` | Browse entities / fetch one by name or alias |
| `aidememo_fact_list` / `aidememo_fact_get` | List facts (filterable; `fact_type:"lesson"` etc) / fetch one by ULID |
| `aidememo_traverse` / `aidememo_backlinks` | Forward / reverse graph walk |
| `aidememo_path` | Shortest path between two entities |
| `aidememo_doctor` / `aidememo_lint` | Health snapshot / raw issues |
| `aidememo_entity_describe` | Set or clear an entity's prose summary |
| `aidememo_fact_add` | Append a single fact (now accepts `preference` / `lesson` / `error`) |
| `aidememo_fact_add_many` | Batched insert (one fsync) — prefer for ≥3 facts |
| `aidememo_fact_supersede` | Mark old fact replaced by a new one |
| `aidememo_fact_edit` | Patch a fact's content (append / prepend / find+replace / content) |
| `aidememo_feedback` | Mark a fact returned by aidememo_search as helpful / not-helpful (closes the adapter loop) |
| `aidememo_extract` | Conversation → candidate facts. Heuristic by default; `llm:true` uses `extract.provider`. `apply:true` persists |
| `aidememo_session_start` | Warmup envelope (legacy — prefer `aidememo_context` without topic) |
| `aidememo_pinned_context` / `aidememo_fact_pin` | "Always loaded" memory tier — pin a fact |

### Recommended agent flow

1. **Workflow trigger / sparse ticket** → `aidememo_workflow_start(title, body?, source?)` — creates a tracked session, records the incoming ticket, and returns the project-memory context pack. Pass its `session_id` to later `aidememo_fact_add` / `aidememo_fact_add_many` calls so facts learned during the task stay attached to the workflow thread.
2. **Top of turn** → `aidememo_context(topic?: <user's intent or relevant entity>)` — one call, gets you pinned facts, user preferences, lessons, errors, recent activity, and (if topic resolves) deep retrieval + topic-specific lessons.
3. **Need more on a sub-topic** → `aidememo_query(topic)` for entity + neighbors, or `aidememo_search(query)` for pure search.
4. **About to record something** → pick the right `fact_type`:
   - User shares a personal preference → `preference`
   - You discovered a non-obvious "X works because Y" → `lesson`
   - You hit a known recurring failure → `error`
   - User decides on technology / approach → `decision`
   - Project convention → `convention`
5. **Long task** → `eval "$(aidememo session new '<topic>')"` once; every `aidememo fact add` thereafter auto-attaches the session entity. Pull the thread later with `aidememo fact list --entity $AIDEMEMO_SESSION_ID`.

## Hermes composition recipes

Hermes can use the plugin as tools, slash commands, hooks, or Python code.
Codex, Claude Code, CI jobs, and local scripts can use the same code-first
surface through `aidememo-agent-sdk`. Pick the workflow profile first, then compose
the smallest primitives that fit.

### coding profile

Use for PRs, issues, and sparse automation triggers.

1. Call `aidememo_workflow_start(title, body, source?)` before planning.
2. Use the returned decisions / lessons / errors as constraints in the plan.
3. Pass `session_id` into `aidememo_fact_add` / `aidememo_fact_add_many` for facts learned
   during the task.

### long-session profile

Use for multi-hour debugging or implementation sessions.

1. Start with `aidememo_context(topic, format:"text", max_chars:<budget>)`.
2. Follow up with `aidememo_query` only for narrower subtopics.
3. Record durable decisions, lessons, and recurring errors; avoid storing
   ordinary progress chatter.

### research profile

Use for experiments, ablations, metric interpretation, or broad evidence
collection. Prefer Python SDK composition when the workflow needs fanout,
coverage checks, dedupe, or batch writes.

```python
from aidememo_agent import Memory

sdk = Memory.open(source_id="research-alpha")
rows = sdk.search_rows([
    {"query": "Hermes top1_mass gate", "tool": "search_query"},
    {"query": "Hermes patch negative prior", "tool": "patch"},
])
coverage = sdk.coverage_by(rows, ["tool", "fact_type"])
sdk.remember([
    {
        "content": "Lesson: top1_mass support gate is the current strongest Hermes signal.",
        "fact_type": "lesson",
        "entities": ["Hermes", "SupportGate"],
    }
])
```

Keep intermediate rows in Python objects or explicit files; do not paste large
candidate lists into the model context. Render only compact coverage tables,
final evidence rows, or the fact batch that will be written.

### team profile

Use when multiple Hermes agents share one store.

1. Configure `plugins.aidememo.source_id` or `AIDEMEMO_SOURCE_ID` once.
2. Let `aidememo_context`, `aidememo_query`, `aidememo_search`, `aidememo_aggregate`, and writes inherit
   that source scope unless a task explicitly needs cross-source reads.
3. Run `/aidememo-doctor` when lock contention or source leakage is suspected.

### safe-capture profile

Use for a new repository or noisy transcript.

1. Set `dry_run: true`.
2. Inspect `/aidememo-pending`.
3. Commit only high-confidence decisions / lessons / errors.

`aidememo_search` / `aidememo_query` / `aidememo_fact_list` default `current_only=true` — the
result set is "what we know now". Pass `current_only:false` for historical
or timeline queries. `aidememo_search` also accepts `since` / `until` / `as_of`
(ISO date or duration like `30d`), `entity` (filter to one entity),
and `min_confidence`.

`aidememo_search` returns `{session_id, results: [...]}`. After acting on the
hits, optionally pass that `session_id` (with the fact_id and a boolean)
to `aidememo_feedback` — the adapter retrains on this signal (`aidememo adapt
train`) and live ranking nudges toward facts you confirmed were useful.

## Install

If `aidememo` is on your PATH, the binary self-installs into the agent of your
choice:

```bash
aidememo skill install --target claude     # → ~/.claude/skills/aidememo/
aidememo skill install --target hermes     # → ~/.hermes/skills/aidememo/
aidememo skill install --target openclaw   # → ~/.openclaw/skills/aidememo/
aidememo skill install --target opencode   # → ~/.config/opencode/AGENTS.md (appended)
aidememo skill install --target pi         # → ~/.config/pi/AGENTS.md (pi has no MCP — skill only)

aidememo mcp-install --target claude --source-id my-project
aidememo mcp-install --target codex --source-id my-project
aidememo mcp-install --target cursor       # writes mcpServers.aidememo in ~/.cursor/mcp.json
aidememo mcp-install --target opencode     # writes mcp.aidememo in ~/.config/opencode/opencode.json
```

`aidememo mcp-install --list-targets` and `aidememo skill install --list-targets` show
every supported agent and the path each would write. Hand-rolled setup steps
are in `setup-claude-code.md`, `setup-codex.md`, and `setup-hermes.md`. The
full API + internals reference is in `REFERENCE.md`.
