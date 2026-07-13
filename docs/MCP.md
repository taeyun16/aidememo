---
title: MCP Setup
description: Register AideMemo as an MCP server and use its core tools.
---

# MCP Setup

AideMemo exposes the same memory store through MCP so agents can search and
write memory as tools.
For the complete tool inventory, see [`Feature Inventory`](FEATURES.md).
For guidance on choosing the right tool per turn, see
[`Agent Workflows`](AGENT_WORKFLOWS.md).

## Stdio MCP

Use stdio MCP for local agents.

```bash
aidememo mcp
```

Example Codex config:

```toml
[mcp_servers.aidememo]
command = "aidememo"
args = ["--backend", "libsqlite", "--store", "/absolute/project/_meta/wiki.sqlite", "mcp"]

[mcp_servers.aidememo.env]
AIDEMEMO_SOURCE_ID = "project:my-app"
AIDEMEMO_ACTOR_ID = "codex:account-a"
```

Claude Code standalone registration:

```bash
aidememo --store /absolute/project/_meta/wiki.sqlite mcp-install \
  --target claude \
  --source-id project:my-app \
  --actor-id claude:local
```

This uses Claude Code's current CLI argument order and pins the resolved store.
The bundled Claude plugin is an alternative that also includes focused skills
and read-only hooks. Hermes, Cursor, OpenClaw, and OpenCode also have installer
targets. pi is intentionally skill-only because it does not accept MCP. See
[`Coding Agent Setup`](CODING_AGENTS.md) for the complete matrix.

## HTTP MCP server

Use HTTP when multiple agents should share one warm process:

```bash
aidememo --store ~/.aidememo/team.sqlite mcp-serve --port 3000
```

Then point MCP clients at:

```text
http://127.0.0.1:3000/mcp
```

HTTP mode is still useful for warm model reuse and shared writes. It is
especially recommended for redb stores, where only one writer process can hold
the database lock at a time.

For a network-exposed shared store, bind each bearer token to one source and
writer identity instead of giving every client the same unscoped token:

```json title="/etc/aidememo/token-bindings.json"
{
  "tokens": [
    {
      "token": "replace-with-a-random-secret",
      "source_id": "project:my-app",
      "actor_id": "codex:account-a"
    }
  ]
}
```

```bash
chmod 600 /etc/aidememo/token-bindings.json
aidememo --store ~/.aidememo/team.sqlite mcp-serve \
  --bind 0.0.0.0 \
  --auth-bindings-file /etc/aidememo/token-bindings.json
```

`AIDEMEMO_MCP_AUTH_BINDINGS_FILE` is the environment-variable equivalent.
The file may be either the wrapped `{"tokens": [...]}` object shown above or
a top-level JSON array. Every `token`, `source_id`, and `actor_id` is trimmed
and must be non-empty, and token values must be unique. `--auth-bindings-file`
cannot be combined with `--auth-token` or `--auth-token-file`; an explicit CLI
auth option takes precedence over the auth environment variables.

For a bound token, the server injects the configured `source_id` and
`actor_id` into every MCP tool call and rejects a caller-supplied mismatch,
including overrides inside `aidememo_fact_add_many` items. Bound tokens cannot
use the unscoped `/sync/since` or `/admin/status` endpoints; `/health` returns
only the health and semantic-prewarm state. Keep the existing
`--auth-token-file` mode for a trusted, unscoped administrator. A single-token
administrator is not source-bound: that client may choose `source_id` and
`actor_id` in tool arguments and can use the global status and sync endpoints.

`mcp-serve` itself speaks plain HTTP. Bearer binding provides identity and
scope enforcement, not transport encryption. For any non-loopback deployment,
put the server behind a TLS-terminating reverse proxy or an encrypted private
tunnel and restrict direct access to the backend port.

## Core tools

Most agent workflows only need these tools:

| Tool | Use when |
|---|---|
| `aidememo_workflow_start` | A task starts from an issue, PR, ticket, or sparse prompt |
| `aidememo_context` | The agent needs opening-turn project context |
| `aidememo_query` | The agent needs a focused topic dive |
| `aidememo_search` | The agent needs pinpoint retrieval |
| `aidememo_aggregate` | The agent needs exact counts, totals, date sets, or timelines |
| `aidememo_session_canvas` | The agent is resuming a long tracked workflow |
| `aidememo_profile_export` | The agent needs a compact read-only project profile |
| `aidememo_fact_add` | The agent learned a new fact |
| `aidememo_fact_add_many` | The agent learned several facts and should batch them |

## Recommended agent pattern

At the start of a ticket:

```json
{
  "title": "Fix Redis timeout in worker",
  "body": "Worker jobs intermittently time out against Redis.",
  "source": "github:org/app#123",
  "source_id": "team-a",
  "bm25_only": true
}
```

Call:

```text
aidememo_workflow_start
```

Then use the returned `session_id` when adding follow-up facts:

```json
{
  "content": "Lesson: the timeout was DNS resolution, not pool size.",
  "fact_type": "lesson",
  "entities": ["Redis", "Worker"],
  "session_id": "session-..."
}
```

Call:

```text
aidememo_fact_add
```

## Source scoping

Use `source_id` when a shared store contains multiple teams, projects, users, or
agents.

```bash
aidememo --backend libsqlite mcp-install --target <agent> --source-id team-a
```

MCP tools then default to that source namespace when the client does not pass an
explicit `source_id`. The installed command also pins the selected storage
backend and resolved store path so an agent process does not drift back to a
different config default or working directory. Use `--actor-id` independently
when multiple agent profiles share that namespace and writes need provenance.

Source scoping applies consistently to fact search/list/get, pinned context,
entity reads, graph traversal/path/export, and ID-based fact mutations. A
source-scoped entity result is returned only when that entity has facts in the
source; global entity metadata without source provenance is omitted. Identical
fact content deduplicates within one source, while the same text in two sources
keeps two independent fact IDs. Graph relations have their own source
provenance: a scoped graph read accepts only an exact relation namespace match,
so legacy unscoped edges and another source's evidence, weight, or relation type
are not exposed. Use token bindings above when the client must not be allowed to
choose or override its own scope.

This is a strong partition for cooperating agents in one trusted team store,
not a full hostile multi-tenant database boundary. Entity names and entity
types intentionally form a shared ontology across sources. If tenants must not
share even that ontology or must be protected from one another's resource use,
give them separate stores (or separate AideMemo processes) instead.
See [`Shared Memory Layer`](SHARED_MEMORY.md) for the deployment shapes,
trust boundary, and production checklist behind this choice.

For isolated Codex accounts, repeat `--codex-home` and `--actor-id` while
pointing every profile at the same explicit store. See
[`Share Memory Across Codex Profiles`](CODEX_MULTI_PROFILE.md).

## Troubleshooting

| Symptom | Fix |
|---|---|
| Agent cannot see tools | Confirm MCP config path and restart the agent |
| Claude isolated profile cannot see its skill | Set `CLAUDE_CONFIG_DIR` before `skill install --target claude` |
| One Codex profile cannot see AideMemo | Install into its active `CODEX_HOME`, or pass `--codex-home` explicitly |
| Hermes isolated profile cannot see AideMemo | Set `HERMES_HOME` before installing both the skill and MCP entry |
| pi suggests an MCP step | Update AideMemo and use `skill install --target pi` only |
| `command not found: aidememo` | Use an absolute path in MCP config |
| Agent opens the wrong store | Reinstall with global `--store`; `aidememo doctor` reports Codex store mismatches |
| Store lock errors | Use one `aidememo mcp-serve` process for shared writes |
| Wrong project context appears | Add or verify `source_id` scoping |
