---
title: Share Memory Across Codex Profiles
description: Let isolated Codex accounts share explicit project memory without sharing login state or chat history.
---

# Share Memory Across Codex Profiles

Codex profiles can keep separate accounts, authentication state, and task history while
working in the same repository. AideMemo can bridge the durable project knowledge that
should survive those profile boundaries.

> Two Codex accounts. One project memory. Switch accounts, not context.

This is project-memory continuity, not session synchronization. AideMemo does not copy
Codex cookies, credentials, chat transcripts, or account state.

## Model the boundary correctly

Use one shared store and one shared `source_id` for the project. Give each Codex profile
its own `actor_id` for provenance.

```text
Codex account A ─┐
                 ├─ AideMemo MCP ─ shared.sqlite
Codex account B ─┘

source_id = project:aidememo
actor_id  = codex:account-a | codex:account-b
```

`source_id` is the retrieval namespace. If account A and account B use different
`source_id` values, their default reads will be isolated instead of shared. `actor_id`
answers a different question: which profile wrote this fact?

## Install two Codex profiles

Choose an explicit store. The installer writes that absolute path into every MCP entry,
so the selected store does not depend on the directory from which Codex later launches
the server.

```bash
STORE="$(pwd)/_meta/wiki.sqlite"

aidememo config set store.lock_retry_ms 5000

aidememo --backend libsqlite --store "$STORE" mcp-install \
  --target codex \
  --codex-home "$HOME/.codex-account-a" \
  --actor-id codex:account-a \
  --codex-home "$HOME/.codex-account-b" \
  --actor-id codex:account-b \
  --source-id project:aidememo
```

Without `--codex-home`, the installer uses the active `CODEX_HOME`, then falls back to
`~/.codex`. Repeat `--actor-id` in the same order as repeated `--codex-home` values. A
single `--actor-id` can be reused for every target profile when that is intentional.

Verify each isolated profile:

```bash
CODEX_HOME="$HOME/.codex-account-a" codex mcp list
CODEX_HOME="$HOME/.codex-account-b" codex mcp list

aidememo --store "$STORE" doctor
```

`aidememo doctor` reports when the active Codex profile is registered but points at a
different or unpinned store.

## Handoff between accounts

Account A can store a durable decision:

```json
{
  "content": "Decision: use SQLite WAL for the shared local memory store.",
  "fact_type": "decision",
  "entities": ["AideMemo", "SQLite"]
}
```

The installed MCP environment supplies both `source_id` and `actor_id`. Account B can
open a new Codex session and call `aidememo_context` or `aidememo_query`; returned facts
retain `actor_id: "codex:account-a"`. Account B can add a lesson, and account A sees it
on the next retrieval from the same project namespace.

For code-first integrations, the same defaults are available as
`AIDEMEMO_SOURCE_ID` and `AIDEMEMO_ACTOR_ID`.

When account B is explicitly continuing a tracked workflow, link the sessions:

```json
{
  "title": "Resume the SQLite contention investigation",
  "parent_session_id": "session-01...",
  "source_id": "project:aidememo"
}
```

`aidememo_workflow_start` creates a `continued_from` graph edge from the new session to
the parent. This preserves lineage without storing or replaying the full Codex chat.

## Choose the shared-write mode

Two local Codex profiles can usually share the default SQLite store directly. AideMemo
uses WAL mode, starts write transactions with `BEGIN IMMEDIATE`, and combines a short
SQLite busy timeout with jittered application retries up to `store.lock_retry_ms`.

For heavier parallel writes, or when using the optional redb backend, run one HTTP MCP
server and point both profiles at it:

```bash
AIDEMEMO_SOURCE_ID=project:aidememo \
  aidememo --backend libsqlite --store "$STORE" mcp-serve --port 3000

CODEX_HOME="$HOME/.codex-account-a" \
  codex mcp add aidememo --url http://127.0.0.1:3000/mcp
```

HTTP clients share the server process, so per-client `actor_id` should be passed in write
tool calls until authenticated client identity is available at the transport layer.

## What AideMemo borrows from Hermes session storage

[Hermes Agent session storage](https://hermes-agent.nousresearch.com/docs/developer-guide/session-storage)
uses SQLite WAL, distinguishes session source and user identity, records parent-session
lineage, and applies jittered retries under write contention. AideMemo adopts the parts
that strengthen project-memory continuity:

- `source_id` and `actor_id` remain separate concepts.
- resumed workflows can point to a parent session through `continued_from`.
- shared SQLite writes use early lock acquisition and jittered retries.

AideMemo deliberately does not copy Hermes' full message, tool-call, reasoning, token,
or billing archive. Its durable layer stays focused on explicit typed facts, relations,
and auditable workflow artifacts. That keeps the memory portable across coding agents
and reduces accidental retention of sensitive transcripts.

## Scope limits

- Different OS users need filesystem permission to the same store, or a shared HTTP MCP
  server reachable by both users.
- A local store does not provide live cross-machine synchronization. Use backup/restore
  or branch push/merge for controlled transfer between machines.
- Sharing project memory across accounts should be intentional. Do not bridge stores
  across organizational or policy boundaries without approval.
