# aidememo Adapter Results

Fresh-checkout validation for `benchmarks/gbrain-evals-adapter/aidememo-adapter.ts`.

## 2026-05-19 — BrainBench Multi-Adapter Smoke

Environment:

- `garrytan/gbrain-evals` commit: `ef7794f`
- Bun: `1.3.11`
- aidememo binary: `/Users/mixlink/dev/aidememo/target/debug/aidememo`
- Adapter mode: `AIDEMEMO_ADAPTER_MODE=bm25`
- Adapter daemon: unset
- Adapter limit: `AIDEMEMO_ADAPTER_LIMIT=10`
- Runs: `BRAINBENCH_N=1`

Setup:

```bash
git clone --depth 1 https://github.com/garrytan/gbrain-evals "$TMP/gbrain-evals"
cd "$TMP/gbrain-evals"
bun install
cp /Users/mixlink/dev/aidememo/benchmarks/gbrain-evals-adapter/aidememo-adapter.ts \
  eval/runner/adapters/aidememo.ts
# Register AideMemoAdapter in eval/runner/multi-adapter.ts.
```

Adapter smoke:

```bash
AIDEMEMO_BIN=/Users/mixlink/dev/aidememo/target/debug/aidememo \
AIDEMEMO_ADAPTER_MODE=bm25 \
bun test ./eval/runner/adapters/aidememo-smoke.test.ts
```

Result:

- `1 pass`
- `0 fail`
- `3 expect() calls`
- wall time: `2.11s`

BrainBench scorecard:

```bash
/usr/bin/time -p env \
  BRAINBENCH_N=1 \
  AIDEMEMO_BIN=/Users/mixlink/dev/aidememo/target/debug/aidememo \
  AIDEMEMO_ADAPTER_MODE=bm25 \
  AIDEMEMO_ADAPTER_LIMIT=10 \
  bun eval/runner/multi-adapter.ts --adapter=aidememo
```

Result:

| Adapter | Runs | Corpus Pages | Queries | P@5 | R@5 | Correct / Expected | Runner Time | Real Time |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| aidememo bm25 | 1 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 63.3s | 63.38s |

## 2026-05-19 — Daemon Adapter Path

Same fresh checkout and registration patch as above.

Adapter smoke:

```bash
AIDEMEMO_BIN=/Users/mixlink/dev/aidememo/target/debug/aidememo \
AIDEMEMO_ADAPTER_MODE=bm25 \
AIDEMEMO_ADAPTER_DAEMON=1 \
bun test ./eval/runner/adapters/aidememo-smoke.test.ts
```

Result:

- `1 pass`
- `0 fail`
- `3 expect() calls`
- wall time: `2.21s`

BrainBench scorecards:

```bash
/usr/bin/time -p env \
  BRAINBENCH_N=1 \
  AIDEMEMO_BIN=/Users/mixlink/dev/aidememo/target/debug/aidememo \
  AIDEMEMO_ADAPTER_MODE=bm25 \
  AIDEMEMO_ADAPTER_DAEMON=1 \
  AIDEMEMO_ADAPTER_LIMIT=10 \
  bun eval/runner/multi-adapter.ts --adapter=aidememo
```

```bash
/usr/bin/time -p env \
  BRAINBENCH_N=1 \
  AIDEMEMO_BIN=/Users/mixlink/dev/aidememo/target/debug/aidememo \
  AIDEMEMO_ADAPTER_MODE=hybrid \
  AIDEMEMO_ADAPTER_DAEMON=1 \
  AIDEMEMO_ADAPTER_LIMIT=10 \
  bun eval/runner/multi-adapter.ts --adapter=aidememo
```

| Adapter | Runs | Corpus Pages | Queries | P@5 | R@5 | Correct / Expected | Runner Time | Real Time |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| aidememo bm25 daemon | 1 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 10.9s | 11.04s |
| aidememo hybrid daemon | 1 | 240 | 145 | 16.7% | 62.5% | 121 / 261 | 45.5s | 45.64s |

Delta vs non-daemon bm25:

- Real time: `63.38s -> 11.04s`, `5.7x` faster.
- R@5: unchanged at `64.1%`.
- Correct / expected: unchanged at `125 / 261`.

Interpretation:

- The daemon path removes most redb open / process-local setup overhead while
  preserving the same scorecard for bm25.
- On this BrainBench relational corpus, hybrid is slower and slightly worse
  than bm25. This corpus is surface-form/slug heavy; semantic ranking is not
  expected to help until the query distribution becomes paraphrase dominant.

Notes:

- This measures the packaged adapter path, not an optimized in-process binding.
- The non-daemon adapter shells out to `aidememo search` once per query; the 63s run
  time is mostly redb open / process-local setup overhead.
- The daemon adapter still shells out once per query, but the expensive store
  and model state live in `aidememo mcp-serve`.
