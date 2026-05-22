# SDK Positioning

`wg` should keep saying "bindings" until a language package owns a complete
developer workflow, not just a native call surface. The line matters because
the product position is local-first agent memory, and premature SDK language
creates support expectations around installers, docs, examples, async clients,
and semantic-version guarantees.

## Current Call

| Package | Current label | Why |
|---|---|---|
| `wg-python` | SDK candidate | Hermes already gets the largest measured lift from in-process Python: Scenario G showed p50 `1795.71ms` CLI vs `13.14ms` binding with shape parity. Scenario K keeps Python shape parity with CLI across sparse tickets. Python now exposes `workflow_start` and typed exceptions, so the remaining blocker is public PyPI install. |
| `wg-napi` | SDK candidate | Node has platform package split, npm pack/install smoke, dry-run publish gates, a native adapter path that was `1.66x` faster than daemon BrainBench on the same checkout, a `workflowStart` API, package README examples, stable `[wg_code]` error messages, and Scenario K parity with CLI. The remaining blocker is public npm install. |
| `wg-nif` | Binding | Useful for Elixir/Erlang systems, but the package is still a thin NIF wrapper over the Rust core. Keep it low-level until Hex packaging, examples, and supervision-friendly lifecycle docs exist. |
| `wg-ffi` | Binding / ABI | C should stay an ABI surface for embedding and downstream language wrappers. Calling it an SDK would imply ownership of memory-management ergonomics across C applications. |

## Promotion Criteria

A language package can be promoted from "binding" to "SDK" when all of these
are true:

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

Run the local gate before changing README or release wording:

```bash
scripts/sdk-promotion-check.sh
WG_SDK_PROMOTION_RUN_SCENARIO_K=1 scripts/sdk-promotion-check.sh
WG_SDK_PROMOTION_REQUIRE_PUBLIC=1 scripts/sdk-promotion-check.sh
```

The default check should keep `local_ready=true` but `sdk_promotable=false`
until public PyPI/npm installs are verified.

## Recommended Sequence

1. Promote `wg-python` first after the PyPI trusted-publisher release succeeds.
   It already has the clearest workflow evidence through Hermes and typed
   exceptions.
2. Promote `wg-napi` after npm trusted publishing succeeds.
3. Keep `wg-nif` and `wg-ffi` as bindings until packaging and lifecycle
   expectations are stronger.

README wording should remain conservative for now: "native bindings" in the
top-level product description, with SDK candidates called out only in release
or roadmap docs.
