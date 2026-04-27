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
wg --json query "<topic>"                     # one-shot context: search + traverse + recent (preferred)
wg --json search "<query>" --limit 10         # raw ranked facts (BM25 + semantic)
wg --json entity list --limit 50              # all entities
wg --json fact list --entity <name> --last 30d
wg --json traverse <entity> --depth 2         # related entities
wg fact add "<content>" --type <type> --entities <a>,<b>
wg lint --json                                # graph health
wg stats --json                               # counts + size
```

`wg query <topic>` collapses the "what do we know about X" workflow into one
call — prefer it when an LLM needs context. Returns
`{topic, entity, search, related, recent_facts}` so the model gets the
resolved entity, top search hits, related entities (graph), and recent facts
in a single response.

`fact_type`: `decision | pattern | convention | claim | note | question`
`entity_type`: `technology | concept | comparison | query | person | team`

## When to add facts

Always link facts to existing entities (run `wg entity list` first). Don't
invent entity names — if no match, ask the user before creating.

## MCP — preferred for tool use

If `wg` is registered as an MCP server (it usually is — see `.mcp.json` at the
repo root, or `claude mcp add wg -- wg mcp`), use the MCP tools `wg_query`
(unified context), `wg_search`, `wg_entity_list`, `wg_traverse`, `wg_lint`,
`wg_fact_add` instead of shelling out. They return structured JSON.

## Install

If `wg` is on your PATH, the binary self-installs into the agent of your
choice:

```bash
wg skill install --target claude     # → ~/.claude/skills/wg/
wg skill install --target hermes     # → ~/.hermes/skills/wg/
wg skill install --target openclaw   # → ~/.openclaw/skills/wg/

wg mcp-install --target claude       # claude mcp add wg -- wg mcp
wg mcp-install --target codex        # writes [mcp_servers.wg] in ~/.codex/config.toml
wg mcp-install --target cursor       # writes mcpServers.wg in ~/.cursor/mcp.json
```

`wg mcp-install --list-targets` and `wg skill install --list-targets` show
every supported agent and the path each would write. Hand-rolled setup steps
are in `setup-claude-code.md`, `setup-codex.md`, and `setup-hermes.md`. The
full API + internals reference is in `REFERENCE.md`.
