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

Validated against `garrytan/gbrain-evals` commit `89445dd` on 2026-05-21.
See `RESULTS.md` for the current fresh-checkout native backend scorecard and
historical CLI / daemon baselines.

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
- `WG_ADAPTER_BACKEND`: `auto`, `cli`, or `napi`. Default: `auto`.
  `auto` uses `wg-napi` when it can be loaded, otherwise falls back to the
  existing CLI path. `napi` fails fast if the native package is unavailable.
- `WG_NAPI_MODULE`: module name or absolute package path to load when using
  the native backend. Default: `wg-napi`.
- `WG_ADAPTER_MODE`: `bm25` or `hybrid`. Default: `hybrid`.
- `WG_ADAPTER_DAEMON`: when set to `1`, start one local `wg mcp-serve`
  for the temp store and route queries through `wg search --via`. Ignored when
  the native backend is active.
- `WG_ADAPTER_DAEMON_PORT`: optional fixed port for daemon mode. Default:
  allocate a free local port.
- `WG_ADAPTER_DAEMON_TIMEOUT_MS`: health-check timeout. Default: `10000`.
- `WG_ADAPTER_LIMIT`: number of raw wg hits to request per query. Default: `10`.
- `WG_ADAPTER_KEEP`: when set, keep the temp store/root for inspection.

The adapter keeps the CLI path as the portable baseline, but can now remove
per-query process spawn when a built `wg-napi` package is available:

```bash
cd crates/wg-napi
npm install
npm run build

cd /tmp/gbrain-evals
WG_ADAPTER_BACKEND=napi \
WG_NAPI_MODULE=/Users/mixlink/dev/wg/crates/wg-napi \
bun eval/runner/multi-adapter.ts --adapter=wg
```

Use `WG_ADAPTER_BACKEND=cli WG_ADAPTER_DAEMON=1` for the daemon baseline and
`WG_ADAPTER_BACKEND=napi` for the in-process binding measurement.
