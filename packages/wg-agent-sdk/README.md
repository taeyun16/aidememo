# wg-agent-sdk

Agent-friendly memory SDK for code-executing agents, including Codex, Claude
Code, Hermes, CI jobs, and local scripts.

The SDK uses `wg-python` when available and otherwise falls back to the `wg`
CLI. This keeps the first-use path portable while still taking the fast
in-process route in environments that install the native binding.

Use it when memory needs to be a programmable working set: fan out searches,
dedupe rows, check coverage, aggregate exact counts/timelines, and batch-write
facts without spending model-visible tool calls on every intermediate step.

## Install

```bash
pip install wg-agent-sdk
pip install "wg-agent-sdk[binding]"   # optional fast path via wg-python
```

The fallback path needs the `wg` CLI on `$PATH`.

## Quick Start

```python
from wg_agent import Memory

mem = Memory.open(source_id="codex-wg")

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
        "entities": ["wg", "Codex"],
    }
])
```

Use MCP/tools for one-off model-visible calls. Use this SDK when the agent
needs memory as code: fanout retrieval, dedupe, coverage checks, aggregation,
or batch writes without spending model turns on intermediate state.

`source_id` can be passed to `Memory.open(...)` or inherited from
`WG_SOURCE_ID`, matching the MCP `wg mcp-install --source-id <namespace>` path.
