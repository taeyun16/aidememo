---
description: Pull wiki context for a topic — one shot search + traverse + recent.
argument-hint: <topic or entity>
allowed-tools: Bash(./target/debug/wg query:*), Bash(wg query:*)
---

Build a context dossier for "$ARGUMENTS" before answering or coding.

Run `wg --json query "$ARGUMENTS" --limit 8 --depth 2 --recent-limit 8`.

The single response contains:
- `entity`: resolved entity (null if topic isn't a known entity name/alias)
- `search`: top hybrid-search hits across all facts
- `related`: entities reachable from the resolved entity (forward + reverse)
- `recent_facts`: facts attached to the resolved entity, newest first

Synthesize into 5–10 bullet points. Cite fact IDs (`[fact:01KP…]`). If
`entity` is null and `search` is thin, say so plainly and suggest
`/wg-add-fact` to record what we discover during the work.
