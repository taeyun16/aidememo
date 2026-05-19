# hermes-wg

Hermes Agent plugin for [wg (Wiki-Graph)](https://github.com/taeyun16/wg)
— exposes the local knowledge graph as native tools, slash commands, and
lifecycle hooks.

## What you get

| Surface | What it does |
|---|---|
| **7 tools** | `wg_query`, `wg_search`, `wg_recent`, `wg_entity_list`, `wg_traverse`, `wg_fact_add`, `wg_lint` — same surface as the wg MCP server, but called in-process (no JSON-RPC overhead). `wg_query`, `wg_search`, and `wg_fact_add` accept `source_id` for shared-store scoping. |
| **4 slash commands** | `/wg <topic>` (one-shot context), `/wg-add <content>` (record a fact), `/wg-recent` (last 7 days), `/wg-pending` (review/commit dry-run captures). `/wg` and `/wg-add` accept `--source-id ID`. |
| **`on_session_start` hook** | Auto-injects recent facts into the conversation so the model has wg context before the user types. |
| **`on_session_end` hook** | Scans the transcript for decision-style phrasings and auto-records them as wg facts. |
| **`hermes wg ...` CLI** | `hermes wg query` / `search` / `recent` / `add` / `stats` / `lint`. |
| **Bundled skill** | The agentskills.io-conformant `SKILL.md` registers automatically. |

## Install

```bash
pip install hermes-wg                    # CLI fallback (universal)
pip install "hermes-wg[binding]"         # adds wg-python (~100× faster)
```

Then enable it in `~/.hermes/config.yaml`:

```yaml
plugins:
  enabled:
    - wg
  wg:
    store_path: ~/.wg/wiki.redb     # optional; uses wg config default otherwise
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
- **No IPC overhead.** When `wg-python` is installed, every tool call is
  a direct Python function call — no JSON encode/decode, no subprocess
  spawn.

## Configuration

Every key is optional; defaults are chosen for safety (high confidence
floor, modest 7-day window, auto-record on).

| Key | Default | Notes |
|---|---|---|
| `store_path` | wg's resolution | Override the redb store location. |
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
/wg redis --source-id team-a       → query only team-a facts in a shared store
/wg-add "Redis cache policy is LRU" --entities Redis --source-id team-a
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
```

Tests use a fake `ctx` so they run without a Hermes install.

## License

MIT OR Apache-2.0 (matches the wg workspace).