- Follow-up implementation note: `AIDEMEMO_ADAPTER_BACKEND=auto|napi|cli` now lets
  the scaffold use `aidememo-napi` in process when the native package is available,
  removing per-query CLI spawn while preserving the previous CLI and daemon
  baselines for apples-to-apples comparisons. Fresh gbrain-evals scorecards
  should record `AIDEMEMO_ADAPTER_BACKEND` and `AIDEMEMO_NAPI_MODULE` alongside the existing
  knobs.

## 2026-05-21 — Native Backend Adapter Smoke

This is a local fixture smoke for the adapter scaffold, not the full public
BrainBench scorecard. It verifies that the copied `aidememo-adapter.ts` can run both
the preserved CLI backend and the new native `aidememo-napi` backend against the same
Bun-shaped harness.

Setup:

- `aidememo-napi`: `npm install && npm run build`
- CLI binary: `/Users/mixlink/dev/aidememo/target/debug/aidememo`
- Adapter mode: `AIDEMEMO_ADAPTER_MODE=bm25`
- Fixture: 3 markdown pages, 30 repeated queries for
  `high availability cache failover`

Result:

| Backend | Queries | Top Hit | p50 | p95 |
|---|---:|---|---:|---:|
| `AIDEMEMO_ADAPTER_BACKEND=cli` | 30 | `redis` | 124.55 ms | 132.08 ms |
| `AIDEMEMO_ADAPTER_BACKEND=napi` | 30 | `redis` | 0.02 ms | 0.03 ms |

Interpretation: the native path removes the per-query CLI process spawn and
store open overhead on this small fixture. The next validation step is the full
fresh-checkout `gbrain-evals` scorecard with `AIDEMEMO_ADAPTER_BACKEND=napi`; that
will measure end-to-end wall time under the public runner.

## 2026-05-21 — BrainBench Native Backend Scorecard

Fresh-checkout validation after adding `AIDEMEMO_ADAPTER_BACKEND=napi`.

Environment:

- `garrytan/gbrain-evals` commit: `89445dd`
- `gbrain` dependency: `garrytan/gbrain#1580c6d`
- Bun: `1.3.11`
- aidememo binary: `/Users/mixlink/dev/aidememo/target/debug/aidememo`
- aidememo-napi module: `/Users/mixlink/dev/aidememo/crates/aidememo-napi`
- Adapter mode: `AIDEMEMO_ADAPTER_MODE=bm25`
- Adapter limit: `AIDEMEMO_ADAPTER_LIMIT=10`
- Runs: `BRAINBENCH_N=1`

Setup:

```bash
git clone --depth 1 https://github.com/garrytan/gbrain-evals "$TMP/gbrain-evals"
cd "$TMP/gbrain-evals"
bun install
cp /Users/mixlink/dev/aidememo/benchmarks/gbrain-evals-adapter/aidememo-adapter.ts \
  eval/runner/adapters/aidememo.ts
# Register AideMemoAdapter in eval/runner/multi-adapter.ts.
```

Native backend scorecard:

```bash
/usr/bin/time -p env \
  BRAINBENCH_N=1 \
  AIDEMEMO_BIN=/Users/mixlink/dev/aidememo/target/debug/aidememo \
  AIDEMEMO_ADAPTER_BACKEND=napi \
  AIDEMEMO_NAPI_MODULE=/Users/mixlink/dev/aidememo/crates/aidememo-napi \
  AIDEMEMO_ADAPTER_MODE=bm25 \
  AIDEMEMO_ADAPTER_LIMIT=10 \
  bun eval/runner/multi-adapter.ts --adapter=aidememo
```

Daemon baseline on the same checkout:

```bash
/usr/bin/time -p env \
  BRAINBENCH_N=1 \
  AIDEMEMO_BIN=/Users/mixlink/dev/aidememo/target/debug/aidememo \
  AIDEMEMO_ADAPTER_BACKEND=cli \
  AIDEMEMO_ADAPTER_DAEMON=1 \
  AIDEMEMO_ADAPTER_MODE=bm25 \
  AIDEMEMO_ADAPTER_LIMIT=10 \
  bun eval/runner/multi-adapter.ts --adapter=aidememo
```

Result:

| Adapter | Runs | Corpus Pages | Queries | P@5 | R@5 | Correct / Expected | Runner Time | Real Time |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| aidememo bm25 daemon | 1 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 10.7s | 10.77s |
| aidememo bm25 napi | 1 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 6.2s | 6.48s |

Delta:

- NAPI vs daemon real time: `10.77s -> 6.48s`, `1.66x` faster.
- NAPI vs historical direct CLI real time: `63.38s -> 6.48s`, `9.78x` faster.
- Quality parity: unchanged at `P@5 17.4%`, `R@5 64.1%`, `125 / 261`.

Interpretation: the NAPI path removes the remaining per-query CLI process spawn
while preserving the same BrainBench scorecard as direct and daemon bm25. The
remaining ~6s are dominated by initial ingest / benchmark runner overhead, not
per-query search dispatch.
