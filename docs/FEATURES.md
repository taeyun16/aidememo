---
title: Feature Inventory
description: Complete AideMemo feature surface tracked by the docs feature gate.
---

# Feature Inventory

This page is the public checklist for AideMemo's user-visible feature surface.
It is intentionally broader than the quickstart pages: every top-level CLI
command and every MCP tool must appear here so release changes cannot add,
remove, or rename a feature without touching the docs.

Run the gate with:

```bash
python3 scripts/docs-feature-gate.py
python3 scripts/docs-site-e2e.py
```

## CLI commands

| Command | What it covers |
|---|---|
| `aidememo entity` | Add, fetch, list, rename, alias, delete, describe, and show entities. |
| `aidememo fact` | Add, fetch, list, delete, pin, unpin, supersede, archive, and inspect facts. |
| `aidememo traverse` | Walk the graph outward from an entity. |
| `aidememo path` | Find the shortest graph path between two entities. |
| `aidememo search` | Search facts with BM25, optional semantic retrieval, filters, and project fanout. |
| `aidememo query` | Fetch a topic context pack with search, graph, recent facts, and result shaping. |
| `aidememo lint` | Run raw graph health checks. |
| `aidememo doctor` | Run user-oriented health checks and shared-store guidance. |
| `aidememo recent` | Show recently added or updated facts. |
| `aidememo edit` | Patch fact content in place for append, prepend, replace, or full-content edits. |
| `aidememo graph` | Render the entity graph as Mermaid or DOT. |
| `aidememo project` | Manage named projects and their store paths. |
| `aidememo bench` | Benchmark search quality against a golden JSONL set. |
| `aidememo skill` | Validate or install agent skill files. |
| `aidememo backup` | Create or restore hot + optional cold-tier SQLite snapshots with manifest verification. |
| `aidememo branch` | Push or merge append-only memory branch logs for cloud agents and speculative experiments. |
| `aidememo export` | Export entities, relations, and facts to JSONL. |
| `aidememo import` | Import JSONL data. |
| `aidememo stats` | Show store statistics. |
| `aidememo ingest` | Ingest markdown files into the store. |
| `aidememo sync` | Incrementally ingest local markdown or pull remote deltas from an MCP server. |
| `aidememo config` | Read and update local configuration. |
| `aidememo model` | Inspect and manage local embedding model cache state. |
| `aidememo feedback` | Record search-result feedback for ranking adaptation. |
| `aidememo adapt` | Train, inspect, and evaluate the ranking adapter. |
| `aidememo init` | Create an AideMemo store and optionally ingest a wiki or register an agent. |
| `aidememo watch` | Watch markdown files and re-ingest on changes. |
| `aidememo mcp-serve` | Serve MCP over HTTP plus SSE for shared warm access. |
| `aidememo mcp` | Serve MCP over stdio for local agents. |
| `aidememo mcp-install` | Register AideMemo MCP with a pinned store, source namespace, writer provenance, and repeatable Codex profile homes. |
| `aidememo completions` | Emit shell completion scripts. |
| `aidememo pending` | Review, approve, or reject dry-run extracted facts. |
| `aidememo vector-rebuild` | Rebuild the HNSW vector sidecar after model or index changes. |
| `aidememo daemon` | Manage a long-lived background `mcp-serve` process. |
| `aidememo extract` | Extract candidate facts from text, optionally using a configured LLM provider. |
| `aidememo session` | Create, inspect, and warm tracked agent sessions. |
| `aidememo workflow` | Start issue, PR, or automation workflows with tracked context. |
| `aidememo profile` | Generate read-only project profile artifacts from current typed facts. |
| `aidememo auto-relate` | Mine related-entity edges from semantic similarity. |
| `aidememo overview` | Produce a first-impression snapshot of an unfamiliar store. |
| `aidememo consolidate` | Deduplicate, expire, or GAC-cluster facts for lifecycle maintenance. |
| `aidememo auth` | Generate, store, list, and clear bearer-token credentials for HTTP MCP. |

## CLI subcommands

