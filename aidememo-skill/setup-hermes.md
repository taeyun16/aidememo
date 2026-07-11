---
kind: doc
title: Hermes Agent setup guide
---

# Using AideMemo with Hermes Agent

Hermes supports two integration paths: lightweight **skill + MCP**, or the
**native plugin** with automatic context injection and slash commands.

Korean: [`setup-hermes.ko.md`](setup-hermes.ko.md)

## Shared prerequisites

```bash
cd ~/dev/aidememo
cargo build -p aidememo-cli --release
export PATH="$PWD/target/release:$PATH"
```

## Path A: skill + MCP

```bash
aidememo skill install --target hermes
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target hermes \
  --source-id project:my-project \
  --actor-id hermes:local
```

`mcp-install` enables every AideMemo tool discovered by Hermes and verifies
registration with `hermes mcp list`. If you use an isolated profile through
`HERMES_HOME=/path/to/profile`, both the skill and MCP configuration are
installed into that profile.

Verify the installation:

```bash
hermes mcp list
hermes mcp test aidememo
hermes skills list
```

## Path B: native plugin

The plugin adds session-start context injection, eight slash commands, and
optional pending-first capture on top of the AideMemo tools. Install it into
Hermes's own Python environment, not an unrelated system Python.

```bash
HERMES_PY="${HERMES_PY:-$HOME/.hermes/hermes-agent/venv/bin/python3}"
"$HERMES_PY" -m pip install -e packages/aidememo-agent-sdk -e plugins/hermes
hermes plugins enable aidememo
```

Configure `~/.hermes/config.yaml`, or `$HERMES_HOME/config.yaml` for an
isolated profile:

```yaml
plugins:
  enabled:
    - aidememo
  aidememo:
    store_path: ~/.aidememo/wiki.sqlite
    source_id: project:my-project
    actor_id: hermes:local
    auto_capture:
      enabled: false
      mode: pending
```

Use the repository's isolated development profile when testing from a
checkout:

```bash
./scripts/setup-hermes-test-env.sh setup
eval "$(./scripts/setup-hermes-test-env.sh env)"
./scripts/setup-hermes-test-env.sh seed
./scripts/test-hermes-e2e.sh
```

## First use

```text
/aidememo-context Redis
/aidememo-start "Fix Redis timeout" --source github:org/repo#123
/aidememo-add "Redis timeout is 30 seconds" --type decision --entities Redis
```

Without the native plugin, Hermes can still use the installed skill to select
MCP tools such as `aidememo_query` and `aidememo_context`.
