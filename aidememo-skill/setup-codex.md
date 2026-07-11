---
kind: doc
title: AideMemo setup for Codex
---

# Use AideMemo with Codex

[한국어](setup-codex.ko.md)

Codex reads stdio MCP servers from the active `CODEX_HOME/config.toml`. When
`CODEX_HOME` is unset, the default profile is `~/.codex`.

## Install the CLI

```bash
cargo install --git https://github.com/taeyun16/aidememo aidememo-cli
aidememo --help
```

## Register MCP

Use the installer so the storage backend and absolute store path do not change
with Codex's working directory:

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target codex \
  --source-id project:my-app \
  --actor-id codex:local
```

For several isolated Codex profiles sharing one store, repeat `--codex-home`
and `--actor-id` in matching order:

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target codex \
  --codex-home "$HOME/.codex-account-a" --actor-id codex:account-a \
  --codex-home "$HOME/.codex-account-b" --actor-id codex:account-b \
  --source-id project:my-app
```

The generated configuration is equivalent to:

```toml
[mcp_servers.aidememo]
command = "aidememo"
args = ["--backend", "libsqlite", "--store", "/absolute/project/_meta/wiki.sqlite", "mcp"]

[mcp_servers.aidememo.env]
AIDEMEMO_SOURCE_ID = "project:my-app"
AIDEMEMO_ACTOR_ID = "codex:local"
```

## Give Codex project guidance

Codex automatically reads project `AGENTS.md` files. Keep the guidance compact
and task-oriented:

```markdown
## Project memory

Use `aidememo_context` once when a task depends on prior project knowledge.
Start issue or PR work with `aidememo_workflow_start`. Record only durable
decisions, conventions, preferences, lessons, and recurring errors.
```

## Verify

```bash
aidememo doctor
codex mcp list
```

Start a new Codex task and confirm the AideMemo tools are available. For the
multi-account handoff and concurrency pattern, see
`docs/CODEX_MULTI_PROFILE.md` in the repository.

| Symptom | Fix |
|---|---|
| AideMemo is absent in one profile | Install into its active `CODEX_HOME`, or pass `--codex-home`. |
| Codex opens another store | Reinstall with global `--store` and an absolute path. |
| `aidememo` is not found | Add Cargo's bin directory to the Codex process `PATH`, then restart Codex. |
| Shared profiles lose writer provenance | Give each `--codex-home` a matching unique `--actor-id`. |
