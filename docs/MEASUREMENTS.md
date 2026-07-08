# Measurements

This file is the public measurement ledger for `aidememo`. Historical scratch-note
files were removed; durable numbers should live here, in
`benchmarks/*/RESULTS.md`, or in JSON under `bench/**/results`.

## Re-run Commands

```bash
cargo check -p aidememo-core -p aidememo-cli
cargo test -p aidememo-core --features semantic
cargo test -p aidememo-cli --bin aidememo
python3 -m pytest plugins/hermes/tests -q

cargo run --release --bin performance
cargo run --release -p aidememo-benchmarks --bin storage_backend_probe
python3 bench/multi-agent/scenario_e_http_shared.py
python3 bench/multi-agent/scenario_f_workflow_triggers.py
python3 bench/multi-agent/scenario_g_hermes_binding.py
python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py
AIDEMEMO_E2E_SETUP_ONLY=1 python3 bench/multi-agent/scenario_h_workflow_natural_prompt.py
cargo build -p aidememo-cli --release --features redb
python3 bench/multi-agent/scenario_d_concurrent_writers.py
python3 bench/multi-agent/scenario_j_lock_retry_sweep.py
python3 bench/multi-agent/scenario_k_sdk_workflow_parity.py
python3 bench/multi-agent/scenario_l_self_extraction.py
python3 bench/multi-agent/scenario_m_mcp_install_source_defaults.py
python3 bench/multi-agent/scenario_n_hermes_memory_as_code.py
scripts/aidememo-agent-sdk-pack-smoke.sh
scripts/hermes-aidememo-pack-smoke.sh
scripts/skillopt-lite-cycle.sh --max-cycles 1
scripts/skillopt-lite-check.sh
scripts/demo-workflow.sh
scripts/ci-local.sh demo
scripts/sdk-promotion-check.sh
scripts/ci-local.sh sdk
```

