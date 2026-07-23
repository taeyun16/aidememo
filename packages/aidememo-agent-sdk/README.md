# aidememo-agent-sdk

Agent-friendly memory SDK for code-executing agents, including Codex, Claude
Code, Hermes, CI jobs, and local scripts.

The SDK uses `aidememo-python` when available and otherwise falls back to the `aidememo`
CLI. This keeps the first-use path portable while still taking the fast
in-process route in environments that install the native binding.

Use it when memory needs to be a programmable working set: fan out searches,
dedupe rows, check coverage, aggregate exact counts/timelines, and batch-write
facts without spending model-visible tool calls on every intermediate step.

## Install

```bash
python -m pip install aidememo-agent-sdk

# Optional in-process native binding
python -m pip install "aidememo-agent-sdk[binding]"
```

The fallback path needs the `aidememo` CLI on `$PATH`. The `binding` extra
installs the published `aidememo-python` package and enables the optional
in-process fast path.

## External Codex / Claude worker lane

The package installs `aidememo-worker-lane`, a shell-free receiver runner for
an already-dispatched handoff:

```bash
aidememo-worker-lane handoff-... \
  --actor-id codex-two \
  --agent codex \
  --workspace "$PWD" \
  --store ~/.aidememo/wiki.sqlite \
  --source-id release-team \
  --kanban-task task-42

aidememo-worker-lane handoff-... \
  --actor-id claude-main \
  --agent claude \
  --workspace "$PWD"
```

Codex runs through `codex exec --ephemeral --sandbox workspace-write`; Claude
runs through `claude --print --permission-mode acceptEdits
--no-session-persistence`. The handoff packet is passed on stdin, and its
resume environment becomes `AIDEMEMO_SESSION_ID`, `AIDEMEMO_SOURCE_ID`, and
`AIDEMEMO_ACTOR_ID` for the child process. No shell command is constructed.
The runner validates the agent binary and workspace before accepting the
assignment, so a local setup error does not claim the handoff.

For recurring accounts, register a credential-free local profile once:

```bash
aidememo agent add codex-two --type codex \
  --home /path/to/codex-two-home --workspace "$PWD" \
  --source-id release-team
aidememo handoff run codex-two
```

The config root becomes `CODEX_HOME` or `CLAUDE_CONFIG_DIR`; the default
`env_policy=core` avoids inheriting unrelated account tokens. Repeat
`--pass-env NAME` to allow a named variable without storing its value.

The adapters request the same `summary`, `changed_files`, `validations`,
`done_when_met`, and `blockers` result. On success, it is recorded on the same
session before acknowledgement completes. On non-zero exit, timeout, or
`done_when_met=false`, an `error`
fact is recorded and the assignment stays `accepted` so Hermes or another
upstream scheduler can decide whether to retry or block. The runner never
claims that a Hermes Kanban card is complete and does not register a Hermes
`spawn_fn`; it is the external process adapter that such a lane can call.

The same path is available from Python:

```python
from pathlib import Path
from aidememo_agent import WorkerLaneConfig, run_external_assignment

result = run_external_assignment(
    mem.client,
    WorkerLaneConfig(
        handoff_id="handoff-...",
        actor_id="codex-two",
        agent="codex",
        workspace=Path.cwd(),
        kanban_task="task-42",
    ),
)
```

## Quick Start

