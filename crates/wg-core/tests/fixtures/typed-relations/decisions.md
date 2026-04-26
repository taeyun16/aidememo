---
type: decision
date: 2026-04-26
---

# DB choice

[[Plan B]] supersedes [[Plan A]]. The new design uses [[Postgres]]
instead of [[MySQL]] — [[Postgres]] is a strict alternative to [[MySQL]]
for our use case.

[[Migration Phase 1]] blocks [[Migration Phase 2]], which depends on
[[Schema v3]]. [[Service Auth]] is part of [[API Gateway]].
