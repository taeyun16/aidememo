---
name: aidememo-context
description: Retrieve relevant decisions, conventions, preferences, lessons, errors, and recent project memory before planning or answering a question that depends on prior work.
allowed-tools: aidememo_workflow_start, aidememo_context, aidememo_query, aidememo_search, aidememo_aggregate
metadata:
  claude:
    when_to_use: before planning or answering from prior project knowledge
---

# Retrieve AideMemo context

1. For a ticket-shaped task, call `aidememo_workflow_start` with its title and
   body. Otherwise call `aidememo_context` with the user's intent as `topic`.
2. Treat returned decisions, conventions, lessons, and errors as constraints,
   but reconcile them with the current repository state.
3. Use `aidememo_query` only for a narrower subtopic. Use `aidememo_search` for
   a pinpoint lookup.
4. Use `aidememo_aggregate` only for exact cross-fact counts, sums, distinct
   dates, or timelines—not simple recall.
5. Summarize relevant memory; do not paste a large raw result into the answer.
