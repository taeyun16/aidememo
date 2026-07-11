# AideMemo plugin for Claude Code

[한국어](README.ko.md)

This self-contained plugin gives Claude Code AideMemo MCP tools, three focused
skills, and safe context hooks. Install the `aidememo` CLI first, then follow
the [complete setup guide](../../aidememo-skill/setup-claude-code.md).

```bash
claude plugin validate ./plugins/claude
claude --plugin-dir ./plugins/claude
```

The hooks are read-only and soft-fail. Fact extraction only presents candidates;
Claude must explicitly call an AideMemo write tool to save one.
