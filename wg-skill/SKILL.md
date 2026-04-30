---
name: wg
description: Local knowledge-graph wiki tool. Use to search, traverse, and append facts to a private markdown wiki indexed with BM25 + semantic vectors. Ideal when the user asks "what do we know about X", wants to record decisions/conventions, or needs context from prior project notes.
license: MIT OR Apache-2.0
compatibility: Requires the wg CLI binary on PATH (cargo install wg-cli, or build from https://github.com/aspect-build/wg). Optionally registers as an MCP server (`wg mcp` for stdio, `wg mcp-serve` for HTTP).
allowed-tools: Bash(wg:*)
metadata:
  homepage: https://github.com/aspect-build/wg
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

`fact_type`: `decision | pattern | convention | claim | note | question`
- *Atomic types* (decision / convention / pattern) are mutually exclusive
  per entity — `wg lint` flags multiple current ones as conflicts. Resolve
  with `wg fact supersede`.
- *Non-atomic* (claim / note / question) coexist freely.

`entity_type`: `technology | concept | comparison | query | person | team`
or any custom string (e.g. `service`, `rfc`, `incident`).

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

| Tool | Use for |
|---|---|
| `wg_query` | One-call context fetch (preferred over chaining) |
| `wg_search` | Pure hybrid search, no graph |
| `wg_recent` | Last N days of facts |
| `wg_entity_list` / `wg_entity_get` | Browse entities / fetch one by name or alias |
| `wg_fact_list` / `wg_fact_get` | List facts (filterable) / fetch one by ULID |
| `wg_traverse` / `wg_backlinks` | Forward / reverse graph walk |
| `wg_path` | Shortest path between two entities |
| `wg_doctor` / `wg_lint` | Health snapshot / raw issues |
| `wg_entity_describe` | Set or clear an entity's prose summary |
| `wg_fact_add` | Append a single fact |
| `wg_fact_add_many` | Batched insert (one fsync) — prefer for ≥3 facts |
| `wg_fact_supersede` | Mark old fact replaced by a new one |
| `wg_fact_edit` | Patch a fact's content (append / prepend / find+replace / content) |
| `wg_feedback` | Mark a fact returned by wg_search as helpful / not-helpful (closes the adapter loop) |
| `wg_extract` | Heuristic conversation → candidate facts. `apply:true` persists them; otherwise returns previews to edit |
| `wg_session_start` | One-call warmup: pinned + recent + top entities + open issues |
| `wg_pinned_context` / `wg_fact_pin` | "Always loaded" memory tier — pin a fact for session start |

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

wg mcp-install --target claude       # claude mcp add wg -- wg mcp
wg mcp-install --target codex        # writes [mcp_servers.wg] in ~/.codex/config.toml
wg mcp-install --target cursor       # writes mcpServers.wg in ~/.cursor/mcp.json
wg mcp-install --target opencode     # writes mcp.wg in ~/.config/opencode/opencode.json
```

`wg mcp-install --list-targets` and `wg skill install --list-targets` show
every supported agent and the path each would write. Hand-rolled setup steps
are in `setup-claude-code.md`, `setup-codex.md`, and `setup-hermes.md`. The
full API + internals reference is in `REFERENCE.md`.
