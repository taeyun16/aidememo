---
description: Search the local wiki (BM25 + semantic). Returns ranked facts.
argument-hint: <query>
allowed-tools: Bash(./target/debug/wg search:*), Bash(wg search:*)
---

Run `wg --json search "$ARGUMENTS" --limit 10` and summarize the top results.
Group facts by entity. Show fact ID, content, confidence, and "when". If nothing
matches, say so plainly — don't fabricate.
