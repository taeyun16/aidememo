#!/usr/bin/env bash
# wg installer — builds & installs the `wg` CLI from source via cargo.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/taeyun16/wg/main/scripts/install.sh | bash
#
# Requirements: cargo (Rust 1.85+).

set -euo pipefail

REPO_URL="${WG_REPO_URL:-https://github.com/taeyun16/wg}"
BIN_NAME="wg"

if ! command -v cargo >/dev/null 2>&1; then
    cat >&2 <<EOF
error: cargo not found. Install Rust 1.85+ first:
  https://rustup.rs

Or, if you have rustup:
  rustup update stable
EOF
    exit 1
fi

echo "→ Installing $BIN_NAME from $REPO_URL (this may take a few minutes)…"
cargo install --git "$REPO_URL" --bin "$BIN_NAME" wg-cli

CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"

if ! command -v "$BIN_NAME" >/dev/null 2>&1; then
    cat >&2 <<EOF
warning: $BIN_NAME installed at $CARGO_BIN/$BIN_NAME but not on your PATH.
Add this line to your shell profile:

  export PATH="$CARGO_BIN:\$PATH"
EOF
    exit 0
fi

VERSION="$($BIN_NAME --version 2>/dev/null || echo "unknown")"
echo "✓ Installed: $VERSION"
echo
echo "Next steps:"
echo "  $BIN_NAME init ./my-wiki"
echo "  $BIN_NAME query \"some topic\""
echo
echo "Register as MCP for Claude Code:"
echo "  claude mcp add wg -- $BIN_NAME mcp"
