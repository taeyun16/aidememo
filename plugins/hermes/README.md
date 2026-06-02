# hermes-wg

Hermes Agent plugin for [wg (Wiki-Graph)](https://github.com/taeyun16/wg)
— exposes the local knowledge graph as native tools, slash commands, and
lifecycle hooks.

## What you get

| Surface | What it does |
|---|---|
| **12 tools** | `wg_workflow_start`, `wg_context`, `wg_query`, `wg_search`, `wg_recent`, `wg_aggregate`, `wg_entity_list`, `wg_traverse`, `wg_fact_add`, `wg_fact_add_many`, `wg_doctor`, `wg_lint` — the Hermes-native surface now covers the MCP core tools plus the legacy raw lint helper. `wg_context`, retrieval/write tools, and `wg_aggregate` accept `source_id` for shared-store scoping and fall back to `plugins.wg.source_id` / `WG_SOURCE_ID` when omitted. |
| **8 slash commands** | `/wg-start <title>` (issue/ticket workflow context), `/wg-context [topic]` (top-of-turn context), `/wg <topic>` (topic query), `/wg-aggregate <query>` (exact count/sum/timeline), `/wg-add <content>` (record a fact), `/wg-recent` (last 7 days), `/wg-doctor` (setup/sharing diagnostics), `/wg-pending` (review/commit dry-run captures). Source-scoped commands accept `--source-id ID`. |
| **Python SDK** | Re-exports `wg_agent.Memory` / `WgMemorySDK` with code-first primitives: `open`, `search_rows`, `search_many`, `query_many`, `aggregate_many`, `coverage_by`, `group_by_entity`, and `remember`. Use it when a Hermes task needs fanout, coverage checks, or deterministic intermediate-state handling. The same `wg-agent-sdk` package also works from Codex, Claude Code, CI, and local scripts. |
| **`pre_llm_call` hook** | Auto-injects recent facts into the first turn so the model has wg context before it answers. |
| **`post_llm_call` hook** | Scans each turn for decision-style phrasings and auto-records them as wg facts. |
| **`hermes wg ...` CLI** | `hermes wg query` / `search` / `recent` / `add` / `stats` / `lint`. |
| **Bundled skill** | The agentskills.io-conformant `SKILL.md` registers automatically. |

## Install

```bash
pip install hermes-wg                    # CLI fallback (universal)
pip install "hermes-wg[binding]"         # adds wg-python (~100× faster)
pip install wg-agent-sdk                 # common SDK for Codex / Claude Code / scripts
```

Then enable it in `~/.hermes/config.yaml`:

```yaml
plugins:
  enabled:
    - wg
  wg:
    store_path: ~/.wg/wiki.redb     # optional; uses wg config default otherwise
    source_id: team-a               # optional default namespace for reads/writes
    recent_window: 7d               # session_start auto-context window
    recent_limit: 10
    auto_record: true               # session_end fact auto-recorder
    dry_run: false                  # if true, log detections to wg-pending.jsonl instead of writing
    confidence_floor: 0.85          # higher = stricter (fewer false positives)
    lock_retry_ms: 5000             # smooth over short redb lock contention, no daemon required
    default_entities: []            # entities to attach to auto-recorded facts
    pending_log: ~/.hermes/state/wg-pending.jsonl  # dry-run audit log
```

The plugin needs **either** `wg-python` (in-process binding) **or** the
`wg` CLI binary on `$PATH`. The CLI fallback is always available — install
via `cargo install wg-cli` or build from source.

## Why a plugin instead of just the MCP server?

`wg mcp-install --target hermes` already works. The plugin route adds
capabilities that MCP can't reach:

- **Auto-context injection.** Every new session gets the last week of
  facts pre-loaded — no tool call, no prompt, no model latency cost.
- **Auto-fact recording.** Decisions like "Decision: ship HNSW as default"
  or "결정: multilingual-128M로 가자" are detected and persisted at
  `on_session_end`. Hermes's "memory that grows with you" + wg's
  structured wiki, with no manual `wg fact add`.
- **Slash commands.** `/wg redis` is one keypress vs the model picking
  to call `wg_query`.
- **Low IPC overhead.** When `wg-python` is installed, common hot-path calls
  (`wg_workflow_start`, `wg_query`, `wg_context`, `wg_search`, recent reads,
  and fact writes) run through direct Python bindings. MCP-only structured
  aggregate operations still use the CLI/MCP path until the binding exposes
  those slots.

## Hermes-fit usage profiles

`wg` is strongest when the Hermes task shape selects the memory behaviour,
not when every turn uses the same generic retrieval call.

| Profile | Use it for | Primary surface | Memory behaviour |
|---|---|---|---|
| `coding` | PRs, issues, sparse automation triggers | `wg_workflow_start`, `/wg-start`, `pre_llm_call` workflow auto-start | Creates a tracked session, stores the trigger, and injects prior decisions / lessons / errors before planning. |
| `long-session` | Multi-hour implementation or debugging sessions | `wg_context`, `/wg-context`, `post_llm_call` capture | Loads recent + personalisation + topic context up front, then records decisions before the session drifts. |
| `research` | Experiments, ablations, metric interpretation | `wg_fact_add_many`, `wg_aggregate`, `/wg-aggregate` | Stores classified experiment observations in batches and answers exact count / timeline / total questions without in-head arithmetic. |
| `team` | Multiple local Hermes agents sharing one store | `source_id`, `lock_retry_ms`, `/wg-doctor` | Keeps each agent/source isolated; use retry for small same-host teams and daemon/MCP for heavier write concurrency. |
| `safe-capture` | First rollout on a noisy chat/workflow | `dry_run: true`, `/wg-pending` | Audits auto-detected facts before committing them to the graph. |

### Memory-as-Code SDK

Use the SDK when a Hermes task needs more than one retrieval call and the
intermediate candidate set should stay in Python objects instead of model
tokens. For non-Hermes agents, import the same API from `wg_agent`.

```python
from wg_agent import Memory

sdk = Memory.open(source_id="research-alpha")

rows = sdk.search_rows([
    {"query": "Hermes top1_mass support gate", "tool": "search_query"},
    {"query": "Hermes patch browser_vision negative prior", "tool": "patch"},
])
coverage = sdk.coverage_by(rows, ["tool", "fact_type"])

sdk.remember([
    {
        "content": "Lesson: support-gated retrieval beats fixed prior residual on Hermes traces.",
        "fact_type": "lesson",
        "entities": ["Hermes", "SupportGate"],
    }
])
```

## Configuration

Every key is optional; defaults are chosen for safety (high confidence
floor, modest 7-day window, auto-record on).

| Key | Default | Notes |
|---|---|---|
| `store_path` | wg's resolution | Override the redb store location. |
| `source_id` | unset | Default namespace for scoped tool reads/writes. Explicit tool `source_id` values override it; `WG_SOURCE_ID` is also honored when config is unset. |
| `recent_window` | `7d` | How far back the session-start preamble looks. |
| `recent_limit` | `10` | Max facts in the preamble. |
| `auto_record` | `true` | Toggle the `on_session_end` recorder. |
| `dry_run` | `false` | When `true`, detections are appended to `pending_log` instead of being written to wg. Useful for auditing precision before trusting writes. |
| `confidence_floor` | `0.85` | 0.7–1.0; lower = more captures (and more noise). |
| `lock_retry_ms` | `5000` | CLI fallback retries short redb lock collisions for this long. Keeps two local Hermes agents smooth without requiring a daemon. Set `0` for fail-fast debugging. |
| `default_entities` | `[]` | Entities to attach to auto-recorded facts. |
| `pending_log` | `~/.hermes/state/wg-pending.jsonl` | Override the dry-run audit log path. |

### Recommended onboarding flow

For a wiki you care about, switch on `dry_run: true` for the first
few sessions, then audit and selectively commit captures.

**From chat (Hermes session):**

```text
/wg-start "Fix Redis timeout in worker" --body "Worker jobs time out" --source github:org/repo#123
/wg-context redis --source-id team-a    → broad opening context for a normal turn
/wg redis --source-id team-a       → query only team-a facts in a shared store
/wg-aggregate "Redis timeout decisions" --op count --type decision --source-id team-a
/wg-add "Redis cache policy is LRU" --entities Redis --source-id team-a
/wg-doctor                      → setup, source-scope, and shared-store diagnostics
/wg-pending                     → list every detection (numbered)
/wg-pending commit 3            → commit only entry #3
/wg-pending commit all          → commit all and clear the log
/wg-pending clear all           → discard everything
```

**From a terminal (any agent — even non-Hermes):**

```bash
wg pending review               # opens an interactive checklist TUI
                                #   space  cycle (commit / discard / —)
                                #   a      mark all for commit
                                #   c, ⏎   apply selections and exit
                                #   q, Esc cancel without changes
```

Failed commits are kept in the pending log so you can retry. Once
the captures consistently look right, flip `dry_run` off — your
reviewed pattern will keep matching from then on.

## Development

```bash
cd plugins/hermes
python -m venv .venv
.venv/bin/pip install -e ".[test]"
.venv/bin/pytest
cd ../..
scripts/hermes-wg-pack-smoke.sh
```

Tests use a fake `ctx` so they run without a Hermes install. The pack smoke
builds the wheel, installs it into a temp venv, and verifies the SDK export,
Hermes plugin entry point, `plugin.yaml`, and bundled `SKILL.md`.

## License

MIT OR Apache-2.0 (matches the wg workspace).
