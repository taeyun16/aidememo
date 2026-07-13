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
aidememo --backend libsqlite --store ~/.aidememo/team.sqlite \
  mcp-install --target codex --source-id team-a
```

The same boundary is applied to fact get/list/mutations, pinned context,
entities, and graph reads. Exact-content deduplication is keyed by
`(source_id, content_hash)`, so an identical sentence in two sources remains
two independent facts. Relations carry a separate owning source namespace;
scoped traversal/path reads require an exact namespace match, and legacy
unscoped edges are hidden rather than inherited by every source. For network
clients that must not select their own scope, use
`mcp-serve --auth-bindings-file`; see [MCP Setup](MCP.md).

Treat this as a trusted-team partition, not a hostile tenant boundary. Entity
names and types intentionally remain a shared ontology, and native/CLI callers
can choose their own `source_id`. Use separate stores for mutually untrusted
tenants. See [`Shared Memory Layer`](SHARED_MEMORY.md) for the reference
deployment patterns and production checklist.

## Avoid local store write contention

SQLite is the default backend. It uses WAL mode, starts writes with
`BEGIN IMMEDIATE`, keeps each SQLite busy wait at most one second, and
retries collisions with 20–150 ms jitter until the total
`store.lock_retry_ms` budget is exhausted. The optional redb backend uses the
same total budget to retry opening the store when another process holds redb's
exclusive file lock.

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
provider and configured HNSW index in a background task after the listener for
`aidememo mcp-serve` has bound. Health checks and lexical requests therefore
become available without waiting for the model cold load. Provider construction
is serialized with any concurrent first semantic request, so the background
task and request path cannot construct duplicate providers. `/admin/status`
reports `semantic_prewarm` as `warming`, `ready`, `failed`, or `disabled`. A
failed prewarm does not stop the server; a later semantic request retries the
normal on-demand path. To prewarm a daemon without changing config, start it
with `AIDEMEMO_PREWARM_SEMANTIC=1`.

### Optional Liquid AI LFM experiments

LFM is an optional external-model path, not a bundled asset or the global
embedding default. Keep first-stage retrieval behind the BM25-first auto-hybrid
gate, treat fact-type output as a review-only shadow hint, and use reranking
only after candidate recall is already high.

See [Liquid AI LFM Experiments](LFM_EXPERIMENTS.md) for model placement,
sidecar setup, training, and evaluation procedures. The concise claim summary
remains in [Evidence](EVIDENCE.md#model-placement).

## Back up a store

AideMemo's default store is SQLite. Use the backup command instead of copying
the hot `.sqlite` file directly: it creates a consistent SQLite snapshot,
writes a manifest with byte counts and SHA-256 checksums, and restore verifies
the manifest plus `PRAGMA integrity_check` before replacing the target store.
When `<store>.cold.sqlite` exists, backup snapshots it as well and records a
separate size and checksum entry in the same manifest. Local backups store it
as `wiki.cold.sqlite`; S3 backups store `wiki.cold.sqlite.zst`. If an archive
move overlaps the hot-then-cold snapshot window, the cold copy wins and backup
removes the duplicate FactId from the hot snapshot and its FTS/entity indexes.
Backup also takes a later hot metadata snapshot and copies only entity/name
mappings required by cold facts; any unresolved cross-tier entity reference
fails the backup instead of producing an orphan after restore.
The backup cursor is captured only after draining snapshot sync export to an
empty page, so `branch push --base` starts after the complete baseline instead
of replaying baseline records.

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
Restore is an offline maintenance operation: stop `aidememo daemon` and every
other process using the target store first. Restore checkpoints each existing
hot/cold SQLite store and writes complete `<store>.restore-prev...` safety
snapshots before removing WAL/SHM or HNSW sidecars; a busy checkpoint refuses
the restore. Every manifested payload is checksum-verified, decoded, and
SQLite-integrity-checked before the target is touched. If tier installation
fails, rollback restores and integrity-checks both prior snapshots, and any
rollback failure is reported together with the original restore error. A legacy
hot-only backup never inherits the target's existing cold tier: with `--force`,
restore preserves that file as the
`<store>.restore-prev.cold.sqlite` safety snapshot. Without `--force`, either an
existing hot store or cold tier blocks restore. Manifest payload names must be
simple filenames under the selected backup prefix, hot and cold objects must be
distinct, and the manifest backend must be SQLite-compatible.

## Pull replicated stores safely

`aidememo sync pull` tracks entities, facts, and relations with independent
high-water marks. Entity and fact update streams paginate by `(updated_at, id)`,
so same-millisecond mutations cannot fall between pages; legacy
timestamp-only cursors replay their boundary once. Relation sync fingerprints
the complete sorted relation snapshot with `relation_generation` and resumes
an unchanged snapshot with `relation_scan_key`. The fingerprint covers each
complete serialized relation record, so any late historical insert or weight,
evidence, scope, or other payload change restarts an idempotent full relation
scan. The older `(created_at, relation_key)` fields remain as a fallback for
pre-upgrade clients.

The complete JSONL envelope (supported leading header, valid record shapes,
and exactly one valid trailing cursor) is checked before the first write, so a
malformed or unsupported batch applies zero records. Store-level failures can
still leave already-applied records, but upserts are idempotent and the cursor
is withheld, making the whole batch safe to retry. The CLI holds a same-directory
cursor lock across pull/import/persist and atomically renames a unique temporary
cursor file after flushing it with `fsync`, so concurrent pull processes cannot
overwrite one another or persist a partially written cursor.

This remains a canonical-writer, pull-only replication model. Relation records
are append/upsert replicated; relation deletions are not propagated. A deletion
does change the snapshot generation and replay the surviving records, but the
wire has no tombstone with which to remove the downstream copy. Route all writes
and deletions through the canonical shared `mcp-serve`, then pull its current
state into local read caches rather than treating caches as peers.

## Share cloud agent branches

For agents that run on separate machines, start from a backup snapshot and push
per-agent branch logs instead of letting every worker write the same hot SQLite
file. The backup manifest records a sync cursor; `branch push --base` uses that
cursor to export only the records written after the baseline. This is also the
right shape for what-if memory experiments: fork several candidate stores from
one backup, let each attempt write local lessons, merge the best branch, and
leave the rest unmerged.
After all selected segments are merged with their original LWW ordering,
changed historical entity/fact IDs receive one coordinator relay timestamp so
downstream cursors that already passed their original IDs still pull the result.
Relations do not need that timestamp: an inserted or replaced relation changes
`relation_generation`, which makes downstreams replay the relation snapshot.

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
the existing idempotent `sync_import` path: identical or stale entity/fact
records are skipped, newer records with the same ID are applied by LWW order,
relation identities are inserted or replaced, and independent new facts are
appended. Use distinct branch ids per cloud agent or worker, and treat semantic
conflict handling between competing decisions as an application policy for
now. See
[`Branch Logs`](BRANCHES.md) for the speculative experiment workflow and
storage layout.
