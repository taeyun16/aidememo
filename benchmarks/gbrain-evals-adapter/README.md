# gbrain-evals Adapter Scaffold

This directory contains a lightweight adapter scaffold for running `aidememo` inside
the public `garrytan/gbrain-evals` harness.

The goal is not to vendor that harness into this repository. The goal is to make
the cross-system validation path concrete and reproducible:

1. Build or install `aidememo`.
2. Clone `gbrain-evals`.
3. Copy `aidememo-adapter.ts` into `gbrain-evals/eval/runner/adapters/aidememo.ts`.
4. Register it in `gbrain-evals/eval/runner/multi-adapter.ts`.
5. Run the BrainBench or LongMemEval command from that repository.

Validated against `garrytan/gbrain-evals` commit `89445dd` on 2026-05-21.
See `RESULTS.md` for the current fresh-checkout native backend scorecard and
historical CLI / daemon baselines.

## Registration Patch

```diff
 import { HybridNoGraphAdapter } from './adapters/vector-grep-rrf-fusion.ts';
+import { AideMemoAdapter } from './adapters/aidememo.ts';

 const allAdapters: Adapter[] = [
   new GbrainAfterAdapter(),
   new HybridNoGraphAdapter(),
   new RipgrepBm25Adapter(),
   new VectorOnlyAdapter(),
+  new AideMemoAdapter(),
 ];
```

## Expected Metrics

Record these fields in the scorecard:

- `P@5`
- `R@5`
- `correct_in_top_k / total_expected`
- wall time
- adapter config: `AIDEMEMO_BIN`, `AIDEMEMO_ADAPTER_MODE`, `AIDEMEMO_ADAPTER_KEEP`

LongMemEval runs should also record `R@10` and `MRR` when that runner emits
them. BrainBench `multi-adapter.ts` currently emits P@5/R@5.

Current internal comparison target from `docs/MEASUREMENTS.md`:

| System | LongMemEval-S 500q R@5 |
|---|---:|
| aidememo + bge-small-en + HNSW | 98.0% |
| gbrain-hybrid published | 97.6% |
| MemPalace published | 96.6% |

The adapter should be treated as complete when a fresh `gbrain-evals` run
reports `aidememo` in the same scorecard format as the built-in adapters.

## Adapter Knobs

- `AIDEMEMO_BIN`: path to the `aidememo` binary. Default: `aidememo`.
- `AIDEMEMO_ADAPTER_BACKEND`: `auto`, `cli`, or `napi`. Default: `auto`.
  `auto` uses `aidememo-napi` when it can be loaded, otherwise falls back to the
  existing CLI path. `napi` fails fast if the native package is unavailable.
- `AIDEMEMO_NAPI_MODULE`: module name or absolute package path to load when using
  the native backend. Default: `aidememo-napi`.
- `AIDEMEMO_ADAPTER_MODE`: `bm25` or `hybrid`. Default: `hybrid`.
- `AIDEMEMO_ADAPTER_DAEMON`: when set to `1`, start one local `aidememo mcp-serve`
  for the temp store and route queries through `aidememo search --via`. Ignored when
  the native backend is active.
- `AIDEMEMO_ADAPTER_DAEMON_PORT`: optional fixed port for daemon mode. Default:
  allocate a free local port.
- `AIDEMEMO_ADAPTER_DAEMON_TIMEOUT_MS`: health-check timeout. Default: `10000`.
- `AIDEMEMO_ADAPTER_LIMIT`: number of raw aidememo hits to request per query. Default: `10`.
- `AIDEMEMO_ADAPTER_KEEP`: when set, keep the temp store/root for inspection.

The adapter keeps the CLI path as the portable baseline, but can now remove
per-query process spawn when a built `aidememo-napi` package is available:

```bash
export AIDEMEMO_ROOT=/path/to/aidememo
cd "$AIDEMEMO_ROOT/crates/aidememo-napi"
npm install
npm run build

cd /tmp/gbrain-evals
AIDEMEMO_ADAPTER_BACKEND=napi \
AIDEMEMO_NAPI_MODULE="$AIDEMEMO_ROOT/crates/aidememo-napi" \
bun eval/runner/multi-adapter.ts --adapter=aidememo
```

Use `AIDEMEMO_ADAPTER_BACKEND=cli AIDEMEMO_ADAPTER_DAEMON=1` for the daemon baseline and
`AIDEMEMO_ADAPTER_BACKEND=napi` for the in-process binding measurement.