| Area | Subcommands |
|---|---|
| Entity management | `aidememo entity add`, `aidememo entity get`, `aidememo entity list`, `aidememo entity rename`, `aidememo entity alias`, `aidememo entity delete`, `aidememo entity describe`, `aidememo entity show` |
| Fact management | `aidememo fact add`, `aidememo fact get`, `aidememo fact list`, `aidememo fact delete`, `aidememo fact feedback`, `aidememo fact supersede`, `aidememo fact pin`, `aidememo fact unpin`, `aidememo fact pinned`, `aidememo fact archive` |
| Fact editing | `aidememo edit fact` |
| Project management | `aidememo project list`, `aidememo project show`, `aidememo project create`, `aidememo project use`, `aidememo project remove` |
| Agent skills | `aidememo skill check`, `aidememo skill install` |
| Backup / restore | `aidememo backup create`, `aidememo backup restore` |
| Branch logs | `aidememo branch push`, `aidememo branch merge` |
| Sync | `aidememo sync ingest`, `aidememo sync pull`, `aidememo sync status` |
| Config | `aidememo config list`, `aidememo config get`, `aidememo config set` |
| Model cache | `aidememo model list`, `aidememo model status`, `aidememo model download` |
| Ranking adapter | `aidememo adapt train`, `aidememo adapt status`, `aidememo adapt eval` |
| Pending fact review | `aidememo pending review`, `aidememo pending list`, `aidememo pending approve`, `aidememo pending reject`, `aidememo pending stats` |
| Daemon | `aidememo daemon start`, `aidememo daemon stop`, `aidememo daemon status` |
| Sessions | `aidememo session start`, `aidememo session new`, `aidememo session current`, `aidememo session resume`, `aidememo session list`, `aidememo session canvas`, `aidememo session handoff` |
| Agent installations | `aidememo agent`, `aidememo agent add`, `aidememo agent list`, `aidememo agent show`, `aidememo agent remove` (friendly); `aidememo installation`, `aidememo installation add`, `aidememo installation list`, `aidememo installation show`, `aidememo installation remove` (compatibility) |
| Handoff assignments | `aidememo handoff`, `aidememo handoff send`, `aidememo handoff run`, `aidememo handoff show` (friendly); `aidememo handoff inbox`, `aidememo handoff outbox`, `aidememo handoff heartbeat`, `aidememo handoff board`, `aidememo handoff status`, `aidememo handoff accept`, `aidememo handoff return`, `aidememo handoff complete` (manual lifecycle) |
| External worker receiver | SDK command `aidememo-worker-lane` for addressed Codex/Claude execution, hourly liveness, optional Hermes Kanban heartbeat forwarding, and same-session result/error return. `--type manual` exposes the protocol to other coding agents without claiming an automatic adapter. |
| Workflows | `aidememo workflow start` |
| Profile artifacts | `aidememo profile export` |
| Auth | `aidememo auth generate`, `aidememo auth login`, `aidememo auth logout`, `aidememo auth list` |

## MCP tools

| Tool | What it covers |
|---|---|
| `aidememo_search` | Search facts with filters, formatting controls, feedback session ids, and optional archive lookup. |
| `aidememo_feedback` | Record helpful or not-helpful feedback for a prior search result. |
| `aidememo_session_start` | Return the session warmup envelope: pinned facts, recent facts, top entities, and lint hints. |
| `aidememo_pinned_context` | Return the always-loaded pinned fact tier. |
| `aidememo_fact_pin` | Pin or unpin a fact. |
| `aidememo_extract` | Extract candidate facts from raw text and optionally persist them. |
| `aidememo_path` | Find a shortest path between two entities. |
| `aidememo_fact_list` | List facts with pagination and filters. |
| `aidememo_entity_get` | Fetch one entity by name or alias. |
| `aidememo_fact_get` | Fetch one fact by id. |
| `aidememo_entity_list` | List entities with type and pagination filters. |
| `aidememo_traverse` | Traverse graph neighbors in forward or reverse direction. |
| `aidememo_aggregate` | Count, enumerate, group, sum, or timeline facts deterministically. |
| `aidememo_doctor` | Return health, lint, and shared-store diagnostics. |
| `aidememo_overview` | Return an orientation snapshot for an unfamiliar wiki. |
| `aidememo_recent` | Return recent facts. |
| `aidememo_context` | Return the broad opening-turn context envelope. |
| `aidememo_workflow_start` | Start a tracked issue, PR, ticket, or automation workflow, optionally linked to a parent session. |
| `aidememo_handoff` | Route a tracked session to another coding agent or profile with bounded, fact-linked evidence, `done_when`, and structured resume metadata. |
| `aidememo_handoff_inbox` | Receiver list/accept/return and sender outbox/status over session pointers; returned evidence is linked by fact id, with no broker delivery semantics. |
| `aidememo_session_canvas` | Return a bounded Markdown + Mermaid canvas for long workflow resumption. |
| `aidememo_profile_export` | Return a read-only project profile text artifact from current typed facts. |
| `aidememo_query` | Return a focused topic context pack. |
| `aidememo_entity_describe` | Set or clear an entity summary. |
| `aidememo_fact_add` | Add one fact with self-classified type, optional session/source scoping, and writer provenance. |
| `aidememo_fact_add_many` | Add many facts in one transaction. |
| `aidememo_fact_supersede` | Retire an old fact in favor of a replacement fact. |
| `aidememo_fact_archive` | Move facts to the cold-tier archive. |
| `aidememo_fact_edit` | Edit fact content in place. |

