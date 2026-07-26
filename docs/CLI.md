---
title: CLI Usage
description: Common AideMemo CLI commands with examples.
---

# CLI Usage

The CLI is the fastest way to add, inspect, and maintain memory.
For the complete top-level command inventory, see
[`Feature Inventory`](FEATURES.md).
For a task-shape guide that maps CLI, MCP, and SDK entry points, see
[`Agent Workflows`](AGENT_WORKFLOWS.md).

## Search and query

Use `search` for direct retrieval. By default AideMemo probes BM25 first and
promotes to semantic retrieval only when the lexical signal is weak or the query
is CJK and the semantic path is ready:

```bash
aidememo search "Redis timeout" --limit 5
aidememo search "레디스 장애 원인" --limit 5
```

Use `--bm25-only` for deterministic demos, hooks, and CI checks that should not
load the embedding model. Use `--hybrid` when you want semantic retrieval on
every query:

```bash
aidememo search "Redis timeout" --bm25-only --limit 5
aidememo search "favorite camera setup" --hybrid --limit 5
```

Use `query` for a richer context pack:

```bash
aidememo query "Redis worker timeout" --limit 8 --depth 2 --recent-limit 5
aidememo query "Redis worker timeout" --bm25-only --limit 8
```

Use `--source-id` when multiple agents, teams, or projects share a store:

```bash
aidememo query "billing webhook duplicates" --source-id team-a
```

## Add facts

```bash
aidememo fact add \
  "Decision: Billing webhook retries must use idempotency keys." \
  --type decision \
  --entities Billing,Webhook \
  --source-id team-a \
  --actor-id codex:account-a
```

Keep `--source-id` as the trusted shared project or agent namespace. Use
`--actor-id` for the profile or agent that authored the fact.
Exact-content dedup is scoped by the normalized source ID: a repeated write in
the same source resolves to the existing fact, while identical content in a
different source remains an independent fact.

Choose fact types intentionally:

| Type | Example |
|---|---|
| `decision` | "Use idempotency keys for billing retries." |
| `lesson` | "Duplicate Stripe events came from retry races." |
| `error` | "Do not disable signature checks while debugging." |
| `preference` | "Prefer local-first tools for agent memory." |
| `note` | "The worker uses Redis for queue state." |

## Start a workflow

Use this when a task starts from a short issue, PR, or ticket.

```bash
aidememo workflow start "Stop duplicate billing webhook processing" \
  --body "Stripe webhooks sometimes process the same invoice twice." \
  --source "linear:ENG-456" \
  --source-id team-a
```

To continue a prior tracked workflow while preserving lineage, pass
`--parent-session <session-id>`. AideMemo records a `continued_from` relation
instead of copying the full chat transcript.

For deterministic demos, hooks, and CI checks, skip semantic model loading:

```bash
aidememo workflow start "Fix Redis timeout" --bm25-only
```

Export the resulting thread as a bounded, auditable canvas:

```bash
aidememo session canvas "$AIDEMEMO_SESSION_ID" --limit 20 \
  --source-id team-a --output session_canvas.md
```

The canvas is a derived Markdown artifact: a Mermaid map first, then fact-id
drill-down lines that point back to `aidememo fact get <id>`.
MCP agents can request the same text with `aidememo_session_canvas`; Python
agents can call `Memory.session_canvas(...)`.

## Hand off to another agent, profile, or account

Create a compact packet after recording the current task's durable findings:

```bash
aidememo session handoff \
  --from-actor codex-one \
  --to-actor codex-two \
  --from codex/coding \
  --to codex/reviewer \
  --source-id team-a \
  --focus "Verify the patch and run release preflight" \
  --done-when "Focused tests and release preflight pass" \
  --dispatch \
  "$AIDEMEMO_SESSION_ID"
```

The command remains read-only unless `--dispatch` is present. The preview
preserves the session id, groups the session's
decisions, open questions, lessons, and errors, and leaves every item linked to
its fact id. Agent/profile values are routing labels; `source_id` controls which
shared-store namespace is included. Use `aidememo_handoff` from MCP or
`Memory.handoff(...)` from Python for the same artifact.

With `--dispatch`, the receiver pulls and acknowledges the session pointer:

```bash
aidememo handoff inbox --actor-id codex-two --source-id team-a
aidememo handoff accept --actor-id codex-two handoff-...
aidememo handoff return --actor-id codex-two --outcome succeeded \
  --result-fact-id 01... handoff-...
aidememo handoff outbox --actor-id codex-one
aidememo handoff show handoff-...
```

`accept` returns a fresh packet and resume environment. `return` links a
persisted result/error fact. `outcome=succeeded` completes the acknowledgement;
`failed` leaves it accepted so the caller can retry or block. Legacy `complete`
still updates the ledger without result evidence and does not assert that tests
passed. Configure a stable default per MCP installation with
`mcp-install --actor-id codex-two`, or set
`AIDEMEMO_ACTOR_ID`. The alias is non-secret routing metadata, not an
authenticated vendor account id.

For repeated local accounts, use the shorter agent-oriented surface:
`agent add --type ... --home ...`, `handoff send ALIAS`, then
`handoff run ALIAS`.

