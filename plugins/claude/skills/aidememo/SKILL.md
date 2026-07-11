---
name: aidememo
description: Use AideMemo when a task needs durable project knowledge from earlier sessions or when a decision, convention, preference, lesson, or recurring error should be remembered. Prefer the MCP tools over shell commands.
allowed-tools: aidememo_workflow_start, aidememo_context, aidememo_query, aidememo_search, aidememo_fact_add, aidememo_fact_add_many, aidememo_doctor
---

# AideMemo project memory

AideMemo is local, persistent memory. Keep normal working state in the current
conversation; store only knowledge that will matter in a later session.

- For an issue, PR, or ticket, begin with `aidememo_workflow_start`.
- For prior context, use `aidememo_context` once, then `aidememo_query` for a
  narrower follow-up. Avoid repeated broad searches.
- Record a durable item with the focused `aidememo-remember` skill.
- Use `aidememo_doctor` when the store or integration appears unhealthy.
- Never invent entity names or save secrets, transient progress, or raw chat.

For CLI fallback and the complete tool surface, see `REFERENCE.md` in the
standalone AideMemo skill package.
