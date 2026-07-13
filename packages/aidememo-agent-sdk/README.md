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

Use MCP/tools for one-off model-visible calls. Use this SDK when the agent
needs memory as code: fanout retrieval, dedupe, coverage checks, aggregation,
artifact hydration, branch-log experiments, or session-aware batch writes
without spending model turns on intermediate state. `session_canvas(...)` and
`project_profile(...)` return read-only Markdown strings suitable for direct
prompt injection before resuming long work.

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

`storage_backend` is optional and matches the CLI/native binding selector:
omit it or pass an empty string for the compiled default, pass `"sqlite"` or
`"libsqlite"` for the default local SQLite backend, or pass `"redb"` when the
installed native binding / CLI was built with the Cargo `redb` feature. The SDK
passes the selector to both the `aidememo-python` fast path and the CLI fallback
(`aidememo --backend ...`).

`branch_push(...)` and `branch_merge(...)` use the native binding for local
paths when available. S3 branch URIs fall back to the CLI so the installed
`aidememo --features s3` binary owns AWS credentials and compression behavior.
