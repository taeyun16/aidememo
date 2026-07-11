---
kind: doc
title: AideMemo hooks for Claude Code
---

# Automate AideMemo with Claude Code hooks

[한국어](README.ko.md)

These optional hooks surface project memory without blocking Claude Code or
automatically saving facts.

| Event | Matcher | Behavior |
|---|---|---|
| `SessionStart` | `startup\|resume\|compact` | Inject pinned facts, overview, and recent activity. |
| `PostToolUse` | `Edit\|Write\|NotebookEdit` | Query facts related to the edited path using the BM25-only fast path. |
| `UserPromptSubmit` | none | Preview fact candidates for prompts of at least 200 characters. |

The recommended plugin already includes these hooks. For a manual install:

```bash
mkdir -p ~/.claude/hooks
cp aidememo-skill/hooks/aidememo-*.py ~/.claude/hooks/
chmod +x ~/.claude/hooks/aidememo-*.py
cp .claude/settings.example.json .claude/settings.json
```

Merge the example into an existing settings file instead of overwriting it.
Set `AIDEMEMO_STORE` or `AIDEMEMO_PROJECT` when the default store is not the
one this project should use. Set `AIDEMEMO_EXTRACT_LLM=1` only when intentional;
otherwise extraction is local and heuristic.

## Verify the output contract

```bash
echo '{}' | python3 ~/.claude/hooks/aidememo-session-start.py | jq .

echo '{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs"}}' \
  | python3 ~/.claude/hooks/aidememo-post-tool.py | jq .
```

When context is available, the output uses Claude Code's event-specific
envelope:

```json
{
  "continue": true,
  "hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "additionalContext": "..."
  }
}
```

All hooks soft-fail with `{"continue": true}` when the binary, store, or query
is unavailable. They never add, edit, supersede, or delete a fact. Remove an
event from settings (or disable the plugin) to turn it off.
