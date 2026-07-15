---
title: Cross-agent continuity demo
description: Record a real Hermes to Codex to Claude Code project-memory handoff.
---

# Cross-agent continuity demo

This recording demonstrates project continuity, not chat synchronization.
Hermes diagnoses an authentication failure and records the durable result.
Codex continues from that knowledge in a new process. Claude Code then reviews
the result with the same project memory and no transcript from either agent.

## Prepare the project

```bash
scripts/prepare-continuity-demo.sh
cd /tmp/aidememo-continuity-demo
```

The script creates a small Node project with one failing refresh-token test and
prints the three `mcp-install` commands. Run those commands once before the
recording. They point every agent at the same SQLite store while preserving a
different `actor_id` for each writer.

Verify the setup before opening an agent:

```bash
aidememo --backend libsqlite \
  --store /tmp/aidememo-continuity-demo/.aidememo/project-memory.sqlite \
  doctor
npm test
```

For a clean retake, rerun `scripts/prepare-continuity-demo.sh`. This removes the
demo store and restores the original failing source file.

## Recording plan

Keep the terminal at 110-120 columns, use a 20-22 px monospace font, hide account
names and API keys, and record at 1440p. The final cut should be 45-55 seconds.

### Scene 1 - Hermes finds the failure

Open Hermes in the demo directory and use this prompt:

```text
Run the authentication test and diagnose the failure, but do not edit the code.
Before finishing, save three durable AideMemo facts for this project: the error,
the lesson, and the implementation decision the next coding agent should follow.
```

The useful facts should say, in substance:

- error: the consumed refresh token is persisted and later triggers replay detection;
- lesson: the provider rotates refresh tokens on every successful refresh;
- decision: persist the new access token and new refresh token together.

Pause briefly on the successful AideMemo writes. Do not spend screen time on raw
tool-call JSON.

On-screen chapter text: `YESTERDAY / HERMES FOUND WHAT FAILED`

### Scene 2 - Codex continues

Quit Hermes completely, clear the terminal, and start Codex in the same folder.
Do not paste the Hermes conversation. Use only:

```text
Continue the authentication fix. Check the project memory before editing, then
implement the agreed fix and run the test.
```

Keep the recovered error, lesson, and decision visible for two seconds. Then show
Codex changing `session.refreshToken` to `refreshed.refreshToken` and the test
passing.

On-screen chapter text: `NEW SESSION / DIFFERENT AGENT / NO RE-EXPLAINING`

### Scene 3 - Claude Code verifies the handoff

Quit Codex and start Claude Code in the same folder. Use:

```text
Review the authentication change. Use the project memory to explain which prior
failure this avoids, then run the test. Do not modify the code unless the review
finds a real issue.
```

Show Claude Code connecting the passing implementation to the failure Hermes
recorded. This proves that the memory belongs to the project rather than to one
agent session.

On-screen chapter text: `THE CONVERSATION ENDED / THE WORK CONTINUED`

## Final frame

Use the homepage wording:

```text
Switch coding agents. Keep the work moving.

Local project memory for Hermes, Codex, Claude Code, and more.
aidememo.taeyun.me
```

## Truthful claim boundary

Do say that decisions, failed attempts, lessons, and structured project context
continue across agents. Do not call the feature live session transfer: AideMemo
does not move chat transcripts, running processes, credentials, or hidden model
state between tools.
