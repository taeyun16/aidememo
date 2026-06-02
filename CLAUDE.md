# CLAUDE.md

Claude Code memory for the `aidememo` (AideMemo) workspace. The full agent guide
lives in `AGENTS.md` (cross-tool spec) — read that first.

@AGENTS.md

## Claude Code specifics

### MCP — registered via `.mcp.json`

This repo ships a project-level `.mcp.json` that wires `aidememo mcp` (stdio) into
Claude Code automatically. After `cargo build -p aidememo-cli`, the wiki tools
(`aidememo_context`, `aidememo_query`, `aidememo_aggregate`, `aidememo_fact_add`, `aidememo_doctor`, …) are
available without extra setup.

If the binary path differs from `./target/debug/aidememo`, edit `.mcp.json`.

### Skills, slash commands, hooks

- `aidememo-skill/SKILL.md` is distributable — users `cp -r aidememo-skill ~/.claude/skills/aidememo/`.
- Project-local slash commands live in `.claude/commands/` (see
  `aidememo-search.md`, `aidememo-add-fact.md`, `aidememo-context.md`).
- **Hooks ship in `aidememo-skill/hooks/`** (3 scripts: `aidememo-session-start.py`,
  `aidememo-post-tool.py`, `aidememo-extract-facts.py`). See
  `aidememo-skill/hooks/README.md` for the install snippet — soft-fail
  read-only injections, no blocking.

### Agent UX defaults to surface

When you (Claude) need wiki context: **call `aidememo_context` first**, not
`aidememo_session_start` / `aidememo_query` separately. `aidememo_context` returns
pinned + personalisation (preference/lesson/error) + recent + (with
topic) search/traverse/lessons in one round-trip.

When recording new facts, pick the right `fact_type`:
  - `decision` / `convention` / `pattern` for governance + architecture
  - `preference` for user 1st-person preferences
  - `lesson` for hard-won learnings ("tried X, hit Y")
  - `error` for recurring failure patterns to avoid
  - `note` only when nothing else fits

The Tier A+B additions (Preference/Lesson/Error types, sessions,
freshness, consolidate) are documented in
`AGENTS.md § Agent-UX cheatsheet` — agents loading AGENTS.md see
the full surface.

### Running tests / building

Always prefer `cargo check -p aidememo-cli` for fast iteration. Reach for full
`cargo build` only when linking a release artifact.

### When you finish a change

1. `cargo build 2>&1 | grep '^error'` — must be empty
2. `cargo test -p aidememo-core -p aidememo-cli`
3. `cargo fmt`
4. Commit with imperative-tense subject; no AI attribution.
