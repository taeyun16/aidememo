---
type: technology
tags: [cache, infra]
---

# Redis

[[Redis]] depends on [[Linux]] for production.
[[Redis]] uses [[Memory]] aggressively.

We considered [[Memcached]] as an alternative to [[Redis]] but ruled it out.

The [[Redis Cluster]] design extends [[Redis]] with horizontal scaling.
[[Redis Sentinel]] is owned by [[Redis]] (same project family).
