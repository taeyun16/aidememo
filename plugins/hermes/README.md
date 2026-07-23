# hermes-aidememo

Hermes Agent plugin for [AideMemo (`aidememo`)](https://github.com/taeyun16/aidememo)
— exposes the local knowledge graph as native tools, slash commands, and
lifecycle hooks.

## What you get

| Surface | What it does |
|---|---|
| **14 tools** | Adds `aidememo_handoff` plus `aidememo_handoff_inbox` to the prior context/query/search/aggregate/write/doctor/lint surface. Handoff can preview or dispatch a session pointer; inbox lists, accepts, or completes assignments for the current account/installation alias. Source-scoped tools fall back to `plugins.aidememo.source_id` / `AIDEMEMO_SOURCE_ID`; actor routing falls back to `AIDEMEMO_ACTOR_ID`. |
| **8 slash commands** | `/aidememo-start <title>` (issue/ticket workflow context), `/aidememo-context [topic]` (top-of-turn context), `/aidememo <topic>` (topic query), `/aidememo-aggregate <query>` (exact count/sum/timeline), `/aidememo-add <content>` (record a fact), `/aidememo-recent` (last 7 days), `/aidememo-doctor` (setup/sharing diagnostics), `/aidememo-pending` (review/commit pending captures). Source-scoped commands accept `--source-id ID`. |
| **Python SDK** | Re-exports `aidememo_agent.Memory` / `AideMemoMemorySDK` with code-first primitives: `open`, `search_rows`, `search_many`, `query_many`, `aggregate_many`, `coverage_by`, `group_by_entity`, and `remember`. Use it when a Hermes task needs fanout, coverage checks, or deterministic intermediate-state handling. The same `aidememo-agent-sdk` package also works from Codex, Claude Code, CI, and local scripts. |
| **`pre_llm_call` hook** | Auto-injects recent facts into the first turn. In a dispatcher worker it detects `HERMES_KANBAN_TASK`, leaves task lifecycle to Kanban, and avoids creating a duplicate ticket workflow. |
| **`post_llm_call` hook** | Optional auto-capture adapter. When explicitly enabled, scans turns for decision-style phrasings and queues them to pending review by default. |
| **`hermes aidememo ...` CLI** | `hermes aidememo query` / `search` / `recent` / `add` / `stats` / `lint`. |
| **Bundled skill** | The agentskills.io-conformant `SKILL.md` registers automatically. |

## Install

```bash
# From a checkout, until the PyPI releases land:
HERMES_PY="${HERMES_PY:-$HOME/.hermes/hermes-agent/venv/bin/python3}"
"$HERMES_PY" -m pip install -e packages/aidememo-agent-sdk -e plugins/hermes

# After the PyPI releases:
"$HERMES_PY" -m pip install hermes-aidememo
"$HERMES_PY" -m pip install "hermes-aidememo[binding]"  # optional aidememo-python fast path
```

Use Hermes's own Python interpreter. Installing with an unrelated system
`python` leaves the package outside Hermes's plugin host and the plugin will
not be discovered.

Then enable it in `~/.hermes/config.yaml`:

```yaml
plugins:
  enabled:
    - aidememo
  aidememo:
    store_path: ~/.aidememo/wiki.sqlite   # optional; uses aidememo config default otherwise
    source_id: team-a               # optional default namespace for reads/writes
    actor_id: hermes:account-a      # optional writer provenance for new facts
    recent_window: 7d               # session_start auto-context window
    recent_limit: 10
    auto_capture:
      enabled: false                # opt-in; canonical writes remain explicit fact_add/SDK/MCP calls
      mode: pending                 # pending = review queue, direct = immediate write
      detect_in: both               # both | user | assistant
    confidence_floor: 0.85          # higher = stricter (fewer false positives)
    lock_retry_ms: 5000             # smooth over short local-store write contention
    default_entities: []            # legacy alias; prefer auto_capture.default_entities
    pending_log: ~/.hermes/state/aidememo-pending.jsonl  # review queue path
```

The plugin needs **either** `aidememo-python` (in-process binding) **or** the
`aidememo` CLI binary on `$PATH`. The CLI fallback is always available — install
from Git or build from source until the crates.io release lands.

## Why a plugin instead of just the MCP server?

`aidememo mcp-install --target hermes` already works. The plugin route adds
capabilities that MCP can't reach:

- **Auto-context injection.** Every new session gets the last week of
  facts pre-loaded — no tool call, no prompt, no model latency cost.
- **Opt-in capture adapter.** Decisions like "Decision: ship HNSW as default"
  or "결정: multilingual-128M로 가자" can be detected and queued to
  `/aidememo-pending`. Direct writes are available only when explicitly
  configured; the canonical path remains `aidememo_fact_add`,
  `aidememo_fact_add_many`, SDK `remember(...)`, or MCP tool calls.
- **Slash commands.** `/aidememo redis` is one keypress vs the model picking
  to call `aidememo_query`.
- **Low IPC overhead.** When `aidememo-python` is installed, common hot-path calls
  (`aidememo_workflow_start`, `aidememo_query`, `aidememo_context`, `aidememo_search`, recent reads,
  and fact writes) run through direct Python bindings. MCP-only structured
  aggregate operations still use the CLI/MCP path until the binding exposes
  those slots.

## Hermes-fit usage profiles

AideMemo is strongest when the Hermes task shape selects the memory behaviour,
not when every turn uses the same generic retrieval call.

| Profile | Use it for | Primary surface | Memory behaviour |
|---|---|---|---|
| `coding` | PRs, issues, sparse automation triggers | `aidememo_workflow_start`, `/aidememo-start`, `pre_llm_call` workflow auto-start | Creates a tracked session, stores the trigger, and injects prior decisions / lessons / errors before planning. |
| `orchestrated` | A task crosses coding-agent installations or accounts | `aidememo_handoff`, `aidememo_handoff_inbox`, SDK handoff methods | Dispatches one tracked-session pointer with focus and `done_when`; the addressed actor pulls current evidence. Same-board Hermes role transitions stay in Kanban. |
| `kanban-worker` | Hermes PM/coder/reviewer lanes, retries, fleet tasks | Kanban lifecycle tools + `aidememo_context` / fact writes | Kanban owns cards, claims, retries, comments, and completion. AideMemo adds cross-card memory and external-worker continuity without becoming a second queue. |
| `long-session` | Multi-hour implementation or debugging sessions | `aidememo_context`, `/aidememo-context`, optional `post_llm_call` capture | Loads recent + personalisation + topic context up front, then optionally queues decisions before the session drifts. |
| `research` | Experiments, ablations, metric interpretation | `aidememo_fact_add_many`, `aidememo_aggregate`, `/aidememo-aggregate` | Stores classified experiment observations in batches and answers exact count / timeline / total questions without in-head arithmetic. |
| `team` | Multiple local Hermes agents sharing one store | `source_id`, `actor_id`, `lock_retry_ms`, `/aidememo-doctor` | Shares project retrieval while preserving writer provenance; use retry for small same-host teams and daemon/MCP for heavier write concurrency. |
| `safe-capture` | First rollout on a noisy chat/workflow | `auto_capture.enabled: true`, `auto_capture.mode: pending`, `/aidememo-pending` | Audits auto-detected facts before committing them to the graph. |

### Memory-as-Code SDK

Use the SDK when a Hermes task needs more than one retrieval call and the
intermediate candidate set should stay in Python objects instead of model
tokens. For non-Hermes agents, import the same API from `aidememo_agent`.

```python
from aidememo_agent import Memory

sdk = Memory.open(source_id="research-alpha", actor_id="hermes:researcher-a")

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

session_id = "session-..."  # carried by the Hermes card/parent handoff
packet = sdk.handoff_packet(
    session_id,
    from_actor="hermes-coding",
    to_actor="codex-reviewer",
    from_route="hermes/coding",
    to_route="codex/reviewer",
    focus="Review the patch and run the release gate",
    done_when="Focused tests pass and review findings are attached to the session",
    source_id="research-alpha",
    dispatch=True,
)
pending = sdk.handoff_inbox(actor_id="codex-reviewer")
accepted = sdk.handoff_accept(pending[0]["handoff_id"], actor_id="codex-reviewer")
next_prompt = accepted["content"]
resume_env = accepted["resume"]["env"]
```

The Hermes tool and SDK return the structured packet directly, so an
orchestrator does not need to parse Markdown to recover routing or resume
state. The MCP tool accepts the same compact `from: "hermes/coding"` and `to:
"hermes/reviewer"` fields. Packets include one validated `aidememo session
resume` command that activates both the tracked session and `source_id` scope.

An orchestrator can inject `next_prompt` into an external worker. Both workers
continue writing to the same `session_id`, while the Hermes card remains the
canonical lifecycle and validation record.

### Hermes Kanban: where handoff helps

Use Kanban alone for internal profile routing: PM → coder → reviewer,
dependencies, retry/reclaim, heartbeats, comments, and completion. Reuse the
AideMemo session named in the card or parent metadata to record durable facts,
but do not call `aidememo_handoff(..., dispatch=True)` merely to move between
Hermes profiles.

Handoff becomes useful at three boundaries:

1. A card crosses to an external Codex/Claude installation whose vendor session
   id Hermes cannot reuse.
2. A retry or sibling card needs durable evidence older or broader than the
   immediate Kanban run summary.
3. A new board/project needs decisions, lessons, or measurements from prior
   completed cards.

In the first case, leave the Kanban card running or blocked, dispatch one
AideMemo pointer to the external `actor_id`, then read the linked result through
`handoff_outbox()` / `handoff_status()`. Validate that fact before completing
the Kanban card. In the other cases, use scoped context/query reads and fact
writes; no AideMemo assignment is needed.

For a long external run, the worker lane emits an AideMemo heartbeat every hour
and forwards it to `hermes kanban heartbeat` when a Kanban task is linked. This
is a liveness bridge, not a second lease: Hermes remains responsible for stale
reclaim, retry, dependencies, comments, and completion. Use
`aidememo_handoff_inbox(action="board", stale_after="1h")` only to inspect
external boundaries; the result is derived and never mutates the Hermes board.

Actor aliases are non-secret routing metadata, not Hermes/Codex account
authentication. The ledger stores a session pointer and acknowledgement state,
not topics, offsets, consumer groups, retries, or message payload copies.

### Run the external receiver

Install `aidememo-agent-sdk` in the worker environment, then run the addressed
assignment outside the Hermes board lifecycle:

```bash
aidememo-worker-lane handoff-... \
  --actor-id codex-two \
  --agent codex \
  --workspace /path/to/worktree \
  --source-id project-a \
  --kanban-task task-42
```

For a recurring Codex/Claude account, Hermes can configure the installation
once and invoke the shorter boundary:

```bash
aidememo agent add codex-two --type codex \
  --home /path/to/codex-two-home --workspace /path/to/worktree \
  --source-id project-a
aidememo handoff run codex-two --kanban-task task-42 --timeout 14400
```

The profile stores no credentials. Codex account state stays behind
`CODEX_HOME`; the default `core` environment policy prevents unrelated tokens
from leaking into the worker.

Use `--agent claude` for Claude Code. The runner accepts the pointer, passes the
bounded packet and resume environment to the child CLI, and returns a result
or error fact to the same AideMemo session. Success completes the acknowledgement;
failure leaves it accepted. It does not mutate Kanban or register a Hermes
`spawn_fn`, so the orchestrator must still validate the returned evidence and
update the card.

## Configuration

Every key is optional; defaults are chosen for safety (high confidence
floor, modest 7-day window, auto-capture off).

| Key | Default | Notes |
|---|---|---|
| `store_path` | AideMemo config resolution | Override the local store location. SQLite is the default; redb requires an explicit redb build/config. |
| `source_id` | unset | Default namespace for scoped tool reads/writes. Explicit tool `source_id` values override it; `AIDEMEMO_SOURCE_ID` is also honored when config is unset. |
| `actor_id` | unset | Default writer identity stored with new facts. Explicit write `actor_id` values override it; `AIDEMEMO_ACTOR_ID` is also honored when config is unset. |
| `recent_window` | `7d` | How far back the session-start preamble looks. |
| `recent_limit` | `10` | Max facts in the preamble. |
| `auto_capture.enabled` | `false` | Opt into the capture adapter. Without this, the hook does not write facts or pending entries. Legacy explicit `auto_record: true` is still honored. |
| `auto_capture.mode` | `pending` | `pending` appends detections to `pending_log` for review; `direct` writes immediately through the SDK and should be treated as a separate opt-in. |
| `auto_capture.detect_in` | `both` | Scan `user`, `assistant`, or `both` sides of each turn. |
| `dry_run` | unset | Legacy alias: explicit `dry_run: true` enables pending capture for old configs. |
| `confidence_floor` | `0.85` | 0.7–1.0; lower = more captures (and more noise). |
| `lock_retry_ms` | `5000` | CLI fallback waits this long for short local-store write collisions. For SQLite this is the busy timeout; for redb this retries the exclusive open lock. Set `0` for fail-fast debugging. |
| `default_entities` | `[]` | Legacy alias for entities to attach when direct capture is explicitly enabled. |
| `pending_log` | `~/.hermes/state/aidememo-pending.jsonl` | Override the dry-run audit log path. |

### Recommended onboarding flow

For a wiki you care about, switch on pending capture for the first few
sessions, then audit and selectively commit captures.

```yaml
plugins:
  aidememo:
    auto_capture:
      enabled: true
      mode: pending
```

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

Failed commits are kept in the pending log so you can retry. If captures
consistently look right and the team accepts immediate writes, switch
`auto_capture.mode` to `direct` explicitly.

### OpenClaw / generic hook adapter

Non-Hermes agents can use the same pending-first adapter by piping hook JSON
into the standalone script:

```bash
scripts/aidememo-capture-adapter.py --enable --provider openclaw --mode pending \
  --pending-log ~/.openclaw/state/aidememo-pending.jsonl < hook-event.json
```

The script accepts generic keys such as `messages`, `conversation`,
`prompt`/`response`, `user_message`/`assistant_response`, or raw text on stdin.
Without `--enable` or `AIDEMEMO_CAPTURE_ENABLE=1`, it is read-only and reports
that capture is disabled.

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

MIT OR Apache-2.0 (matches the AideMemo workspace).
