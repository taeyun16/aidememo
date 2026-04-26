# CLAUDE.md

Claude Code memory for the `wg` (Wiki-Graph) workspace. The full agent guide
lives in `AGENTS.md` (cross-tool spec) — read that first.

@AGENTS.md

## Claude Code specifics

### MCP — registered via `.mcp.json`

This repo ships a project-level `.mcp.json` that wires `wg mcp` (stdio) into
Claude Code automatically. After `cargo build -p wg-cli`, the wiki tools
(`wg_search`, `wg_fact_add`, `wg_lint`, `wg_traverse`, `wg_entity_list`) are
available without extra setup.

If the binary path differs from `./target/debug/wg`, edit `.mcp.json`.

### Skills, slash commands, hooks

- `wg-skill/SKILL.md` is distributable — users `cp -r wg-skill ~/.claude/skills/wg/`.
- Project-local slash commands live in `.claude/commands/` (see
  `wg-search.md`, `wg-add-fact.md`, `wg-context.md`).
- No hooks are configured by default; suggest `wg sync` on `SessionStart` if
  the wiki should track unsaved markdown.

### Running tests / building

Always prefer `cargo check -p wg-cli` for fast iteration. Reach for full
`cargo build` only when linking a release artifact.

### When you finish a change

1. `cargo build 2>&1 | grep '^error'` — must be empty
2. `cargo test -p wg-core -p wg-cli`
3. `cargo fmt`
4. Commit with imperative-tense subject; no AI attribution.
