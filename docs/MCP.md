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

Example Claude Code command:

```bash
claude mcp add aidememo -- aidememo mcp
```

## HTTP MCP server

Use HTTP when multiple agents should share one warm process:

```bash
aidememo mcp-serve --port 3000 --store ~/.aidememo/team.sqlite
```

Then point MCP clients at:

```text
http://127.0.0.1:3000/mcp
```

HTTP mode is still useful for warm model reuse and shared writes. It is
especially recommended for redb stores, where only one writer process can hold
the database lock at a time.

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
aidememo --backend libsqlite mcp-install --target codex --source-id team-a
```

MCP tools then default to that source namespace when the client does not pass an
explicit `source_id`. The installed command also pins the selected storage
backend and resolved store path so an agent process does not drift back to a
different config default or working directory. Use `--actor-id` independently
when multiple agent profiles share that namespace and writes need provenance.

For isolated Codex accounts, repeat `--codex-home` and `--actor-id` while
pointing every profile at the same explicit store. See
[`Share Memory Across Codex Profiles`](CODEX_MULTI_PROFILE.md).

## Troubleshooting

| Symptom | Fix |
|---|---|
| Agent cannot see tools | Confirm MCP config path and restart the agent |
| One Codex profile cannot see AideMemo | Install into its active `CODEX_HOME`, or pass `--codex-home` explicitly |
| `command not found: aidememo` | Use an absolute path in MCP config |
| Agent opens the wrong store | Reinstall with global `--store`; `aidememo doctor` reports Codex store mismatches |
| Store lock errors | Use one `aidememo mcp-serve` process for shared writes |
| Wrong project context appears | Add or verify `source_id` scoping |
