---
title: Python SDK
description: Use AideMemo memory from code with aidememo-agent-sdk.
---

# Python SDK

Use `aidememo-agent-sdk` when an agent or script needs memory as a programmable
working set instead of one tool call at a time.

## Install

```bash
# From a checkout, until the PyPI release lands:
python -m pip install -e packages/aidememo-agent-sdk
```

After the PyPI release:

```bash
python -m pip install aidememo-agent-sdk
```

Without the native binding, the SDK falls back to the `aidememo` CLI on `PATH`.
After `aidememo-python` is published, install the optional fast path with:

```bash
python -m pip install "aidememo-agent-sdk[binding]"
```

## Open memory

```python
from aidememo_agent import Memory

mem = Memory.open(source_id="team-a")
```

Use `source_id` to isolate one team, agent, tenant, or project inside a shared
store.

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

## When to use SDK vs MCP

| Use SDK | Use MCP |
|---|---|
| The agent is writing Python or running scripts | The model should call tools directly |
| You need fanout search and dedupe | You need one focused search/query |
| You need coverage checks or aggregation in code | You need model-visible tool results |
| You want to batch writes | You want an interactive agent workflow |
