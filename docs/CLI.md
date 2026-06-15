---
title: CLI Usage
description: Common AideMemo CLI commands with examples.
---

# CLI Usage

The CLI is the fastest way to add, inspect, and maintain memory.
For the complete top-level command inventory, see
[`Feature Inventory`](FEATURES.md).

## Search and query

Use `search` for direct retrieval:

```bash
aidememo search "Redis timeout" --limit 5
```

Use `query` for a richer context pack:

```bash
aidememo query "Redis worker timeout" --limit 8 --depth 2 --recent-limit 5
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
  --source-id team-a
```

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

For deterministic demos, hooks, and CI checks, skip semantic model loading:

```bash
aidememo workflow start "Fix Redis timeout" --bm25-only
```

Export the resulting thread as a bounded, auditable canvas:

```bash
aidememo session canvas "$AIDEMEMO_SESSION_ID" --limit 20 --output session_canvas.md
```

The canvas is a derived Markdown artifact: a Mermaid map first, then fact-id
drill-down lines that point back to `aidememo fact get <id>`.
MCP agents can request the same text with `aidememo_session_canvas`; Python
agents can call `Memory.session_canvas(...)`.

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
aidememo entity list
aidememo entity show Redis
aidememo fact list --type decision --limit 20
aidememo fact get 01H...
```

## Traverse the graph

```bash
aidememo traverse Redis --depth 2
aidememo path Worker Redis
aidememo graph --from Redis --depth 2 --format mermaid
```

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
