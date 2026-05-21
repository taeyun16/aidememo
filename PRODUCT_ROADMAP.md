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
| P0.2a | done | `gbrain-evals` adapter still pays per-query CLI spawn even after daemon work | Adapter supports `WG_ADAPTER_BACKEND=auto|cli|napi`; `napi` uses `wg-napi` in process while preserving CLI/daemon baselines | Temp Bun harness, 30 BM25 queries: CLI p50 124.55 ms / p95 132.08 ms; NAPI p50 0.02 ms / p95 0.03 ms; both returned top=`redis` |
| P0.2b | done | Native adapter speedup was only fixture-level, not public-runner validated | Fresh `gbrain-evals` BrainBench scorecard runs with `WG_ADAPTER_BACKEND=napi` and daemon baseline on the same checkout | `gbrain-evals@89445dd`, `BRAINBENCH_N=1`: NAPI bm25 P@5 17.4%, R@5 64.1%, 125/261, real 6.48s; daemon bm25 same score, real 10.77s; NAPI 1.66x faster |
| P0.2c | done | External users still need a local `wg-napi` build for the native adapter | `wg-napi` artifact workflow builds/test/packs platform packages and uploads tarballs; local pack smoke verifies package contents | `scripts/wg-napi-pack-smoke.sh`: `npm run build`, `npm test`, `npm pack`; tarball contains `index.js`, `index.d.ts`, and platform `.node`; `.github/workflows/wg-napi-artifacts.yml` covers Ubuntu/macOS/Windows |
| P0.2d | done | `wg-napi` had artifacts but no publish-readiness gate | Local script and CI workflow run `npm publish --dry-run --access public`, verify publish payload files, and keep real publish separate until npm ownership/trusted-publisher policy is set | `scripts/wg-napi-publish-dry-run.sh`: build/test passed; dry-run payload `wg-napi@0.1.0`, 4 files, 2.79 MB packed, includes `index.js`, `index.d.ts`, `package.json`, `wg-napi.darwin-arm64.node`; caveat logged for cross-platform release structure |
| P0.3 | done | Capture quality is not measured | Pending approval rate and extraction precision can be computed from one JSONL log | `wg pending stats --from LOG --json` returns total/count-by-type/confidence histogram |
| P1.1 | done | First-run setup requires several commands | One command prints or applies init + MCP install + skill install for a target agent | `wg init --agent codex --no-ingest PATH --json` reports steps and elapsed ms |
| P1.2 | done | Shared daemon is operational but opaque | HTTP MCP exposes health/sync/admin status without exposing secrets | `curl /health` and `curl /admin/status` return request count, store path, auth mode, sync cursors |
| P1.3 | done | Feedback loop exists but is manual | `wg feedback` count and `wg adapt train` status are visible in doctor/overview | `wg doctor --json` includes `adaptation.feedback_count`, `has_adapter`, `generation`, `ready`; smoke: before train 1/false/0/false + `wg adapt train` fix, after train 1/true/1/true |
| P2.1 | done | Per-user/source scoping is project-level only | Facts carry optional `source_id`; `wg fact add/list`, `wg search`, `wg query`, MCP equivalents, and Hermes plugin tools filter by source | Unit: `cargo test -p wg-core source_id --features semantic` 2 passed; `cargo test -p wg-cli source_id` 1 passed. Hermes agent mixed-source eval: unscoped beta inclusion 2/2 → scoped beta leakage 0/2 while alpha recall stayed 2/2 |
| P2.2 | done (non-goal) | Distributed multi-writer merge | No hidden multi-master writes; docs steer users to canonical daemon + pull cache | `AGENTS.md` documents single shared `wg mcp-serve`, no multi-stdio writers, and pull-only delta sync |
| P2.3 | done | Two local Hermes agents sharing one store need a daemon or hit redb lock errors | Hermes plugin retries short CLI fallback lock collisions by default (`lock_retry_ms=5000`), so ordinary same-host sharing works without a user-visible server step | Serverless Hermes `WgClient` smoke, 2 processes x 10 writes: retry `0` persisted 10/20 with 10 lock errors; retry `5000` persisted 20/20 with 0 errors, wall 2.16s, p50 98.1ms, max 1.22s |
| P2.4 | done | Serverless retry had no measured smoothness ceiling | `store.lock_retry_ms=5000` is recommended for small same-host teams; shared daemon remains the guidance beyond the smooth serverless envelope | `python3 bench/multi-agent/scenario_j_lock_retry_sweep.py`: retry `5000` smooth until 4 concurrent writers (40/40 persisted, p95 1282.56 ms); at 8 writers 79/80 persisted, p95 2988.4 ms; retry `0` at 8 writers persisted 10/80 |
| P2.5 | done | Shared-store guidance was buried in benchmark docs | `wg doctor --json` emits a dedicated `sharing` block with retry setting, daemon state, writer thresholds, and actionable hints/fixes | `cargo test -p wg-cli doctor`: 25 passed; `doctor_json_includes_shared_store_guidance` validates `sharing.lock_retry_ms`, `serverless_recommended_writers=4`, daemon state, mode, and hint actions |
| P2.6 | done | Agent-facing `wg_doctor` lacked the shared-store guidance now visible in CLI doctor | MCP `wg_doctor` reuses the CLI sharing report, so agents see retry/daemon guidance without shelling out to `wg doctor --json` | `cargo test -p wg-cli doctor_groups_by_code_with_action_hints` validates `sharing.serverless_recommended_writers=4`, `recommended_mode=serverless_fail_fast`, and `sharing_retry_disabled` in MCP doctor output |
| P3.1 | done | Sparse issues/tickets require agents to manually chain session + search + write | `wg workflow start` and MCP `wg_workflow_start` create a tracked session, store the trigger as a question fact, and return a project context pack | `cargo test -p wg-cli workflow_start`; `python3 bench/multi-agent/scenario_f_workflow_triggers.py` validates 4 distinct tickets across CLI/MCP/Hermes with 10/10 invariants |
| P3.2 | done | Workflow trigger quality claims are pass/fail only | Scenario F reports latency, context size, prior type distribution, and forbidden context leakage for each ticket | `python3 bench/multi-agent/scenario_f_workflow_triggers.py` validates 4 tickets with 13/13 invariants; p95 workflow latency < 5s; max context < 12k chars; leakage total = 0 |
| P3.3 | done | Hermes workflow start still shells out even when `wg-python` is installed | `wg-python` exposes `source_id` and Hermes composes workflow packs in process when the binding is available, falling back to CLI otherwise | `python3 bench/multi-agent/scenario_g_hermes_binding.py`: 5/5 invariants; shape parity 4/4; leakage 0; p50 1795.71ms CLI vs 13.14ms binding (136.66x) |
| P3.4 | done | Natural-language workflow adoption differs by agent | Claude / Codex / Hermes sparse-ticket prompts naturally call `wg_workflow_start` or produce its store side effect | `python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py`: 4/4 invariants; 3/3 agents passed; each created 1 scoped workflow fact; prior reflection Claude 3/3, Codex 2/3, Hermes 3/3; forbidden leakage 0 |
| P3.5 | done | Workflow setup failures are hard to diagnose | `wg doctor --json` reports workflow readiness, recent workflow tickets, and agent integration hints | `cargo test -p wg-cli doctor_json_includes_workflow_readiness_hints`; unit smoke covers `workflow.ready`, `recent_ticket_count`, recent ticket summaries, and actionable `hints[]` |
| P3.6 | done | Doctor readiness exists but is not validated against real workflow traces | Scenario I creates workflow tickets through CLI, MCP, and Hermes, then validates `wg doctor --json` from an isolated agent config | `python3 bench/multi-agent/scenario_i_workflow_doctor.py`: 10/10 invariants; `workflow.ready=true`; `recent_ticket_count=3`; drivers CLI/MCP/Hermes; p95 workflow latency 1891.48 ms |
| P3.7 | done | Release workflow checks require remembering several commands | One script builds `wg`, runs Scenario F + I, and asserts `wg doctor --json` workflow readiness on a fixture store | `scripts/workflow-release-smoke.sh`: Scenario F 13/13, Scenario I 10/10, fixture doctor `workflow_ready=true`, `recent_ticket_count=1` |
| P3.8 | done | Workflow release smoke is local-only | CI runs the zero-token workflow release smoke as a named check after lint | `.github/workflows/ci.yml` job `workflow-release-smoke`; local command remains `scripts/workflow-release-smoke.sh` |
| P3.9 | done | Workflow release smoke runtime is opaque in CI | Smoke script prints per-step timing and writes the same markdown table to `$GITHUB_STEP_SUMMARY` | `scripts/workflow-release-smoke.sh`: latest local total 15.13s; Scenario F 7.53s, Scenario I 5.21s, fixture workflow start 1.94s |