```bash
aidememo agent add codex-two --type codex \
  --home /path/to/codex-two-home --workspace /path/to/repo \
  --source-id team-a --env-policy core
aidememo agent add claude-main --type claude \
  --home /path/to/claude-home --workspace /path/to/repo
aidememo agent list
aidememo handoff send codex-two --focus "Review the patch"
aidememo handoff run codex-two
aidememo handoff board --stale-after 1h --include-completed
```

`installation` and `handoff run --installation ALIAS --next` remain supported
for existing scripts. Completed results are included in outbox by default;
pass `--pending-only` to hide them.

Long-running external workers record `handoff heartbeat` every hour. When the
handoff carries `HERMES_KANBAN_TASK` / `--kanban-task`, the worker forwards the
pulse to Hermes while leaving card claim, retry, and completion in Kanban.
Pass `handoff run --timeout 14400` when work may exceed the 1800-second default;
`--heartbeat-interval` defaults to 3600 seconds.
Register `--type manual` for another coding agent that consumes the CLI/MCP/SDK
protocol itself; automatic `handoff run` remains limited to verified adapters.

Profiles never store credentials or environment values. `config_home` maps to
`CODEX_HOME` for Codex and `CLAUDE_CONFIG_DIR` for Claude. The default `core`
policy passes a small process environment plus the AideMemo resume values;
repeat `--pass-env NAME` when a worker needs another named variable.

This interface deliberately stops short of queue semantics: no topics,
offsets, consumer groups, leases, retries, copied payloads, or exactly-once
delivery. Each assignment points to the existing tracked session.

The packet includes a one-command receiver bootstrap. It validates that the
session exists and activates both continuity and retrieval scope:

```bash
eval "$(aidememo session resume --source-id team-a session-...)"
```

The longer `--from-agent`, `--from-profile`, `--to-agent`, and `--to-profile`
options remain available when an integration already emits separate fields.
When a read-only `--output` writes a packet file, stdout also prints the
validated receiver resume command, so an operator does not need to reopen the
file to activate it.

## Export a project profile

Generate a read-only profile from current typed facts:

```bash
aidememo profile export --output project_profile.md
aidememo profile export --source-id team-a --limit 80
```

This does not create or modify facts. It gives agents a compact project/persona
view while keeping AideMemo's typed facts as the evidence trail.
MCP agents can request the same text with `aidememo_profile_export`; Python
agents can call `Memory.project_profile(...)`.

## Browse entities and facts

```bash
aidememo entity list --source-id team-a
aidememo entity get Redis --source-id team-a
aidememo entity show Redis --source-id team-a
aidememo fact list --type decision --limit 20 --source-id team-a
aidememo fact get 01H... --source-id team-a
aidememo fact pinned --source-id team-a
aidememo fact pin 01H... --source-id team-a
aidememo fact unpin 01H... --source-id team-a
aidememo fact delete 01H... --source-id team-a
aidememo fact feedback 01H... --helpful --source-id team-a
aidememo fact supersede 01HOLD... 01HNEW... --source-id team-a
aidememo fact archive --ids 01H... --source-id team-a
```

Scoped entity output is fact-backed and omits global prose metadata. A scoped
fact lookup or ID-based mutation returns not-found for an ID owned by another
source. Omitting `--source-id` preserves trusted unscoped administrator behavior.

## Traverse the graph

```bash
aidememo traverse Redis --depth 2 --source-id team-a
aidememo path Worker Redis --source-id team-a
aidememo graph --from Redis --depth 2 --format mermaid --source-id team-a
```

Scoped graph reads include only relations explicitly owned by the same source;
legacy unscoped edges are not inherited.

## Scope tracked sessions

Keep session markers and their derived context in the same namespace as the
facts they collect:

```bash
eval "$(aidememo session new 'billing retry audit' --source-id team-a)"
aidememo session current --source-id team-a
aidememo session list --source-id team-a
aidememo session start --source-id team-a
```

## Pin identities on a shared HTTP server

`AIDEMEMO_SOURCE_ID` is only a trusted-process default. When independently
authenticated agents share one HTTP server, bind every bearer token to a fixed
source and writer identity instead:

```json
{"tokens":[{"token":"replace-me","source_id":"team-a","actor_id":"codex:a"}]}
```

```bash
chmod 600 ./token-bindings.json
aidememo mcp-serve --port 3000 --auth-bindings-file ./token-bindings.json
```

The binding is injected into every MCP call, including batch items, and a
caller cannot override either `source_id` or `actor_id`. Bound tokens cannot
read `/admin/status` or `/sync/since`. Put TLS termination or an encrypted
private tunnel in front of non-loopback deployments because `mcp-serve` speaks
plain HTTP.

## Maintain memory

Run `doctor` when something feels wrong:

```bash
aidememo doctor
aidememo doctor --json
```

Run `lint` for raw graph health checks:

```bash
aidememo lint
```

Consolidate old or duplicate memory:

```bash
aidememo consolidate --semantic-threshold 0.85 --dry-run
aidememo consolidate --ttl note=30 --ttl question=14
```

## Use an explicit store

For scripts, pass `--store` so the command cannot accidentally read your default
store:

```bash
aidememo --store ./team.sqlite search "release checklist"
```
