# Agent UX design decisions (2026-05-03)

Following the LongMemEval bench journey (multi-session 50% ceiling),
we audited the MCP surface for agent ergonomics. Five tracks were
proposed; three required real validation.

## A — Does `wg_aggregate` break the multi-session 50% ceiling?

**Setup**: built `scripts/longmemeval_aggregate_style.py` — pre-processes
each question's retrievals into an aggregate-style view (deduped by
fact_id + per-session grouping + matched_count) and feeds the reader a
structured prompt with explicit "use the count, don't recount" rules.
Compared on the same 60q hybrid retrievals (10q multi-session subset)
with MiniMax temp=0.

**Result**: **catastrophic regression — 10% (1/10) vs hybrid v6 50% (-40pt)**.

| Setup | multi-session 10q acc |
|---|---|
| hybrid v6 (omega-style prompts, baseline) | 50% (5/10) |
| **wg_aggregate-style (matched_count + items)** | **10% (1/10)** ⚠ |

**Why it failed**: the reader literally returned `matched_count=30`
as the numeric answer for every counting question. Sample failures:
* "How many projects have I led?" → GOLD 2, HYP **"30 projects"**
* "How many different doctors did I visit?" → GOLD 3, HYP **"30 doctors"**
* "How many model kits have I worked on or bought?" → GOLD 5,
  HYP "**19 model kits** in total" (summed per-session group counts —
  also wrong)

**Root insight**: `matched_count` is **count-of-facts**, not
**count-of-distinct-items-mentioned**. LongMemEval multi-session
questions all want the latter (semantic dedup of "doctors mentioned",
"projects led", "model kits acquired"). The primitive in its current
form solves the wrong problem for this benchmark.

`wg_aggregate(op=count)` IS useful for queries where each fact IS the
unit — "how many decisions about Postgres?" returns the right answer.
It is NOT useful when each fact mentions multiple instances of the
counted thing or when the same thing is mentioned in multiple facts.

**Decision**: keep `wg_aggregate` shipped (it's correct for its actual
use case — count-of-typed-facts) but document the limitation clearly.
Multi-session aggregation in LongMemEval-style benchmarks needs
semantic dedup beyond what hybrid_search + fact_id-dedup provides;
that is a deeper research problem (multi-hop retrieval, tool-use
agentic loops, structured fact extraction).

The 60q deterministic baseline stays at multi-session 50% — true
ceiling at the current measurement scale with MiniMax. To break
through requires either (a) gpt-4.1 reader (quota recovery), (b)
500q sample (noise floor down to ±2pt so fact-extraction tweaks
become measurable), or (c) DSPy-style multi-hop tool agent.

## B — Should `wg_query` default change to `format:"text"`?

**Verification**: searched all wg_query callers in the repo.

* `bench/multi-agent/scenario_a_mcp_smoke.py` does
  `json.loads(content[0]["text"])` and asserts `topic` field —
  treats wg_query output as a JSON document. Changing default to text
  would produce a markdown string and break the smoke.
* `crates/wg-cli/src/main.rs::run_query_via_daemon` parses the
  daemon's wg_query JSON into the local CLI render path. Same break.
* External agent integrations (Claude Code, Codex CLI, hermes) likely
  follow the same pattern — parse JSON, render or extract.

**Decision**: keep `format:"full"` as default. Document `format:"text"`
prominently in the schema description for agents that want the
markdown bullet form (~5× smaller). Already shipped in commit
799618e. The opt-in is the right contract; flipping the default is
unsafe without a major-version migration.

## C — Should the four fact-write tools collapse into one
`wg_fact_write(op, ...)` umbrella?

**Tools considered**:
* `wg_fact_add` — single write, dedup check, auto-create entities
* `wg_fact_add_many` — bulk write, single fsync per batch
* `wg_fact_supersede` — atomic replacement, validity-window invalidate
* `wg_fact_edit` — patch content (append / prepend / find+replace)

**Reasoning**: each of the four has a small, clean, distinct schema.
Combining into `wg_fact_write(op, ...)` would force a union schema
where every field is conditionally required by op. Modern agent
literature (e.g. Anthropic's tool-use guidelines) recommends *more
small tools* over *one tool with a dispatcher op param* — small
schemas reduce tool-call validation errors and make the right tool
discoverable from the schema alone.

**Decision**: do NOT consolidate. The wg_traverse + wg_backlinks
merge worked because both tools had the *same* schema with only a
direction flag; fact-write tools are genuinely different operations
with different inputs. Tool count stays 18 (16 + wg_aggregate +
backwards-compat aliases). The friction reduction comes from
description quality and tier markers, not collapse.

## Net agent UX changes shipped

| Change | Effect |
|---|---|
| `format`/`max_chars`/`preview_chars` on wg_query / wg_search / wg_context | 3-5× smaller agent context per call |
| `level:"entity"` on wg_query | Markdown text 350 chars (vs flat 574) — bench's hybrid-ingest pattern at agent layer |
| `wg_traverse` direction = forward / reverse | Merged wg_backlinks; deeper reverse traversal now possible |
| `wg_lint` → wg_doctor superset | Doctor returns issues + stats + action hints in one call |
| `error_kind` classification (invalid_input / not_found / conflict / unknown_tool / internal) | Agent decides retry vs fix-input vs give-up without parsing message |
| `wg_aggregate` (count / enumerate / by_entity) | Pulls agent out of synthesis loop for counting questions |

All changes are backwards-compatible. Existing agents keep working;
new agents that read the updated schema get better tools.
