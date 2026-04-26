---
description: Add a fact to the local wiki, linked to the relevant entities.
argument-hint: <fact content>
allowed-tools: Bash(./target/debug/wg fact add:*), Bash(wg fact add:*), Bash(./target/debug/wg entity list:*)
---

Add this fact to the wiki: "$ARGUMENTS"

1. Run `wg --json entity list --limit 50` to see what entities already exist.
2. Pick the 1–3 entities the fact references (use existing names; don't invent).
3. Classify the fact_type: decision | pattern | convention | claim | note | question.
4. Run `wg fact add "$ARGUMENTS" --type <type> --entities <name1>,<name2>` and
   report the new fact ID.
5. If no entity matches, ask the user whether to create one before adding the
   fact.
