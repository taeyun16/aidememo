#!/usr/bin/env bash
# Prepare a deterministic three-agent demo for the AideMemo homepage video.

set -euo pipefail

DEMO_DIR="${AIDEMEMO_DEMO_DIR:-/tmp/aidememo-continuity-demo}"
STORE="$DEMO_DIR/.aidememo/project-memory.sqlite"
SOURCE_ID="demo:auth-refresh"

rm -rf "$DEMO_DIR"
mkdir -p "$DEMO_DIR/src" "$DEMO_DIR/test" "$DEMO_DIR/.aidememo"

cat >"$DEMO_DIR/package.json" <<'EOF'
{
  "name": "aidememo-continuity-demo",
  "private": true,
  "type": "module",
  "scripts": {"test": "node --test"}
}
EOF

cat >"$DEMO_DIR/src/auth.js" <<'EOF'
export async function refreshSession(session, provider, sessions) {
  const refreshed = await provider.refresh(session.refreshToken);

  // BUG: the provider rotates refresh tokens, but this keeps the consumed token.
  await sessions.save({
    ...session,
    accessToken: refreshed.accessToken,
    refreshToken: session.refreshToken,
  });

  return refreshed.accessToken;
}
EOF

cat >"$DEMO_DIR/test/auth.test.js" <<'EOF'
import assert from 'node:assert/strict';
import test from 'node:test';

import {refreshSession} from '../src/auth.js';

test('persists the rotated refresh token with the new access token', async () => {
  let saved;
  const sessions = {save: async (session) => { saved = session; }};
  const provider = {
    refresh: async (token) => {
      assert.equal(token, 'refresh-old');
      return {accessToken: 'access-new', refreshToken: 'refresh-new'};
    },
  };

  await refreshSession(
    {userId: 'u-1', accessToken: 'access-old', refreshToken: 'refresh-old'},
    provider,
    sessions,
  );

  assert.equal(saved.accessToken, 'access-new');
  assert.equal(saved.refreshToken, 'refresh-new');
});
EOF

cat >"$DEMO_DIR/README.md" <<'EOF'
# Authentication continuity demo

This tiny project reproduces a refresh-token rotation bug. The point of the demo
is not the fix itself: Hermes diagnoses the failure, Codex continues from the
recorded project memory, and Claude Code reviews the result without being given
the earlier conversation.

Run `npm test` to reproduce the failure.
EOF

printf '%s\n' \
  "Prepared: $DEMO_DIR" \
  "Store:    $STORE" \
  "Source:   $SOURCE_ID" \
  "" \
  "Next:" \
  "  cd $DEMO_DIR" \
  "  npm test" \
  "" \
  "Connect the same store to each installed agent:" \
  "  aidememo --backend libsqlite --store $STORE mcp-install --target hermes --source-id $SOURCE_ID --actor-id hermes --force" \
  "  aidememo --backend libsqlite --store $STORE mcp-install --target codex --source-id $SOURCE_ID --actor-id codex --force" \
  "  aidememo --backend libsqlite --store $STORE mcp-install --target claude --source-id $SOURCE_ID --actor-id claude-code --force"