## Current Sprint

All planned P0-P3.9 roadmap items are closed. Scenario H now isolates each
agent's integration path: Claude project MCP, Codex temp `CODEX_HOME` MCP, and
Hermes MCP-only profile to avoid redb lock contention with the in-process
plugin. `wg doctor --json` now exposes workflow readiness, recent workflow
tickets, and actionable setup hints for sparse ticket automation. Scenario I
now validates that doctor view against actual CLI/MCP/Hermes workflow traces.
Scenario J defines the serverless shared-store envelope: `lock_retry_ms=5000`
is smooth through four concurrent local writers, while eight writers should use
the shared daemon path if every write matters. `wg doctor --json` now surfaces
that as a first-class `sharing` report instead of burying it in workflow hints.
MCP `wg_doctor` now carries the same sharing summary so coding agents can react
to retry/daemon guidance through the tool surface they already call.

Next measurement candidates:
1. Design the cross-platform npm package split for `wg-napi` (`wg-napi` wrapper + platform optional dependencies) before real publish.
2. Prototype daemon auto-discovery only if a future scenario needs more than four concurrent local writers without user-visible setup.
3. Add an end-to-end MCP doctor scenario if future agent regressions show the unit contract is not enough.

## Positioning Guardrails

- Preserve `wg`'s default zero-LLM, local-first path.
- Make LLM extraction opt-in and measurable, not implicit.
- Optimize for coding-agent memory next to a repo, not hosted consumer memory.
- Prefer explicit approval queues over silent memory rewrites.
