# wg Adapter Results

Fresh-checkout validation for `benchmarks/gbrain-evals-adapter/wg-adapter.ts`.

## 2026-05-19 — BrainBench Multi-Adapter Smoke

Environment:

- `garrytan/gbrain-evals` commit: `ef7794f`
- Bun: `1.3.11`
- wg binary: `/Users/mixlink/dev/wg/target/debug/wg`
- Adapter mode: `WG_ADAPTER_MODE=bm25`
- Adapter daemon: unset
- Adapter limit: `WG_ADAPTER_LIMIT=10`
- Runs: `BRAINBENCH_N=1`

Setup:

```bash
git clone --depth 1 https://github.com/garrytan/gbrain-evals "$TMP/gbrain-evals"
cd "$TMP/gbrain-evals"
bun install
cp /Users/mixlink/dev/wg/benchmarks/gbrain-evals-adapter/wg-adapter.ts \
  eval/runner/adapters/wg.ts
# Register WgAdapter in eval/runner/multi-adapter.ts.
```

Adapter smoke:

```bash
WG_BIN=/Users/mixlink/dev/wg/target/debug/wg \
WG_ADAPTER_MODE=bm25 \
bun test ./eval/runner/adapters/wg-smoke.test.ts
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
  WG_BIN=/Users/mixlink/dev/wg/target/debug/wg \
  WG_ADAPTER_MODE=bm25 \
  WG_ADAPTER_LIMIT=10 \
  bun eval/runner/multi-adapter.ts --adapter=wg
```

Result:

| Adapter | Runs | Corpus Pages | Queries | P@5 | R@5 | Correct / Expected | Runner Time | Real Time |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| wg bm25 | 1 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 63.3s | 63.38s |

## 2026-05-19 — Daemon Adapter Path

Same fresh checkout and registration patch as above.

Adapter smoke:

```bash
WG_BIN=/Users/mixlink/dev/wg/target/debug/wg \
WG_ADAPTER_MODE=bm25 \
WG_ADAPTER_DAEMON=1 \
bun test ./eval/runner/adapters/wg-smoke.test.ts
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
  WG_BIN=/Users/mixlink/dev/wg/target/debug/wg \
  WG_ADAPTER_MODE=bm25 \
  WG_ADAPTER_DAEMON=1 \
  WG_ADAPTER_LIMIT=10 \
  bun eval/runner/multi-adapter.ts --adapter=wg
```

```bash
/usr/bin/time -p env \
  BRAINBENCH_N=1 \
  WG_BIN=/Users/mixlink/dev/wg/target/debug/wg \
  WG_ADAPTER_MODE=hybrid \
  WG_ADAPTER_DAEMON=1 \
  WG_ADAPTER_LIMIT=10 \
  bun eval/runner/multi-adapter.ts --adapter=wg
```

| Adapter | Runs | Corpus Pages | Queries | P@5 | R@5 | Correct / Expected | Runner Time | Real Time |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| wg bm25 daemon | 1 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 10.9s | 11.04s |
| wg hybrid daemon | 1 | 240 | 145 | 16.7% | 62.5% | 121 / 261 | 45.5s | 45.64s |

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
- The non-daemon adapter shells out to `wg search` once per query; the 63s run
  time is mostly redb open / process-local setup overhead.
- The daemon adapter still shells out once per query, but the expensive store
  and model state live in `wg mcp-serve`.
- `wg-napi` can still reduce overhead further by removing per-query CLI process
  spawn.
