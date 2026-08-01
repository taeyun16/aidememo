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
> not include these commands. Install both current-main entry points before
> following the automatic path:
>
> ```bash
> cargo install --path crates/aidememo-cli
> python -m pip install -e ./packages/aidememo-agent-sdk
> aidememo-worker-lane --help
> ```
>
> `aidememo handoff run` delegates to the SDK-installed
> `aidememo-worker-lane`; the final command is the setup preflight.

## The short path

### 1. Start tracked work

```bash
eval "$(aidememo session new --source-id release-team 'Review the Redis timeout patch')"
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

## Two accounts through one remote project

Use named remote credential profiles when `codex-p1`, `codex-p2`, or a Hermes
gateway run as separate authenticated actors against one server. These are
credential profiles, unlike the credential-free local `agent` profiles used by
`handoff run`.

```bash
aidememo auth login https://memory.example.com \
  --profile codex-p1 --project-id aidememo \
  --token-file ~/.config/aidememo/codex-p1.token
aidememo auth login https://memory.example.com \
  --profile codex-p2 --project-id aidememo \
  --token-file ~/.config/aidememo/codex-p2.token

# The sender routes the existing local tracked session into the remote SSOT.
aidememo handoff --remote-profile codex-p1 send codex-p2 \
  --source-id project:aidememo \
  --focus "Review the remote boundary" \
  "$AIDEMEMO_SESSION_ID"

# The receiver identity comes from the codex-p2 bearer token.
aidememo handoff --remote-profile codex-p2 inbox \
  --source-id project:aidememo
aidememo handoff --remote-profile codex-p2 accept handoff_...

# Resume the session shown in the inbox, then write receiver-owned evidence.
eval "$(aidememo session resume --source-id project:aidememo session-...)"
aidememo fact add "Remote review passed" --type note --entities Release \
  --source-id project:aidememo --actor-id codex-p2
aidememo handoff --remote-profile codex-p2 return \
  --outcome succeeded --result-fact-id 01... handoff_...

aidememo handoff --remote-profile codex-p1 outbox
```

Several named profiles may use the same URL, but each keeps its own bearer
token and fixed project. `AIDEMEMO_REMOTE_PROFILE=codex-p2` may replace the
repeated flag. Remote operations reject `--actor-id` and `--from`: the server's
persisted token binding is the only actor authority.

This is currently a connected-write bridge. The CLI uploads the typed session
pointer and the receiver's result fact to the canonical server ledger, while
the embedded local store still provides packet construction, session resume,
and retrieval. Remote `send`, `inbox`, `outbox`, `show/status`, `accept`, and
`return` are implemented; remote `run`, heartbeat, board, offline outbox, local
read replica, and MCP profile routing remain future work. Typed server facts are
canonical handoff evidence but are not yet indexed by the embedded search
engine.

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
aidememo agent add cursor-review --type manual --workspace "$PWD" \
  --source-id release-team
aidememo handoff send cursor-review --focus "Review the patch"

AIDEMEMO_ACTOR_ID=cursor-review aidememo handoff inbox
AIDEMEMO_ACTOR_ID=cursor-review aidememo handoff accept handoff-...

# Resume the session id printed by accept, then record receiver-owned evidence:
eval "$(aidememo session resume --source-id release-team session-...)"
export AIDEMEMO_ACTOR_ID=cursor-review
aidememo fact add "Review passed" --type note --entities Release \
  --source-id release-team --actor-id cursor-review
aidememo handoff return \
  --outcome succeeded \
  --result-fact-id 01... \
  handoff-...
```

`handoff run cursor-review` intentionally refuses because a manual profile has
no process adapter. `return` also fails closed unless the result fact is
attached to the handed-off session, uses its exact `source_id`, and was written
by the receiving actor.

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
- Concurrent accept, heartbeat, and return writers use compare-and-swap
  revisions. Automatic workers also use a unique claim token, so an active
  assignment rejects competing claims. A fact-linked failed assignment may be
  reclaimed by a new token and increments `attempt_count`. A stale writer fails
  with `transaction_conflict`; this prevents lost updates and duplicate active
  claims but does not provide a renewable worker lease or crash retry.
- Result return is fail-closed: the fact must belong to the handed-off session
  and exact source scope and carry the receiving actor as writer provenance.
- A returned result is linked evidence, not automatic proof that the downstream
  model completed the task correctly. Validate `done_when` separately.

## Hermes project and tenant scopes

The Hermes plugin can derive a default AideMemo scope from dispatcher metadata
without copying Kanban lifecycle state into the handoff ledger:

```yaml
plugins:
  aidememo:
    store_path: ~/.aidememo/hermes-shared.sqlite
    source_from_hermes: board_tenant
    actor_from_hermes_profile: true
    lock_retry_ms: 5000
```

`board` maps `HERMES_KANBAN_BOARD=aidememo` to
`source_id=hermes:board:aidememo`. The recommended `board_tenant` mode appends
the task tenant when `HERMES_TENANT` is present, while non-tenant cards share
the board scope. An explicit plugin `source_id` or `AIDEMEMO_SOURCE_ID` always
wins. Likewise, an explicit actor wins before the optional
`hermes:<HERMES_PROFILE>` provenance mapping.

This mapping is a trusted-process convenience. It does not authenticate a
gateway user or channel. A single gateway process with one fixed plugin source
shares that memory across its sessions. For mutually untrusted gateway clients,
run separate Hermes profiles/gateways and stores, or use AideMemo HTTP MCP with
`--auth-bindings-file` so bearer tokens are bound to fixed `source_id` and
`actor_id` values.

For tool-level schemas, read [`MCP setup`](MCP.md). For SDK and lower-level
orchestrator patterns, read [`Agent workflows`](AGENT_WORKFLOWS.md). Run
`scripts/demo-agent-handoff.sh` for the zero-token protocol smoke.