## SDKs and bindings

| Surface | What it covers |
|---|---|
| `aidememo-agent-sdk` | Python composition layer for code-executing agents, including `session_canvas()` and `project_profile()` artifact helpers. |
| `aidememo-python` | PyO3 native bindings for Python. |
| `aidememo-napi` | Node.js native bindings. |
| `aidememo-nif` | Elixir/Erlang NIF bindings. |
| `aidememo-ffi` | C ABI bindings. |
| `hermes-aidememo` | Hermes Agent plugin, slash commands, lifecycle hooks, SDK re-exports, and opt-in pending-first capture adapter. |
| Claude Code plugin | Self-contained MCP definition, three focused skills, and three read-only context hooks under `plugins/claude`. |
| Agent skill installers | Profile-aware installs for Claude (`CLAUDE_CONFIG_DIR`), Hermes (`HERMES_HOME`), and pi (`PI_CODING_AGENT_DIR`); pi intentionally uses CLI rather than MCP. |

See [`Coding Agent Setup`](CODING_AGENTS.md) for the supported agent matrix and
verified installation paths.

The native Python, Node, Elixir, and C bindings use the same backend selector
as the CLI. Default builds include the local SQLite backend; build with Cargo
`redb` when you need to open a redb store. The Python composition SDK exposes
the same values through `Memory.open(storage_backend=...)` and forwards them to
both the `aidememo-python` fast path and the CLI fallback.

## Gate contract

`scripts/docs-feature-gate.py` enforces source-level drift checks:

1. Every command listed by `aidememo --help` must appear in this page as
   `` `aidememo <command>` ``.
2. Every MCP tool declared in `cmd/mcp_tools.rs::list_tools()` must appear in
   this page as a backticked tool name.
3. Count claims in public docs, such as MCP tool counts, CLI command counts,
   architecture diagram counts, and AGENTS core-tool counts, must match the
   implementation-derived numbers.
4. The Docusaurus sidebar must include this page, and public-facing docs/source
   strings must not contain known stale lowercase product wording.
5. Public storage positioning must continue to describe SQLite as the default
   backend and redb as the optional Cargo-feature backend.
6. Onboarding docs must keep the installer, checkout path, and deterministic
   `query --bm25-only` quickstart aligned with the CLI.
7. Community files must keep contributor, security, issue, and PR templates
   present and consistent with the implemented CLI surface.
8. The Korean locale must keep its homepage messages, translated-doc coverage,
   source fingerprints, and explicit English fallbacks synchronized with the
   public sidebar.
9. The English and Korean root READMEs must keep reciprocal language links and
   install commands, while `COMPARE.md` participates in count, wording, and
   storage-positioning drift checks.

The gate also runs an internal count-claim self-test by default. Use
`python3 scripts/docs-feature-gate.py --self-test` when you only need to verify
that the drift detector still rejects stale numeric claims.

`scripts/docs-site-e2e.py` then builds the rendered Docusaurus site and verifies
that the English and Korean route graphs still match the sidebar/homepage
contract, all baseUrl-scoped links/assets/anchors resolve, locale-specific page
H1s and `html lang` / `hreflang` metadata are correct, and architecture-doc
implementation paths still exist in the repo.

The gates cannot prove that prose is semantically perfect. They do make feature
and structure drift noisy: adding or renaming a CLI command or MCP tool without
updating this inventory, breaking the deployed root route graph, or
reverting the storage positioning fails CI.
