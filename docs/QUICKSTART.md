---
title: Quickstart
description: Add memory, search it, and start a workflow in a few commands.
---

# Quickstart

This guide creates a small local store, writes memory, searches it, and starts a
workflow from a sparse ticket.

Install AideMemo first using the [Installation guide](INSTALLATION.md), then
confirm that the CLI is available:

```bash
aidememo --help
```

## 1. Create a demo store

```bash
export AIDEMEMO_DEMO_STORE="$(mktemp -d)/wiki.sqlite"
```

All commands below use this store:

```bash
am() {
  aidememo --store "$AIDEMEMO_DEMO_STORE" "$@"
}
```

## 2. Add facts

Add a decision:

```bash
am fact add \
  "Decision: Redis timeout fixes must go through the Worker job wrapper." \
  --type decision \
  --entities Redis,Worker
```

Add a lesson:

```bash
am fact add \
  "Lesson: The last Worker Redis timeout was DNS resolution, not pool size." \
  --type lesson \
  --entities Redis,Worker
```

Add an error to avoid:

```bash
am fact add \
  "Error: Avoid increasing Redis pool size before checking DNS metrics." \
  --type error \
  --entities Redis,Worker
```

## 3. Search memory

```bash
am search "Redis timeout"
```

Use `query` when you want search plus nearby graph context:

```bash
am query "Fix Redis timeout in worker" --bm25-only --limit 5 --depth 2
```

## 4. Start a workflow from a ticket

`workflow start` is the recommended entry point for issue, PR, or ticket
automation. It creates a tracked session, stores the ticket, and returns prior
decisions, lessons, errors, and search hits.

```bash
am workflow start "Fix Redis timeout in worker" \
  --body "Worker jobs intermittently time out against Redis." \
  --source "github:org/app#123" \
  --bm25-only
```

The output includes:

- `session_id`: attach future facts to this task.
- `ticket_fact_id`: the stored incoming ticket fact.
- `relevant_decisions`: decisions that should guide the work.
- `prior_lessons`: lessons from similar work.
- `prior_errors`: known failure modes to avoid.

## 5. Continue the session

The CLI prints an export command. Use it so future `fact add` calls attach to
the active workflow session:

```bash
export AIDEMEMO_SESSION_ID=session-...

am fact add \
  "Lesson: This timeout was caused by a missing DNS retry around the worker wrapper." \
  --type lesson \
  --entities Redis,Worker
```

## 6. Inspect recent memory

```bash
am recent --last 1d
am stats
```

## 7. Hand the session to another coding-agent account

Register a recurring Codex or Claude account once. The profile stores paths
and routing metadata, never credentials:

```bash
am agent add codex-two --type codex \
  --home /path/to/codex-two-home \
  --workspace "$PWD"
```

Send the active session. The destination profile supplies its runtime and
default source scope:

```bash
export AIDEMEMO_ACTOR_ID=codex-one

am handoff send codex-two \
  --focus "Review the Redis timeout patch" \
  --done-when "Focused tests pass and findings are recorded"
```

Run the oldest pending assignment for that account, then inspect the returned
result using the id printed by `send`:

```bash
am handoff run codex-two
am handoff show handoff-...
am handoff board --stale-after 1h --include-completed
```

Use `handoff inbox`, `accept`, and `return` only when manually controlling the
receiver lifecycle. Completed results remain visible in `handoff outbox` by
default; pass `--pending-only` when only active work is wanted.

The worker lane emits an AideMemo heartbeat every hour during long external
runs. A linked Hermes card receives the same heartbeat, while Hermes remains
the owner of claims, retries, dependencies, and completion. For coding agents
without a built-in runner, register `--type manual` and use the CLI/MCP/SDK
accept, heartbeat, and return calls; `handoff board` provides a derived work
view without adding another Kanban state machine.

At this point you have a working local memory store that can be used from the
CLI, MCP, or SDK.
