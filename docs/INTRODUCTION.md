---
title: What is AideMemo?
description: A user-facing overview of AideMemo and when to use it.
---

# What is AideMemo?

AideMemo is a local memory layer for coding agents and developer tools. It stores
facts, decisions, lessons, and errors in a single local database, then exposes
that memory through a CLI, MCP tools, the agent SDK, and native bindings.

The default memory path covers capture, typed writes, BM25-first search, and MCP
or agent SDK reads. It runs locally without an external LLM call. Remote
extraction, embedding, and reranking remain opt-in.

Use it when your agent needs project memory that survives across sessions,
editors, and model providers.

For the system map, read [`Architecture`](ARCHITECTURE.md). For the validated
scorecard, read [`Evidence`](EVIDENCE.md). For the per-turn tool choice guide,
read [`Agent Workflows`](AGENT_WORKFLOWS.md).

## What it gives you

| Need | AideMemo feature |
|---|---|
| Remember project decisions | `aidememo fact add --type decision` |
| Search prior context | `aidememo search` and `aidememo query` |
| Start work from a sparse ticket | `aidememo workflow start` |
| Share memory across agents | `source_id` scoping and `aidememo mcp-serve` |
| Give agents tool access | `aidememo mcp` or HTTP MCP |
| Use memory from code | `aidememo-agent-sdk` |

## Basic model

AideMemo stores three main things:

- **Entities**: topics such as `Redis`, `Billing`, `Codex`, or a session id.
- **Facts**: typed memory attached to entities.
- **Relations**: graph edges between entities.

Facts can be typed so the right memories rank higher:

| Fact type | Use for |
|---|---|
| `decision` | A choice that should guide future work |
| `lesson` | Something learned from a past attempt |
| `error` | A failure mode to avoid |
| `preference` | A user or team preference |
| `pattern` | A recurring architecture or workflow pattern |
| `note` | General context |
| `question` | Incoming issue, ticket, or open investigation |

## Typical workflow

1. Install the `aidememo` CLI.
2. Add facts while you work.
3. Search/query memory before making a plan.
4. Register AideMemo as an MCP server for your agent.
5. Use `workflow start` for issue, PR, or ticket automation.

```bash
aidememo fact add \
  "Decision: Redis timeout fixes must go through the Worker job wrapper." \
  --type decision \
  --entities Redis,Worker

aidememo query "Fix Redis timeout in worker"
```

## What AideMemo is not

AideMemo is not a hosted memory service, a full agent runtime, or a replacement
for your issue tracker. It is a local memory system that your existing tools can
call.

If you want a cloud-managed agent platform, use a hosted memory/runtime product.
If you want explicit local memory with a CLI, MCP tools, and SDK access, use
AideMemo.
