# gbrain-evals Adapter Scaffold

This directory contains a lightweight adapter scaffold for running `wg` inside
the public `garrytan/gbrain-evals` harness.

The goal is not to vendor that harness into this repository. The goal is to make
the cross-system validation path concrete and reproducible:

1. Build or install `wg`.
2. Clone `gbrain-evals`.
3. Copy `wg-adapter.ts` into `gbrain-evals/eval/runner/adapters/wg.ts`.
4. Register it in `gbrain-evals/eval/runner/multi-adapter.ts`.
5. Run the BrainBench or LongMemEval command from that repository.

Validated against `garrytan/gbrain-evals` commit `ef7794f` on 2026-05-19.
See `RESULTS.md` for the current fresh-checkout smoke and scorecard.

## Registration Patch

```diff
 import { HybridNoGraphAdapter } from './adapters/vector-grep-rrf-fusion.ts';
+import { WgAdapter } from './adapters/wg.ts';

 const allAdapters: Adapter[] = [
   new GbrainAfterAdapter(),
   new HybridNoGraphAdapter(),
   new RipgrepBm25Adapter(),
   new VectorOnlyAdapter(),
+  new WgAdapter(),
 ];
```

## Expected Metrics

Record these fields in the scorecard:

- `P@5`
- `R@5`
- `correct_in_top_k / total_expected`
- wall time
- adapter config: `WG_BIN`, `WG_ADAPTER_MODE`, `WG_ADAPTER_KEEP`

LongMemEval runs should also record `R@10` and `MRR` when that runner emits
them. BrainBench `multi-adapter.ts` currently emits P@5/R@5.

Current internal comparison target from `docs/MEASUREMENTS.md`:

| System | LongMemEval-S 500q R@5 |
|---|---:|
| wg + bge-small-en + HNSW | 98.0% |
| gbrain-hybrid published | 97.6% |
| MemPalace published | 96.6% |

The adapter should be treated as complete when a fresh `gbrain-evals` run
reports `wg` in the same scorecard format as the built-in adapters.

## Adapter Knobs

- `WG_BIN`: path to the `wg` binary. Default: `wg`.
- `WG_ADAPTER_MODE`: `bm25` or `hybrid`. Default: `hybrid`.
- `WG_ADAPTER_DAEMON`: when set to `1`, start one local `wg mcp-serve`
  for the temp store and route queries through `wg search --via`.
- `WG_ADAPTER_DAEMON_PORT`: optional fixed port for daemon mode. Default:
  allocate a free local port.
- `WG_ADAPTER_DAEMON_TIMEOUT_MS`: health-check timeout. Default: `10000`.
- `WG_ADAPTER_LIMIT`: number of raw wg hits to request per query. Default: `10`.
- `WG_ADAPTER_KEEP`: when set, keep the temp store/root for inspection.

The adapter shells out to the CLI rather than importing `wg-napi` so it works
before native Node packaging is published. Use `WG_ADAPTER_DAEMON=1` for the
current low-overhead path; once `wg-napi` is packaged, the same adapter can be
converted to in-process calls.
