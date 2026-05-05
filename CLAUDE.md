# CLAUDE.md

Claude Code memory for the `wg` (Wiki-Graph) workspace. The full agent guide
lives in `AGENTS.md` (cross-tool spec) — read that first.

@AGENTS.md

## Claude Code specifics

### MCP — registered via `.mcp.json`

This repo ships a project-level `.mcp.json` that wires `wg mcp` (stdio) into
Claude Code automatically. After `cargo build -p wg-cli`, the wiki tools
(`wg_context`, `wg_query`, `wg_aggregate`, `wg_fact_add`, `wg_doctor`, …) are
available without extra setup.

If the binary path differs from `./target/debug/wg`, edit `.mcp.json`.

### Skills, slash commands, hooks

- `wg-skill/SKILL.md` is distributable — users `cp -r wg-skill ~/.claude/skills/wg/`.
- Project-local slash commands live in `.claude/commands/` (see
  `wg-search.md`, `wg-add-fact.md`, `wg-context.md`).
- **Hooks ship in `wg-skill/hooks/`** (3 scripts: `wg-session-start.py`,
  `wg-post-tool.py`, `wg-extract-facts.py`). See
  `wg-skill/hooks/README.md` for the install snippet — soft-fail
  read-only injections, no blocking.

### Agent UX defaults to surface

When you (Claude) need wiki context: **call `wg_context` first**, not
`wg_session_start` / `wg_query` separately. `wg_context` returns
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

Always prefer `cargo check -p wg-cli` for fast iteration. Reach for full
`cargo build` only when linking a release artifact.

### When you finish a change

1. `cargo build 2>&1 | grep '^error'` — must be empty
2. `cargo test -p wg-core -p wg-cli`
3. `cargo fmt`
4. Commit with imperative-tense subject; no AI attribution.
