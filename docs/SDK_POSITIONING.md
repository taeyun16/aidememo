# SDK Positioning

`wg` should now lead with an SDK-based memory-system position, but keep the
word "SDK" scoped carefully. `wg-agent-sdk` owns the agent workflow and can be
called an agent SDK. Low-level native packages should keep saying "binding" or
"SDK candidate" until they own a complete developer workflow, not just a native
call surface.

The line matters because the product position is agent-friendly local memory.
Premature SDK language for a low-level binding creates support expectations
around installers, docs, examples, async clients, and semantic-version
guarantees.

## Current Call

| Package | Current label | Why |
|---|---|---|
| `wg-agent-sdk` | Agent SDK | Pure-Python composition layer for agents that can execute code. It owns the code-first memory workflow (`Memory.open`, `search_rows`, `coverage_by`, `aggregate_many`, `remember`) and can run through either `wg-python` or the `wg` CLI fallback, making it usable from Codex, Claude Code, Hermes, CI, and local scripts. Local pack/install smoke exists; public PyPI release remains the distribution blocker, not a blocker for SDK positioning. |
| `wg-python` | SDK candidate | Hermes already gets the largest measured lift from in-process Python: Scenario G showed p50 `1795.71ms` CLI vs `13.14ms` binding with shape parity. Scenario K keeps Python shape parity with CLI across sparse tickets. Python now exposes `workflow_start` and typed exceptions, so the remaining blocker is public PyPI install. |
| `wg-napi` | SDK candidate | Node has platform package split, npm pack/install smoke, dry-run publish gates, a native adapter path that was `1.66x` faster than daemon BrainBench on the same checkout, a `workflowStart` API, package README examples, stable `[wg_code]` error messages, and Scenario K parity with CLI. The remaining blocker is public npm install. |
| `wg-nif` | Binding | Useful for Elixir/Erlang systems, but the package is still a thin NIF wrapper over the Rust core. Keep it low-level until Hex packaging, examples, and supervision-friendly lifecycle docs exist. |
| `wg-ffi` | Binding / ABI | C should stay an ABI surface for embedding and downstream language wrappers. Calling it an SDK would imply ownership of memory-management ergonomics across C applications. |

## Promotion Criteria

A low-level language package can be promoted from "binding" to "SDK" when all
of these are true:

1. The package is installable from its public registry without a local Rust
   checkout.
2. The public API has a workflow-level example for sparse ticket start,
   context/query, fact write, and source-scoped search.
3. Release preflight covers version drift, package contents, install smoke,
   and publish dry-run for that package.
4. Runtime version reporting matches package metadata.
5. Error handling is idiomatic for the language rather than just passing Rust
   error strings through. Python should expose typed exceptions; Node should at
   least expose `error.code` plus stable `[wg_code]` message prefixes.
6. For agent composition packages, the public API should own intermediate-state
   workflows such as fanout retrieval, dedupe, coverage, aggregation, and batch
   writes rather than only wrapping one low-level native call at a time.

Run the local gate before changing README or release wording:

```bash
scripts/sdk-promotion-check.sh
WG_SDK_PROMOTION_RUN_SCENARIO_K=1 scripts/sdk-promotion-check.sh
WG_SDK_PROMOTION_REQUIRE_PUBLIC=1 scripts/sdk-promotion-check.sh
WG_RELEASE_PREFLIGHT_SDK_REQUIRE_PUBLIC=1 scripts/release-preflight.sh
```

The default check should keep `local_ready=true` but `sdk_promotable=false` for
`wg-python` / `wg-napi` until public PyPI/npm installs are verified. That does
not block the top-level product wording from calling `wg-agent-sdk` the
agent-facing SDK path.

## Recommended Sequence

1. Lead public product wording with `wg-agent-sdk` as the agent-facing SDK path.
   It is the broadest Codex / Claude Code / Hermes-facing API and can still use
   `wg-python` underneath when the native binding is available.
2. Promote `wg-python` after the PyPI trusted-publisher release succeeds.
   It already has the clearest workflow evidence through Hermes and typed
   exceptions.
3. Promote `wg-napi` after npm trusted publishing succeeds.
4. Keep `wg-nif` and `wg-ffi` as bindings until packaging and lifecycle
   expectations are stronger.

README wording should distinguish the layers: `wg-agent-sdk` is the
agent-facing SDK, while Python / Node / Elixir / C remain native bindings unless
their package-specific promotion criteria pass.
