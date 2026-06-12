# SkillOpt-lite For AideMemo

AideMemo borrows the useful part of SkillOpt without making runtime memory
self-modifying: treat the agent memory skill/profile as an offline trainable
artifact, then deploy only the validated artifact.

This is not a built-in LLM optimizer yet. It is the product boundary and local
gate for one:

1. collect scored rollouts from existing zero-token scenarios or real agent
   traces;
2. propose bounded add/delete/replace edits to the skill/profile;
3. accept the candidate only if validation gates pass and the selected metric
   improves;
4. keep rejected edits as negative evidence;
5. deploy the final `SKILL.md` / memory profile with no runtime optimizer call.

## Trainable Artifact

The artifact is the procedure that tells agents how to use `aidememo` memory:

- `aidememo-skill/SKILL.md` for model-visible agent instructions;
- `aidememo-skill/reader-prompts.md` for reader prompt snippets;
- `packages/aidememo-agent-sdk/README.md` examples for code-first memory;
- a future generated `best_skill.md` / `SKILL.md`-compatible profile copied
  into the right agent skill location by `aidememo skill install`.

The artifact should teach when to use:

- `aidememo_workflow_start` for sparse tickets and PR/issue automation;
- `aidememo_context` for the top of a normal turn;
- `aidememo_query` / `aidememo_search` for narrower follow-up retrieval;
- `aidememo_aggregate` only for counts, totals, date sets, and timelines;
- `Memory.open`, `search_rows`, `coverage_by`, `aggregate_many`, and
  `remember` when an agent can keep intermediate memory state in Python.

## Bounded Edits

Skill changes should be patches, not rewrites. A candidate may:

- add a short operating rule;
- delete a rule proven harmful by validation;
- replace one narrow rule with a more accurate one.

Avoid broad rewrites that change many workflows at once. Keep `aidememo_aggregate`
especially constrained: it is an exact arithmetic primitive, not a general
accuracy lever.

## Validation Gate

Run the cheap gate before accepting any candidate:

```bash
scripts/skillopt-lite-check.sh
```

By default this checks the bundled skill/profile path, `git diff --check`,
`cargo check -p aidememo-cli`, the zero-token workflow demo, and the SDK promotion
gate. To validate a candidate file:

```bash
AIDEMEMO_SKILLOPT_CANDIDATE=/tmp/SKILL.md scripts/skillopt-lite-check.sh
```

Optional product-boundary scenarios are available when a candidate changes SDK
composition, source scoping, or self-extraction rules:

```bash
AIDEMEMO_SKILLOPT_RUN_SCENARIOS=1 scripts/skillopt-lite-check.sh
```

The optional scenario gate runs:

- Scenario L: self-extraction typed batch -> workflow recall;
- Scenario M: `aidememo mcp-install --source-id` -> scoped MCP write/search;
- Scenario N: `aidememo_agent.Memory` fanout/dedupe/coverage/aggregate path.

## Periodic Cycle

Use the cycle runner for weekly or nightly profile maintenance:

```bash
scripts/skillopt-lite-cycle.sh
```

With no queued candidates, it checks the current `aidememo-skill/SKILL.md` and writes
health/run records under `target/skillopt-lite/`. To validate a proposed
candidate without applying it:

```bash
scripts/skillopt-lite-cycle.sh --candidate /tmp/SKILL.md
```

To accept a passing candidate and replace the source profile:

```bash
scripts/skillopt-lite-cycle.sh --candidate /tmp/SKILL.md --apply
```

For a periodic queue, drop `*.md` candidates into
`target/skillopt-lite/candidates/` and run:

```bash
AIDEMEMO_SKILLOPT_MAX_CYCLES=0 scripts/skillopt-lite-cycle.sh
```

Useful state files:

- `target/skillopt-lite/runs.jsonl` records accepted dry-runs and applied
  candidates;
- `target/skillopt-lite/rejected_edits.jsonl` records failed candidates and the
  gate log path;
- `target/skillopt-lite/logs/*.log` contains full validation output.

## Rejected Edit Buffer

Rejected candidates should be retained as negative evidence. A simple local
format is JSONL:

```jsonl
{"candidate":"best_skill.step3.md","reason":"Scenario N lost beta-source exclusion","metric":"scenario_n","status":"rejected"}
```

For durable project memory, also record recurring failures as `error` or
`lesson` facts with entities `aidememo`, `SkillOptLite`, and the affected workflow.

## Acceptance Rule

A candidate is acceptable only when:

- the check script exits 0;
- no required scenario regresses;
- the target metric improves or the change is purely clarifying documentation;
- the diff is small enough to audit manually;
- rejected edits and validation output are saved with the run.

The important SkillOpt lesson for AideMemo is the discipline, not the benchmark
claim: skill/profile edits should be bounded, evidence-backed, validated, and
deployed as a static artifact.
