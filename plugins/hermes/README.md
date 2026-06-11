# hermes-aidememo

Hermes Agent plugin for [aidememo (AideMemo)](https://github.com/taeyun16/aidememo)
— exposes the local knowledge graph as native tools, slash commands, and
lifecycle hooks.

## What you get

| Surface | What it does |
|---|---|
| **12 tools** | `aidememo_workflow_start`, `aidememo_context`, `aidememo_query`, `aidememo_search`, `aidememo_recent`, `aidememo_aggregate`, `aidememo_entity_list`, `aidememo_traverse`, `aidememo_fact_add`, `aidememo_fact_add_many`, `aidememo_doctor`, `aidememo_lint` — the Hermes-native surface now covers the MCP core tools plus the legacy raw lint helper. `aidememo_context`, retrieval/write tools, and `aidememo_aggregate` accept `source_id` for shared-store scoping and fall back to `plugins.aidememo.source_id` / `AIDEMEMO_SOURCE_ID` when omitted. |
| **8 slash commands** | `/aidememo-start <title>` (issue/ticket workflow context), `/aidememo-context [topic]` (top-of-turn context), `/aidememo <topic>` (topic query), `/aidememo-aggregate <query>` (exact count/sum/timeline), `/aidememo-add <content>` (record a fact), `/aidememo-recent` (last 7 days), `/aidememo-doctor` (setup/sharing diagnostics), `/aidememo-pending` (review/commit dry-run captures). Source-scoped commands accept `--source-id ID`. |
| **Python SDK** | Re-exports `aidememo_agent.Memory` / `AideMemoMemorySDK` with code-first primitives: `open`, `search_rows`, `search_many`, `query_many`, `aggregate_many`, `coverage_by`, `group_by_entity`, and `remember`. Use it when a Hermes task needs fanout, coverage checks, or deterministic intermediate-state handling. The same `aidememo-agent-sdk` package also works from Codex, Claude Code, CI, and local scripts. |
| **`pre_llm_call` hook** | Auto-injects recent facts into the first turn so the model has aidememo context before it answers. |
| **`post_llm_call` hook** | Scans each turn for decision-style phrasings and auto-records them as aidememo facts. |
| **`hermes aidememo ...` CLI** | `hermes aidememo query` / `search` / `recent` / `add` / `stats` / `lint`. |
| **Bundled skill** | The agentskills.io-conformant `SKILL.md` registers automatically. |

## Install

```bash
# From a checkout, until the PyPI releases land:
python -m pip install -e packages/aidememo-agent-sdk
python -m pip install -e plugins/hermes

# After the PyPI releases:
python -m pip install hermes-aidememo
python -m pip install "hermes-aidememo[binding]"  # optional aidememo-python fast path
```

Then enable it in `~/.hermes/config.yaml`:

```yaml
plugins:
  enabled:
    - aidememo
  aidememo:
    store_path: ~/.aidememo/wiki.redb     # optional; uses aidememo config default otherwise
    source_id: team-a               # optional default namespace for reads/writes
    recent_window: 7d               # session_start auto-context window
    recent_limit: 10
    auto_record: true               # session_end fact auto-recorder
    dry_run: false                  # if true, log detections to aidememo-pending.jsonl instead of writing
    confidence_floor: 0.85          # higher = stricter (fewer false positives)
    lock_retry_ms: 5000             # smooth over short redb lock contention, no daemon required
    default_entities: []            # entities to attach to auto-recorded facts
    pending_log: ~/.hermes/state/aidememo-pending.jsonl  # dry-run audit log
```

The plugin needs **either** `aidememo-python` (in-process binding) **or** the
`aidememo` CLI binary on `$PATH`. The CLI fallback is always available — install
from Git or build from source until the crates.io release lands.

## Why a plugin instead of just the MCP server?

`aidememo mcp-install --target hermes` already works. The plugin route adds
capabilities that MCP can't reach:

- **Auto-context injection.** Every new session gets the last week of
  facts pre-loaded — no tool call, no prompt, no model latency cost.
- **Auto-fact recording.** Decisions like "Decision: ship HNSW as default"
  or "결정: multilingual-128M로 가자" are detected and persisted at
  `on_session_end`. Hermes's "memory that grows with you" + aidememo's
  structured wiki, with no manual `aidememo fact add`.
- **Slash commands.** `/aidememo redis` is one keypress vs the model picking
  to call `aidememo_query`.
- **Low IPC overhead.** When `aidememo-python` is installed, common hot-path calls
  (`aidememo_workflow_start`, `aidememo_query`, `aidememo_context`, `aidememo_search`, recent reads,
  and fact writes) run through direct Python bindings. MCP-only structured
  aggregate operations still use the CLI/MCP path until the binding exposes
  those slots.

## Hermes-fit usage profiles

`aidememo` is strongest when the Hermes task shape selects the memory behaviour,
not when every turn uses the same generic retrieval call.

| Profile | Use it for | Primary surface | Memory behaviour |
|---|---|---|---|
| `coding` | PRs, issues, sparse automation triggers | `aidememo_workflow_start`, `/aidememo-start`, `pre_llm_call` workflow auto-start | Creates a tracked session, stores the trigger, and injects prior decisions / lessons / errors before planning. |
| `long-session` | Multi-hour implementation or debugging sessions | `aidememo_context`, `/aidememo-context`, `post_llm_call` capture | Loads recent + personalisation + topic context up front, then records decisions before the session drifts. |
| `research` | Experiments, ablations, metric interpretation | `aidememo_fact_add_many`, `aidememo_aggregate`, `/aidememo-aggregate` | Stores classified experiment observations in batches and answers exact count / timeline / total questions without in-head arithmetic. |
| `team` | Multiple local Hermes agents sharing one store | `source_id`, `lock_retry_ms`, `/aidememo-doctor` | Keeps each agent/source isolated; use retry for small same-host teams and daemon/MCP for heavier write concurrency. |
| `safe-capture` | First rollout on a noisy chat/workflow | `dry_run: true`, `/aidememo-pending` | Audits auto-detected facts before committing them to the graph. |

### Memory-as-Code SDK

Use the SDK when a Hermes task needs more than one retrieval call and the
intermediate candidate set should stay in Python objects instead of model
tokens. For non-Hermes agents, import the same API from `aidememo_agent`.

```python
from aidememo_agent import Memory

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
| `store_path` | aidememo's resolution | Override the redb store location. |
| `source_id` | unset | Default namespace for scoped tool reads/writes. Explicit tool `source_id` values override it; `AIDEMEMO_SOURCE_ID` is also honored when config is unset. |
| `recent_window` | `7d` | How far back the session-start preamble looks. |
| `recent_limit` | `10` | Max facts in the preamble. |
| `auto_record` | `true` | Toggle the `on_session_end` recorder. |
| `dry_run` | `false` | When `true`, detections are appended to `pending_log` instead of being written to aidememo. Useful for auditing precision before trusting writes. |
| `confidence_floor` | `0.85` | 0.7–1.0; lower = more captures (and more noise). |
| `lock_retry_ms` | `5000` | CLI fallback retries short redb lock collisions for this long. Keeps two local Hermes agents smooth without requiring a daemon. Set `0` for fail-fast debugging. |
| `default_entities` | `[]` | Entities to attach to auto-recorded facts. |
| `pending_log` | `~/.hermes/state/aidememo-pending.jsonl` | Override the dry-run audit log path. |

### Recommended onboarding flow

For a wiki you care about, switch on `dry_run: true` for the first
few sessions, then audit and selectively commit captures.

**From chat (Hermes session):**

```text
/aidememo-start "Fix Redis timeout in worker" --body "Worker jobs time out" --source github:org/repo#123
/aidememo-context redis --source-id team-a    → broad opening context for a normal turn
/aidememo redis --source-id team-a       → query only team-a facts in a shared store
/aidememo-aggregate "Redis timeout decisions" --op count --type decision --source-id team-a
/aidememo-add "Redis cache policy is LRU" --entities Redis --source-id team-a
/aidememo-doctor                      → setup, source-scope, and shared-store diagnostics
/aidememo-pending                     → list every detection (numbered)
/aidememo-pending commit 3            → commit only entry #3
/aidememo-pending commit all          → commit all and clear the log
/aidememo-pending clear all           → discard everything
```

**From a terminal (any agent — even non-Hermes):**

```bash
aidememo pending review               # opens an interactive checklist TUI
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
scripts/hermes-aidememo-pack-smoke.sh
```

Tests use a fake `ctx` so they run without a Hermes install. The pack smoke
builds the wheel, installs it into a temp venv, and verifies the SDK export,
Hermes plugin entry point, `plugin.yaml`, and bundled `SKILL.md`.

## License

MIT OR Apache-2.0 (matches the aidememo workspace).
