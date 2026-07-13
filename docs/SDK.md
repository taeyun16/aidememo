---
title: Python SDK
description: Use AideMemo memory from code with aidememo-agent-sdk.
---

# Python SDK

Use `aidememo-agent-sdk` when an agent or script needs memory as a programmable
working set instead of one tool call at a time.

## Install

```bash
python -m pip install aidememo-agent-sdk

# Optional in-process native binding
python -m pip install "aidememo-agent-sdk[binding]"
```

Without the native binding, the SDK falls back to the `aidememo` CLI on `PATH`.
The `binding` extra installs the published `aidememo-python` package for the
optional in-process fast path.

## Native bindings

This page covers the Python composition SDK. Runtime-specific native bindings
are documented in their package READMEs:

| Runtime | Package | Release path | Docs |
|---|---|---|---|
| Python native | `aidememo-python` | Published on PyPI | [README](https://github.com/taeyun16/aidememo/tree/main/crates/aidememo-python) |
| Node.js | `aidememo-napi` | npm trusted-publisher workflow is ready; platform packages publish before the root wrapper | [README](https://github.com/taeyun16/aidememo/tree/main/crates/aidememo-napi) |
| Elixir | `aidememo_nif` | Local/path binding docs are ready; no Hex publish workflow yet | [README](https://github.com/taeyun16/aidememo/tree/main/crates/aidememo-nif) |
| C ABI | `aidememo-ffi` | Rust crate plus C header/linking docs | [README](https://github.com/taeyun16/aidememo/tree/main/crates/aidememo-ffi) |

All native bindings use the same backend selector as the CLI. Omitting the
backend or passing an empty string uses the compiled default. Default builds
include SQLite and can select it at open time (`backend="sqlite"` or
`backend="libsqlite"` / `{ backend: "sqlite" }` or `{ backend: "libsqlite" }` /
`backend: "sqlite"` or `backend: "libsqlite"` /
`aidememo_open_with_backend(..., "sqlite")` or
`aidememo_open_with_backend(..., "libsqlite")`). Build with Cargo `redb` when
you need to open redb stores.

Branch-log helpers are currently exposed in the Python composition SDK,
`aidememo-python`, `aidememo-napi`, and `aidememo_nif` for local branch
artifacts through already-open handles. C ABI callers should use the CLI
`aidememo branch ...` commands until the lower-level ABI needs that surface.

## Open memory

```python
from aidememo_agent import Memory

mem = Memory.open(source_id="team-a", storage_backend="libsqlite")
```

Use `source_id` to partition one team, agent, or project inside a trusted
shared store. The default is forwarded across search/query, entity and graph
reads, fact get/list/pinned operations, workflow context, and source-aware
relations—not only writes. Exact-content dedup also applies within that source,
so the same text can exist independently in two sources.

This is not a hostile multi-tenant security boundary. Native SDK callers can
choose another source, and entity names/types form a shared ontology. Use
separate stores for mutually untrusted tenants; use HTTP bearer identity
bindings when authenticated agents must not override their assigned source.

`storage_backend` is optional. It uses the same values as the CLI/native
binding selector: omit it or pass an empty string for the compiled default,
`"sqlite"` or `"libsqlite"` for the default local SQLite backend, or `"redb"`
when the installed binding / CLI was built with Cargo `redb`. The SDK forwards
the selector to both `aidememo-python` and the subprocess fallback
(`aidememo --backend ...`).

## Search several topics

```python
rows = mem.search_rows([
    "Redis timeout decisions",
    {"query": "billing webhook duplicates", "topic": "Billing"},
])

for row in rows:
    print(row["fact_type"], row["content"])
```

## Check coverage

```python
coverage = mem.coverage_by(rows, ["fact_type"])
print(coverage)
```

This is useful when an agent needs to know whether it found decisions, lessons,
and errors before planning.

## Aggregate memory

```python
timeline = mem.aggregate_many([
    {"query": "release preflight", "op": "timeline"},
    {"query": "Redis timeout", "op": "count", "fact_type": "error"},
])

print(timeline)
```

Use aggregation for questions such as:

- "How many times did this happen?"
- "What is the timeline?"
- "How much total cost did we record?"

## Remember new facts

```python
mem.remember([
    {
        "content": "Decision: Redis timeout fixes must start with DNS metrics.",
        "fact_type": "decision",
        "entities": ["Redis", "Worker"],
    },
    {
        "content": "Lesson: pool-size changes hid the real DNS failure mode.",
        "fact_type": "lesson",
        "entities": ["Redis", "Worker"],
    },
])
```

Batching writes is faster and gives the agent one clear side effect.

## Branch speculative runs

Use branch logs when a script or agent forks several candidate stores from one
backup and wants to merge only the best result.

```python
from aidememo_agent import Memory

candidate = Memory.open(store_path="./candidate-b.sqlite", storage_backend="libsqlite")

push = candidate.branch_push(
    "candidate-b",
    "./shared",
    base="./shared/backup-01...",
)
print(push["records_exported"])

main = Memory.open(store_path="./main.sqlite", storage_backend="libsqlite")
merge = main.branch_merge("./shared", branch="candidate-b")
print(merge["facts_inserted"])
```

Local branch paths use the `aidememo-python` fast path when available. S3
branch URIs fall back to the CLI so the installed `aidememo --features s3`
binary owns AWS credentials and compression behavior.

## When to use SDK vs MCP

| Use SDK | Use MCP |
|---|---|
| The agent is writing Python or running scripts | The model should call tools directly |
| You need fanout search and dedupe | You need one focused search/query |
| You need coverage checks or aggregation in code | You need model-visible tool results |
| You want to batch writes | You want an interactive agent workflow |
