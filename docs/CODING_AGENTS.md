---
title: Coding Agent Setup
description: Install AideMemo for Claude Code, Codex, Hermes Agent, pi, Cursor, OpenClaw, and OpenCode.
---

# Coding Agent Setup

AideMemo supports MCP, Agent Skills, native plugins, and direct CLI use. Pick
the smallest integration that your coding agent supports.

## Choose an integration

| Agent | Recommended path | Alternative | Profile-aware setting |
|---|---|---|---|
| Claude Code | Claude plugin: MCP + focused skills + read-only hooks | standalone MCP + skill | `CLAUDE_CONFIG_DIR` |
| Codex | stdio MCP with a pinned store | project `AGENTS.md` usage guidance | `CODEX_HOME` / `--codex-home` |
| Hermes Agent | skill + MCP | native Python plugin with hooks and slash commands | `HERMES_HOME` |
| pi coding agent | native Agent Skill + local CLI | none; pi does not accept MCP | `PI_CODING_AGENT_DIR` |
| Cursor | stdio MCP | manual `mcp.json` | Cursor config directory |
| OpenClaw | skill + stdio MCP | shared `~/.agents/skills` skill | OpenClaw config directory |
| OpenCode | appended `AGENTS.md` instructions + stdio MCP | manual JSON config | OpenCode config directory |

## Prepare AideMemo

Install the CLI and create or select a store before configuring an agent:

```bash
cargo install --git https://github.com/taeyun16/aidememo aidememo-cli
mkdir -p ./_meta
aidememo --store "$(pwd)/_meta/wiki.sqlite" stats
```

Use an absolute store path for agent registration. Add a `source_id` when one
trusted store contains more than one project or agent namespace, and an
`actor_id` when writes must retain which agent profile created them. An MCP
install sets environment defaults that a caller can override; use HTTP token
bindings when the source assignment must be enforced.

## Claude Code

The repository ships a self-contained Claude Code plugin with AideMemo MCP,
three focused skills, and three read-only context hooks.

```bash
claude plugin marketplace add /absolute/path/to/aidememo
claude plugin install aidememo@aidememo
claude plugin list
```

The plugin uses the default store or environment inherited by Claude Code. Set
`AIDEMEMO_STORE`, `AIDEMEMO_SOURCE_ID`, and `AIDEMEMO_ACTOR_ID` before starting
Claude when an explicit store and provenance are required.

Choose the standalone path instead of the plugin when registration should be
persisted by Claude Code itself:

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target claude \
  --source-id project:my-app \
  --actor-id claude:local
aidememo skill install --target claude
claude mcp list
```

The skill installer writes to `$CLAUDE_CONFIG_DIR/skills/aidememo` when the
variable is set, otherwise `~/.claude/skills/aidememo`. New installations use
skills; `.claude/commands` is retained only for legacy compatibility.

Plugin development checks:

```bash
claude plugin validate ./plugins/claude
claude --plugin-dir ./plugins/claude
```

## Codex

Register a pinned stdio MCP server in the active Codex profile:

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target codex \
  --source-id project:my-app \
  --actor-id codex:local
```

For several isolated profiles sharing one store, repeat `--codex-home` and
`--actor-id` in the same order:

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target codex \
  --codex-home "$HOME/.codex-account-a" --actor-id codex:account-a \
  --codex-home "$HOME/.codex-account-b" --actor-id codex:account-b \
  --source-id project:my-app
```

See [Share Memory Across Codex Profiles](CODEX_MULTI_PROFILE.md) for the full
concurrency and workflow-lineage pattern.

## Hermes Agent

The lightweight path installs a native skill and registers all AideMemo MCP
tools. Both installers honor `HERMES_HOME`.

```bash
aidememo skill install --target hermes
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target hermes \
  --source-id project:my-app \
  --actor-id hermes:local
hermes mcp test aidememo
hermes skills list
```

The native plugin adds session context, slash commands, SDK composition, and
opt-in pending-first capture. Install it into Hermes's own Python environment:

```bash
HERMES_PY="${HERMES_PY:-$HOME/.hermes/hermes-agent/venv/bin/python3}"
"$HERMES_PY" -m pip install hermes-aidememo
hermes plugins enable aidememo
```

Use `plugins.aidememo.store_path`, `source_id`, and `actor_id` in
`$HERMES_HOME/config.yaml` (or `~/.hermes/config.yaml`) to select the store and
write provenance.

## pi coding agent

pi uses Agent Skills and its local `bash` tool. It intentionally has no MCP
registration step.

```bash
aidememo skill install --target pi
```

The default destination is `~/.pi/agent/skills/aidememo`. Isolated profiles can
select another native skill directory:

```bash
export PI_CODING_AGENT_DIR="$HOME/.pi/work-profile"
aidememo skill install --target pi
```

Start a new pi session and invoke `/skill:aidememo`, or ask pi to retrieve or
record project memory naturally. If an older installer suggests
`mcp-install --target pi`, update AideMemo; pi rejects MCP upstream.

## Cursor, OpenClaw, and OpenCode

```bash
# Cursor: writes mcpServers.aidememo in ~/.cursor/mcp.json
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target cursor \
  --source-id project:my-app --actor-id cursor:local

# OpenClaw: native skill plus MCP registration
aidememo skill install --target openclaw
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target openclaw \
  --source-id project:my-app --actor-id openclaw:local

# OpenCode: appends managed instructions and writes mcp.aidememo
aidememo skill install --target opencode
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target opencode \
  --source-id project:my-app --actor-id opencode:local
```

Run either installer with `--list-targets` to inspect every supported target
and destination. Use `--print` with `mcp-install` to preview changes and
`--force` only when replacing an existing AideMemo entry intentionally.

## Verify and use

```bash
aidememo doctor
aidememo mcp-install --list-targets
aidememo skill install --list-targets
```

For an MCP agent, confirm that `aidememo` is connected, then begin a normal
turn with `aidememo_context` or a ticket with `aidememo_workflow_start`. Use
`aidememo_query` for a narrower follow-up and write only durable decisions,
conventions, preferences, lessons, and recurring errors.

`aidememo doctor` above is an administrator-side CLI check. Source-scoped MCP
identities cannot call `aidememo_doctor` or `aidememo_overview` because both
return global store metadata. Run those diagnostics outside the scoped agent;
use `aidememo_context`, `aidememo_query`, and scoped entity/fact tools inside it.

| Symptom | Fix |
|---|---|
| `aidememo: command not found` | Add Cargo's bin directory to the agent process `PATH`, then restart the agent. |
| Agent opens the wrong store | Reinstall with global `--store` and an absolute path. |
| Skill is absent in an isolated profile | Export the agent-specific profile variable before installing. |
| MCP is registered but disconnected | Run the agent's MCP list/test command and `aidememo doctor`. |
| Shared-store results leak across projects | Install with a stable `--source-id`. |

For tool selection after installation, continue with
[Agent Workflows](AGENT_WORKFLOWS.md) and [MCP Setup](MCP.md).
