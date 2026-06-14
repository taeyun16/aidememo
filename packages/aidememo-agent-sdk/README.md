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
# From a checkout, until the PyPI release lands:
python -m pip install -e packages/aidememo-agent-sdk

# After the PyPI release:
python -m pip install aidememo-agent-sdk
```

The fallback path needs the `aidememo` CLI on `$PATH`. After `aidememo-python`
is published, `python -m pip install "aidememo-agent-sdk[binding]"` enables the
optional in-process binding fast path.

## Quick Start

```python
from aidememo_agent import Memory

mem = Memory.open(source_id="codex-aidememo", storage_backend="libsqlite")

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
    source_id="codex-aidememo",
)
mem.remember(
    [
        {
            "content": "Decision: Redis timeout follow-ups stay attached to the workflow session.",
            "fact_type": "decision",
            "entities": ["Redis"],
        }
    ],
    source_id="codex-aidememo",
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
branch-log experiments, or session-aware batch writes without spending model
turns on intermediate state.

`source_id` can be passed to `Memory.open(...)` or inherited from
`AIDEMEMO_SOURCE_ID`, matching the MCP `aidememo mcp-install --source-id <namespace>` path.

`storage_backend` is optional and matches the CLI/native binding selector:
omit it or pass an empty string for the compiled default, pass `"sqlite"` or
`"libsqlite"` for the default local SQLite backend, or pass `"redb"` when the
installed native binding / CLI was built with the Cargo `redb` feature. The SDK
passes the selector to both the `aidememo-python` fast path and the CLI fallback
(`aidememo --backend ...`).

`branch_push(...)` and `branch_merge(...)` use the native binding for local
paths when available. S3 branch URIs fall back to the CLI so the installed
`aidememo --features s3` binary owns AWS credentials and compression behavior.
