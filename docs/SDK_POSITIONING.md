# SDK Positioning

AideMemo should now lead with an SDK-based memory-system position, but keep the
word "SDK" scoped carefully. `aidememo-agent-sdk` owns the agent workflow and can be
called an agent SDK. Low-level native packages should keep saying "binding" or
"SDK candidate" until they own a complete developer workflow, not just a native
call surface.

The line matters because the product position is agent-friendly local memory.
Premature SDK language for a low-level binding creates support expectations
around installers, docs, examples, async clients, and semantic-version
guarantees.

## Strongest Orchestrator Use Case

The SDK's most differentiated workflow is not merely recalling a fact inside
one agent. It is preserving a task when the worker changes:

1. `workflow_start` creates the durable task thread.
2. Codex, Claude Code, or a Hermes `coding` profile attaches decisions,
   lessons, errors, and questions to the session.
3. `handoff_packet(...)` previews a structured route/resume/content envelope or
   dispatches a session pointer to a user-assigned account/installation alias.
   `handoff_inbox` / `handoff_accept` receive it, `handoff_return` links the
   result fact, and `handoff_outbox` / `handoff_status` give the sender the
   return path; `handoff(...)` is the Markdown-only convenience view.
4. The receiver continues the same `session_id`; `source_id` keeps shared-store
   retrieval scoped independently of the profile route.
5. A credential-free installation profile plus `aidememo handoff run
   --installation ALIAS --next` can accept the pointer, invoke Codex or Claude,
   and return success/error evidence to that session; task validation remains
   with the orchestrator.
6. On another machine, branch logs transfer the source records while the
   handoff packet remains the prompt-sized routing artifact.

This gives an orchestrator a stable memory protocol without requiring every
agent vendor to share chat-history formats or profile schemas.

It is intentionally smaller than a queue. `session_id` carries task continuity,
`source_id` carries retrieval scope, `actor_id` addresses an installation, and
agent/profile describes the role. There are no topics, offsets, consumer
groups, retries, leases, copied payloads, authentication, or exactly-once
guarantee; completion remains a separately validated outcome.

## Current Call

| Package | Current label | Why |
|---|---|---|
| `aidememo-agent-sdk` | Agent SDK | Pure-Python composition layer for agents that can execute code. It owns the code-first memory workflow (`Memory.open`, `search_rows`, `coverage_by`, `aggregate_many`, `remember`, `handoff`) plus the external Codex/Claude worker lane, and can run through either the published `aidememo-python` package or the `aidememo` CLI fallback, making it usable from Codex, Claude Code, Hermes, CI, and local scripts. |
| `aidememo-python` | SDK candidate | Hermes already gets the largest measured lift from in-process Python: Scenario G showed p50 `1795.71ms` CLI vs `13.14ms` binding with shape parity. Scenario K keeps Python shape parity with CLI across sparse tickets. Python exposes `workflow_start` and typed exceptions and is installable from PyPI; promotion now depends on the remaining workflow-level SDK criteria rather than distribution. |
| `aidememo-napi` | SDK candidate | Node has a published platform-package split, a native adapter path that was `1.66x` faster than daemon BrainBench on the same checkout, a `workflowStart` API, package README examples, stable `[aidememo_code]` error messages, and Scenario K parity with CLI. Promotion now depends on the remaining workflow-level SDK criteria rather than distribution. |
| `aidememo-nif` | Binding | Useful for Elixir/Erlang systems, but the package is still a thin NIF wrapper over the Rust core. Keep it low-level until Hex packaging, examples, and supervision-friendly lifecycle docs exist. |
| `aidememo-ffi` | Binding / ABI | C should stay an ABI surface for embedding and downstream language wrappers. Calling it an SDK would imply ownership of memory-management ergonomics across C applications. |

## Promotion Criteria

A low-level language package can be promoted from "binding" to "SDK" when all
of these are true:

1. The package is installable from its public registry without a local Rust
   checkout.
2. The public API has a workflow-level example for sparse ticket start,
   context/query, fact write, and source-scoped search.
3. Release preflight covers version drift, changelog cut, package contents,
   install smoke, and publish dry-run for that package.
4. Runtime version reporting matches package metadata.
5. Error handling is idiomatic for the language rather than just passing Rust
   error strings through. Python should expose typed exceptions; Node should at
   least expose `error.code` plus stable `[aidememo_code]` message prefixes.
6. For agent composition packages, the public API should own intermediate-state
   workflows such as fanout retrieval, dedupe, coverage, aggregation, and batch
   writes rather than only wrapping one low-level native call at a time.

Run the local gate before changing README or release wording:

```bash
scripts/sdk-promotion-check.sh
AIDEMEMO_SDK_PROMOTION_RUN_SCENARIO_K=1 scripts/sdk-promotion-check.sh
AIDEMEMO_SDK_PROMOTION_REQUIRE_PUBLIC=1 scripts/sdk-promotion-check.sh
AIDEMEMO_RELEASE_PREFLIGHT_SDK_REQUIRE_PUBLIC=1 scripts/release-preflight.sh
```

The public-registry-required check verifies the published PyPI/npm installs in
addition to the local package checks. The top-level product wording can call
`aidememo-agent-sdk` the agent-facing SDK path independently of whether the
lower-level bindings meet every promotion criterion. The same gate also protects the SDK-consumer contract:
session-aware writes, pinned context access, and PyO3/Node/agent-SDK parity must
stay documented and present in code before release wording changes pass.

## Recommended Sequence

1. Lead public product wording with `aidememo-agent-sdk` as the agent-facing SDK path.
   It is the broadest Codex / Claude Code / Hermes-facing API and can still use
   `aidememo-python` underneath when the native binding is available.
2. Evaluate `aidememo-python` against the remaining workflow-level criteria. It
   already has the clearest workflow evidence through Hermes, typed exceptions,
   and a public PyPI package.
3. Evaluate the published `aidememo-napi` package against the same remaining
   workflow-level criteria.
4. Keep `aidememo-nif` and `aidememo-ffi` as bindings until packaging and lifecycle
   expectations are stronger.

README wording should distinguish the layers: `aidememo-agent-sdk` is the
agent-facing SDK, while Python / Node / Elixir / C remain native bindings unless
their package-specific promotion criteria pass.
