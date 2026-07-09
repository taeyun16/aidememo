---
title: Release Checklist
description: Publish order and preflight checks for AideMemo packages.
---

# Release Checklist

This page records the package publish order. It is intentionally conservative:
publish lower-level packages first, then install and smoke the layers above
them.

## 1. Registry and repository setup

Create the repository environments and registry trusted-publisher entries before
the first live publish run. The configured PyPI and npm workflows use OIDC, so
do not add long-lived `PYPI_API_TOKEN` or `NPM_TOKEN` repository secrets for the
normal release path.

GitHub environments:

| Environment | Workflows | Purpose |
|---|---|---|
| `pypi-publish` | `.github/workflows/aidememo-python-publish.yml`, `.github/workflows/aidememo-agent-sdk-publish.yml`, `.github/workflows/hermes-aidememo-publish.yml` | Approval gate for PyPI trusted publishing |
| `npm-publish` | `.github/workflows/aidememo-napi-publish.yml` | Approval gate for npm trusted publishing |

Recommended protection: require a reviewer for both environments and restrict
deployment branches/tags to the release branches or tags that the project uses.

PyPI trusted publishers:

| Project | GitHub owner/repo | Workflow | Environment | Status |
|---|---|---|---|---|
| `aidememo-python` | `taeyun16/aidememo` | `aidememo-python-publish.yml` | `pypi-publish` | Workflow ready |
| `aidememo-agent-sdk` | `taeyun16/aidememo` | `aidememo-agent-sdk-publish.yml` | `pypi-publish` | Workflow ready |
| `hermes-aidememo` | `taeyun16/aidememo` | `hermes-aidememo-publish.yml` | `pypi-publish` | Workflow ready |

npm trusted publishers:

Register each npm package with GitHub owner/repo `taeyun16/aidememo`, workflow
`aidememo-napi-publish.yml`, and environment `npm-publish`:

1. `aidememo-napi`
2. `aidememo-napi-darwin-arm64`
3. `aidememo-napi-darwin-x64`
4. `aidememo-napi-linux-arm64-gnu`
5. `aidememo-napi-linux-x64-gnu`
6. `aidememo-napi-win32-x64-msvc`

Rust crates currently publish from an operator machine, not from GitHub Actions.
Use `cargo login` or a local `CARGO_REGISTRY_TOKEN` when running
`cargo publish`; do not add a repository secret unless a Rust publish workflow
is introduced.

The Elixir NIF is currently documented as a local/path binding. There is no Hex
publish workflow or repository `HEX_API_KEY` requirement yet; add those only if
the project decides to publish `aidememo_nif` through Hex.

Optional runtime keys such as `OPENAI_API_KEY` are for local feature use
(`aidememo extract --llm`) and are not release secrets.

## 2. Local preflight

Run the local release gate from a clean checkout:

```bash
scripts/release-preflight.sh
```

This includes the version pins, changelog release gate, workflow syntax lint,
docs feature coverage gate, registry readiness gate, docs-site build, binding
smoke, agent SDK/Hermes wheel smoke, workflow smoke, and SDK promotion check.

The changelog release gate is offline and should pass after cutting the current
release notes out of `Unreleased`:

```bash
mise run changelog-release-check
python3 scripts/changelog-release-check.py 0.1.0
```

It verifies that `CHANGELOG.md` has an empty `[Unreleased]` section, one dated
current-version section immediately below it, and non-empty release-note
content. Set `AIDEMEMO_RELEASE_PREFLIGHT_CHANGELOG=0` only for focused
non-release debugging.

The registry readiness gate is offline and should pass before creating or
editing registry entries:

```bash
python3 scripts/registry-readiness-check.py
```

It verifies that PyPI trusted-publisher project names, workflow names,
GitHub environments, npm root/platform package names, and this release document
stay aligned. It also rejects first-party publish workflows that drift back to
long-lived publish-token assumptions.

`maturin` is intentionally run through `uvx` using the pinned spec from
`mise.toml`, not from whichever `maturin` happens to be on `PATH`.

The Python binding uses PyO3 0.29. Release smoke scripts prefer the same
Python 3.13 interpreter used by CI, while accepting PyO3-supported local
interpreters. To force a specific interpreter, set it explicitly:

```bash
AIDEMEMO_PYO3_PYTHON=python3.13 scripts/release-preflight.sh
```

For a full registry dry-run:

```bash
AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full scripts/release-preflight.sh 0.1.0
```

The full profile also runs the Rust package readiness gate. Standalone use is:

```bash
scripts/cargo-package-readiness.sh
```

CI also runs the same gate in the `cargo-package-readiness` job. That PR guard
enforces `aidememo-core` packageability while keeping dependent Rust crates as
the documented publish-order skip until `aidememo-core` exists on crates.io.

## 3. Rust crates

Publish in dependency order:

1. `aidememo-core`
2. `aidememo-cli`
3. `aidememo-ffi`, `aidememo-napi`, `aidememo-nif`, `aidememo-python`

`aidememo-cli` and all native bindings depend on `aidememo-core`, so their
`cargo package` checks will fail against crates.io until `aidememo-core` is
published at the matching version.

The readiness script packages `aidememo-core` by default and records dependent
Rust crates as a deliberate skip until that first publish-order blocker is
removed. After `aidememo-core` is visible on crates.io at the matching version,
run the full dependent check:

```bash
AIDEMEMO_CARGO_PACKAGE_CHECK_DEPENDENTS=1 scripts/cargo-package-readiness.sh
```

## 4. Python packages

Publish native bindings before composition packages:

1. `aidememo-python`
2. `aidememo-agent-sdk`
3. `hermes-aidememo`

Local Python payload checks:

```bash
mise run python-pack-smoke
mise run python-publish-dry-run
mise run agent-sdk-publish-dry-run
mise run hermes-publish-dry-run
```

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

## 5. Node package

Publish the platform packages before the root wrapper:

1. `aidememo-napi-*` platform packages
2. `aidememo-napi`

Use each trusted-publisher workflow with the exact version input. The default
workflow mode is dry-run:

1. `.github/workflows/aidememo-python-publish.yml`
2. `.github/workflows/aidememo-agent-sdk-publish.yml`
3. `.github/workflows/hermes-aidememo-publish.yml`

## 6. Post-release checks

After each registry publish:

```bash
cargo install aidememo-cli
python -m pip install aidememo-agent-sdk
python -m pip install "aidememo-agent-sdk[binding]"
npm install aidememo-napi
```

Then update README and docs to remove "from checkout until release" caveats for
the packages that are actually available from public registries.
