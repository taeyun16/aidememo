---
name: wg
description: Local knowledge-graph wiki tool. Use to search, traverse, and append facts to a private markdown wiki indexed with BM25 + semantic vectors. Ideal when the user asks "what do we know about X", wants to record decisions/conventions, or needs context from prior project notes.
license: MIT OR Apache-2.0
compatibility: Requires the wg CLI binary on PATH (cargo install wg-cli, or build from https://github.com/taeyun16/wg). Optionally registers as an MCP server (`wg mcp` for stdio, `wg mcp-serve` for HTTP).
allowed-tools: Bash(wg:*)
metadata:
  homepage: https://github.com/taeyun16/wg
  version: "1.0"
  claude:
    when_to_use:
      - 'User asks "what do we know about ...", "do we have notes on ...", "search the wiki"'
      - 'User states a decision/convention worth recording ("we decided X", "always do Y")'
      - 'You need persistent context across conversations beyond CLAUDE.md'
---

# wg — Wiki-Graph

Structured local wiki: redb store + BM25 + semantic vectors + entity graph.
All operations are offline and private.

## Quick reference

```bash
# Read
wg --json query "<topic>"                       # one-shot context (search + traverse + recent)
wg --json search "<query>" --limit 10           # ranked facts (BM25 + semantic)
wg --json search "<query>" --as-of 2026-01-01   # what we knew on a given date
wg --json entity list --limit 50                # all entities
wg --json entity show <name>                    # compiled summary + recent facts
wg --json fact list --entity <name> --last 30d --current
wg --json traverse <entity> --depth 2           # related entities
wg --json recent --last 7d                      # what changed recently

# Write
wg fact add "<content>" --type <type> --entities <a>,<b>
wg fact supersede <OLD_ID> <NEW_ID>             # validity-window: old becomes "no longer current"
wg edit fact <ID> --append/--prepend/--find+--replace/--content
wg entity describe <name> "<prose>"             # set / clear compiled-truth summary
wg relation add <source> <target> <rel_type>

# Maintenance
wg lint --json                                  # graph health (orphan / duplicate / conflict / stale)
wg doctor [--json] [--fix]                      # health + memory/disk + agent integration
wg vector-rebuild                               # rebuild HNSW after a model swap
wg stats --json

# Optional: HuggingFace text-embeddings-inference (TEI)
wg config set model.provider tei                 # native /embed + auto /info dimension
wg config set model.endpoint http://localhost:8080
wg config set rerank.provider tei                # cross-encoder rerank of top-K results
wg config set rerank.endpoint http://localhost:8081
wg config set rerank.model BAAI/bge-reranker-base
wg config set rerank.top_k 32
```

`wg query <topic>` collapses the "what do we know about X" workflow into one
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
| **`preference`** ⭐ | first-person preference ("I prefer dark mode") | 2× boost; never decays; surfaced in `wg_context.personalisation` |
| **`lesson`** ⭐ | "tried X, hit Y" / learned-the-hard-way | 2× boost; never decays; surfaced in `wg_context.personalisation` AND `wg_context.topic.topic_lessons` |
| **`error`** ⭐ | recurring failure mode to avoid | 2× boost; never decays; surfaced in `wg_context.personalisation` AND `wg_context.topic.topic_errors` |
| `claim` | factual assertion | 1× weight |
| `note` | observational, default fallback | 1× weight, decays |
| `question` | open investigation | 0.5× (deprioritised) |

**Atomic types** (`decision`/`convention`) — only one current per entity
when `lifecycle.auto_supersede_atomic_types` is on. `wg lint` flags
multiple current ones as conflicts; resolve with `wg fact supersede`.

`entity_type`: `technology | concept | comparison | query | person | team`
or any custom string (e.g. `service`, `rfc`, `incident`, `session`).

## When to add facts

- Always link facts to existing entities (run `wg entity list` first). Don't
  invent entity names — if no match, ask the user before creating.
- For decisions / conventions / patterns, use `wg fact supersede` rather
  than editing in place when the *meaning* changes — the validity window
  preserves the timeline (`--as-of` queries can replay past state).
- Use `wg edit fact` only for typo / clarification fixes that don't
  alter what the fact asserts.

## MCP — preferred for tool use

If `wg` is registered as an MCP server (`.mcp.json` at the repo root, or
`wg mcp-install --target <agent>`), use the MCP tools instead of shelling
out. They return structured JSON.

When a shared store needs per-agent / per-project isolation, install with
`wg mcp-install --target <agent> --source-id <namespace>`. That sets
`WG_SOURCE_ID` in the MCP server environment. `wg_search`, `wg_query`,
`wg_context`, `wg_workflow_start`, `wg_fact_add`, `wg_fact_add_many`, and
`wg_fact_list` use it as the default source namespace unless the tool call
passes an explicit `source_id`.

| Tool | Use for |
|---|---|
| **`wg_workflow_start`** ⭐ | **Issue/PR/ticket automation entry point** — tracked session + ticket fact + context pack with relevant decisions, lessons, errors, and hits |
| **`wg_context`** ⭐ | **Top-of-turn entry point** — pinned + personalisation + recent + (with topic) search/traverse/lessons. Replaces session_start → query → search chain |
| `wg_query` | Topic-only retrieval (search + entity + traverse + recent). Lighter than wg_context — use for follow-up topic dives |
| `wg_overview` | First-impression snapshot of an unfamiliar wiki — call once at session start |
| `wg_search` | Pure hybrid search, no graph |
| `wg_recent` | Last N days of facts |
| `wg_entity_list` / `wg_entity_get` | Browse entities / fetch one by name or alias |
| `wg_fact_list` / `wg_fact_get` | List facts (filterable; `fact_type:"lesson"` etc) / fetch one by ULID |
| `wg_traverse` / `wg_backlinks` | Forward / reverse graph walk |
| `wg_path` | Shortest path between two entities |
| `wg_doctor` / `wg_lint` | Health snapshot / raw issues |
| `wg_entity_describe` | Set or clear an entity's prose summary |
| `wg_fact_add` | Append a single fact (now accepts `preference` / `lesson` / `error`) |
| `wg_fact_add_many` | Batched insert (one fsync) — prefer for ≥3 facts |
| `wg_fact_supersede` | Mark old fact replaced by a new one |
| `wg_fact_edit` | Patch a fact's content (append / prepend / find+replace / content) |
| `wg_feedback` | Mark a fact returned by wg_search as helpful / not-helpful (closes the adapter loop) |
| `wg_extract` | Conversation → candidate facts. Heuristic by default; `llm:true` uses `extract.provider`. `apply:true` persists |
| `wg_session_start` | Warmup envelope (legacy — prefer `wg_context` without topic) |
| `wg_pinned_context` / `wg_fact_pin` | "Always loaded" memory tier — pin a fact |

### Recommended agent flow

1. **Workflow trigger / sparse ticket** → `wg_workflow_start(title, body?, source?)` — creates a tracked session, records the incoming ticket, and returns the project-memory context pack. Pass its `session_id` to later `wg_fact_add` / `wg_fact_add_many` calls so facts learned during the task stay attached to the workflow thread.
2. **Top of turn** → `wg_context(topic?: <user's intent or relevant entity>)` — one call, gets you pinned facts, user preferences, lessons, errors, recent activity, and (if topic resolves) deep retrieval + topic-specific lessons.
3. **Need more on a sub-topic** → `wg_query(topic)` for entity + neighbors, or `wg_search(query)` for pure search.
4. **About to record something** → pick the right `fact_type`:
   - User shares a personal preference → `preference`
   - You discovered a non-obvious "X works because Y" → `lesson`
   - You hit a known recurring failure → `error`
   - User decides on technology / approach → `decision`
   - Project convention → `convention`
5. **Long task** → `eval "$(wg session new '<topic>')"` once; every `wg fact add` thereafter auto-attaches the session entity. Pull the thread later with `wg fact list --entity $WG_SESSION_ID`.

## Hermes composition recipes

Hermes can use the plugin as tools, slash commands, hooks, or Python code. Pick
the workflow profile first, then compose the smallest primitives that fit.

### coding profile

Use for PRs, issues, and sparse automation triggers.

1. Call `wg_workflow_start(title, body, source?)` before planning.
2. Use the returned decisions / lessons / errors as constraints in the plan.
3. Pass `session_id` into `wg_fact_add` / `wg_fact_add_many` for facts learned
   during the task.

### long-session profile

Use for multi-hour debugging or implementation sessions.

1. Start with `wg_context(topic, format:"text", max_chars:<budget>)`.
2. Follow up with `wg_query` only for narrower subtopics.
3. Record durable decisions, lessons, and recurring errors; avoid storing
   ordinary progress chatter.

### research profile

Use for experiments, ablations, metric interpretation, or broad evidence
collection. Prefer Python SDK composition when the workflow needs fanout,
coverage checks, dedupe, or batch writes.

```python
from hermes_wg import WgClient, WgMemorySDK

sdk = WgMemorySDK(WgClient(source_id="research-alpha"))
fanout = sdk.search_many([
    {"query": "Hermes top1_mass gate", "tool": "search_query"},
    {"query": "Hermes patch negative prior", "tool": "patch"},
])
rows = sdk.dedupe_by_fact(sdk.flatten_hits(fanout))
coverage = sdk.coverage_by(rows, ["tool", "fact_type"])
items = sdk.to_fact_batch([
    {
        "content": "Lesson: top1_mass support gate is the current strongest Hermes signal.",
        "fact_type": "lesson",
        "entities": ["Hermes", "SupportGate"],
    }
])
sdk.commit_fact_batch(items)
```

Keep intermediate rows in Python objects or explicit files; do not paste large
candidate lists into the model context. Render only compact coverage tables,
final evidence rows, or the fact batch that will be written.

### team profile

Use when multiple Hermes agents share one store.

1. Configure `plugins.wg.source_id` or `WG_SOURCE_ID` once.
2. Let `wg_context`, `wg_query`, `wg_search`, `wg_aggregate`, and writes inherit
   that source scope unless a task explicitly needs cross-source reads.
3. Run `/wg-doctor` when lock contention or source leakage is suspected.

### safe-capture profile

Use for a new repository or noisy transcript.

1. Set `dry_run: true`.
2. Inspect `/wg-pending`.
3. Commit only high-confidence decisions / lessons / errors.

`wg_search` / `wg_query` / `wg_fact_list` default `current_only=true` — the
result set is "what we know now". Pass `current_only:false` for historical
or timeline queries. `wg_search` also accepts `since` / `until` / `as_of`
(ISO date or duration like `30d`), `entity` (filter to one entity),
and `min_confidence`.

`wg_search` returns `{session_id, results: [...]}`. After acting on the
hits, optionally pass that `session_id` (with the fact_id and a boolean)
to `wg_feedback` — the adapter retrains on this signal (`wg adapt
train`) and live ranking nudges toward facts you confirmed were useful.

## Install

If `wg` is on your PATH, the binary self-installs into the agent of your
choice:

```bash
wg skill install --target claude     # → ~/.claude/skills/wg/
wg skill install --target hermes     # → ~/.hermes/skills/wg/
wg skill install --target openclaw   # → ~/.openclaw/skills/wg/
wg skill install --target opencode   # → ~/.config/opencode/AGENTS.md (appended)
wg skill install --target pi         # → ~/.config/pi/AGENTS.md (pi has no MCP — skill only)

wg mcp-install --target claude --source-id my-project
wg mcp-install --target codex --source-id my-project
wg mcp-install --target cursor       # writes mcpServers.wg in ~/.cursor/mcp.json
wg mcp-install --target opencode     # writes mcp.wg in ~/.config/opencode/opencode.json
```

`wg mcp-install --list-targets` and `wg skill install --list-targets` show
every supported agent and the path each would write. Hand-rolled setup steps
are in `setup-claude-code.md`, `setup-codex.md`, and `setup-hermes.md`. The
full API + internals reference is in `REFERENCE.md`.