The gbrain adapter path is documented in
[`benchmarks/gbrain-evals-adapter/README.md`](https://github.com/taeyun16/aidememo/blob/main/benchmarks/gbrain-evals-adapter/README.md),
with current scorecards in
[`benchmarks/gbrain-evals-adapter/RESULTS.md`](https://github.com/taeyun16/aidememo/blob/main/benchmarks/gbrain-evals-adapter/RESULTS.md).

## Agent UX

| Scenario | Result | Interpretation |
|---|---:|---|
| Hermes mixed-source prompt, unscoped | beta facts included 2/2 | Shared stores leak neighbouring source context unless scoped. |
| Hermes mixed-source prompt, `source_id=alpha` | alpha recall 2/2, beta leakage 0/2 | `source_id` gives clean per-agent/per-source reads. |
| Hermes serverless shared store (optional redb), retry `0` | 10/20 writes persisted, 10 lock errors | redb's process lock is visible without smoothing. |
| Hermes serverless shared store (optional redb), retry `5000` | 20/20 writes persisted, 0 errors; wall 2.16s, p50 98.1ms, max 1.22s | The redb plugin path can smooth ordinary two-agent local sharing when retry is enabled. |
| Scenario J redb serverless lock-retry sweep, retry `5000` | Smooth until 4 concurrent writers; at 8 writers 79/80 persisted, p95 2.99s | Keep optional-redb serverless sharing for small same-host teams; switch to daemon when high parallel write volume is normal. |
| HTTP shared `aidememo mcp-serve`, 2 clients x 10 writes | 20/20 persisted; p50 18.4ms, p95 41.8ms, wall 251ms | Daemon mode is still the faster high-concurrency path; the SQLite MCP soak covers the default backend's concurrent write path. |
| Workflow trigger Scenario F, 4 sparse tickets | 13/13 invariants; p95 2.48s; max context 3,023 chars; forbidden leakage 0 | CLI, MCP, and Hermes paths create distinct sessions/ticket facts and keep `source_id`-scoped ticket context separated. |
| Hermes workflow binding Scenario G, 4 sparse tickets | 5/5 invariants; shape parity 4/4; leakage 0; p50 1,795.71ms CLI vs 13.14ms binding | When `aidememo-python` is installed, Hermes composes workflow packs in process: same context contract, about 137x lower p50 after the first model/index warmup. |
| SDK workflow parity Scenario K, 4 sparse tickets | 8/8 invariants; Python and Node shape parity 4/4 each; leakage 0; p50 CLI 1,864.55ms, Python 16.19ms, Node 13.69ms | `aidememo-python` and `aidememo-napi` expose the same sparse-ticket context contract as CLI while avoiding per-command CLI spawn overhead. |
| Self-extraction Scenario L, MCP batch + default source | 13/13 invariants; `aidememo_fact_add_many` inserted 7 classified facts in 82.41ms; omitted-type batch inferred preference=1, lesson=1, error=1 with `fact_type_source=inferred`; explicit `note` on an error cue returned `fact_type_hint=error`; alpha sparse-ticket workflow recovered decision=1, lesson=1, error=1 with beta leakage 0; `AIDEMEMO_SOURCE_ID` scoped an env-default MCP write and search | If the calling agent classifies facts before `aidememo_fact_add_many`, aidememo preserves typed memory and returns it in the workflow shape agents consume without a built-in LLM extraction pipeline. When the agent omits `fact_type`, deterministic strong-cue inference prevents the common all-note failure for explicit preference / lesson / error / decision / convention phrases, while explicit `note` is preserved and surfaced with a hint instead of silently overriding caller intent. |
| MCP install source/backend defaults | `cargo test -p aidememo-cli --bin aidememo mcp_install`: 12/12 tests passed; `aidememo --backend libsqlite mcp-install --target codex --source-id agent-alpha --print --json` reports `source_id=agent-alpha`, `storage_backend=libsqlite`, env detail, and `--backend libsqlite mcp` args | `aidememo --backend <selected> mcp-install --source-id` makes the smooth path installable: the MCP server starts with `AIDEMEMO_SOURCE_ID` and the selected storage backend already set instead of asking users to hand-edit agent config. |
| Doctor scoped setup hint | `cargo test -p aidememo-cli --bin aidememo`: 120+ tests passed, 1 ignored; temp `aidememo skill install --target opencode --dest ...` output includes `aidememo --backend libsqlite mcp-install --target opencode` and `--source-id <namespace>` | Setup diagnostics now preserve project namespace and backend context: a scoped recent workflow ticket turns the no-MCP doctor hint into `aidememo --backend <selected> mcp-install --target codex --source-id <namespace>`, and skill install output points shared-store users at the same backend-pinned path. |
| MCP install source/backend defaults Scenario M | 21/21 invariants; elapsed 111.6ms; Codex / Cursor / OpenCode configs all contain `AIDEMEMO_SOURCE_ID=agent-alpha` and `--backend libsqlite`; Claude / Hermes / OpenClaw print-mode commands include env injection plus backend args; MCP write/search without explicit `source_id` returned only `agent-alpha` facts through the installed backend args | The install story is now end-to-end testable without real agent CLIs: generated configs provide the env value and storage backend selector that MCP tools consume for scoped defaults. |
| Scenario H source/backend-default setup | 5/5 setup invariants; Codex config created through `aidememo --backend libsqlite mcp-install --source-id workflow-alpha`; Claude project MCP and Hermes plugin config both carry `AIDEMEMO_SOURCE_ID` / `source_id` plus backend-pinned MCP args; no model calls | The token-burning natural prompt scenario can now run without asking agents to pass `source_id` per tool call or infer the storage backend; setup itself is zero-cost and regression-testable. |
| MCP workflow session attachment | `cargo test -p aidememo-cli --bin aidememo workflow_start_creates_session_ticket_and_scoped_context` and `cargo test -p aidememo-cli --bin aidememo fact_add_many_attaches_top_level_session_id_to_each_item` passed | MCP agents can pass `session_id` from `aidememo_workflow_start` into `aidememo_fact_add` / `aidememo_fact_add_many`, keeping follow-up facts on the workflow thread without relying on CLI-only `AIDEMEMO_SESSION_ID`. |
| Zero-token workflow demo | decision + lesson + error surfaced; search hits 4; workflow latency 128ms | `scripts/demo-workflow.sh` demonstrates the product position without an agent, model call, or persistent store. It uses CLI `workflow start --bm25-only` for deterministic first-run behaviour. `scripts/ci-local.sh demo` wraps the same smoke for daily local checks in about 0.91s warm. |
| Natural workflow adoption Scenario H, 3 agents | 4/4 invariants; 3/3 agents passed; each created 1 scoped workflow fact; prior reflection Claude 3/3, Codex 2/3, Hermes 3/3; forbidden leakage 0 | Sparse-ticket prompts can drive the workflow entry point across Claude, Codex, and Hermes when each runtime gets an isolated, deterministic MCP config. The fixture uses the default SQLite store path and MCP source defaults for scoped sharing. |
| Hermes native core parity | `python3 -m pytest plugins/hermes/tests -q`; `cargo test -p aidememo-cli --bin aidememo mcp_tools::` | Hermes now exposes `aidememo_context`, `aidememo_aggregate`, `aidememo_fact_add_many`, and `aidememo_doctor` through native tools/slash commands, and source-scoped aggregate calls honor explicit `source_id` / `AIDEMEMO_SOURCE_ID` instead of mixing shared-store facts. |
| Hermes Memory-as-Code Scenario N | `python3 bench/multi-agent/scenario_n_hermes_memory_as_code.py`: 9/9 invariants; fanout search + dedupe + coverage + derived batch + aggregate completed with beta source excluded from scoped rows | The Hermes research profile now has a zero-token, code-first regression through the shared `aidememo_agent.Memory` API: intermediate candidate sets stay in Python and only compact coverage/aggregate artifacts need to reach model context. |
| aidememo-agent-sdk wheel install smoke | `scripts/aidememo-agent-sdk-pack-smoke.sh`: built `aidememo_agent_sdk-0.1.0-py3-none-any.whl`, installed it into a temp venv, verified `Memory`, `AideMemoClient`, `AideMemoMemorySDK`, `session_canvas`, and `project_profile`; total 3.38s | The code-first SDK path is installable independently of Hermes, so Codex / Claude Code / CI scripts do not need the Hermes plugin package. |
| hermes-aidememo wheel install smoke | `scripts/hermes-aidememo-pack-smoke.sh`: built `aidememo_agent_sdk-0.1.0-py3-none-any.whl` + `hermes_aidememo-0.1.0-py3-none-any.whl`, installed both into a temp venv, verified `hermes_aidememo.AideMemoMemorySDK` re-exports `aidememo_agent.Memory`, `hermes.plugins` entry point, `plugin.yaml`, bundled `SKILL.md`, artifact SDK methods, and opt-in capture adapter defaults; total 4.69s | The Hermes plugin remains pip-installable while delegating the code-first SDK layer to the shared package. |
| SkillOpt-lite profile gate | `scripts/skillopt-lite-check.sh` validates the bundled `aidememo-skill/SKILL.md` as the current trainable memory profile candidate, then runs `aidememo skill check`, `git diff --check`, `cargo check -p aidememo-cli`, `scripts/demo-workflow.sh`, and `scripts/sdk-promotion-check.sh`; optional Scenario L/M/N gates run with `AIDEMEMO_SKILLOPT_RUN_SCENARIOS=1` | This turns SkillOpt's useful discipline into a local `aidememo` product boundary: memory skill edits are bounded, auditable, and accepted only after zero-token workflow / SDK gates pass. |
| SkillOpt-lite periodic cycle | `scripts/skillopt-lite-cycle.sh --max-cycles 1` checks the current profile when no candidate is queued, records the accepted dry-run under `target/skillopt-lite/runs.jsonl`, and stores the full gate output in `target/skillopt-lite/logs/`; passing candidates are applied only with `--apply` | Periodic skill/profile improvement can run without dirtying the repo or turning rejected candidates into failures by default; rejected edits are preserved for optimizer feedback. |

## Privacy Filter Write Guard

July 8 local run on macOS arm64 with `openai/privacy-filter` installed from
`openai/privacy-filter.git` commit `f7f00ca7` and checkpoint cached at
`~/.opf/privacy_filter`:

| Path | Result | Interpretation |
|---|---:|---|
| Checkpoint download | 2.6 GB under `~/.opf/privacy_filter`; venv 503 MB | Too large for default-on. Keep disabled until explicitly configured. |
| OPF direct Python API | first inference 2369.6 ms; warm 25-run mean 233.1 ms, p50 229.3 ms, max 281.6 ms | OPF constructor is lazy; the first redaction pays model load. Warm CPU latency is acceptable for write-side guard, not read hot path. |
| HTTP sidecar (`scripts/privacy_filter_sidecar.py`) | first `/filter` 2402.6 ms; warm mean 247.9 ms, p50 244.2 ms, max 291.4 ms | HTTP overhead is modest compared with model inference. Run a daemon/sidecar for repeated agent writes. |
| Warm sidecar RSS | ~3,977,872 KB (`ps rss`) | About 3.8 GB resident once warm. This is an opt-in team/project safety layer, not the default memory loop. |
| `aidememo fact add` baseline | mean 22.3 ms, p50 16.7 ms for 10 writes | Baseline remains the right default path. |
| `aidememo fact add` with privacy guard | mean 261.3 ms, p50 261.2 ms for 10 writes | Adds ~240 ms/write on CPU. Good for safety-sensitive capture, too expensive for every high-volume scratch write unless batched. |
| Redaction smoke | email + phone stored as `[private_email]` / `[private_phone]`; `private_person` kept for review policy | Utility-preserving default: person names remain entity-usable unless policy changes. |
| Block smoke | `OPENAI_API_KEY=sk-proj-...` rejected with `privacy filter blocked fact write; labels=secret` | Secret persistence is fail-closed in `privacy.mode=redact` because default `block_labels = ["secret"]`. |

Detection notes from the 5-case smoke:

| Case | Labels observed | Caveat |
|---|---|---|
| Email + phone | `private_person`, `private_email`, `private_phone` | Good write-guard fit. |
| Secret-like OpenAI key | `secret` | Correctly blocked by AideMemo. |
| Address/date sentence | none | Do not claim broad PII coverage from this smoke; needs fixture-level recall before default-on. |
| Code decision with no PII | none | Good no-op behavior. |
| Korean sentence with email | `private_person`, `private_email` | Korean + email shape worked in this sample. |

Artifacts:
`/private/tmp/aidememo-privacy-filter-bench.json`,
`/private/tmp/aidememo-privacy-filter-http-bench.json`, and
`/private/tmp/aidememo-privacy-write-bench.json`.

### MLX Quantized Follow-up

July 8 follow-up on the same macOS arm64 host with
`mlx-embeddings` installed from GitHub main (`0.1.1`, commit `9b28270`)
because PyPI `0.1.0` does not load `model_type = "openai_privacy_filter"`.
All three MLX variants passed the five-case smoke used for this run
(email/phone, secret, address/date, no-PII code fact, Korean email). The first
attempt with an ad-hoc loader shim produced false misses; only the official
`mlx-embeddings` 0.1.1 loader numbers below should be treated as evidence.

| Path | Size / RSS | Latency | Detection result | Interpretation |
|---|---:|---:|---|---|
| `mlx-community/openai-privacy-filter-4bit` | 780 MB; 1,328,752 KB RSS | direct warm mean 18.5 ms, p50 17.9 ms | no missing expected labels; Korean email plus person | Good quality smoke; slightly larger than mxfp4. |
| `mlx-community/openai-privacy-filter-mxfp4` | 739 MB; 1,286,336 KB RSS | direct warm mean 19.5 ms, p50 19.2 ms | no missing expected labels; both synthetic secrets detected | Best local default candidate among measured MLX variants. |
| `mlx-community/openai-privacy-filter-8bit` | 1.4 GB; 2,006,368 KB RSS | direct warm mean 18.0 ms, p50 17.3 ms | no missing expected labels | No practical win over mxfp4 for this write-guard path. |
| mxfp4 HTTP sidecar | 1,281,424 KB RSS | first `/filter` 48.4 ms; warm mean 18.6 ms, p50 18.4 ms | email/person/phone, secret, address/date, Korean email all detected | HTTP overhead is effectively hidden once warm. Use a single-threaded server; `ThreadingHTTPServer` hit an MLX stream/thread-local error. |
| `aidememo fact add` via mxfp4 sidecar | same sidecar | baseline mean 24.0 ms, p50 22.5 ms; privacy mean 53.4 ms, p50 51.1 ms | email/phone persisted as `[private_email]` / `[private_phone]`; secret write blocked | Adds about 29 ms p50 over baseline, far better than CPU OPF's ~240 ms overhead. |

Current product conclusion: MLX/mxfp4 makes the privacy guard viable as a
recommended prewarmed project/team option on Apple Silicon, but not a universal
default yet. It still needs a 739 MB model download, about 1.28 GB warm RSS,
Metal access, and the unpublished `mlx-embeddings` 0.1.1 path until PyPI catches
up. Keep `privacy.provider` disabled by default, then make `mxfp4` the preferred
sidecar recipe for stores that opt into write-time privacy filtering.

Safety follow-up: both the official OPF CPU path and the MLX mxfp4 path labelled
a bare `sk-proj-abc123` token as `private_person` when it lacked an
`OPENAI_API_KEY=`-style cue. AideMemo now adds a deterministic local secret
prefilter before applying sidecar spans, so common key prefixes such as
`sk-proj-`, `sk-`, `github_pat_`, `ghp_`, `gho_`, `xoxb-`, `AKIA`, and `AIza`
still become `secret` and hit the default block policy.

Artifacts:
`/private/tmp/aidememo-privacy-filter-mlx-4bit-bench-v2.json`,
`/private/tmp/aidememo-privacy-filter-mlx-mxfp4-bench-v2.json`,
`/private/tmp/aidememo-privacy-filter-mlx-8bit-bench-v2.json`,
`/private/tmp/aidememo-privacy-filter-mlx-mxfp4-http-bench.json`, and
`/private/tmp/aidememo-privacy-write-mlx-mxfp4-bench.json`.

## Agentic Loop Calibration

`aidememo_aggregate` should be described as a deterministic arithmetic primitive, not
as a general accuracy lever. The stable product rule is:

| Question shape | Recommended path |
|---|---|
| "What did I say about X?" / "When did I last do Y?" / "What's my preference for Z?" | Answer from `aidememo_context`, `aidememo_query`, or `aidememo_search` snippets. |
| "How much total did I spend on X?" | `aidememo_aggregate(op=sum_currency)` |
| "How many hours/days total?" | `aidememo_aggregate(op=sum_duration)` |
| "How many distinct days had event X?" | `aidememo_aggregate(op=count_distinct_dates)` |
| "Timeline of all X events" | `aidememo_aggregate(op=timeline)` |
| "How many times did I decide/try X?" | `aidememo_aggregate(op=count)` or `op=enumerate` |

The earlier 60-question focused run showed a large multi-session gain when the
agent used aggregation. A later balanced LongMemEval-style run with 240
questions, MiniMax temperature 0, and a 3-run mean put the agentic-loop variant
within reader noise of the single-call baseline: roughly `-1.9pt` versus the
mean with `sigma ~= 1.1pt`. Forced agentic-loop dispatch also caused
single-fact SS-pref / temporal regressions because extra JSON tool-call
structure can disturb simple recall.

A single-shot "does this need aggregation?" classifier netted to baseline at
240 questions (`+0pt` mean, around 40% precision). The practical conclusion is
to expose `aidememo_aggregate` in reader prompts as insurance for counting, summing,
and timelines, while keeping normal recall on `aidememo_context` / `aidememo_query`.

## gbrain-evals Adapter

Fresh-checkout validation against `garrytan/gbrain-evals` commit `ef7794f`
on 2026-05-19:

| Adapter | Corpus Pages | Queries | P@5 | R@5 | Correct / Expected | Real Time |
|---|---:|---:|---:|---:|---:|---:|
| aidememo bm25 | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 63.38s |
| aidememo bm25 daemon | 240 | 145 | 17.4% | 64.1% | 125 / 261 | 11.04s |
| aidememo hybrid daemon | 240 | 145 | 16.7% | 62.5% | 121 / 261 | 45.64s |

Daemon BM25 preserves the score and cuts wall time by `5.7x`. Hybrid is
slower and slightly worse on this surface-form-heavy BrainBench slice; keep
semantic retrieval for paraphrase-heavy workloads.

## LongMemEval-S Retrieval

500 questions from the cleaned LongMemEval-S set, measured with the Rust
benchmark harness:

| Stack | R@1 | R@5 | R@10 | MRR |
|---|---:|---:|---:|---:|
| aidememo BM25-only | 0.866 | 0.952 | 0.974 | 0.902 |
| aidememo + time-decay soft bias | 0.858 | 0.958 | 0.978 | 0.898 |
| aidememo + bge-small-en-v1.5 | 0.914 | 0.976 | 0.986 | 0.941 |
| aidememo + bge + reranker K=10 | 0.938 | 0.984 | 0.986 | 0.957 |
| aidememo + bge + two-stage reranker K=20 -> 10 | 0.940 | 0.984 | 0.992 | 0.958 |

The retrieval ceiling is high: answer evidence lands in the top-10 set for
496/500 questions. Remaining E2E errors are mostly reader-side temporal or
multi-session reasoning, not missing evidence.

## LongMemEval-S E2E

LLM-graded with a `gpt-4o` judge:

| Stack | Reader | Overall |
|---|---|---:|
| Mem0 published baseline | gpt-4o | 49.0% |
| aidememo @ model2vec + decay | gpt-4o-mini | 60.0% |
| aidememo @ model2vec + decay | gpt-4o | 60.4% |
| aidememo @ bge + reranker K=20 -> 10 | gpt-4o-mini | 65.6% |
| aidememo @ bge + reranker K=20 -> 10 | gpt-4o | 67.6% |
| aidememo @ bge + reranker K=20 -> 10 | gpt-4.1 | 72.6% |
| aidememo @ bge + reranker K=20 -> 10 | MiniMax-M2.7-highspeed | 74.0% |
| Zep / Graphiti 2026 published | gpt-4o | 71.2% |
| Mastra published | gpt-4o | 84.2% |
| OMEGA published local | gpt-4.1 | 95.4% |

Use these numbers carefully. AideMemo should lead with deployment and temporal
memory ergonomics, not a SOTA claim.

## Model And Rerank Trade-offs

| Workload | Recommended path | Measured reason |
|---|---|---|
| Code/docs/news where query terms overlap answer text | default `model2vec` / BM25-hybrid | HotpotQA and MultiHop-RAG were saturated: model2vec and BGE both landed R@5 around 94-96%. |
| English paraphrase-heavy personal memory | `fastembed` + `bge-small-en-v1.5` | LongMemEval-S R@5 improved from 96.2% to 98.0%; SS-pref went 93.3 -> 100. |
| Korean / multilingual repositories | default multilingual model2vec | `bge-small-en-v1.5` is English-tuned. |
| Retrieval-bound search where top-K recall is low | enable rerank | MIRACL/ko improved MRR@10 by 5.8% and nDCG@10 by 4.6%. |
| Reader-bound agent loops where top-K already overlaps | keep rerank off | Cross-encoder rerank can add roughly 85x latency at top_k=8 with no E2E lift. |

HNSW is the default semantic index because it closed a Korean MIRACL candidate
drop caused by BM25 prefiltering while keeping query latency below brute-force
cosine on larger corpora.

### LFM ColBERT Sidecar Micro-eval

Command:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_colbert_eval.py \
  --aidememo target/debug/aidememo \
  --model LiquidAI/LFM2.5-ColBERT-350M \
  --candidate-limit 8
```

Result on a 6-question synthetic AideMemo store, with `trust_remote_code=false`:

| Model | Candidate recall | BM25 hit@1 | ColBERT hit@1 | BM25 MRR | ColBERT MRR | Mean BM25 search | Mean ColBERT rerank |
|---|---:|---:|---:|---:|---:|---:|---:|
| `LiquidAI/LFM2.5-ColBERT-350M` | 1.00 | 0.83 | 1.00 | 0.92 | 1.00 | 31 ms | 403 ms |
| `LiquidAI/LFM2-ColBERT-350M` | 1.00 | 0.67 | 0.83 | 0.83 | 0.92 | 31 ms | 258 ms |

Interpretation: LFM ColBERT is useful as a warmed sidecar reranker when BM25 or
hybrid retrieval already has high candidate recall but the head ordering is
wrong. In the LFM2.5 run, it fixed the "database migration finish faster" case
from rank 2 to rank 1. This does not solve first-stage misses; if the gold fact
is absent from the candidate pool, reranking cannot recover it. Do not load the
model per query in an agent loop. Keep it warm behind a sidecar/daemon and
reserve it for retrieval-bound, higher-stakes queries where a few hundred
milliseconds is acceptable.

### LFM Dense Embedding Scenario Checks

Command:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_dense_eval.py \
  --aidememo target/debug/aidememo \
  --model LiquidAI/LFM2.5-Embedding-350M \
  --candidate-limit 8
```

Result with `trust_remote_code=false`:

| Path | Embedding health | Candidate recall | BM25 hit@1 | Dense hit@1 | Dense rerank hit@1 | Mean query embed |
|---|---|---:|---:|---:|---:|---:|
| `sentence-transformers`, remote code off | invalid: all document embeddings identical (`max_pairwise_diff_from_first=0.0`) | 0.86 | 0.71 | 0.29 | 0.71 | 249 ms |
| `mlx-community/LFM2.5-Embedding-350M-4bit` | valid: document embeddings differ (`max_pairwise_diff_from_first=0.195`) | 0.86 | 0.57 | 1.00 | 0.86 | 23 ms |
| `mlx-community/LFM2.5-Embedding-350M-8bit` | valid: document embeddings differ (`max_pairwise_diff_from_first=0.208`) | 0.86 | 0.57 | 0.86 | 0.86 | 21 ms |

The safe `sentence-transformers` path is not usable for AideMemo quality
claims: it returns a 1024-dimensional vector, but every tested document vector
was identical. The model config points `AutoModel` at
`modeling_lfm2_bidirectional.Lfm2BidirectionalModel`, and Liquid's model card
states that `trust_remote_code=True` applies the bidirectional encoder patches.
That code was downloaded for inspection only: 139 lines, importing only
`torch`, `torch.nn.functional`, and `transformers.models.lfm2`, with no
`os`/`subprocess`/`socket`/`requests`/`eval`/`exec` usage found. Executing it
still requires explicit operator approval because `trust_remote_code` runs Hub
Python in the local process.

MLX dense command:

```bash
hf download mlx-community/LFM2.5-Embedding-350M-4bit \
  --local-dir /private/tmp/lfm25-embedding-mlx-4bit

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_dense_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --candidate-limit 8
```

The MLX 4-bit run is the useful dense result. In the July 6 rerun, it improved
the synthetic scenario set from BM25 hit@1 0.57 / MRR 0.71 to dense hit@1 1.00
/ MRR 1.00, with a mean warmed query embedding latency of about 23 ms. The
8-bit run did not improve this micro-eval (`dense_hit1=0.86`,
`dense_mrr=0.93`); document encoding was similar in the rerun (130-132 ms for
the 12-document fixture), so the placement decision is quality-driven rather
than latency-driven.
For Mac-local experimentation, 4-bit is the better default until a larger
quality gate shows an 8-bit lift. The 4-bit run also recovered
the Korean query "레디스 타임아웃의 원인이 뭐였지" as dense rank 1 when BM25 returned
no candidates. Dense rerank over BM25 candidates cannot recover that case
because the first-stage candidate set is empty; this is evidence for using LFM
dense as a first-stage semantic retrieval path, not only as a reranker.

### LFM Project-Docs Candidate Recall

The larger July 7 gate moved beyond the tiny synthetic fixture and used
`scripts/lfm_mlx_docs_recall_eval.py` against the tracked AideMemo Markdown
corpus: `README.md`, `AGENTS.md`, the tracked docs files, `scripts/README.md`,
and the agent SDK README. The script chunks the real docs, validates each gold
query against a chunk needle, imports the corpus into a temporary AideMemo
store, and measures `R@8` / hit@1 / MRR for 32 surface, paraphrase, and Korean
cross-lingual queries.

Command:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_docs_recall_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --summary-only
```

July 8 rerun after forcing the lexical baseline through
`aidememo search --bm25-only`; the tracked docs had grown to 335 chunks:

| Path | R@8 | Hit@1 | MRR | Notes |
|---|---:|---:|---:|---|
| BM25 | 0.656 | 0.312 | 0.446 | Strong on surface-overlap (`R@8=0.929`) and still decent on paraphrase (`0.727`), but zero on the Korean query slice. |
| model2vec + HNSW via prewarmed daemon | 0.750 | 0.250 | 0.410 | Recovers some Korean/cross-lingual misses (`3/7`) but hurts head ordering on surface queries. |
| LFM 4-bit pure dense all-doc rank | 0.625 | 0.250 | 0.349 | Worse than BM25 overall: `R@8 -0.031`, hit@1 `-0.062`, MRR `-0.097`. Not a default embedding replacement. |
| LFM 4-bit rerank of BM25 candidates | 0.656 | 0.250 | 0.391 | Cannot recover first-stage misses and worsens head ordering. |
| BM25 + LFM only when the BM25 gate promotes | 0.812 | 0.344 | 0.499 | Promoted only the 7 CJK queries; `R@8 +0.156`, hit@1 `+0.031`, MRR `+0.054` over BM25 while leaving the 25 BM25-confident queries untouched. |

Latency notes from the same run: LFM model load was 4.40s, document encoding was
45.47s for 335 chunks, warmed query embedding averaged 27.02ms, and dense
scoring averaged 1.24ms. model2vec HNSW rebuild took 9.16s and daemon start took
7.77s. The BM25 latency in this script includes fresh CLI startup per query, so
do not compare it directly to in-process or daemon latency; this gate is
primarily a quality/candidate-recall test.

Interpretation: the larger real-docs gate confirms the placement boundary. LFM
4-bit should not replace the default embedding model globally, and LFM dense
rerank is the wrong fix when BM25 misses the candidate entirely. The useful
shape is still `search.auto_hybrid=true` plus daemon prewarm, with LFM-style
semantic retrieval only at the lexical failure point. The next external gate is
a true LongMemEval or MIRACL-style candidate-recall run once the larger datasets
are available locally.

Implementation follow-up: `scripts/lfm_mlx_docs_recall_eval.py` now accepts
external corpus JSONL plus query/qrels files, so larger LongMemEval/MIRACL-style
candidate-recall gates can run without changing the script. Use
`--no-default-docs --corpus-jsonl corpus.jsonl --queries-jsonl queries.jsonl
--qrels-tsv qrels.tsv --max-cases N`. The BM25 baseline path is forced through
`aidememo search --bm25-only` so the default `auto_hybrid` policy does not
contaminate lexical baseline rows.

### LFM MLX ColBERT Scenario Checks

Native MLX MaxSim was checked with the 4-bit conversion:

```bash
hf download mlx-community/LFM2.5-ColBERT-350M-4bit \
  --local-dir /private/tmp/lfm25-colbert-mlx-4bit

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_colbert_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-colbert-mlx-4bit \
  --candidate-limit 8 \
  --summary-only
```

Result on the same synthetic AideMemo fixture:

| Path | Candidate recall | BM25 hit@1 | ColBERT rerank hit@1 | ColBERT all-doc hit@1 | BM25 MRR | ColBERT rerank MRR | Mean query encode | Mean rerank scoring |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `mlx-community/LFM2.5-ColBERT-350M-4bit` | 0.86 | 0.57 | 0.86 | 1.00 | 0.71 | 0.86 | 23 ms | 0.18 ms |

Interpretation: MLX ColBERT works locally and is much more operationally
attractive than the earlier PyLate sidecar for per-query scoring once document
token vectors are precomputed. It still has a different cost profile from dense
single-vector retrieval: document token encoding was about 1.87 s for the tiny
12-document fixture, versus 132 ms for dense 4-bit. That makes it a strong
candidate for a warmed sidecar / offline index, not an in-process per-query
model load. It is useful for ranking, and small-corpus brute-force ColBERT can
recover first-stage misses, but scaling that requires a real multi-vector index
design rather than pretending it is an HNSW vector swap.

Safer GGUF path checked:

```bash
hf download LiquidAI/LFM2.5-Embedding-350M-GGUF \
  LFM2.5-Embedding-350M-Q4_0.gguf \
  --local-dir /private/tmp/lfm25-embedding-gguf
```

The Q4_0 GGUF (about 219 MB) downloaded successfully and exposes a
1024-dimensional LFM2 embedding model. In this sandbox, `llama-cpp-python`
failed to create a context because the local build still initialized Metal and
could not create a command queue, even after a CPU-only rebuild attempt. The
operationally preferred dense path remains an isolated `llama-server
--embeddings` or TEI/OpenAI-compatible embedding server, then AideMemo's
`model.query_prefix="query: "` / `model.document_prefix="document: "` config.
Dense is worth validating further for first-stage multilingual/paraphrase
recall, not as a per-query Python model load inside the agent process.

### LFM MLX Text-Generation Control Tasks

MLX text-generation models were evaluated on AideMemo control tasks with
`scripts/lfm_mlx_lm_eval.py`: fact extraction/type classification, query
routing, and consolidation decisions. The evaluation asks for JSON only and
scores exact labels. `LiquidAI/LFM2.5-230M-MLX-4bit` and
`LiquidAI/LFM2.5-350M-MLX-4bit` required a local tokenizer metadata workaround:
their `tokenizer_config.json` names `TokenizersBackend`, while the current
`mlx-lm` / `transformers<5` runtime loads the same `tokenizer.json` correctly
when the class is treated as `PreTrainedTokenizerFast`. The script applies this
patch in a temporary symlinked directory and does not modify the downloaded
model repo.

Representative commands:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_lm_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --suite all \
  --prompt-style compact \
  --summary-only

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_lm_eval.py \
  --model-dir /private/tmp/lfm25-230m-mlx-4bit \
  --suite all \
  --prompt-style fewshot \
  --summary-only
```

Best observed score per model family in this micro-eval:

| Model | Prompt style | Mean latency | Extraction fact_type | Extraction entity recall | Router accuracy | Consolidation accuracy | Placement |
|---|---|---:|---:|---:|---:|---:|---|
| `LFM2.5-230M-MLX-4bit` | fewshot | 93 ms | 0.25 | 0.48 | 0.38 | 0.50 | Too inaccurate for automatic writes; possible cheap review signal only. |
| `LFM2.5-350M-MLX-4bit` | compact/fewshot best-of-suite | 92-105 ms | 0.00 | 0.52 | 0.38 | 0.25 | Not useful enough for extraction; router still worse than deterministic rules. |
| `LFM2.5-1.2B-Instruct-MLX-4bit` | compact | 324 ms | 0.63 | 0.88 | 0.00 | 0.00 | Best local extraction helper; keep pending-review only. |
| `LFM2.5-1.2B-Instruct-MLX-4bit` | fewshot | 300 ms | 0.38 | 0.54 | 0.25 | 0.25 | Still not strong enough for routing or automatic consolidation. |
| `LFM2.5-1.2B-Thinking-MLX-4bit` | compact | 664 ms | 0.00 | 0.00 | 0.00 | 0.00 | Not suitable for JSON control tasks; it emits reasoning traces. |

The useful placement is narrower than the model inventory suggests:

* Use `LFM2.5-1.2B-Instruct-MLX-4bit` only as a local pending-review extraction
  helper when large-model calls are undesirable. It can propose entities well,
  but `fact_type` accuracy is not high enough for durable automatic writes.
* Do not use the tested LFM text-generation models as AideMemo's query router.
  The deterministic route table plus search-confidence signals remain cheaper
  and more accurate.
* Do not use the tested LFM text-generation models for automatic consolidation
  or branch merge decisions. Even the best result was only 0.50 on a 4-case
  consolidation slice, which is enough to justify "review signal" but not
  "write decision."
* `LFM2.5-1.2B-Thinking-MLX-4bit` needs long reasoning output before an answer;
  at 256 tokens it still produced no valid JSON in the consolidation smoke and
  averaged 1.57 s. Keep it out of AideMemo control paths until a different
  prompt/runtime proves otherwise.

### LFM MLX Closed-Label Fact Type Classifier

The user-visible weakness in capture is narrower than free-form extraction:
`fact_type` is a closed-label classification problem. To isolate that from JSON
generation quality, `scripts/lfm_mlx_fact_type_eval.py` scores the likelihood of
the nine allowed labels (`preference`, `decision`, `lesson`, `error`,
`convention`, `pattern`, `claim`, `note`, `question`) and compares the best
label against AideMemo's deterministic strong-cue baseline.

Fixture: 45 hand-labelled cases, balanced 5 per label, mixing explicit cues,
weak cues, passive `note` examples, and Korean cases that the English cue table
usually leaves as `note`. The baseline column mirrors
`aidememo_core::extract::infer_fact_type`; it scored 0.69 overall on this
fixture.

Representative commands:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --prompt-style compact \
  --summary-only

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-350m-mlx-4bit \
  --prompt-style compact \
  --template plain \
  --summary-only
```

| Model | Prompt/template | Accuracy | Baseline accuracy | Mean latency | Residual rescues | If trusted everywhere |
|---|---|---:|---:|---:|---:|---|
| `LFM2.5-230M-MLX-4bit` | compact/chat | 0.11 | 0.69 | 151 ms | 1/13 | Harms 27 baseline-correct cases; collapses mostly to `preference`. |
| `LFM2.5-230M-MLX-4bit` | compact/plain | 0.11 | 0.69 | 125 ms | 1/13 | Same `preference` prior collapse; not a classifier. |
| `LFM2.5-350M-MLX-4bit` | compact/chat | 0.27 | 0.69 | 168 ms | 3/13 | Harms 22 baseline-correct cases; over-predicts `pattern`. |
| `LFM2.5-350M-MLX-4bit` | compact/plain | 0.18 | 0.69 | 138 ms | 3/13 | Still over-predicts `pattern`; confidence is not useful. |
| `LFM2.5-1.2B-Instruct-MLX-4bit` | compact/chat | 0.33 | 0.69 | 735 ms | 3/13 | Harms 19 baseline-correct cases; useful only as a narrow high-confidence hint. |
| `LFM2.5-1.2B-Instruct-MLX-4bit` | compact/plain | 0.22 | 0.69 | 607 ms | 1/13 | Worse than chat; no automatic placement. |
| `LFM2.5-1.2B-Thinking-MLX-4bit` | compact/chat | 0.11 | 0.69 | 734 ms | 1/13 | Collapses mostly to `preference`; keep out of capture control paths. |

The best 1.2B-Instruct run had one narrow positive: at `confidence >= 0.8`, it
accepted 6/45 cases and got 6/6 correct. Those cases were mostly clear
`preference`, `lesson`, and labelled `convention` examples, including Korean
preference / lesson cases that deterministic English cues missed. That is not
enough coverage for automatic capture, but it is enough to justify this
placement:

* Keep deterministic inference as the default `fact_add` path.
* Run LFM only on residual candidates: omitted `fact_type` that defaulted to
  `note`, explicit `note` with suspicious content, or pending-review batches.
* Treat LFM output as `fact_type_hint` with confidence/margin metadata. Promote
  automatically only after a larger false-memory-sensitive benchmark proves a
  high-precision threshold.
* Do not place 230M/350M LFM text models in this path; their latency is lower
  but their label prior collapse makes their hints actively risky.

### LFM 1.2B Fact Type LoRA Smoke

Because `fact_type` is a fixed small label set, a narrow supervised adapter is a
better fit than asking the base model to infer the taxonomy zero-shot. The first
local smoke used `LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit` with MLX LoRA:

```bash
python3 scripts/lfm_fact_type_sft_data.py \
  --out /private/tmp/aidememo-lfm-fact-type-sft \
  --examples-per-label 80

/private/tmp/aidememo-lfm-venv/bin/mlx_lm.lora \
  --model /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --train \
  --data /private/tmp/aidememo-lfm-fact-type-sft \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-lora-20260707-240i \
  --fine-tune-type lora \
  --mask-prompt \
  --num-layers 8 \
  --batch-size 1 \
  --grad-accumulation-steps 8 \
  --iters 240 \
  --learning-rate 5e-5 \
  --steps-per-eval 60 \
  --val-batches 16 \
  --max-seq-length 512

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-lora-20260707-240i \
  --prompt-style compact \
  --summary-only
```

Training data: 576 synthetic training rows and 144 validation rows generated
from AideMemo-specific templates; test data stayed the separate 45-case
hand-labelled fixture from `lfm_mlx_fact_type_eval.py`. This is enough to test
whether the adapter path can move the model, but not enough to claim production
generalisation.

| Run | Validation loss | Holdout accuracy | Baseline accuracy | Residual rescues | Baseline-correct harms | High-confidence precision |
|---|---:|---:|---:|---:|---:|---:|
| Base `1.2B-Instruct` compact/chat | n/a | 0.33 | 0.69 | 3/13 | 19 | 6/6 at confidence >= 0.8 |
| LoRA 60 iter, 8 layers | 2.580 | 0.49 | 0.69 | 5/13 | 15 | 11/16 at confidence >= 0.8 |
| LoRA 240 iter, 8 layers | 1.476 | 0.84 | 0.69 | 10/13 | 4 | 31/31 at confidence >= 0.8 |

The 240-iteration adapter is the first positive evidence that fine-tuning can
make LFM useful for AideMemo's capture weakness. It improves `claim`,
`question`, Korean preference/lesson-style cases, and non-English residuals
that the deterministic English cue table misses. The remaining weak spot is
`pattern` (1/5): the adapter confuses implementation-pattern statements with
`convention`, `claim`, and `decision`.

Placement after this smoke:

* Keep deterministic inference as the zero-cost default.
* A LoRA-tuned LFM sidecar is worth pursuing for residual/default-note cases and
  pending-review batches.
* Do not ship automatic type promotion from this synthetic-only adapter. The
  next gate is a real capture corpus labelled by the nine AideMemo fact types,
  with explicit false-memory and baseline-correct-harm metrics.

### Coding-Agent Shadow Corpus Fact Type LoRA

The next step was to move the adapter test away from synthetic-only examples and
toward the data shape AideMemo should collect in production: coding agents or
review hooks classify a fact before `aidememo_fact_add`, the reviewed label is
saved, and a local LFM LoRA is trained in shadow mode.

Seed corpus: `fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl`.

* 108 reviewed-style coding-agent facts, balanced at 12 rows per fact type.
* Splits are explicit: 72 train, 18 validation, 18 test.
* Mixed language coverage: 79 English rows, 29 Korean rows.
* Scenarios cover code review, release readiness, MCP capture, branch merge,
  model runtime failures, search evaluation, corpus design, and taxonomy notes.

Dataset generation:

```bash
python3 scripts/lfm_fact_type_sft_data.py \
  --out /private/tmp/aidememo-lfm-fact-type-corpus-sft

python3 scripts/lfm_fact_type_sft_data.py \
  --out /private/tmp/aidememo-lfm-fact-type-corpus-synth40-sft \
  --examples-per-label 40
```

Representative training command:

```bash
/private/tmp/aidememo-lfm-venv/bin/mlx_lm.lora \
  --model /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --train \
  --data /private/tmp/aidememo-lfm-fact-type-corpus-sft \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora-20260707-240i \
  --fine-tune-type lora \
  --mask-prompt \
  --num-layers 8 \
  --batch-size 1 \
  --grad-accumulation-steps 8 \
  --iters 240 \
  --learning-rate 5e-5 \
  --steps-per-eval 60 \
  --val-batches -1 \
  --max-seq-length 512
```

Evaluation command:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora-20260707-240i \
  --cases-file fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl \
  --case-split test \
  --prompt-style compact \
  --summary-only
```

Results from the July 7 Mac MLX run:

| Run | Eval set | Accuracy | Baseline accuracy | Residual rescues | Baseline-correct harms | High-confidence precision |
|---|---|---:|---:|---:|---:|---:|
| Base `1.2B-Instruct` | 18-case corpus test | 0.11 | 0.39 | n/a | n/a | 0/1 at confidence >= 0.8 |
| Corpus-only LoRA 240 iter | 18-case corpus test | 0.61 | 0.39 | 5/11 | 1 | 11/14 at confidence >= 0.8 |
| Corpus-only LoRA 240 iter | 45-case original holdout | 0.82 | 0.69 | 10/13 | 4 | 33/38 at confidence >= 0.8 |
| Corpus + 40 synthetic/label LoRA 240 iter | 18-case corpus test | 0.61 | 0.39 | 6/11 | 2 | 7/8 at confidence >= 0.8 |
| Corpus + 40 synthetic/label LoRA 240 iter | 45-case original holdout | 0.71 | 0.69 | 9/13 | 8 | 24/26 at confidence >= 0.8 |

July 8 sidecar-threshold rerun:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora-20260707-240i \
  --input-jsonl fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-fact-type-hints-test.jsonl

python3 scripts/lfm_fact_type_threshold_eval.py \
  --labels-jsonl fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl \
  --label-split test \
  --predictions-jsonl /private/tmp/aidememo-fact-type-hints-test.jsonl
```

With a strict zero-baseline-harm gate, no threshold was viable on the 18-case
test split. Relaxing the gate to allow one baseline-correct harm selected
`confidence >= 0.80`: 14/18 hints accepted, 11 correct, 3 incorrect, precision
0.786, false-memory rate 0.214, residual rescues 5, baseline-correct harms 1,
net rescue-minus-harm +4. The single harm was a high-confidence `note ->
pattern`, so confidence alone is not yet sufficient for automatic writes.

July 8 real-log probe:

```bash
python3 scripts/lfm_fact_type_log_fixture.py \
  --out-dir /private/tmp/aidememo-lfm-log-probes-v2 \
  --max-rows 72 \
  --max-per-label 12

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora-20260707-240i \
  --input-jsonl fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl \
  --input-jsonl /private/tmp/aidememo-lfm-log-probes-v2/agentstep_fact_type_probe.jsonl \
  --input-jsonl /private/tmp/aidememo-lfm-log-probes-v2/hermes_fact_type_probe.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-lfm-log-probes-v2/fact_type_hints_all_101.jsonl
```

`lfm_fact_type_log_fixture.py` produces weak-labelled probes from local
AgentStep traces and Hermes session logs without copying raw logs into the repo.
These are not reviewed labels; they are a stress test for behavior-log-shaped
text. The sidecar used only local MLX scoring, with no external LLM calls. On
101 rows, per-row scoring averaged 1092.79ms, p50 884.59ms, max 3759.32ms
because some raw tool evidence rows are long.

At `confidence >= 0.80`, no margin filter:

| Dataset | Rows | Baseline acc. | LFM acc. | Accepted | Precision | False-memory rate | Residual rescues | Baseline-correct harms | Net |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Reviewed coding-agent test | 18 | 0.389 | 0.611 | 14/18 | 0.786 | 0.214 | 5 | 1 | +4 |
| AgentStep trace weak probe | 39 | 0.333 | 0.462 | 31/39 | 0.516 | 0.484 | 6 | 3 | +3 |
| Hermes session weak probe | 44 | 0.545 | 0.636 | 33/44 | 0.788 | 0.212 | 7 | 4 | +3 |
| Combined | 101 | 0.436 | 0.564 | 78/101 | 0.679 | 0.321 | 18 | 8 | +10 |

Higher thresholds helped the Hermes-shaped data but not raw AgentStep route
events. Hermes reached precision 0.852 at `confidence >= 0.90` (27/44 accepted,
7 rescues, 2 harms) and 0.875 at `confidence >= 0.98` (16/44 accepted, 7
rescues, 1 harm). AgentStep stayed below 0.80 precision even at `confidence >=
0.98` because route/cache rows such as "target action call_tool" and
"cache-hit behavior" sit on ambiguous `decision` / `lesson` / `error` boundaries.

Interpretation: lowering the visible hint threshold to 0.80 is useful for
shadow review and UI surfacing when the input is already a candidate memory
fact, but it is still not safe for automatic fact_type override. For raw
behavior traces, first convert events into explicit memory-candidate facts or
keep them as `note` / trace metadata; do not ask the LoRA classifier to infer a
durable memory type directly from every route/tool event.

July 8 Hugging Face public-trace probe:

```bash
python3 scripts/lfm_fact_type_hf_probe.py \
  --out-dir /private/tmp/aidememo-lfm-hf-probes-v3 \
  --source-rows 100 \
  --max-rows-per-dataset 100 \
  --max-per-label 25

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora-20260707-240i \
  --input-jsonl /private/tmp/aidememo-lfm-hf-probes-v3/combined_hf_fact_type_probe.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-lfm-hf-probes-v3/fact_type_hints_hf_254.jsonl
```

Datasets:

| Probe | Hugging Face dataset | Config / split | Source rows | Probe rows |
|---|---|---|---:|---:|
| Hermes | `lambda/hermes-agent-reasoning-traces` | `kimi` / `train` | 100 | 88 |
| TauBench | `sammshen/taubench-sonnet-traces` | `default` / `train` | 100 | 66 |
| SWE-smith | `SWE-bench/SWE-smith-trajectories` | `default` / `tool` | 100 | 100 |
| Combined | above | above | 300 | 254 |

`lfm_fact_type_hf_probe.py` fetches public Dataset Viewer rows, redacts emails
and long IDs, and emits compact weak-labelled candidate-memory rows. These are
structural labels, not reviewed truth. They intentionally stress the sidecar on
raw agent trace shapes: tool calls, tool observations, task prompts, policies,
and outcome metadata. Local MLX sidecar scoring for the 254-row combined file
averaged 1261.80ms per row, p50 1019.12ms, max 4173.06ms.

At `confidence >= 0.80`, no margin filter:

| Dataset | Rows | Baseline acc. | LFM acc. | Accepted | Precision | False-memory rate | Residual rescues | Baseline-correct harms | Net |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Hermes HF trace weak probe | 88 | 0.239 | 0.239 | 55/88 | 0.273 | 0.727 | 10 | 9 | +1 |
| TauBench HF trace weak probe | 66 | 0.121 | 0.348 | 43/66 | 0.419 | 0.581 | 16 | 0 | +16 |
| SWE-smith HF trace weak probe | 100 | 0.180 | 0.210 | 62/100 | 0.242 | 0.758 | 11 | 7 | +4 |
| Combined HF trace weak probe | 254 | 0.185 | 0.256 | 160/254 | 0.300 | 0.700 | 37 | 16 | +21 |

Raising the threshold alone did not make raw traces safe. On the 254-row
combined file, `confidence >= 0.98` accepted 71 rows with precision 0.352,
false-memory rate 0.648, 19 rescues, and 11 harms. The accepted-label confusion
explains the failure mode: raw `decision` tool-call rows were usually predicted
as `pattern`, raw `question` user prompts often looked like `preference`, and
raw `note` tool observations sometimes looked like `error`.

The same run becomes much more useful after deterministic extraction has
already filtered the stream down to durable memory-candidate labels
(`preference`, `lesson`, `error`, `convention`, `pattern`, `claim`) and left
raw `decision` / `note` / `question` events as trace metadata.

High-signal subset: 81 rows (`error=4`, `convention=23`, `pattern=12`,
`preference=1`, `claim=21`, `lesson=20`).

| Gate | Accepted | Precision | False-memory rate | Residual rescues | Baseline-correct harms | Net |
|---|---:|---:|---:|---:|---:|---:|
| `confidence >= 0.80` | 54/81 | 0.556 | 0.444 | 30 | 0 | +30 |
| `confidence >= 0.95` | 32/81 | 0.781 | 0.219 | 25 | 0 | +25 |
| `confidence >= 0.98` | 19/81 | 0.947 | 0.053 | 18 | 0 | +18 |

Placement after the public-trace gate: do not run the LFM classifier directly
over every raw trace event, and do not treat `confidence >= 0.80` as an
automatic write threshold. The useful long-term path is a two-stage local loop:
deterministically extract/filter candidate memories first, then let the LoRA
sidecar add high-confidence `fact_type_hint` values for pending review or UI
surfacing. For automatic type promotion, the next threshold to test on reviewed
traffic is closer to `confidence >= 0.98` on high-signal candidates, not 0.80
on raw traces.

July 8 expanded Hugging Face multi-model gate:

The larger public-trace probe fetched 300 source rows from each of three HF
agent-trace datasets and produced 539 compact weak-labelled candidate-memory
rows:

| HF source | Source rows | Probe rows |
|---|---:|---:|
| `lambda/hermes-agent-reasoning-traces` (`kimi` / `train`) | 300 | 165 |
| `sammshen/taubench-sonnet-traces` (`default` / `train`) | 300 | 134 |
| `SWE-bench/SWE-smith-trajectories` (`default` / `tool`) | 300 | 240 |

Fact-type sidecar on the expanded high-signal subset
(`error=18`, `convention=25`, `pattern=13`, `preference=4`, `claim=50`,
`lesson=45`; 155 rows):

| Gate | Accepted | Precision | False-memory rate | Baseline-correct harms | Residual rescues | Mean / p50 latency |
|---|---:|---:|---:|---:|---:|---:|
| `confidence >= 0.95` | 56/155 | 0.839 | 0.161 | 0 | 47 | 1198 / 958 ms |
| `confidence >= 0.98` | 39/155 | 0.923 | 0.077 | 0 | 36 | 1198 / 958 ms |

Raw prediction accuracy was 0.503 against weak labels, versus deterministic
baseline accuracy 0.006 on this intentionally high-signal subset. The 0.98 gate
still misses the desired 0.95 precision bar, but the false-memory rate is low
enough for pending review / UI hinting and again shows why automatic writes
need reviewed production labels before promotion.

For retrieval, `scripts/lfm_hf_agent_trace_retrieval_fixture.py` converted the
same HF probe rows into BEIR/MIRACL-style corpus, query, and qrels files. The
balanced large retrieval slice used 180 documents (60 per HF source) and 540
queries: `surface`, deterministic paraphrase, and Korean/CJK-style wrappers.

| Method | R@8 | Hit@1 | MRR | Latency / setup |
|---|---:|---:|---:|---|
| BM25 | 0.991 | 0.839 | 0.891 | 59.64 ms/query fresh CLI |
| model2vec + HNSW daemon | 0.991 | 0.694 | 0.794 | 103.78 ms/query; rebuild 8.11s; daemon start 7.50s |
| LFM 350M pure dense | 0.887 | 0.557 | 0.672 | query embed 25.64 ms; score 0.13 ms; doc encode 10.40s |
| LFM rerank of BM25 candidates | 0.991 | 0.656 | 0.773 | cannot recover BM25 candidate misses |
| BM25 + guarded LFM auto | 0.993 | 0.839 | 0.892 | promoted 2/540 weak BM25 cases |

Scenario detail from the 180-doc slice:

| Scenario | BM25 R@8 / Hit@1 | LFM dense R@8 / Hit@1 | Guarded auto R@8 / Hit@1 |
|---|---:|---:|---:|
| surface | 1.000 / 0.944 | 0.956 / 0.761 | 1.000 / 0.944 |
| paraphrase | 0.983 / 0.756 | 0.872 / 0.494 | 0.989 / 0.756 |
| CJK-style wrapper | 0.989 / 0.817 | 0.833 / 0.417 | 0.989 / 0.817 |

The first attempt used the older "CJK always promotes" simulation and dropped
auto-Hybrid MRR from 0.891 to 0.784 because these HF trace CJK queries still
contained strong English tool/file anchors. AideMemo now guards CJK promotion:
CJK can promote more eagerly when BM25 evidence is not strong, but strong BM25
matches stay lexical.

BGE / BERT-family baseline: the CLI now forwards `--features fastembed` to
`aidememo-core/fastembed`, so `bge-small-en-v1.5` can be tested through the
same HNSW semantic path. The full 180-doc BGE loop was manually aborted after
more than 7 minutes in the fastembed daemon query loop. A smaller balanced
60-doc / 180-query slice completed:

| Method | R@8 | Hit@1 | MRR | Latency / setup |
|---|---:|---:|---:|---|
| BM25 | 0.989 | 0.850 | 0.909 | 38.05 ms/query fresh CLI |
| model2vec + HNSW daemon | 0.989 | 0.756 | 0.854 | 57.81 ms/query; rebuild 8.03s |
| LFM 350M pure dense | 0.978 | 0.633 | 0.755 | query embed 30.87 ms; doc encode 3.31s |
| `fastembed` BGE-small-en-v1.5 | 0.994 | 0.833 | 0.896 | 1680.30 ms/query; rebuild 2.68s |

BGE had the best R@8 on the small slice and matched BM25 on surface queries,
but it was slower by one to two orders of magnitude on repeated daemon queries
and did not beat BM25 on Hit@1 / MRR. Treat it as an offline or high-stakes
reranking/semantic comparison baseline, not the default hot path.

Interpretation:

* The corpus-only adapter is the best current placement: it beats deterministic
  inference on the harder coding-agent corpus test and still generalizes to the
  older 45-case holdout.
* Synthetic augmentation is not automatically helpful. It made the classifier
  more selective at high confidence on the 18-case corpus test, but it hurt the
  older holdout and collapsed every `note` case there into `lesson`.
* The seed test set is still too small for automatic writes. The right product
  path is a shadow loop: collect reviewed agent-labelled facts, retrain local
  LoRA adapters, and expose only high-confidence `fact_type_hint` output until
  real capture traffic proves low false-memory risk.
* The weakest labels remain `claim`, `convention`, and `note` because their
  boundary is semantic rather than cue-driven. Future corpora should oversample
  these near-miss pairs instead of adding more obvious preference/decision
  templates.

Implementation follow-up: MCP writes now support an opt-in
`AIDEMEMO_FACT_TYPE_SHADOW_LOG=/path/to/file.jsonl` side-channel. It appends
successful `aidememo_fact_add` / `aidememo_fact_add_many` writes with
`label_source` metadata so agent-explicit labels can become supervised LoRA
training rows while inferred/default labels and `fact_type_hint` disagreements
remain audit data by default. The companion `scripts/lfm_fact_type_sidecar.py`
loads the tuned adapter and returns hint-only `suggested_fact_type` /
confidence / margin output; it does not write or override the stored fact.

### LFM MLX Vision Smoke

`LiquidAI/LFM2.5-VL-450M-MLX-4bit` downloaded and loaded only after a runtime
compatibility patch: `mlx-vlm` swaps in a slow SigLIP image processor, but the
current `transformers` processor class check rejects that object. Relaxing that
check in the one-off smoke allowed generation to run. The model saw a synthetic
incident-note screenshot title, but JSON output degenerated into repeated `{`
tokens, and simpler OCR prompts returned empty text. This is not production
evidence for screenshot-to-fact capture. Treat VL capture as blocked pending a
clean `mlx-vlm` / `transformers` compatibility path and a real screenshot
precision benchmark.

### LFM Model Placement Strategy

The useful result is not "replace the memory system with LFM." The durable
strategy is to put small LFM models at the exact memory-system failure point
they address, then measure quality and latency per layer.

| Layer | Candidate model | Current evidence | Next quality gate |
|---|---|---|---|
| First-stage semantic retrieval | `mlx-community/LFM2.5-Embedding-350M-4bit` | Tiny fixture: dense hit@1 1.00 / MRR 1.00 and Korean BM25-miss recovery. Larger tracked-docs gate: pure dense is not a default replacement (`R@8=0.688`, hit@1 0.219), but BM25-gated LFM improved over BM25 (`R@8 0.656 -> 0.812`) by promoting only the CJK failure slice. HF agent-trace retrieval confirmed the boundary: pure LFM dense underperformed BM25 (`R@8 0.887 vs 0.991` on 180 docs / 540 queries), while guarded auto stayed effectively neutral and promoted only 2 weak BM25 cases. | Keep `search.auto_hybrid=true` as a BM25-first guarded path. Run true candidate-recall tests where BM25 recall is materially below 1.0 before expanding LFM promotion. |
| Rerank after lexical/hybrid search | `mlx-community/LFM2.5-ColBERT-350M-4bit` | Native MLX MaxSim rerank improved hit@1 from 0.57 to 0.86; all-doc brute-force reached 1.00 on the tiny fixture, but document token encoding is much heavier than dense. | Gate on `candidate_recall >= 0.95`, then design a warmed sidecar or multi-vector index before using beyond small stores. |
| Local capture classification | `LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit` with a fact-type LoRA adapter | Zero-shot closed-label scoring trailed deterministic inference (0.33 vs 0.69 on the original holdout; 0.11 vs 0.39 on the coding-agent corpus test). A corpus-only 240-iteration LoRA reached 0.61 on the 18-case coding-agent test and 0.82 on the older 45-case holdout. On expanded HF high-signal traces, `confidence >= 0.98` accepted 39/155 hints at precision 0.923 with 0 baseline-correct harms. | Keep it as a shadow `fact_type_hint` path. Grow reviewed capture logs and validate confidence+margin thresholds before any automatic type promotion. |
| Query router | Deterministic rules and search confidence, not LFM LM | Tested MLX LMs were weak routers: best observed route accuracy was 0.38 for 230M/350M and 0.25 for 1.2B-Instruct. | Add route-confidence features to the existing deterministic router shape before spending model calls here. |
| Batch consolidation / branch review | No automatic LFM placement yet | Best observed consolidation was 0.50 on a 4-case slice with 230M fewshot; 1.2B-Instruct stayed at 0.25 and Thinking emitted reasoning traces instead of valid JSON. | Treat LFM output as review hints only; require a larger duplicate/supersede/keep-distinct benchmark before automatic writes. |
| Visual memory capture | Blocked for now | `LFM2.5-VL-450M-MLX-4bit` required a runtime processor compatibility patch and did not produce stable JSON/OCR in the smoke. | Re-test after a clean `mlx-vlm` compatibility path, then build a screenshot/diagram-to-fact precision benchmark. |
| Audio memory capture | No MLX placement yet | Current checked LFM audio inventory did not provide a Mac-local MLX path comparable to the text/VL candidates. | Keep audio out of core retrieval until there is a local runtime plus WER/fact-precision evidence. |

This is the long-term differentiation from generic memory systems: AideMemo can
be an instrumented memory runtime where small local models are proven useful by
scenario gates. The claim should be specific: dense LFM helps when lexical
candidate generation fails; ColBERT helps when candidate recall is high but
ranking is wrong; text-generation LFM helps if it improves capture hygiene or
routing while reducing large-model calls. A result that shows "no lift" on a
saturated workload is still useful because it tells the router to stay on the
cheap path.

Default-on check for `search.auto_hybrid`:

| Environment | Query shape | Result |
|---|---|---|
| Fresh HOME, no model cache | strong lexical `Redis timeout` | Auto-hybrid stayed lexical; no model load on the confident BM25 path. |
| Fresh HOME, no model cache and no HNSW sidecar | CJK `레디스 장애 원인` | Auto-hybrid stays on the BM25 probe instead of loading the provider just to discover the missing sidecar. |
| Current HOME, model cached, no HNSW sidecar | weak lexical `Redis` | Auto-hybrid stays on the BM25 probe; smoke returned the same hit as `--bm25-only` with no HNSW warning or cold model load. |
| Current HOME, HNSW sidecar present, fresh CLI | CJK `레디스 장애 원인` | still about 7.8 s because query embedding loads the model in the fresh process |
| Current HOME, HNSW sidecar present, warm HTTP daemon | CJK `레디스 장애 원인` | first daemon call about 7.4 s warm-up; second call about 10 ms |
| Current HOME, HNSW sidecar present, HTTP daemon with semantic prewarm | CJK `레디스 장애 원인` | startup paid the warm-up; first user auto-hybrid call was 14.9 ms |

Interpretation: auto-hybrid is the default search policy because the confident
BM25 path stays lexical, the default HNSW policy does not cold-load a provider
when the sidecar is missing, semantic promotion falls back to BM25 on provider
failure, and the tracked-docs gate showed the gated path can outperform both
BM25 and pure dense when lexical recall is genuinely weak. The HF agent-trace
gate narrowed the policy: CJK no longer promotes unconditionally; Korean
wrappers around strong English anchors stay lexical. The explicit escape hatch
is `aidememo search --bm25-only` or `aidememo config set search.auto_hybrid
false` for deterministic demos, hooks, and saturated lexical stores. When
`search.auto_hybrid=true`, `mcp-serve` prewarms the semantic provider on
startup; `AIDEMEMO_PREWARM_SEMANTIC=1` forces the same behavior for explicit
daemon experiments.

Implementation follow-up: `scripts/lfm_mlx_embedding_sidecar.py` exposes
`mlx-community/LFM2.5-Embedding-350M-4bit` through TEI-compatible `/embed` and
`/info` endpoints. AideMemo maps `model.provider=lfm-sidecar` to the existing
TEI provider path, so the model can sit behind the real `auto_hybrid` + HNSW
flow without linking MLX into the Rust process. July 8 local sidecar smoke on
port 18088 reported model load 3.52s, dimension 1024, and 10 warm `/embed`
query calls at mean 24.24ms / p50 22.50ms / max 39.59ms. This matches the
daemon-prewarm placement: startup pays the load, promoted queries pay roughly
20-30ms for the LFM query vector before HNSW lookup.

For fact-type rollout, `scripts/lfm_fact_type_threshold_eval.py` joins reviewed
shadow-label JSONL with `lfm_fact_type_sidecar.py --jsonl` predictions and
reports precision, false-memory rate, baseline-correct harms, and residual
rescues across confidence/margin grids. Use that gate before promoting any
`fact_type_hint` automatically.

## Performance

Reference numbers from `benchmarks/src/bin/performance.rs` on a 10,000-fact
synthetic wiki, p95 latency, default config:

| Operation | p95 |
|---|---:|
| `traverse_d3` | ~0.01 ms |
| `search_bm25` | ~0.5 ms |
| `search_hybrid` | ~3.4 ms |
| `lint` | ~34 ms |
| `fact_add_many` per fact | ~0.07 ms |
| `fact_add` single | ~5 ms |
| `startup` open + first traverse | ~12 ms |

`fact_add` is limited by the OS fsync floor under immediate durability. Use
`fact_add_many` or `store.durability = eventual` when ingest throughput matters.

### Storage Backend Probe

`benchmarks/src/bin/storage_backend_probe.rs` compares the optional redb-backed
AideMemo API path with the default SQLite schema. SQLite is now the default
runtime backend (`store.backend = "sqlite"`, default Cargo features) with
normalized `entities`, `facts`, `fact_entities`, `relations`, feedback tables,
secondary indexes, JSON record payloads, and FTS5. Results are written to
`benchmarks/results/storage_backend_probe.json`.

Local macOS arm64 run on 2026-06-13:

```bash
cargo run --release -p aidememo-benchmarks --bin storage_backend_probe
```

10,000-fact synthetic store, p95 latency after the redb
`fact_list(entity_id=...)` fast path started using the existing
`fact_by_entity` prefix index:

| Backend | Build | Single `fact_add` | Batch per fact | All facts scan | Entity facts | Search | Open existing |
|---|---:|---:|---:|---:|---:|---:|---:|
| redb immediate | 3086.79 ms | 6.38 ms | 0.122 ms | 30.00 ms | 6.12 ms | BM25 0.92 ms | 38.23 ms |
| redb eventual | 425.35 ms | 1.23 ms | 0.050 ms | 30.04 ms | 6.04 ms | BM25 0.96 ms | 32.90 ms |
| SQLite WAL FULL | 95.23 ms | 0.25 ms | 0.022 ms | 1.66 ms | 0.022 ms | FTS5 0.12 ms | 1.53 ms |
| SQLite WAL NORMAL | 89.24 ms | 0.19 ms | 0.023 ms | 1.59 ms | 0.017 ms | FTS5 0.13 ms | 0.77 ms |

Interpretation:

* SQLite is now the default backend. It naturally fits entity/fact joins,
  indexed filtered lists, migrations, introspection, and persistent FTS.
* The redb `fact_list_entity` path improved from the earlier all-scan result
  (`~29 ms`) to `~6 ms` after using `fact_by_entity`. The remaining gap is no
  longer from total-store scanning; it is the cost of point-hydrating JSON facts
  through the redb table path versus SQLite's join/index cursor.
* redb immediate write latency is dominated by per-transaction fsync. The
  existing `store.durability = eventual` knob narrows single-write cost, but
  SQLite still wins this synthetic write path while also maintaining FTS rows.
* SQLite FTS5 is not directly equivalent to the current in-memory BM25 index,
  but the persistent-index result supports a deeper local SQLite spike.

Runtime promotion status:

* `crates/aidememo-core/src/backend.rs` defines `StoreBackend` plus `StoreKind`,
  so `AideMemo::open` can select redb or SQLite from `config.store.backend`.
  The trait also owns the shared archive-transfer contract
  (`existing_fact_ids`, `fact_archive_to`): cold-tier moves must preserve
  `FactId`, skip already-archived hot misses, and delete from hot only after the
  cold store can address the archived id.
* `crates/aidememo-core/src/sqlite_store.rs` implements the same public store
  surface used by entity/fact CRUD, relation graph traversal, lint, ingest,
  BM25 search, query, archive/cold-tier moves, JSONL import/export, pull sync,
  feedback, and semantic-adapt adapter state.
* JSONL import now uses the same ID-preserving upsert path as sync import.
  This makes `aidememo export` from a redb store followed by `aidememo import`
  into a SQLite store a usable migration primitive instead of allocating fresh
  ULIDs.
* `aidememo-cli` defaults to SQLite and exposes a `redb` Cargo feature that
  forwards `aidememo-core/redb`; build with
  `cargo build -p aidememo-cli --features redb` before setting
  `aidememo config set store.backend redb`. Redb-only builds
  (`--no-default-features --features redb`) default to `store.backend = "redb"`
  and `./_meta/wiki.redb`, so the optional backend remains directly usable
  without a SQLite feature compiled in.
* The Python, Node, Elixir, and C native binding crates share the same default
  SQLite backend and optional `redb` feature. This keeps the SDK replacement
  path aligned with CLI/MCP instead of making backend choice a CLI-only spike.
* `sqlite_matches_redb_for_core_public_api_fixture` seeds the same fixture into
  redb and SQLite, then compares stats, fact contents, traversal output, and
  BM25 search results.
  `sqlite_matches_redb_for_mutation_feedback_and_relation_contract` applies the
  same entity rename/delete, fact delete, pin, direct fact feedback, search
  feedback, and relation remove sequence to redb and SQLite, then compares the
  resulting user-visible snapshot. This caught and fixed the legacy redb
  `fact_feedback` path that changed an in-memory record but did not persist the
  updated relevance score.
  `archive_contract_matches_redb_sqlite_and_libsqlite_public_api` exercises
  the same hot-to-cold archive contract through the public `AideMemo` API on
  redb, canonical SQLite, and the `libsqlite` alias. Together these are the
  current semantic parity gates for backend promotion.
* `sqlite_import_preserves_redb_export_ids_for_migration_gate` exports a redb
  fixture, imports it into canonical SQLite and the `libsqlite` alias, verifies
  entity/fact IDs and graph/search parity, then replays the same JSONL to prove
  the migration path is idempotent.
* `sync_export_import_is_backend_compatible` verifies the pull-sync wire format
  in both directions for redb, canonical SQLite, and the `libsqlite` alias
  (`redb → sqlite/libsqlite`, `sqlite/libsqlite → redb`). It checks full sync
  entity/fact/relation ID preservation and then applies an incremental delta
  with `entity_describe` plus `fact_supersede`, proving in-place updates cross
  the backend boundary as well as fresh inserts.
* `scripts/storage-backend-parity.sh` is the CLI/MCP gate: redb/SQLite
  mutation and feedback parity, redb export/import into canonical SQLite and
  the `libsqlite` alias, redb/SQLite/libsqlite sync compatibility, relation
  preservation, SQLite cold-tier archive/search, and 24 concurrent MCP writes
  through `mcp-serve`. It also starts a `libsqlite` daemon and verifies the
  registry records the canonical SQLite backend, accepts the `sqlite` /
  `libsqlite` aliases, and rejects same-path redb discovery.
* `scripts/storage-backend-sqlite-full-surface.sh` is the SQLite-only
  full-surface smoke: it builds the CLI with `--no-default-features --features
  sqlite` and exercises init, ingest, entity/fact writes, entity rename/delete,
  fact delete, BM25 search/query, graph traversal, sessions, workflow start,
  archive, export, and import without redb compiled in. It defaults
  `store.backend` to the `libsqlite` alias, so the full public surface proves
  both the canonical SQLite backend and the user-facing libsqlite spelling
  resolve to the same implementation.
* `scripts/storage-backend-sqlite-advanced-surface.sh` is the SQLite-only
  advanced-surface smoke: it builds the same no-default SQLite CLI and verifies
  CLI-level `store.lock_retry_ms` busy-timeout behaviour under a held SQLite
  writer lock, fact-level feedback, search feedback, adapter train/status/eval,
  heuristic extract preview/apply, pending approve/reject, and TTL-only
  consolidate without model downloads. It also defaults `store.backend` to the
  `libsqlite` alias. The TTL gate explicitly runs with `--semantic-threshold
  0`, proving expiry is independent from semantic dedup.
* `fact_archive_preserves_mcp_fact_get_for_cold_tier` is the MCP archive
  invariant gate: archived facts leave the hot store, `aidememo_fact_get` still
  resolves them from the backend-specific cold tier, default search hides them,
  and `include_archive:true` search returns them.
* `scripts/storage-backend-real-corpus-diff.sh` ingests the repo's real docs
  corpus into redb, canonical SQLite, and the `libsqlite` alias independently,
  normalizes away backend-specific ULIDs/timestamps, compares entity/fact/
  relation exports, then compares BM25 search results across representative
  queries.
* `scripts/storage-backend-sqlite-mcp-soak.sh` builds the same no-default
  SQLite CLI, defaults `store.backend` to the `libsqlite` alias, and runs
  representative SQLite-backed MCP tools before the load soak: batch add,
  entity describe/get/list, fact edit/get/list/pin/supersede/archive,
  search/query/context/recent/session_start, overview/doctor, traverse/path,
  aggregate, extract preview, workflow_start, and include-archive search. It
  then writes 200 facts through 16 parallel HTTP MCP callers, verifies unique
  fact IDs, final stats, and BM25 visibility for a tail write. It also starts
  and stops a SQLite-only daemon and checks daemon-discovered BM25 search. Set
  `AIDEMEMO_SQLITE_MCP_SOAK_BACKEND=sqlite` to exercise the canonical spelling.
* `scripts/storage-backend-sdk-bindings-check.sh` verifies the SDK/binding
  surface: Python compile checks for default SQLite / explicit SQLite-only /
  redb-only builds, plus backend open/write/header tests for Node, Elixir NIF,
  and C. The tests cover omitted/empty backend arguments inheriting the
  compiled default, the `libsqlite` alias, and redb-only Cargo feature builds.
  Python runtime packaging is covered by `scripts/aidememo-python-pack-smoke.sh`.
* The CI `storage-backend-compat` job runs the parity, real-corpus diff,
  SQLite MCP soak, and SDK binding backend gates on Ubuntu after lint. That
  keeps redb/SQLite sync/import/archive compatibility, libsqlite runtime
  spelling, concurrent SQLite MCP writes, and SDK feature boundaries visible in
  PR checks instead of relying only on local `ci-local.sh test` runs.
* `scripts/release-preflight.sh` runs the storage backend feature, SQLite
  full-surface, SQLite advanced-surface, parity, real-corpus diff, SQLite MCP
  soak, and SDK binding backend gates by default. Set
  `AIDEMEMO_RELEASE_PREFLIGHT_STORAGE_BACKEND=0` only for narrow non-storage
  release checks.
* `s3` no longer enables the `redb` feature. Its local WAL staging path uses
  SQLite (`wal.sqlite`), so `cargo check -p aidememo-core --no-default-features
  --features s3` proves the S3/manifest code can build without compiling the
  optional redb backend.
* `scripts/storage-backend-feature-gate.sh` locks the Cargo feature boundary:
  default and SQLite-only core/CLI/SDK builds plus S3-only core builds must
  omit the `redb` crate from `cargo tree`, while redb still appears only when
  the explicit `redb` feature is selected. It also builds a redb-only CLI and
  smoke-tests that empty-config defaults report `store.backend = "redb"` /
  `./_meta/wiki.redb` and create a `.redb` store rather than a SQLite file.

Validation added in the runtime spike:

```bash
cargo test -p aidememo-core
cargo test -p aidememo-core --no-default-features --features sqlite
cargo test -p aidememo-core --no-default-features --features redb
cargo test -p aidememo-core --features sqlite,redb archive_contract_matches_redb_sqlite_and_libsqlite_public_api
cargo test -p aidememo-core --features sqlite,redb sqlite_matches_redb_for_mutation_feedback_and_relation_contract
cargo test -p aidememo-core --features sqlite,redb sync_export_import_is_backend_compatible
cargo test -p aidememo-core --features sqlite,redb,semantic sqlite_import_preserves_redb_export_ids_for_migration_gate
cargo test -p aidememo-core --features sqlite,semantic,semantic-adapt
cargo check -p aidememo-core --no-default-features --features s3
./scripts/storage-backend-feature-gate.sh
cargo check -p aidememo-cli
cargo test -p aidememo-cli --no-default-features --features redb --bin aidememo
./scripts/storage-backend-sqlite-full-surface.sh
./scripts/storage-backend-sqlite-advanced-surface.sh
./scripts/storage-backend-parity.sh
./scripts/storage-backend-real-corpus-diff.sh
./scripts/storage-backend-sqlite-mcp-soak.sh
./scripts/storage-backend-sdk-bindings-check.sh
```

Current replacement read:

* SQLite is now the default backend and runs through the full public
  `AideMemo` API surface in tests. The main replacement blockers are no longer
  graph/search/lint/ingest wiring.
* The redb path remains available as an explicit Cargo feature and runtime
  backend selection. Archive siblings use backend-specific suffixes
  (`.cold.redb` for redb, `.cold.sqlite` for SQLite), the docs corpus parity
  gate covers a representative real markdown corpus, and the local MCP soak
  covers concurrent SQLite write traffic.
* `aidememo doctor` warns when `store.backend` and the store path extension
  imply different persistence layers, for example `store.backend = "redb"`
  with `wiki.sqlite`. The engine can open those combinations, but release
  guidance keeps suffixes aligned so users can tell which backend owns a file.
* libSQL/Turso remote operation is still a separate decision. The current
  implementation uses bundled SQLite through rusqlite and the S3 manifest/WAL
  path uses local SQLite staging; this proves relational schema fit and local
  runtime replaceability, not managed remote replication semantics.

### SQLite Snapshot Backup

`aidememo backup create <DIR>` uses SQLite's Online Backup API through
`rusqlite` to create a point-in-time snapshot of the selected SQLite store,
then writes `manifest.json` plus `wiki.sqlite` into a time-sortable backup
directory. The manifest records byte counts and SHA-256 for the stored object
and SQLite payload. `aidememo backup restore <DIR> --force` verifies the
manifest, runs SQLite `integrity_check`, replaces the selected `--store`, and
removes stale SQLite WAL/SHM plus HNSW sidecar files.

S3 backup / restore is intentionally an optional build surface:
`cargo build -p aidememo-cli --features s3` enables `s3://bucket/prefix`
targets. S3 stores a zstd-compressed `wiki.sqlite.zst` plus the same manifest.
S3 remains backup storage, not the live database backend.

Validation added with the backup command:

```bash
cargo check -p aidememo-core --features sqlite
cargo check -p aidememo-cli
cargo check -p aidememo-core --no-default-features --features redb
cargo check -p aidememo-core --no-default-features --features s3
cargo check -p aidememo-cli --no-default-features --features redb
cargo check -p aidememo-cli --features s3
cargo test -p aidememo-core --features sqlite backup
python3 scripts/docs-feature-gate.py
python3 scripts/docs-site-e2e.py
scripts/storage-backend-feature-gate.sh
```

Manual local smoke:

```bash
aidememo --backend libsqlite --store "$STORE" fact add \
  "SQLite backup smoke fact" --entities Backup --type claim
aidememo --backend libsqlite --store "$STORE" backup create "$BACKUPS"
aidememo --backend libsqlite --store "$RESTORED" backup restore "$BACKUP_DIR" --force
aidememo --backend libsqlite --store "$RESTORED" --json stats
```

The smoke restored `1` entity and `1` fact from the backup manifest.

### Branch Log Push / Merge

`aidememo branch push --branch <ID> [--base <BACKUP>] <DEST>` exports a
branch segment for cloud-deployed agents. When `--base` points at a backup
manifest that contains a sync cursor, the segment is a compact delta after that
baseline. `aidememo branch merge [--branch <ID>] <SOURCE>` verifies the segment
manifest and imports the JSONL payload through the existing idempotent
`sync_import` path.

Local layout:

```text
<DEST>/branches/<branch-id>/segments/<segment-id>.jsonl
<DEST>/branches/<branch-id>/segments/<segment-id>.manifest.json
```

S3 layout uses the same prefix shape, stores `.jsonl.zst` payloads, and remains
an optional `--features s3` transport. This is branch-log sync, not S3 as a live
database backend.

Validation added with the branch command:

```bash
cargo check -p aidememo-core --features sqlite
cargo check -p aidememo-cli
cargo check -p aidememo-core --features s3
cargo check -p aidememo-cli --features s3
cargo test -p aidememo-core --features sqlite branch -- --nocapture
cargo test -p aidememo-core --features sqlite backup -- --nocapture
cargo check -p aidememo-python -p aidememo-napi
cargo test -p aidememo-napi -- --nocapture
PYTHONPATH=packages/aidememo-agent-sdk/src uvx --from pytest pytest -q packages/aidememo-agent-sdk/tests
scripts/aidememo-python-pack-smoke.sh
```

The branch unit test suite now covers two workflows:

| Workflow | Expected result |
|---|---|
| Single agent delta after a backup cursor | `since_base` export inserts exactly the new branch fact into the restored target. |
| Candidate A/B experiment from one baseline | Merging `candidate-b` imports only B's fact, leaves A absent, and a repeated merge inserts `0` duplicate facts. Merging all branches into a clean target imports both candidate facts. |

SDK / binding validation:

| Surface | Result |
|---|---|
| `aidememo-agent-sdk` | `branch_push` / `branch_merge` forward to local `aidememo-python` when available and fall back to CLI for S3 URIs. Unit tests passed: `13 passed`. |
| `aidememo-python` | Wheel smoke passed after adding native `branch_push` / `branch_merge`; local branch merge inserted the exported facts and repeated merge inserted `0` duplicates. |
| `aidememo-napi` | Native Rust test passed: branch push/merge round-trips through already-open handles and repeated merge is idempotent. |
| `aidememo_nif` | Elixir `mix test` branch smoke covers candidate A/B push, selected-branch merge, repeat-merge idempotency, and local-only S3 URI guard. |
| C ABI | Not exposed yet; use the CLI branch commands for branch artifacts until that lower-level ABI needs the surface. |

Manual local smoke:

```bash
aidememo --store "$BASE" --backend sqlite fact add \
  "Base fact for branch smoke" --entities BranchSmoke --type claim
BACKUP_DIR="$(aidememo --store "$BASE" --backend sqlite backup create --json "$BACKUPS" \
  | jq -r '.destination')"
aidememo --store "$AGENT" --backend sqlite backup restore --force "$BACKUP_DIR"
aidememo --store "$MERGED" --backend sqlite backup restore --force "$BACKUP_DIR"
aidememo --store "$AGENT" --backend sqlite fact add \
  "Agent branch fact for branch smoke" --entities BranchSmoke --type lesson
aidememo --store "$AGENT" --backend sqlite branch push \
  --branch agent-smoke --base "$BACKUP_DIR" --json "$BRANCHES"
aidememo --store "$MERGED" --backend sqlite branch merge \
  --branch agent-smoke --json "$BRANCHES"
aidememo --store "$MERGED" --backend sqlite stats --json
```

The smoke exported `1` record in `since_base` mode, merged `1` segment with
`1` inserted fact, and the merged store reported `1` entity and `2` facts.

Manual A/B branch experiment smoke:

```bash
# Baseline store -> backup -> candidate-a / candidate-b / selected / all stores.
# candidate-a writes one noisy lesson, candidate-b writes one winning lesson.
aidememo --store "$A" --backend sqlite branch push \
  --branch candidate-a --base "$BACKUP_DIR" --json "$SHARED"
aidememo --store "$B" --backend sqlite branch push \
  --branch candidate-b --base "$BACKUP_DIR" --json "$SHARED"
aidememo --store "$SELECTED" --backend sqlite branch merge \
  --branch candidate-b --json "$SHARED"
aidememo --store "$SELECTED" --backend sqlite branch merge \
  --branch candidate-b --json "$SHARED"
aidememo --store "$ALL" --backend sqlite branch merge --json "$SHARED"
```

Observed local CLI result:

| Step | Result |
|---|---|
| Push `candidate-a` | `records_exported=1`, `export_mode=since_base` |
| Push `candidate-b` | `records_exported=1`, `export_mode=since_base` |
| Merge selected `candidate-b` | `segments_merged=1`, `facts_inserted=1`, selected store `fact_count=2` |
| Merge selected `candidate-b` again | `facts_inserted=0`, `facts_skipped=1` |
| Merge all branches into a clean target | `segments_merged=2`, `facts_inserted=2`, all-branches store `fact_count=3` |

## Workflow Doctor Readiness

P3.5 adds a `workflow` block and P2.5 adds a separate `sharing` block to
`aidememo doctor --json` so sparse ticket automation and shared-store ergonomics are
visible without manually inspecting agent configs, fact lists, or benchmark
notes. P2.6 threads the same `sharing` contract through MCP `aidememo_doctor`, so
agents can see it without shelling out. The stable workflow contract is:

| Field | Meaning |
|---|---|
| `workflow.ready` | At least one checked agent has `aidememo` registered as MCP. |
| `workflow.recent_ticket_count` | Current workflow-start ticket facts in the last 30 days. |
| `workflow.recent_tickets[]` | Up to five recent ticket summaries with `source` / `source_id`. |
| `workflow.hints[]` | Actionable setup or usage hints with a concrete command in `action`. |

The sharing contract is:

| Field | Meaning |
|---|---|
| `sharing.lock_retry_ms` | Current local-store contention wait budget: SQLite busy timeout and redb open retry. |
| `sharing.serverless_recommended_writers` | Measured smooth same-host writer envelope, currently 4. |
| `sharing.high_concurrency_writers` | Stress point used by Scenario J, currently 8. |
| `sharing.daemon.state` | `healthy`, `stale_registry`, or `none`. |
| `sharing.recommended_mode` | `daemon`, `serverless_retry`, or `serverless_fail_fast`. |
| `sharing.hints[]` | Actionable retry / daemon guidance with a concrete command in `action`. |

Validation:

| Command | Result |
|---|---|
| `cargo test -p aidememo-cli doctor` | 25 passed; workflow unit tests cover ready/count/hints, sharing unit tests cover retry advisory behaviour, and integration tests cover the JSON `sharing` contract. |
| `cargo test -p aidememo-cli doctor_json_includes_workflow_readiness_hints` | fixture CLI smoke validates JSON `workflow.ready`, `recent_ticket_count`, and actionable hints. |
| `cargo test -p aidememo-cli doctor_json_includes_shared_store_guidance` | fixture CLI smoke validates JSON `sharing.lock_retry_ms`, `serverless_recommended_writers=4`, `daemon.state`, `recommended_mode`, and actionable hints. |
| `cargo test -p aidememo-cli doctor_groups_by_code_with_action_hints` | MCP `aidememo_doctor` unit validates lint grouping plus `sharing.serverless_recommended_writers=4`, `recommended_mode=serverless_fail_fast`, and `sharing_retry_disabled`. |
| `python3 bench/multi-agent/scenario_i_workflow_doctor.py` | 10/10 invariants; CLI/MCP/Hermes each created a workflow ticket; doctor reported `workflow.ready=true`, `recent_ticket_count=3`, and no false no-MCP/no-recent-ticket hints. |
| `cargo build -p aidememo-cli --release --features redb && python3 bench/multi-agent/scenario_j_lock_retry_sweep.py` | 7/7 invariants; optional-redb `store.lock_retry_ms=5000` stayed smooth through 4 concurrent serverless writers and mostly recovered 8-writer contention. |
| `scripts/workflow-release-smoke.sh` | Bundles the first-run workflow demo, Scenario F + I, and a fresh fixture `aidememo doctor --json` assert for release checks. Latest run: demo recovered decision/lesson/error, Scenario F 13/13, Scenario I 10/10, fixture doctor `workflow_ready=true`, `recent_ticket_count=1`, total 13.40s. The timing table includes `ok`/`fail` status and is printed from an EXIT trap so partial failures still leave context; forced `AIDEMEMO_BIN=/bin/false` failure records `fail ... exit 1`. |
| CI `workflow-release-smoke` job | Runs the same script on Ubuntu after lint with Python 3.13 and a 10-minute timeout. |
| CI `workflow-lint` job | Runs `actionlint@v1.7.1` across `.github/workflows/*.yml` before heavier Rust checks. Local `actionlint .github/workflows/*.yml`: 0 issues. |

Latest local `workflow-release-smoke` timing:

| Status | Step | Seconds |
|---|---|---:|
| ok | `cargo build -p aidememo-cli` | 0.22 |
| ok | `bash -n scripts/demo-workflow.sh` | 0.02 |
| ok | `py_compile` | 0.04 |
| ok | `scripts/demo-workflow.sh` | 0.65 |
| ok | Scenario F | 7.17 |
| ok | Scenario I | 5.10 |
| ok | Fixture `aidememo workflow start --bm25-only` | 0.20 |
| total | | 13.40 |

Scenario I measurement, May 21 2026:

| Metric | Value |
|---|---:|
| Workflow tickets | 3 |
| Drivers | CLI, MCP, Hermes |
| Workflow p50 latency | 1887.69 ms |
| Workflow p95 latency | 1891.48 ms |
| Doctor recent ticket count | 3 |
| Doctor hint codes | `workflow_no_skill_prompt` |

Scenario J lock-retry sweep, May 21 2026:

| Writers | Retry ms | Persisted | Success | p50 ms | p95 ms | Max ms | Wall ms |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 0 | 10/10 | 100.0% | 94.80 | 185.26 | 185.26 | 1113.06 |
| 2 | 0 | 10/20 | 50.0% | 92.80 | 124.25 | 179.96 | 1150.86 |
| 4 | 0 | 10/40 | 25.0% | 8.72 | 106.01 | 192.92 | 1171.85 |
| 8 | 0 | 10/80 | 12.5% | 9.52 | 107.85 | 200.79 | 1242.34 |
| 1 | 5000 | 10/10 | 100.0% | 106.71 | 186.70 | 186.70 | 1207.83 |
| 2 | 5000 | 20/20 | 100.0% | 103.61 | 196.69 | 1274.25 | 2255.45 |
| 4 | 5000 | 40/40 | 100.0% | 101.86 | 1282.56 | 3452.56 | 4431.45 |
| 8 | 5000 | 79/80 | 98.8% | 103.02 | 2988.40 | 5049.66 | 8529.39 |

Product read: optional-redb serverless CLI retry is appropriate for one to four
same-host writers. At eight concurrent writers it is still much better than
fail-fast mode, but p95 approaches three seconds and one write can still
exhaust the 5s retry budget; use a shared `aidememo mcp-serve` daemon when that
level of parallelism is normal. The default SQLite backend has separate
coverage through `scripts/storage-backend-sqlite-mcp-soak.sh`.

## GBrain Adapter Native Backend

The `gbrain-evals` scaffold now supports `AIDEMEMO_ADAPTER_BACKEND=auto|cli|napi`.
The native path imports `aidememo-napi` in process and keeps the existing CLI and
daemon paths as baselines.

Local Bun-shaped fixture, 3 pages, 30 repeated BM25 queries:

| Backend | Top Hit | p50 | p95 |
|---|---|---:|---:|
| CLI | `redis` | 124.55 ms | 132.08 ms |
| NAPI | `redis` | 0.02 ms | 0.03 ms |

This is a scaffold-level latency check, not the full public `gbrain-evals`
scorecard. Record full runner wall time separately when re-running BrainBench.

Fresh-checkout BrainBench scorecard, `garrytan/gbrain-evals@89445dd`,
`BRAINBENCH_N=1`, 240 pages, 145 queries:

| Backend | P@5 | R@5 | Correct / Expected | Real Time |
|---|---:|---:|---:|---:|
| CLI daemon bm25 | 17.4% | 64.1% | 125 / 261 | 10.77 s |
| NAPI bm25 | 17.4% | 64.1% | 125 / 261 | 6.48 s |

NAPI preserves score parity and is `1.66x` faster than the daemon baseline
(`9.78x` faster than the historical direct CLI `63.38 s` run).

Packaging readiness:

| Command / workflow | Result |
|---|---|
| `scripts/release-preflight.sh` | One-command release gate with timed summary rows. `local` profile runs version gate, registry readiness gate, workflow syntax lint when `actionlint` is available, docs feature gate/build, binding release smoke, workflow release smoke, and SDK promotion check; the binding smoke now includes NAPI pack/install, `aidememo-agent-sdk` wheel install, and `hermes-aidememo` wheel install by default. `full` profile adds optional Python/Elixir/C binding smokes plus `aidememo-python`, `aidememo-agent-sdk`, `hermes-aidememo`, and `aidememo-napi` publish dry-runs. Child scripts still print their own summaries to stdout, but preflight clears child `$GITHUB_STEP_SUMMARY` so CI gets one top-level `release-preflight` table. Binding-only measured path (`AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT=0 AIDEMEMO_RELEASE_PREFLIGHT_DOCS=0 AIDEMEMO_RELEASE_PREFLIGHT_STORAGE_BACKEND=0 AIDEMEMO_RELEASE_PREFLIGHT_WORKFLOW=0 AIDEMEMO_RELEASE_PREFLIGHT_SDK_PROMOTION=0 AIDEMEMO_RELEASE_PREFLIGHT_PUBLISH=0`): version 0.14s, binding smoke 53.13s, total 53.27s. Publish-only full-profile path (`AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT=0 AIDEMEMO_RELEASE_PREFLIGHT_DOCS=0 AIDEMEMO_RELEASE_PREFLIGHT_STORAGE_BACKEND=0 AIDEMEMO_RELEASE_PREFLIGHT_BINDINGS=0 AIDEMEMO_RELEASE_PREFLIGHT_WORKFLOW=0 AIDEMEMO_RELEASE_PREFLIGHT_SDK_PROMOTION=0 AIDEMEMO_RELEASE_PREFLIGHT_PUBLISH=1`): version 0.13s, `aidememo-python` publish 56.19s, agent SDK publish 3.86s, Hermes publish 3.86s, NAPI publish 4.05s, total 68.09s. `AIDEMEMO_RELEASE_PREFLIGHT_SDK_REQUIRE_PUBLIC=1` fails the SDK step with exit 1 while recording the failure row. Forced `AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full AIDEMEMO_RELEASE_PREFLIGHT_ACTIONLINT_BIN=/nonexistent/actionlint` records `fail | workflow syntax lint | 0.00 | /nonexistent/actionlint not installed`. |
| `scripts/registry-readiness-check.py` | Offline registry mapping gate. Verifies PyPI project names, workflow names, `pypi-publish` / `npm-publish` environments, OIDC `id-token: write` publish permissions, npm root/platform package graph, release-doc package lists, and rejects long-lived publish-token assumptions in first-party publish workflows. Latest local run: pypi=3, npm=6, docs/workflows aligned. |
| CI `sdk-promotion-check` job | Runs `scripts/sdk-promotion-check.sh` on Ubuntu after lint with Python 3.13 and Node 22, keeping SDK wording and binding surface drift visible in PR checks without running package smokes. Local workflow lint: `actionlint .github/workflows/*.yml` 0 issues; local gate: ok=13, ready=3, blocked=2, fail=0. |
| `scripts/ci-local.sh demo` | Local first-run workflow smoke with a timed Markdown summary. `bash -n scripts/ci-local.sh` passes; `scripts/ci-local.sh demo` recovered decision=1, lesson=1, error=1, search_hits=4, workflow latency 128ms, wall 0.91s. `scripts/ci-local.sh all` now runs this check between lint and SDK promotion. |
| `scripts/ci-local.sh sdk` | Local CI parity hook for the same SDK wording/parity gate. `bash -n scripts/ci-local.sh` passes; direct `scripts/sdk-promotion-check.sh` reports ok=13, ready=3, blocked=2, fail=0. Child SDK details remain in stdout, but `$GITHUB_STEP_SUMMARY` contains only the top-level `ci-local timings` table (`rg -n '^## ' "$summary_file"` returns one heading). Latest local SDK mode total: 0.12s. `scripts/ci-local.sh all` now runs this check between the workflow demo and tests. |
| SDK promotion GitHub summary | `GITHUB_STEP_SUMMARY=$(mktemp) scripts/sdk-promotion-check.sh` writes a Markdown table with 18 check rows plus metric rows while keeping stdout unchanged. `GITHUB_STEP_SUMMARY=$(mktemp) AIDEMEMO_SDK_PROMOTION_JSON=1 scripts/sdk-promotion-check.sh` still emits valid JSON to stdout. |
| `scripts/aidememo-release-version.sh` | Unified release version gate. With no args, verifies Cargo, Python, npm, NIF, agent SDK, and Hermes package versions together; with a semver arg, updates every managed package version. Latest run: `0.1.0` pinned; temp-copy bump to `0.1.1` updated Cargo workspace, Python `pyproject.toml`, npm root/platform packages plus optionalDependency pins, `aidememo-nif` `mix.exs`, `aidememo-agent-sdk`, and `hermes-aidememo` metadata/runtime versions. |
| `scripts/aidememo-python-version.sh` | Version gate for the Python wheel. With no args, verifies `Cargo.toml` workspace version equals `crates/aidememo-python/pyproject.toml` `project.version`; with a semver arg, updates both. Latest run: `0.1.0` pinned. |
| `scripts/aidememo-python-pack-smoke.sh` | Builds a `aidememo-python` wheel with maturin for a temp venv interpreter, installs that wheel into the venv, runs `crates/aidememo-python/tests/smoke.py`, verifies installed wheel metadata equals `aidememo_python.__version__`, and writes timed rows to stdout and `$GITHUB_STEP_SUMMARY`. The smoke accepts `AIDEMEMO_PYTHON_SMOKE_BACKEND=sqlite\|libsqlite\|redb` and checks the created store file header so an ignored backend argument fails visibly. Local macOS arm64 default SQLite wheel: total 44.93s; version gate 0.04s; `maturin build --release` 42.03s; install 0.72s; smoke 1.41s; version check 0.06s. libsqlite alias smoke (`AIDEMEMO_PYTHON_SMOKE_BACKEND=libsqlite`): total 33.12s; `maturin build --release` 30.59s; install 0.64s; smoke 1.33s; version check 0.05s. redb-only wheel (`AIDEMEMO_PYTHON_PACK_SMOKE_NO_DEFAULT_FEATURES=1 AIDEMEMO_PYTHON_PACK_SMOKE_FEATURES=redb AIDEMEMO_PYTHON_SMOKE_BACKEND=redb`): total 37.14s; `maturin build --release --no-default-features --features redb` 34.58s; install 0.65s; smoke 1.48s; version check 0.06s. All built `aidememo_python-0.1.0-cp313-cp313-macosx_11_0_arm64.whl`; smoke passed including `workflow_start(..., bm25_only=True)` and `AideMemoNotFoundError` typed exception handling, installed version `0.1.0`. |
| `scripts/aidememo-python-publish-dry-run.sh` | Builds PyPI publish payloads without uploading and writes a timed Markdown summary. Local macOS arm64: version gate 0.05s, venv 1.36s, `maturin build --release --sdist` 59.48s, payload validation 0.05s, total 60.94s; built `aidememo_python-0.1.0-cp313-cp313-macosx_11_0_arm64.whl` (2.8M) + `aidememo_python-0.1.0.tar.gz` (349K). The dry-run rebuilds the wheel from the sdist, proving the vendored `tokenizers` patch is present; forced `AIDEMEMO_PYTHON_EXPECT_VERSION=9.9.9` records a `fail | version expectation` summary row. |
| `scripts/aidememo-agent-sdk-publish-dry-run.sh` | Builds pure-Python PyPI payloads without uploading and writes a timed Markdown summary. Local macOS arm64: venv 1.68s, build backend install 0.53s, `python -m build --wheel --sdist` 1.72s, payload validation 0.06s, total 3.99s; built `aidememo_agent_sdk-0.1.0-py3-none-any.whl` + `aidememo_agent_sdk-0.1.0.tar.gz`. Validator checks metadata, wheel/sdist source files, and forbids cache/build artifacts. |
| `scripts/hermes-aidememo-publish-dry-run.sh` | Builds pure-Python PyPI payloads without uploading and writes a timed Markdown summary. Local macOS arm64: venv 1.65s, build backend install 0.61s, `python -m build --wheel --sdist` 1.60s, payload validation 0.06s, total 3.92s; built `hermes_aidememo-0.1.0-py3-none-any.whl` + `hermes_aidememo-0.1.0.tar.gz`. Validator checks metadata, `aidememo-agent-sdk` and `PyYAML` dependency declarations, `plugin.yaml`, bundled skill files, and cache/build artifact exclusion. |
| `scripts/aidememo-napi-version.sh` | Version gate for root + platform package graph. With no args, verifies all versions and root `optionalDependencies`; with a semver arg, updates every package. Latest run: `0.1.0` pinned across 5 platform packages; temp-copy bump to `0.1.1` updated root, platform packages, and optionalDependency pins. |
| `scripts/aidememo-napi-pack-smoke.sh` | Builds release addon, runs `npm test`, packs root `aidememo-napi`, packs the current platform package, then installs both tarballs into a temp project with offline/no-audit npm flags and verifies `require("aidememo-napi").version()`. Writes timed rows to stdout and `$GITHUB_STEP_SUMMARY`. Local macOS arm64: total 2.81s; build 0.67s; test 1.00s; root pack 0.22s; platform pack 0.53s; install 0.30s. Payloads: root `aidememo-napi-0.1.0.tgz` is 4.18 KB / 4 files / includes README + no `.node`; platform `aidememo-napi-darwin-arm64-0.1.0.tgz` is 2.79 MB / 2 files / includes `aidememo-napi.darwin-arm64.node`; smoke includes `workflowStart(..., bm25Only:true)` and JS Error `code=InvalidArg` with `[entity_not_found]` prefix. |
| `scripts/aidememo-napi-publish.sh` | Shared publish engine with a timed Markdown summary. `AIDEMEMO_NAPI_PUBLISH_MODE=dry-run|publish`, `AIDEMEMO_NAPI_PUBLISH_SCOPE=platform|root|both`, and optional `AIDEMEMO_NAPI_EXPECT_VERSION` gate both local and CI release flows. Local dry-run passed both scopes and wrote stdout + `$GITHUB_STEP_SUMMARY`: total 4.14s, build 0.65s, test 1.50s, platform publish 0.60s, root publish 0.70s. The payload validator accepts both legacy npm JSON and the current package-name-wrapped npm JSON shape. |
| `scripts/aidememo-napi-publish-dry-run.sh` | Wrapper around the publish engine with `AIDEMEMO_NAPI_PUBLISH_MODE=dry-run`. Local macOS arm64: root payload `aidememo-napi@0.1.0`, 4 files, 4.18 KB packed, README included, no `.node`; platform payload `aidememo-napi-darwin-arm64@0.1.0`, 2 files, 2.79 MB packed; payload validators passed for both. |
| `scripts/aidememo-nif-version.sh` | Version gate for the Elixir package. With no args, verifies `Cargo.toml` workspace version equals `crates/aidememo-nif/mix.exs`; with a semver arg, updates both. Latest run: `0.1.0` pinned. |
| `scripts/bindings-release-smoke.sh` | Cross-binding readiness smoke with a timed Markdown summary. Runs `cargo check -p aidememo-python -p aidememo-napi -p aidememo-nif -p aidememo-ffi`, npm version/pack/install smoke, `aidememo-agent-sdk` wheel install smoke, `hermes-aidememo` wheel install smoke, and reports Python/Elixir/C optional package smokes based on local tools. Local macOS arm64 default path through `release-preflight`: cargo check 2.52s, npm version gate 0.05s, NAPI pack/install smoke 42.01s, agent SDK wheel smoke 3.61s, Hermes wheel smoke 4.70s, Python version gate 0.04s, total 52.93s; child pack-smoke summaries remain in stdout, but `$GITHUB_STEP_SUMMARY` contains only the one top-level `bindings-release-smoke` table. With `AIDEMEMO_BINDINGS_SMOKE_NPM=0 AIDEMEMO_BINDINGS_SMOKE_OPTIONAL=1`, the Python wheel build, Elixir `mix compile.cargo --force && mix test`, and C FFI smoke all passed. |
| `scripts/sdk-promotion-check.sh` | Package-SDK wording and parity gate for `aidememo-python`, `aidememo-napi`, and `aidememo-agent-sdk`. Default local run: ok=13, ready=3, blocked=2, fail=0, `local_ready=true`, `sdk_promotable=false` because public PyPI/npm installs are not verified. The gate now explicitly checks session-aware writes and pinned context API/docs for Python, Node, and the agent SDK. This does not block positioning `aidememo-agent-sdk` as the agent-facing SDK path. With `AIDEMEMO_SDK_PROMOTION_RUN_SCENARIO_K=1`, Scenario K still covers end-to-end workflow parity. Release preflight runs this gate by default; CI gets the same table in `$GITHUB_STEP_SUMMARY`; set `AIDEMEMO_RELEASE_PREFLIGHT_SDK_PROMOTION=0` only for focused debugging. |
| `.github/workflows/aidememo-napi-artifacts.yml` | Manual/tag workflow builds, tests, packs, and uploads root + platform `aidememo-napi` artifacts on Ubuntu, macOS, and Windows. |
| `.github/workflows/aidememo-python-publish-dry-run.yml` | Manual/tag workflow builds and validates `aidememo-python` PyPI payloads on Ubuntu without uploading. |
| `.github/workflows/aidememo-python-publish.yml` | Manual trusted-publisher workflow. It builds and validates distributions without PyPI permissions, uploads them as artifacts, then publishes via `pypa/gh-action-pypi-publish@release/v1` only when `dry_run=false`. Default `dry_run=true`; real publish requires a PyPI trusted publisher for this workflow and the `pypi-publish` environment. Local artifact-mode check: `AIDEMEMO_PYTHON_DIST_DIR=$(mktemp -d) scripts/aidememo-python-publish-dry-run.sh` produced wheel 2.8M + sdist 349K. |
| `.github/workflows/aidememo-agent-sdk-publish-dry-run.yml` | Manual/tag workflow builds and validates `aidememo-agent-sdk` PyPI payloads on Ubuntu without uploading. Tags use `aidememo-agent-sdk-v*`. |
| `.github/workflows/aidememo-agent-sdk-publish.yml` | Manual trusted-publisher workflow for `aidememo-agent-sdk`. It builds and validates distributions, uploads them as artifacts, then publishes via PyPA OIDC only when `dry_run=false`; real publish requires PyPI trusted-publisher setup for workflow `aidememo-agent-sdk-publish.yml` and environment `pypi-publish`. |
| `.github/workflows/hermes-aidememo-publish-dry-run.yml` | Manual/tag workflow builds and validates `hermes-aidememo` PyPI payloads on Ubuntu without uploading. Tags use `hermes-aidememo-v*`. |
| `.github/workflows/hermes-aidememo-publish.yml` | Manual trusted-publisher workflow for `hermes-aidememo`. It builds and validates distributions, uploads them as artifacts, then publishes via PyPA OIDC only when `dry_run=false`; real publish requires PyPI trusted-publisher setup for workflow `hermes-aidememo-publish.yml` and environment `pypi-publish`. |
| `.github/workflows/aidememo-napi-publish-dry-run.yml` | Manual/tag workflow runs the publish dry-run on Ubuntu with `id-token: write` reserved for the later trusted-publisher publish path. |
| `.github/workflows/aidememo-napi-publish.yml` | Manual trusted-publisher workflow. It publishes current-platform packages first, then the root wrapper. Default `dry_run=true`; real publish requires npm trusted-publisher setup for the exact workflow filename, `dry_run=false`, and `version` matching `package.json`. |

Package split: root `aidememo-napi` now ships the generated JS loader, types, README,
and optional dependencies. Platform packages ship exactly one native binary each:
`aidememo-napi-darwin-arm64`, `aidememo-napi-darwin-x64`,
`aidememo-napi-linux-arm64-gnu`, `aidememo-napi-linux-x64-gnu`, and
`aidememo-napi-win32-x64-msvc`. This matches the generated NAPI loader fallback names
and avoids publishing one platform's `.node` binary as the whole package.

Trusted-publisher notes: npm's current guidance requires Node 22.14+ and npm
11.5.1+ for trusted publishing, `id-token: write` in GitHub Actions, a
cloud-hosted runner, and an exact trusted-publisher registration for the
workflow filename. npm also notes trusted publishing automatically generates
provenance for public packages from public repositories, so the release
workflow uses OIDC rather than a long-lived `NPM_TOKEN`.

## Historical Notes

The old scratch-note directory was intentionally removed to keep the repository
focused on durable documentation and executable benchmarks. When adding a new
finding:

1. Put reusable code under `benchmarks/`, `bench/`, or `scripts/`.
2. Store machine-readable outputs under `bench/**/results` or
   `benchmarks/results`.
3. Summarize the user-facing result in this file or in the relevant
   `RESULTS.md`.