```python
from aidememo_agent import Memory

mem = Memory.open(
    source_id="project:aidememo",
    actor_id="codex:account-a",
    storage_backend="libsqlite",
)

rows = mem.search_rows([
    "release preflight decisions",
    {"query": "Hermes source_id lock retry lessons", "topic": "Hermes"},
])
coverage = mem.coverage_by(rows, ["fact_type"])
timeline = mem.aggregate_many([
    {"query": "release preflight", "op": "timeline"},
])

mem.remember([
    {
        "content": "Lesson: use source-scoped SDK fanout for multi-agent memory checks.",
        "fact_type": "lesson",
        "entities": ["aidememo", "Codex"],
    }
])

pack = mem.client.workflow_start(
    "Fix Redis timeout in worker",
    source="github:org/app#123",
    source_id="project:aidememo",
    actor_id="codex:account-a",
)
canvas = mem.session_canvas(pack["session_id"], limit=20)
handoff = mem.handoff(
    pack["session_id"],
    from_route="codex/coding",
    to_route="hermes/reviewer",
    focus="Verify package metadata, then run release preflight",
    done_when="The installed wheel matches workspace metadata and preflight passes",
    source_id="codex-aidememo",
)
handoff_packet = mem.handoff_packet(
    pack["session_id"],
    from_actor="codex-one",
    to_actor="codex-two",
    from_route="codex/coding",
    to_route="codex/reviewer",
    focus="Verify package metadata, then run release preflight",
    done_when="The installed wheel matches workspace metadata and preflight passes",
    source_id="codex-aidememo",
    dispatch=True,
)
pending = mem.handoff_inbox(actor_id="codex-two")
accepted = mem.handoff_accept(pending[0]["handoff_id"], actor_id="codex-two")
print(accepted["resume"]["env"])
print(accepted["content"])
[result_id] = mem.remember([
    {"content": "Focused tests pass", "entities": ["Release"]}
])
mem.handoff_return(pending[0]["handoff_id"], result_id,
                   outcome="succeeded", actor_id="codex-two")
mem.handoff_outbox(actor_id="codex-one", include_completed=True)
profile = mem.project_profile(limit=80)

mem.remember(
    [
        {
            "content": "Decision: Redis timeout follow-ups stay attached to the workflow session.",
            "fact_type": "decision",
            "entities": ["Redis"],
        }
    ],
    source_id="project:aidememo",
    actor_id="codex:account-a",
    session_id=pack["session_id"],
)

branch = mem.branch_push(
    "candidate-b",
    "./shared",
    base="./shared/backup-01...",
)
print(branch["records_exported"])

main = Memory.open(store_path="./main.sqlite", storage_backend="libsqlite")
main.branch_merge("./shared", branch="candidate-b")
```

Use `handoff()` when only prompt-ready Markdown is needed. `handoff_packet()`
returns the structured envelope and remains read-only unless `dispatch=True`.
When dispatched, `handoff_inbox()`, `handoff_accept()`, `handoff_return()`,
`handoff_outbox()`, and `handoff_show()` implement the default round trip.
`handoff_status()` retains the actor-scoped check for callers that need it.

Use MCP/tools for one-off model-visible calls. Use this SDK when the agent
needs memory as code: fanout retrieval, dedupe, coverage checks, aggregation,
artifact hydration, cross-agent handoff, branch-log experiments, or
session-aware batch writes without spending model turns on intermediate state.
`session_canvas(...)`, `handoff(...)`, and `project_profile(...)` return
read-only Markdown strings suitable for direct prompt injection;
`handoff_packet(...)` retains the machine-readable envelope. The handoff
packet separates durable `session_id`, scoped `source_id`, installation
`actor_id`, and agent/profile route. Actor ids are user-assigned routing
aliases, not authentication. Assignments point to sessions and intentionally
do not implement topics, offsets, retries, leases, or copied payloads.

## Identity defaults and source scope

`source_id` can be passed to `Memory.open(...)` or inherited from
`AIDEMEMO_SOURCE_ID`, matching the MCP
`aidememo mcp-install --source-id <namespace>` path. The client forwards it
through search/query/context, recent/entity/traverse reads, aggregation,
workflow/session/project context, and fact writes. `actor_id`, inherited from
`AIDEMEMO_ACTOR_ID`, records which account or agent wrote workflow and fact
records without splitting shared project retrieval.

Open-time identities are defaults, not an authorization boundary. Explicit
per-call values take precedence; for `remember(...)` / `fact_add_many(...)`, an
identity on an individual item wins over the method and open-time defaults.
Native and CLI callers can therefore select another source. Use separate
stores for untrusted tenants, or the HTTP MCP server's bearer-token bindings
when callers must not override `source_id` or `actor_id`.

Because `doctor`, `lint`, and `stats` expose global store metadata,
`mem.client.doctor()`, `lint()`, and `stats()` raise `AideMemoUnavailable` when
the client has a default source. Run those diagnostics from an unscoped
administrator client instead.

Exact-content deduplication is source-local. Entity names and types remain a
shared ontology, while source-scoped entity visibility is fact-backed and
source-scoped graph traversal uses only relations created in that source.

This matches `aidememo mcp-install --source-id <namespace> --actor-id
<installation-alias>`; handoff calls use the same actor default for routing.

`storage_backend` is optional and matches the CLI/native binding selector:
omit it or pass an empty string for the compiled default, pass `"sqlite"` or
`"libsqlite"` for the default local SQLite backend, or pass `"redb"` when the
installed native binding / CLI was built with the Cargo `redb` feature. The SDK
passes the selector to both the `aidememo-python` fast path and the CLI fallback
(`aidememo --backend ...`).

`branch_push(...)` and `branch_merge(...)` use the native binding for local
paths when available. S3 branch URIs fall back to the CLI so the installed
`aidememo --features s3` binary owns AWS credentials and compression behavior.
