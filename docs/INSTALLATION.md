---
title: Install
description: Install AideMemo and verify the CLI.
---

# Install

The main binary is `aidememo`. It includes the CLI and MCP server.

## From Git

```bash
cargo install --git https://github.com/taeyun16/aidememo aidememo-cli
```

The crates.io release path is being prepared. Until the first registry release
lands, use the Git or checkout install paths.

Verify the binary:

```bash
aidememo --help
aidememo stats
```

If your shell cannot find the command, add Cargo's bin directory to your path:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## From a checkout

```bash
git clone https://github.com/taeyun16/aidememo.git
cd aidememo
mise install
cargo build -p aidememo-cli --release
export PATH="$PWD/target/release:$PATH"
```

For local development, the repo pins tool versions in `mise.toml`.

```bash
mise run changelog-release-check
mise run release-preflight
mise run cargo-package-readiness
mise run public-registry-smoke
mise run public-portability-check
mise run fresh-checkout-smoke
mise run docs-build
mise run ci-lint
mise run ci-test
```

The same release preflight is available in GitHub Actions as
`.github/workflows/release-preflight.yml` for a runner-backed pre-publish pass.
The clean-checkout onboarding path is also available as
`.github/workflows/fresh-checkout-smoke.yml`; it runs manually and on pull
requests that change the installer, CLI, core, or the smoke itself.

`changelog-release-check` is the fast release-note gate. It verifies that
`CHANGELOG.md` has been cut for the current workspace version before the broader
release preflight runs docs, registry, and workflow checks. The full release
preflight and `cargo-package-readiness` gate cover Rust publish dry-runs:
`aidememo-core` is verified first, and dependent Rust crates are checked only
after that core crate is published at the matching version.
`scripts/fresh-checkout-smoke.sh` copies the checkout to a temporary directory
without `target` or `node_modules`, builds the CLI, and verifies the deterministic
quickstart path.
`scripts/public-portability-check.py` rejects developer-specific macOS, Linux,
and Windows home paths from first-party tracked files so public examples and
workflows do not silently depend on one machine.
`public-registry-smoke` is a post-release plan by default; after a real
registry publish, run it with `AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE=verify` to
install the public packages in temporary environments.

The native Python binding release path uses `uvx` to run the pinned `maturin`
build tool, so `mise install` is enough to reproduce the local wheel and
publish dry-run checks:

```bash
mise run python-pack-smoke
mise run python-publish-dry-run
```

The pure-Python agent package publish dry-runs use the local Python build
backend:

```bash
mise run agent-sdk-publish-dry-run
mise run hermes-publish-dry-run
```

## Store location

By default, AideMemo uses its configured local store. For examples and scripts,
you can pass an explicit store path:

```bash
aidememo --store ./memory.sqlite stats
aidememo --store ./memory.sqlite fact add "A first note" --entities Project
```

Using `--store` is useful for demos, tests, and per-project memory files.

## Recommended first check

Run this in a temporary directory:

```bash
STORE="$(mktemp -d)/wiki.sqlite"

aidememo --store "$STORE" fact add \
  "Decision: AideMemo stores typed project memory locally." \
  --type decision \
  --entities AideMemo

aidememo --store "$STORE" search "typed project memory"
```

You should see the fact you just added.
