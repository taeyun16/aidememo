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

Keep the file suffix aligned with the backend (`.sqlite` for SQLite /
`libsqlite`, `.redb` for redb). The suffix is not required by the storage
engine, but `aidememo doctor` warns when `store.backend` and the path extension
point to different persistence layers because that is easy to misread later.

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
aidememo --backend libsqlite mcp-install --target codex --source-id team-a
```

## Avoid local store write contention

SQLite is the default backend and uses `store.lock_retry_ms` as its busy
timeout for short write collisions. The optional redb backend uses the same
setting to retry opening the store when another process holds redb's exclusive
file lock.

For shared writes, run one daemon:

```bash
aidememo --backend libsqlite daemon start --store ~/.aidememo/team.sqlite --port 3000
```

Daemon auto-discovery is backend-aware. A daemon started for the same path with
`redb` will not be reused by a CLI invocation configured for `sqlite` /
`libsqlite`, and vice versa.

For brief local contention, configure the wait budget:

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

AideMemo's default store is SQLite. Use the backup command instead of copying
the hot `.sqlite` file directly: it creates a consistent SQLite snapshot,
writes a manifest with byte counts and SHA-256 checksums, and restore verifies
the manifest plus `PRAGMA integrity_check` before replacing the target store.

```bash
aidememo --store ~/.aidememo/wiki.sqlite backup create ~/backups/aidememo
aidememo --store ~/.aidememo/wiki.sqlite backup restore ~/backups/aidememo/backup-01... --force
```

S3 backup / restore is an optional build feature. Build the CLI with
`--features s3`, then use an S3 prefix as the destination or source:

```bash
aidememo --store ~/.aidememo/wiki.sqlite backup create s3://my-bucket/aidememo
aidememo --store ~/.aidememo/wiki.sqlite backup restore s3://my-bucket/aidememo/backup-01... --force
```

The S3 path is for backup storage, not for using S3 as the live database.
Restores replace the local SQLite store and remove stale SQLite WAL/SHM and
HNSW sidecar files next to the target.
