---
title: Release Checklist
description: Publish order and preflight checks for AideMemo packages.
---

# Release Checklist

This page records the package publish order. It is intentionally conservative:
publish lower-level packages first, then install and smoke the layers above
them.

## 1. Local preflight

Run the local release gate from a clean checkout:

```bash
scripts/release-preflight.sh
```

For a full registry dry-run:

```bash
AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full scripts/release-preflight.sh 0.1.0
```

The core package must pass packaging before any dependent package can be
published:

```bash
cargo package -p aidememo-core
```

## 2. Rust crates

Publish in dependency order:

1. `aidememo-core`
2. `aidememo-cli`
3. `aidememo-ffi`, `aidememo-napi`, `aidememo-nif`, `aidememo-python`

`aidememo-cli` and all native bindings depend on `aidememo-core`, so their
`cargo package` checks will fail against crates.io until `aidememo-core` is
published at the matching version.

## 3. Python packages

Publish native bindings before composition packages:

1. `aidememo-python`
2. `aidememo-agent-sdk`
3. `hermes-aidememo`

Before the first PyPI release, user-facing docs should show checkout installs:

```bash
python -m pip install -e packages/aidememo-agent-sdk
python -m pip install -e plugins/hermes
```

After PyPI release, docs can promote:

```bash
python -m pip install aidememo-agent-sdk
python -m pip install "aidememo-agent-sdk[binding]"
python -m pip install hermes-aidememo
```

## 4. Node package

Publish the platform packages before the root wrapper:

1. `aidememo-napi-*` platform packages
2. `aidememo-napi`

Use the trusted-publisher workflow with the exact version input. The default
workflow mode is dry-run.

## 5. Post-release checks

After each registry publish:

```bash
cargo install aidememo-cli
python -m pip install aidememo-agent-sdk
python -m pip install "aidememo-agent-sdk[binding]"
npm install aidememo-napi
```

Then update README and docs to remove "from checkout until release" caveats for
the packages that are actually available from public registries.
