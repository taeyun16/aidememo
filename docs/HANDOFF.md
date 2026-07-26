---
title: Hand off a tracked task
description: Move one AideMemo workflow to another coding-agent account without copying chat history.
---

# Hand off a tracked task

Shared memory keeps project context available to every connected agent. A
handoff is the deliberate step you take when ownership changes: it sends the
same tracked session to a named agent account with a clear focus and definition
of done, then links the returned evidence to that session.

> Handoff is currently an unreleased `main` feature. Public v0.1.0 artifacts do
> not include these commands. Build the current checkout with
> `cargo install --path crates/aidememo-cli` before following this guide.

## The short path

### 1. Start tracked work

```bash
eval "$(aidememo session new 'Review the Redis timeout patch')"
export AIDEMEMO_ACTOR_ID=codex-one
```

Facts added while `AIDEMEMO_SESSION_ID` is set stay attached to this workflow.
Record any decision, failed attempt, lesson, or open question that the next
worker will need.

### 2. Connect the destination once

```bash
aidememo agent add codex-two --type codex \
  --home /path/to/codex-two-home \
  --workspace "$PWD" \
  --source-id release-team
```

The profile contains routing metadata and paths, never credentials. AideMemo
passes the configured home to the coding agent at execution time.

### 3. Send the active session

```bash
aidememo handoff send codex-two \
  --focus "Review the Redis timeout patch" \
  --done-when "Focused tests pass and review findings are recorded"
```

`send` infers the current session and sender from the environment. It stores a
small assignment pointer rather than copying the chat or session facts.

### 4. Continue in the receiving account

```bash
aidememo handoff run codex-two
```

The runner accepts the oldest pending assignment for `codex-two`, reconstructs
the current packet from the tracked session, launches the configured coding
agent, and returns its result to the same session.

### 5. Inspect what came back

Use the id printed by `send`:

```bash
aidememo handoff show handoff-...
aidememo handoff outbox --actor-id codex-one
```

The sender sees the returned outcome and linked result fact without opening the
receiver's vendor-local chat.

## What stays the same

| Stays with the workflow | Changes with the worker |
|---|---|
| `session_id`, durable facts, decisions, failures, and result evidence | `actor_id`, coding-agent installation, runtime, and role |
| Project or tenant scope under `source_id` | The explicit `focus` and `done_when` for this assignment |
| The auditable fact history | Vendor-local chat or process state |

Shared memory and handoff are complementary:

- **Shared memory is always on.** Connected agents can retrieve durable project
  knowledge from the same source-scoped store.
- **Handoff is deliberate.** Use it when a named worker should take ownership of
  a tracked task and return evidence.

## Manual receiver flow

Use the manual lifecycle when the receiving runtime has no verified automatic
adapter or an orchestrator needs explicit control:

```bash
aidememo agent add cursor-review --type manual --workspace "$PWD"
aidememo handoff send cursor-review --focus "Review the patch"

AIDEMEMO_ACTOR_ID=cursor-review aidememo handoff inbox
AIDEMEMO_ACTOR_ID=cursor-review aidememo handoff accept handoff-...

# Record the result as a fact on the returned session, then:
AIDEMEMO_ACTOR_ID=cursor-review aidememo handoff return \
  --outcome succeeded \
  --result-fact-id 01... \
  handoff-...
```

`handoff run cursor-review` intentionally refuses because a manual profile has
no process adapter.

## Routing model

| Field | Job |
|---|---|
| `session_id` | Continuity: the tracked workflow the receiver resumes |
| `source_id` | Scope: the project, team, or tenant facts the workflow can retrieve |
| `actor_id` | Address: a user-assigned account or installation alias; not authentication |
| agent/profile | Runtime metadata describing where the work should run |
| `focus` | The next concrete objective |
| `done_when` | The observable completion condition |

Without dispatch, `aidememo session handoff` remains a read-only packet preview.
With dispatch, the receiver pulls one assignment pointer and `accept`
reconstructs the packet from current session evidence.

## Operational boundary

- `handoff board` is a derived view of `ready`, `in_progress`, `attention`, and
  `returned` assignments. It is not another Kanban system.
- Automatic runs time out after 1800 seconds by default. Use
  `handoff run codex-two --timeout 14400` for longer work.
- Long-running workers record an AideMemo heartbeat every 3600 seconds by
  default.
- A linked Hermes card remains owned by Hermes for claims, dependencies, retry,
  and completion. AideMemo carries the external session pointer and result
  evidence.
- The assignment ledger is not a message broker. It has no topics, offsets,
  consumer groups, delivery retries, or exactly-once execution guarantee.
- A returned result is linked evidence, not automatic proof that the downstream
  model completed the task correctly. Validate `done_when` separately.

For tool-level schemas, read [`MCP setup`](MCP.md). For SDK and lower-level
orchestrator patterns, read [`Agent workflows`](AGENT_WORKFLOWS.md). Run
`scripts/demo-agent-handoff.sh` for the zero-token protocol smoke.
