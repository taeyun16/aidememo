# wg Product Roadmap

This roadmap tracks product gaps against agent-memory peers (GBrain,
Graphiti/Zep, Mem0, Hindsight, Letta) and ties each item to a measurable
acceptance metric. Keep durable measurement write-ups in `docs/MEASUREMENTS.md`
or benchmark-specific `RESULTS.md` files; keep user-facing product work here.

## Measurement Rules

- Every shipped item needs a command that can be re-run locally.
- Prefer counts, latency, recall, pass rates, or bytes over prose claims.
- Record before/after numbers in the PR or changelog entry.
- Do not count a feature as complete if only the internal API exists; the CLI,
  MCP, docs, and tests must match the intended user path.

## Milestones

| ID | Status | Product Gap | Target Metric | Measurement Command |
|---|---|---|---|---|
| P0.1 | done | Capture inbox is TUI-only, hard to automate | `wg pending list/approve/reject` work non-interactively; JSON includes `count`, `selected`, `committed`, `discarded`, `failed`, `remaining` | `cargo test -p wg-cli pending::` plus a CLI smoke with a temp pending log |
| P0.2 | done | Cross-system validation is not packaged | A `gbrain-evals` adapter exists, matches the current Adapter interface, and has fresh-checkout scorecards for direct and daemon modes | direct bm25: P@5 17.4%, R@5 64.1%, 125/261, real 63.38s; daemon bm25: same score, real 11.04s (5.7x faster); daemon hybrid: R@5 62.5%, real 45.64s |
| P0.3 | done | Capture quality is not measured | Pending approval rate and extraction precision can be computed from one JSONL log | `wg pending stats --from LOG --json` returns total/count-by-type/confidence histogram |
| P1.1 | done | First-run setup requires several commands | One command prints or applies init + MCP install + skill install for a target agent | `wg init --agent codex --no-ingest PATH --json` reports steps and elapsed ms |
| P1.2 | done | Shared daemon is operational but opaque | HTTP MCP exposes health/sync/admin status without exposing secrets | `curl /health` and `curl /admin/status` return request count, store path, auth mode, sync cursors |
| P1.3 | done | Feedback loop exists but is manual | `wg feedback` count and `wg adapt train` status are visible in doctor/overview | `wg doctor --json` includes `adaptation.feedback_count`, `has_adapter`, `generation`, `ready`; smoke: before train 1/false/0/false + `wg adapt train` fix, after train 1/true/1/true |
| P2.1 | done | Per-user/source scoping is project-level only | Facts carry optional `source_id`; `wg fact add/list`, `wg search`, `wg query`, MCP equivalents, and Hermes plugin tools filter by source | Unit: `cargo test -p wg-core source_id --features semantic` 2 passed; `cargo test -p wg-cli source_id` 1 passed. Hermes agent mixed-source eval: unscoped beta inclusion 2/2 → scoped beta leakage 0/2 while alpha recall stayed 2/2 |
| P2.2 | done (non-goal) | Distributed multi-writer merge | No hidden multi-master writes; docs steer users to canonical daemon + pull cache | `AGENTS.md` documents single shared `wg mcp-serve`, no multi-stdio writers, and pull-only delta sync |
| P2.3 | done | Two local Hermes agents sharing one store need a daemon or hit redb lock errors | Hermes plugin retries short CLI fallback lock collisions by default (`lock_retry_ms=5000`), so ordinary same-host sharing works without a user-visible server step | Serverless Hermes `WgClient` smoke, 2 processes x 10 writes: retry `0` persisted 10/20 with 10 lock errors; retry `5000` persisted 20/20 with 0 errors, wall 2.16s, p50 98.1ms, max 1.22s |

## Current Sprint

All planned P0-P2 roadmap items are closed.

Next measurement candidates:
1. Reduce P0.2 adapter overhead further with `wg-napi` to remove per-query CLI spawn.
2. Hide a future daemon/socket broker behind auto-discovery only if higher-concurrency writes make serverless retry feel slow.
3. Keep baseline verification green: `cargo check -p wg-core -p wg-cli`, `cargo test -p wg-cli --bin wg`, and the gbrain adapter smoke.

## Positioning Guardrails

- Preserve `wg`'s default zero-LLM, local-first path.
- Make LLM extraction opt-in and measurable, not implicit.
- Optimize for coding-agent memory next to a repo, not hosted consumer memory.
- Prefer explicit approval queues over silent memory rewrites.
