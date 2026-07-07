---
title: Operations
description: Practical guidance for stores, source scoping, daemon mode, and maintenance.
---

# Operations

This page covers the operational choices users usually need after the first
quickstart.

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

Enable semantic/hybrid retrieval when wording may differ between the question
and the stored fact:

```bash
aidememo search "favorite camera setup" --hybrid
```

Use auto-hybrid when you want the LFM/HNSW path only at the recall failure
point. AideMemo first runs a BM25 probe and promotes to semantic retrieval when
the probe is empty, the top score is weak, or the query is CJK:

```bash
aidememo search "레디스 장애 원인" --auto
aidememo config set search.auto_hybrid true
aidememo config set search.auto_hybrid_min_bm25_hits 1
aidememo config set search.auto_hybrid_min_top_score 1.0
```

Do not make this the global default for every fresh CLI install yet. It is
safe to opt in per store after the embedding provider and HNSW sidecar are
ready, or when searches go through a warm daemon. In a fresh offline HOME,
semantic promotion can fail because the embedding model is not cached; in a
fresh CLI process with the model cached, promoted CJK queries still pay the
embedding-model cold load. The `--auto` path falls back to BM25 if semantic
promotion fails, but the latency contract is still different from the default
BM25-only CLI search.

For daemon-backed stores, `search.auto_hybrid=true` also prewarms the semantic
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
| First-stage semantic retrieval | `mlx-community/LFM2.5-Embedding-350M-4bit` | BM25 returns weak/empty candidates, especially multilingual or paraphrase-heavy queries. In the Mac MLX smoke, 4-bit beat 8-bit on this fixture. Use `aidememo search --auto` or `search.auto_hybrid=true` to gate this path by BM25 confidence. | Plain lexical/code/doc search is already saturated. |
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
