---
title: Operations
description: Practical guidance for stores, source scoping, daemon mode, and maintenance.
---

# Operations

This page covers the operational choices users usually need after the first
quickstart.

## Choose a store layout

| Layout | Use when |
|---|---|
| One local default store | Personal memory on one machine |
| One store per project | Repos should not share memory |
| One shared team store | Several local agents should share context |
| `source_id` inside one store | Teams, agents, or tenants share infrastructure but need scoped retrieval |

For scripts and CI, prefer explicit store paths:

```bash
aidememo --store ./project.sqlite query "release checklist"
```

## Use source scoping

`source_id` prevents neighbouring project or team facts from leaking into a
query.

```bash
aidememo fact add \
  "Decision: Team A deploys through release train alpha." \
  --type decision \
  --entities Release \
  --source-id team-a

aidememo query "release train" --source-id team-a
```

For MCP installs:

```bash
aidememo mcp-install --target codex --source-id team-a
```

## Avoid redb store lock issues

SQLite is the default backend. The optional redb backend is a single-writer
embedded database; if several processes open the same redb store and write at
the same time, one process may need to wait or fail.

For shared writes, run one daemon:

```bash
aidememo mcp-serve --port 3000 --store ~/.aidememo/team.redb
```

For brief local contention, configure retry:

```bash
aidememo config set store.lock_retry_ms 5000
```

## Keep memory useful

Run health checks:

```bash
aidememo doctor
aidememo lint
```

Archive or consolidate old memory:

```bash
aidememo fact archive --older-than 90d --type note
aidememo consolidate --semantic-threshold 0.85 --dry-run
```

After large consolidation, rebuild current vectors:

```bash
aidememo vector-rebuild --current-only
```

## Pick embedding mode

The default path is good for most code and docs workflows. Use `--bm25-only` for
deterministic hooks, demos, and CI checks:

```bash
aidememo workflow start "Release smoke ticket" --bm25-only
```

Enable semantic/hybrid retrieval when wording may differ between the question
and the stored fact:

```bash
aidememo search "favorite camera setup" --hybrid
```

## Back up a store

AideMemo uses a local database file. Back it up like any other project artifact.
Stop long-running writers before copying the file.

```bash
cp ~/.aidememo/wiki.sqlite ~/backups/wiki.sqlite
```
