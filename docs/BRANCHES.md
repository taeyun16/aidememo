---
title: Branch Logs
description: Use AideMemo branch logs for speculative memory changes and cloud agent experiments.
---

# Branch Logs

Branch logs let several agents start from the same backup snapshot, write local
memory independently, and later merge only the branch you want to keep.

This is useful when the memory itself is part of an experiment:

| Situation | Why branch logs help |
|---|---|
| Several cloud agents try the same task from one baseline | Each worker writes its own facts without sharing a hot SQLite file. |
| You compare prompting, extraction, or retrieval strategies | Keep candidate lessons separate, run an external eval, then merge the winner. |
| A risky automation may write noisy memory | Push the branch log, inspect it, and merge only if the result is useful. |
| You need a replayable artifact for an agent run | The segment JSONL plus manifest records what that branch tried to add. |

Do not use branch logs as full multi-master conflict resolution. Merge is
idempotent and append-oriented: duplicate records are skipped, independent new
facts are appended, and semantic conflicts such as two competing decisions are
left to the caller's policy.

## Workflow

Create a baseline backup:

```bash
aidememo --store ./main.sqlite backup create ./shared
```

Restore that backup into separate candidate stores:

```bash
aidememo --store ./candidate-a.sqlite backup restore ./shared/backup-01... --force
aidememo --store ./candidate-b.sqlite backup restore ./shared/backup-01... --force
```

Run different attempts and write their memory locally:

```bash
aidememo --store ./candidate-a.sqlite fact add \
  "Candidate A used broad context and produced noisy results." \
  --type lesson \
  --entities Experiment

aidememo --store ./candidate-b.sqlite fact add \
  "Candidate B used focused context and produced the best result." \
  --type lesson \
  --entities Experiment
```

Push each branch as a delta after the backup cursor:

```bash
aidememo --store ./candidate-a.sqlite branch push \
  --branch candidate-a \
  --base ./shared/backup-01... \
  ./shared

aidememo --store ./candidate-b.sqlite branch push \
  --branch candidate-b \
  --base ./shared/backup-01... \
  ./shared
```

Merge only the winning branch:

```bash
aidememo --store ./main.sqlite branch merge --branch candidate-b ./shared
```

Discarding a branch means not merging it. If you want to remove the stored
artifact too, delete that branch directory or S3 prefix:

```text
./shared/branches/candidate-a/
s3://bucket/prefix/branches/candidate-a/
```

Omit `--branch` to merge every branch under the source:

```bash
aidememo --store ./main.sqlite branch merge ./shared
```

## SDK And Binding Calls

The Python composition SDK exposes the same flow for code-first agents:

```python
from aidememo_agent import Memory

candidate = Memory.open(store_path="./candidate-b.sqlite", storage_backend="libsqlite")
candidate.branch_push(
    "candidate-b",
    "./shared",
    base="./shared/backup-01...",
)

main = Memory.open(store_path="./main.sqlite", storage_backend="libsqlite")
main.branch_merge("./shared", branch="candidate-b")
```

`aidememo-python` exposes `branch_push(branch, destination, base=None)` and
`branch_merge(source, branch=None)`. `aidememo-napi` exposes `branchPush` and
`branchMerge` with JSON-string reports. `aidememo_nif` exposes
`AideMemoNif.branch_push/4` and `AideMemoNif.branch_merge/3` with decoded map
reports. Local paths use the already-open native store handle, which avoids
reopening the same file from SDK/plugin code. S3 branch URIs should go through
the CLI, because S3 support is controlled by the CLI's `--features s3` build.

## Storage Layout

Local branch logs are stored under:

```text
<DEST>/branches/<branch-id>/segments/<segment-id>.jsonl
<DEST>/branches/<branch-id>/segments/<segment-id>.manifest.json
```

S3 branch logs use the same prefix shape and compress payloads:

```text
s3://bucket/prefix/branches/<branch-id>/segments/<segment-id>.jsonl.zst
s3://bucket/prefix/branches/<branch-id>/segments/<segment-id>.manifest.json
```

Every segment manifest stores byte counts and SHA-256 checksums for both the
stored object and decoded JSONL payload. Merge verifies these before import.

## Guarantees And Limits

What is covered:

- `branch push --base <BACKUP>` exports records written after the backup
  manifest's sync cursor.
- `branch merge --branch <ID>` imports only that branch.
- Merging the same segment again does not duplicate facts because it goes
  through `sync_import`.
- S3 is a transport for branch artifacts, not the live database backend.

What is not covered yet:

- Automatic quality scoring of candidates.
- Semantic conflict resolution between competing decisions.
- A first-class `branch delete` command.
- Bidirectional live replication between running stores.

For the current validation evidence, see `docs/MEASUREMENTS.md`,
"Branch Log Push / Merge".
