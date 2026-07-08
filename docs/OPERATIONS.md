---
title: Operations
description: Practical guidance for stores, source scoping, daemon mode, and maintenance.
---

# Operations

This page covers the operational choices users usually need after the first
quickstart.

## Default local-only path

AideMemo's default path does not call external LLM APIs. The calling agent may
be an LLM, but AideMemo itself stores, searches, and serves memory through local
deterministic code plus optional local embedding sidecars.

| Surface | Default behavior | External LLM call? | Local model load? | Opt-in upgrade |
|---|---|---:|---:|---|
| Store | SQLite file with local BM25 and graph indexes | No | No | redb backend, S3 backup/branch transport |
| `fact add` / MCP writes | Preserve explicit `fact_type`; if omitted, use deterministic strong-cue inference and otherwise store `note` | No | No | shadow `fact_type_hint` logs and reviewed LFM LoRA sidecar |
| Privacy guard | Disabled until configured; no PII model is loaded on the default path | No | No | local OpenAI Privacy Filter sidecar before persistence |
| Search | BM25 probe first; `search.auto_hybrid=true` promotes only weak lexical probes when the semantic sidecar is ready | No | No for confident BM25 or missing sidecar | force `--hybrid`, MLX LFM embedding sidecar, fastembed/BGE eval path |
| Daemon / MCP server | Reuse one warm local process for repeated agent calls | No | Prewarm only when semantic config is enabled and ready | `AIDEMEMO_PREWARM_SEMANTIC=1`, remote TEI-compatible local network service |
| Extraction | Heuristic/local capture only | No | No | `extract.provider=openai` is explicit opt-in |
| Rerank | Off | No | No | TEI/ColBERT/BGE rerank sidecar |
| SDK / bindings | Same Rust core in process or via CLI fallback | No | Follows the selected search path | agent-specific capture policies before `fact_add` |

That is the product boundary: the default memory loop is zero-token and
vendor-independent, while small local models are measured sidecars placed only
where BM25 or deterministic capture is known to be weak.

## Use write-time privacy filtering

