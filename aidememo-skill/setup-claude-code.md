---
kind: doc
title: AideMemo setup for Claude Code
---

# Use AideMemo with Claude Code

[한국어](setup-claude-code.ko.md)

The recommended setup is the Claude Code plugin: it bundles the MCP server
definition, focused skills, and safe context hooks. Use the standalone MCP and
skill installers when you want a smaller or profile-specific setup.

## Prerequisites

```bash
cargo install aidememo-cli
aidememo init ./wiki
claude --version
```

`aidememo` and `claude` must both be on `PATH`.

## Option A: install the plugin (recommended)

From a clone of this repository:

```bash
claude plugin marketplace add /absolute/path/to/aidememo
claude plugin install aidememo@aidememo
claude plugin list
```

Restart Claude Code after installation. The plugin provides:

- AideMemo's stdio MCP server and its current tool surface
- focused `aidememo`, `aidememo-context`, and `aidememo-remember` skills
- `SessionStart`, `PostToolUse`, and `UserPromptSubmit` hooks

The plugin uses the selected default AideMemo store. Set `AIDEMEMO_STORE`,
`AIDEMEMO_SOURCE_ID`, and `AIDEMEMO_ACTOR_ID` before starting Claude Code when
the plugin needs an explicit store or provenance. Choose Option B instead when
you want Claude Code to persist that registration without launch-time env.

For plugin development without installation:

```bash
claude plugin validate ./plugins/claude
claude --plugin-dir ./plugins/claude
```

## Option B: install MCP and a standalone skill (without the plugin)

Pin the resolved store so Claude Code behaves the same from every working
directory:

```bash
aidememo --store "$(pwd)/wiki.sqlite" mcp-install \
  --target claude \
  --source-id "project:my-project" \
  --actor-id "claude:local"

aidememo skill install --target claude
claude mcp list
```

The skill installer honors `CLAUDE_CONFIG_DIR`; otherwise it installs to
`~/.claude/skills/aidememo`. Use `--force` to replace an older copy.

Claude Code MCP scopes can also be managed directly with `claude mcp add`.
Prefer `local` for a private checkout, `project` for a shared `.mcp.json`, and
`user` when the same store should be available across projects.

## Verify

Inside Claude Code, run `/mcp` and confirm `aidememo` is connected. Then ask:

```text
Use AideMemo to show the current project context.
```

For a configuration health check:

```bash
aidememo doctor
claude mcp list
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| `aidememo: command not found` | Install the CLI and restart the shell/Claude Code so its `PATH` refreshes. |
| MCP is disconnected | Run `aidememo mcp-install --target claude --force`, then `claude mcp list`. |
| Wrong store is used | Reinstall with an absolute `--store` path. |
| Skill is missing in an isolated profile | Set `CLAUDE_CONFIG_DIR` before `aidememo skill install --target claude`. |
| Hooks add no context | Run the hook manually as described in [the hook guide](hooks/README.md), and check `AIDEMEMO_STORE`. |

The legacy files under `.claude/commands/` remain compatible, but new setups
should use skills because they are the current Claude Code extension surface.