OpenAI Privacy Filter
([model](https://huggingface.co/openai/privacy-filter),
[source](https://github.com/openai/privacy-filter), and
[model card](https://cdn.openai.com/pdf/c66281ed-b638-456a-8ce1-97e9f5264a90/OpenAI-Privacy-Filter-Model-Card.pdf))
can run as an opt-in pre-persistence guard. It is a bidirectional
token-classification model, not a generative LLM, and is intended for on-prem
PII detection and masking workflows. The official model card lists eight span
categories:
`account_number`, `private_address`, `private_email`, `private_person`,
`private_phone`, `private_url`, `private_date`, and `secret`.

Do not treat it as a blanket anonymization or compliance guarantee. AideMemo
keeps this guard disabled by default, then fails closed when you explicitly
enable it and the sidecar cannot be reached.

AideMemo applies the guard inside `AideMemo::add_fact` / `fact_add_many`, so
CLI, MCP, extract-apply, pending approve, and native bindings that call the core
write API share the same pre-store behavior.

Before sidecar policy is applied, AideMemo also runs a deterministic local
secret prefilter for common API-key prefixes (`sk-proj-`, `sk-`, `github_pat_`,
`ghp_`, `gho_`, `xoxb-`, `AKIA`, `AIza`). This covers the observed OPF failure
mode where a bare key-like token can be labelled as `private_person` instead of
`secret`.

| Write surface | Placement | Default policy to test |
|---|---|---|
| CLI `aidememo fact add` | Before `FactInput` is persisted; daemon path reuses a warm local filter process | `report` first, then `redact` for high-confidence non-name spans |
| MCP `aidememo_fact_add` / `aidememo_fact_add_many` | Shared helper before batch insert so single and batch writes match | Batch `redact`; detected labels are logged, original spans are not persisted |
| `aidememo extract --apply` and pending approve | Filter the candidate content after extraction but before approval writes | Block `secret`; redact email/phone/address/account/url/date; review person names |
| Markdown ingest | Optional project-level guard for imported notes and logs | `report` mode unless the project explicitly opts into destructive redaction |

Run the local sidecar:

```bash
python3 -m pip install git+https://github.com/openai/privacy-filter.git
python3 scripts/privacy_filter_sidecar.py --device cpu --port 8090
```

On Apple Silicon, the measured lower-latency path is the MLX mxfp4 conversion:

```bash
python3 -m pip install git+https://github.com/Blaizzy/mlx-embeddings.git
hf download mlx-community/openai-privacy-filter-mxfp4 \
  --local-dir /private/tmp/openai-privacy-filter-mlx-mxfp4
python3 scripts/privacy_filter_mlx_sidecar.py \
  --model-dir /private/tmp/openai-privacy-filter-mlx-mxfp4 \
  --port 8091
```

Enable the write guard:

```bash
aidememo config set privacy.provider openai-privacy-filter
aidememo config set privacy.endpoint http://127.0.0.1:8090  # or the MLX sidecar port
aidememo config set privacy.mode redact
```

Equivalent config shape:

```toml
[privacy]
provider = "openai-privacy-filter"   # empty disables the guard
mode = "report"                      # report | redact | block
endpoint = "http://127.0.0.1:8090"   # local sidecar; no remote inference by default
api_key_env = ""
block_labels = ["secret"]
redact_labels = ["private_email", "private_phone", "private_address", "account_number", "private_url", "private_date"]
review_labels = ["private_person"]
store_summary = true                 # reserved for label/count summaries; raw spans are never stored
```

Keep `private_person` separate from the first automatic-redaction pass. In
project memory, names can be legitimate entity keys, but in personal/team logs
they can also be sensitive. Treat this as a policy decision, not a model
accuracy decision.

Measured local CPU cost on macOS arm64 after the checkpoint is warm:
sidecar `/filter` p50 was about 244 ms and `aidememo fact add` with the guard
was about 261 ms p50, versus about 17 ms p50 without privacy filtering. Warm
sidecar RSS was about 3.8 GB. Keep this opt-in for safety-sensitive writes or
team/project stores rather than enabling it for every scratch-memory capture.

On Apple Silicon, prefer the MLX mxfp4 sidecar once the runtime is available:
`mlx-community/openai-privacy-filter-mxfp4` measured at about 739 MB on disk,
1.28 GB warm RSS, sidecar `/filter` p50 about 18 ms, and `aidememo fact add`
p50 about 51 ms versus a 22.5 ms baseline. The measured MLX path requires
`mlx-embeddings` 0.1.1 from GitHub main until the PyPI release catches up, and
the sidecar should be single-threaded because MLX stream state is thread-local.
This makes the guard a strong prewarmed opt-in for shared/project stores, but
still not a universal default.

Evaluation gate before enabling writes:

1. Build a fixture from synthetic examples, local agent traces, and redacted
   public traces with expected spans and labels.
2. Measure span recall for `secret`, email, phone, address, URL, account/date,
   and person-name cases separately.
3. Track utility loss: entity recall, fact-type accuracy, retrieval R@K, and
   answerability before/after redaction.
4. Require zero raw secret persistence in strict mode and inspect all false
   negatives before promotion.
5. Pin the source to the official OpenAI repository/model handle. Avoid
   typosquatted repositories that mimic the model card.

## Choose a store layout

| Layout | Use when |
|---|---|
| One local default store | Personal memory on one machine |
| One store per project | Repos should not share memory |
| One shared team store | Several local agents should share context |
| `source_id` inside one store | Teams, agents, or tenants share infrastructure but need scoped retrieval |

For scripts and CI, prefer explicit store paths:

```bash
aidememo --store ./project.sqlite query "release checklist"
```

Keep the file suffix aligned with the backend (`.sqlite` for SQLite /
`libsqlite`, `.redb` for redb). The suffix is not required by the storage
engine, but `aidememo doctor` warns when `store.backend` and the path extension
point to different persistence layers because that is easy to misread later.

## Use source scoping

`source_id` prevents neighbouring project or team facts from leaking into a
query.

```bash
aidememo fact add \
  "Decision: Team A deploys through release train alpha." \
  --type decision \
  --entities Release \
  --source-id team-a

aidememo query "release train" --source-id team-a
```

For MCP installs:

```bash
aidememo --backend libsqlite mcp-install --target codex --source-id team-a
```

## Avoid local store write contention

SQLite is the default backend and uses `store.lock_retry_ms` as its busy
timeout for short write collisions. The optional redb backend uses the same
setting to retry opening the store when another process holds redb's exclusive
file lock.

For shared writes, run one daemon:

```bash
aidememo --backend libsqlite daemon start --store ~/.aidememo/team.sqlite --port 3000
```

Daemon auto-discovery is backend-aware. A daemon started for the same path with
`redb` will not be reused by a CLI invocation configured for `sqlite` /
`libsqlite`, and vice versa.

For brief local contention, configure the wait budget:

```bash
aidememo config set store.lock_retry_ms 5000
```

## Keep memory useful

Run health checks:

```bash
aidememo doctor
aidememo lint
```

Archive or consolidate old memory:

```bash
aidememo fact archive --older-than 90d --type note
aidememo consolidate --semantic-threshold 0.85 --dry-run
```

After large consolidation, rebuild current vectors:

```bash
aidememo vector-rebuild --current-only
```

## Pick embedding mode

The default path is good for most code and docs workflows. Use `--bm25-only` for
deterministic hooks, demos, and CI checks:

```bash
aidememo workflow start "Release smoke ticket" --bm25-only
```

Force semantic/hybrid retrieval when wording may differ between the question
and the stored fact and you want to pay the semantic path on every query:

```bash
aidememo search "favorite camera setup" --hybrid
```

Auto-hybrid is the default search policy. AideMemo first runs a BM25 probe and
promotes to semantic retrieval when the probe is empty, the top score is weak,
or the query is CJK and BM25 evidence is not strong. Keep the default
thresholds unless a store-specific eval shows a better cutoff:

```bash
aidememo config set search.auto_hybrid true
aidememo config set search.auto_hybrid_min_bm25_hits 1
aidememo config set search.auto_hybrid_min_top_score 1.0
```

Use `--bm25-only` or `search.auto_hybrid=false` for deterministic demos, hooks,
CI checks, or stores where surface-form BM25 is already saturated:

```bash
aidememo search "Redis timeout" --bm25-only
aidememo config set search.auto_hybrid false
```

With the default HNSW semantic index, auto-hybrid does not cold-load the
embedding provider when the HNSW sidecar is missing; it stays on the BM25 probe
until `aidememo vector-rebuild` creates the sidecar. In a fresh CLI process with
a sidecar present, promoted weak/CJK queries still pay the embedding-model cold
load. The auto-hybrid path falls back to BM25 if semantic promotion fails. For
repeated agent calls, run through the daemon so the model is warm.

For daemon-backed stores, `search.auto_hybrid=true` prewarms the semantic
provider when `aidememo mcp-serve` starts, so the startup pays the model load
instead of the first user query. To prewarm a daemon without changing config,
start it with `AIDEMEMO_PREWARM_SEMANTIC=1`.

### Try Liquid AI LFM models

Treat LFM models as optional external models, not bundled AideMemo assets. The
LFM Open License v1.0 has commercial-use conditions, and different LFM model
families fit different parts of the system.

Use the LFM family as a layered small-model stack, not as one global model
switch:

| AideMemo surface | First LFM candidate | Use when | Do not use when |
|---|---|---|---|
| First-stage semantic retrieval | `mlx-community/LFM2.5-Embedding-350M-4bit` | BM25 returns weak/empty candidates, especially multilingual queries without strong lexical anchors. The tracked-docs gate improved `R@8` from 0.656 to 0.812 by promoting only the lexical failure slice; the HF agent-trace gate showed pure LFM dense underperforms when BM25 is already strong. Keep `search.auto_hybrid=true` and use a warm daemon for repeated calls. | Plain lexical/code/doc search is already saturated, or you would run LFM dense for every query. |
| BERT-family semantic baseline | `fastembed` / `bge-small-en-v1.5` | Offline or high-stakes comparison where a heavier BERT-style encoder is acceptable. On the HF 60-doc slice it reached `R@8=0.994`, close to BM25/BGE head quality, and is useful as a validation baseline. | Agent hot paths: measured daemon query mean was ~1680 ms/query, far slower than LFM query embedding (~31 ms) and BM25/model2vec paths. |
| Reranking | `mlx-community/LFM2.5-ColBERT-350M-4bit` or `LiquidAI/LFM2.5-ColBERT-350M` sidecar | BM25/hybrid candidate recall is high but the top result is often misordered. | The right fact is absent from the candidate set unless you deliberately build an all-doc / multi-vector ColBERT index. |
| Fact extraction / type classification | `LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit` with a fact-type LoRA adapter | You want local high-confidence `fact_type_hint` candidates for residual/default-note cases or pending review. The corpus-only 240-iteration LoRA beat deterministic inference on both the seed coding-agent corpus test and the older 45-case holdout. | You need automatic high-precision writes from unlabelled real traffic. Grow and validate a reviewed shadow corpus first. |
| Query routing | deterministic rules + search confidence | You need to choose BM25-only vs dense vs ColBERT vs aggregate. | Do not spend an LFM text-generation call here yet; the MLX LM router micro-eval was weaker than rules. |
| Batch consolidation / conflict review | review hints only; no automatic LFM placement yet | A human or stronger agent reviews duplicate/supersede suggestions. | Do not let the tested LFM text-generation models write supersede/archive decisions automatically. |
| Image / screenshot memory capture | blocked pending cleaner `mlx-vlm` compatibility | You are experimenting with screenshots or diagrams and can tolerate review-only output. | Do not ship screenshot-to-fact capture from the current VL smoke; JSON/OCR output was unstable. |
| Voice / meeting capture | no Mac MLX placement yet | A future local audio runtime has WER and fact-precision measurements. | You only need text retrieval over an existing wiki. |

This is the product distinction to validate: AideMemo should be able to prove
that small local models improve the memory system at the exact failure point
where they are used, instead of depending on one large remote model for every
memory operation.

Use `LiquidAI/LFM2.5-Embedding-350M` for a dense multilingual retrieval
experiment. It is an asymmetric embedding model, so configure query and document
prefixes before rebuilding vectors:

```bash
aidememo config set model.provider openai
aidememo config set model.endpoint http://127.0.0.1:8080/v1/embeddings
aidememo config set model.name LiquidAI/LFM2.5-Embedding-350M
aidememo config set model.dimension 1024
aidememo config set model.query_prefix "query: "
aidememo config set model.document_prefix "document: "
aidememo vector-rebuild --current-only
aidememo search "redis timeout root cause" --hybrid
```

When running this model directly through `sentence-transformers`, Liquid's model
card loads it with `trust_remote_code=True` for the bidirectional patches. Prefer
serving it behind an audited, isolated OpenAI-compatible embedding endpoint
rather than executing Hub code inside the AideMemo process.

To check whether dense helps a workload before wiring a production server, run
the scenario micro-eval:

```bash
python3 scripts/lfm_dense_eval.py \
  --aidememo target/debug/aidememo \
  --model LiquidAI/LFM2.5-Embedding-350M
```

If `embedding_health.valid` is false, do not use the numbers as retrieval
evidence. The safe `sentence-transformers` path can load a 1024-dimensional
vector while still producing identical embeddings without the bidirectional
patches.

On Apple Silicon, prefer the MLX conversion for local dense validation:

```bash
hf download mlx-community/LFM2.5-Embedding-350M-4bit \
  --local-dir /private/tmp/lfm25-embedding-mlx-4bit

python3 scripts/lfm_mlx_dense_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit
```

The MLX path needs Metal access. In headless or sandboxed sessions, `mlx` may
fail with `No Metal device available`; run it from a process that can see the
Apple GPU.

To put the MLX LFM embedder behind the real AideMemo semantic path, run the
TEI-compatible sidecar and configure the experimental provider alias:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_embedding_sidecar.py \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --port 8088

aidememo config set model.provider lfm-sidecar
aidememo config set model.endpoint http://127.0.0.1:8088
aidememo config set model.name mlx-community/LFM2.5-Embedding-350M-4bit
aidememo config set model.dimension 1024
aidememo config set model.query_prefix "query: "
aidememo config set model.document_prefix "document: "
aidememo vector-rebuild --current-only
aidememo daemon start
aidememo search "레디스 장애 원인" -l 8
```

This keeps MLX and Liquid model code outside the Rust process while still using
the normal `auto_hybrid` + HNSW flow. If the sidecar is not running, semantic
promotion fails non-fatally and AideMemo falls back to the BM25 probe.

Use `LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit` for local fact extraction and type
classification experiments only as a pending-review helper. The Mac MLX
micro-eval did not support automatic writes: best observed extraction was
`fact_type_accuracy=0.63`, `entity_recall=0.88`, while router/consolidation
accuracy stayed too low. The 230M/350M MLX models were faster but less accurate
for extraction. The closed-label classifier eval confirms the same placement:
scoring one of the fixed AideMemo labels with `lfm_mlx_fact_type_eval.py`
still trailed deterministic strong-cue inference on the 45-case mixed-language
fixture (`0.33` best LFM vs `0.69` baseline), though the best 1.2B-Instruct run
had a tiny high-confidence review slice (`confidence >= 0.8`: 6/6 correct).
Use that as `fact_type_hint` / pending-review evidence, not as a default writer.
If you want to test whether fine-tuning changes that conclusion, generate a
supervised label dataset and train an MLX LoRA adapter. The first synthetic-only
smoke reached `0.84` on the 45-case holdout after 240 iterations. A later
coding-agent seed corpus added `fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl`
(108 reviewed-style rows, 12 per label, train/valid/test = 72/18/18); the
corpus-only adapter reached `0.61` on that harder 18-case test versus the
deterministic baseline at `0.39`, and `0.82` on the original 45-case holdout
versus baseline `0.69`. Keep this as a shadow `fact_type_hint` path until real
capture traffic confirms the threshold and the `claim` / `convention` / `note`
boundaries are stable.

```bash
hf download LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit \
  --local-dir /private/tmp/lfm25-12b-instruct-mlx-4bit

python3 scripts/lfm_mlx_lm_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --suite extraction \
  --prompt-style compact

python3 scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --prompt-style compact \
  --summary-only

python3 scripts/lfm_fact_type_sft_data.py \
  --out /private/tmp/aidememo-lfm-fact-type-corpus-sft

/private/tmp/aidememo-lfm-venv/bin/mlx_lm.lora \
  --model /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --train \
  --data /private/tmp/aidememo-lfm-fact-type-corpus-sft \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --fine-tune-type lora \
  --mask-prompt \
  --num-layers 8 \
  --batch-size 1 \
  --grad-accumulation-steps 8 \
  --iters 240 \
  --learning-rate 5e-5

python3 scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --cases-file fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl \
  --case-split test \
  --prompt-style compact \
  --summary-only
```

For the ongoing shadow loop, append reviewed agent-labelled facts as JSONL
instead of editing the seed fixture directly:

```json
{"id":"agent-run-20260707-001","text":"Tried the daemon prewarm path, but the real issue was a stale HNSW sidecar.","fact_type":"lesson","scenario":"daemon_ops","language":"en","split":"train"}
```

For live MCP agents, let AideMemo append successful writes to a supervised
shadow corpus without changing the fact that lands in the store:

```bash
export AIDEMEMO_FACT_TYPE_SHADOW_LOG=~/.aidememo/fact-type-shadow.jsonl
aidememo mcp-serve --port 3000
```

Rows are append-only JSONL and include the stored fact id, text, fact_type,
`label_source` (`explicit`, `inferred`, or `default`), entities, source_id,
origin (`mcp_fact_add` / `mcp_fact_add_many`), and timestamp. Use the explicit
agent-labelled rows for supervised training; inferred/default rows are useful
for audit, but are skipped by `lfm_fact_type_sft_data.py` unless
`--include-inferred-labels` is passed. Rows with `fact_type_hint` are also
skipped by default because the stored label and strong-cue hint disagree; pass
`--include-disputed-labels` only after review.

Then train with both the seed corpus and the reviewed capture file:

```bash
python3 scripts/lfm_fact_type_sft_data.py \
  --corpus ~/.aidememo/fact-type-shadow.jsonl \
  --out /private/tmp/aidememo-lfm-fact-type-shadow-sft
```

Run the tuned adapter as a hint-only sidecar:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --confidence-threshold 0.8 \
  --input-jsonl ~/.aidememo/fact-type-shadow.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-fact-type-hints.jsonl

python3 scripts/lfm_fact_type_threshold_eval.py \
  --labels-jsonl ~/.aidememo/fact-type-shadow.jsonl \
  --label-split test \
  --predictions-jsonl /private/tmp/aidememo-fact-type-hints.jsonl \
  --min-precision 0.95 \
  --max-baseline-correct-harms 0
```

To stress-test the same sidecar on local agent behavior logs without external
LLM calls, build weak-labelled probes from AgentStep / Hermes logs and run the
same threshold gate:

```bash
python3 scripts/lfm_fact_type_log_fixture.py \
  --out-dir /private/tmp/aidememo-lfm-log-probes \
  --max-rows 72 \
  --max-per-label 12
```

To run the broader public-trace gate, build weak-labelled probes from Hugging
Face agent traces. This fetches rows through the Dataset Viewer API, redacts
emails and long identifiers, and writes only compact candidate-memory JSONL
under `/private/tmp`:

```bash
python3 scripts/lfm_fact_type_hf_probe.py \
  --out-dir /private/tmp/aidememo-lfm-hf-probes \
  --source-rows 100 \
  --max-rows-per-dataset 100 \
  --max-per-label 25

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --input-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-lfm-hf-probes/fact_type_hints_hf.jsonl

python3 scripts/lfm_fact_type_threshold_eval.py \
  --labels-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --label-split test \
  --predictions-jsonl /private/tmp/aidememo-lfm-hf-probes/fact_type_hints_hf.jsonl \
  --confidence-grid 0.80,0.90,0.95,0.98
```

Treat this as a raw-trace stress test, not reviewed truth. The July 8 public
HF run showed that `confidence >= 0.80` is too loose on raw trace events, but
high-confidence hints become useful after deterministic extraction has already
filtered the stream down to durable memory-candidate categories. The script
also writes `high_signal_hf_fact_type_probe.jsonl` for that second gate.

To test retrieval placement on the same HF traces, convert the compact probe
rows into corpus / query / qrels files, then run BM25, model2vec, LFM dense,
and optional fastembed/BGE profiles:

```bash
python3 scripts/lfm_hf_agent_trace_retrieval_fixture.py \
  --probe-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --out-dir /private/tmp/aidememo-lfm-hf-retrieval \
  --variants surface,paraphrase,cjk \
  --max-docs-per-source 60

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_docs_recall_eval.py \
  --aidememo target/debug/aidememo \
  --no-default-docs \
  --corpus-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_corpus.jsonl \
  --queries-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_queries.jsonl \
  --qrels-tsv /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_qrels.tsv \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --candidate-limit 8 \
  --summary-only \
  --output-json /private/tmp/aidememo-lfm-hf-retrieval/eval_summary.json
```

For the BGE/BERT-family fastembed baseline, build the CLI with the optional
feature and keep the slice smaller unless you are running an offline batch:

```bash
cargo build -p aidememo-cli --features fastembed

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_docs_recall_eval.py \
  --aidememo target/debug/aidememo \
  --no-default-docs \
  --corpus-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_corpus.jsonl \
  --queries-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_queries.jsonl \
  --qrels-tsv /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_qrels.tsv \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --fastembed-model bge-small-en-v1.5 \
  --candidate-limit 8 \
  --summary-only
```

For one-off manual hints, pass text directly:

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --confidence-threshold 0.8 \
  --text "Tried daemon prewarm, but the real issue was a stale HNSW sidecar."
```

The sidecar returns `suggested_fact_type`, confidence, margin, runner-up, and
`accepted`; it does not write to AideMemo. Hook it into pending review or UI
surfacing first. Do not let it override an explicit agent `fact_type` until a
larger reviewed capture corpus proves low baseline-correct harm.

Use synthetic augmentation only as an experiment:

```bash
python3 scripts/lfm_fact_type_sft_data.py \
  --corpus /path/to/reviewed-agent-facts.jsonl \
  --examples-per-label 40 \
  --out /private/tmp/aidememo-lfm-fact-type-shadow-synth40-sft
```

The July 7 comparison showed that synthetic augmentation can bias the taxonomy:
the mixed adapter kept corpus-test accuracy at `0.61`, but dropped the older
holdout from `0.82` to `0.71` and classified every original `note` example as
`lesson`. Treat synthetic rows as a stress test, not as the default way to grow
the training set.

Use `LiquidAI/LFM2.5-ColBERT-350M` as a sidecar reranker before considering a
native multi-vector index. `LiquidAI/LFM2-ColBERT-350M` works with the same
script via `--model LiquidAI/LFM2-ColBERT-350M`; keep the default on 2.5 unless
you are reproducing the original LFM2 results. AideMemo stores one vector per
fact today, while ColBERT stores one vector per token and scores with MaxSim.
On Apple Silicon, prefer the MLX conversion for local MaxSim validation:

```bash
hf download mlx-community/LFM2.5-ColBERT-350M-4bit \
  --local-dir /private/tmp/lfm25-colbert-mlx-4bit

python3 scripts/lfm_mlx_colbert_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-colbert-mlx-4bit \
  --candidate-limit 8
```

```bash
aidememo search "redis timeout root cause" --json -l 50 \
  | python3 scripts/lfm_colbert_rerank.py \
      --query "redis timeout root cause" \
      --top-k 10
```

The sidecar keeps `trust_remote_code` off by default. Only pass
`--trust-remote-code` after auditing and approving the model repository code.

## Back up a store

AideMemo's default store is SQLite. Use the backup command instead of copying
the hot `.sqlite` file directly: it creates a consistent SQLite snapshot,
writes a manifest with byte counts and SHA-256 checksums, and restore verifies
the manifest plus `PRAGMA integrity_check` before replacing the target store.

```bash
aidememo --store ~/.aidememo/wiki.sqlite backup create ~/backups/aidememo
aidememo --store ~/.aidememo/wiki.sqlite backup restore ~/backups/aidememo/backup-01... --force
```

S3 backup / restore is an optional build feature. Build the CLI with
`--features s3`, then use an S3 prefix as the destination or source:

```bash
aidememo --store ~/.aidememo/wiki.sqlite backup create s3://my-bucket/aidememo
aidememo --store ~/.aidememo/wiki.sqlite backup restore s3://my-bucket/aidememo/backup-01... --force
```

The S3 path is for backup storage, not for using S3 as the live database.
Restores replace the local SQLite store and remove stale SQLite WAL/SHM and
HNSW sidecar files next to the target.

## Share cloud agent branches

For agents that run on separate machines, start from a backup snapshot and push
per-agent branch logs instead of letting every worker write the same hot SQLite
file. The backup manifest records a sync cursor; `branch push --base` uses that
cursor to export only the records written after the baseline. This is also the
right shape for what-if memory experiments: fork several candidate stores from
one backup, let each attempt write local lessons, merge the best branch, and
leave the rest unmerged.

```bash
# Coordinator creates a baseline snapshot.
aidememo --store ./coordinator.sqlite backup create ./shared

# Agent restores the baseline, writes local memory, then pushes a branch segment.
aidememo --store ./agent-a.sqlite backup restore ./shared/backup-01... --force
aidememo --store ./agent-a.sqlite fact add "Agent A learned X" --entities AgentA --type lesson
aidememo --store ./agent-a.sqlite branch push \
  --branch agent-a \
  --base ./shared/backup-01... \
  ./shared

# Coordinator merges one branch, or omit --branch to merge every branch under SOURCE.
aidememo --store ./coordinator.sqlite branch merge --branch agent-a ./shared
```

With `--features s3`, the same commands accept `s3://bucket/prefix` for the
backup and branch-log locations. S3 branch payloads are zstd-compressed JSONL
segments with a manifest containing byte counts and SHA-256 checksums.

This is not full multi-master conflict resolution. Merge currently relies on
the existing idempotent `sync_import` path: duplicate entities, facts, and
relations are skipped, while independent new facts are appended. Use distinct
branch ids per cloud agent or worker, and treat semantic conflict handling
between competing decisions as an application policy for now. See
[`Branch Logs`](BRANCHES.md) for the speculative experiment workflow and
storage layout.
