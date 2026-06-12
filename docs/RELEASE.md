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
| `pypi-publish` | `.github/workflows/aidememo-python-publish.yml` | Approval gate for PyPI trusted publishing |
| `npm-publish` | `.github/workflows/aidememo-napi-publish.yml` | Approval gate for npm trusted publishing |

Recommended protection: require a reviewer for both environments and restrict
deployment branches/tags to the release branches or tags that the project uses.

PyPI trusted publishers:

| Project | GitHub owner/repo | Workflow | Environment | Status |
|---|---|---|---|---|
| `aidememo-python` | `taeyun16/aidememo` | `aidememo-python-publish.yml` | `pypi-publish` | Workflow ready |
| `aidememo-agent-sdk` | `taeyun16/aidememo` | none yet | none yet | Keep checkout/manual publish docs until a workflow is added |
| `hermes-aidememo` | `taeyun16/aidememo` | none yet | none yet | Keep checkout/manual publish docs until a workflow is added |

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

This includes the version pins, workflow syntax lint, docs feature coverage
gate, docs-site build, binding smoke, workflow smoke, and SDK promotion check.

`maturin` is intentionally run through `uvx` using the pinned spec from
`mise.toml`, not from whichever `maturin` happens to be on `PATH`.

The Python binding uses PyO3 0.23, so release smoke scripts select a Python
3.9-3.13 interpreter for native builds. If your default `python3` is newer,
set the interpreter explicitly:

```bash
AIDEMEMO_PYO3_PYTHON=python3.13 scripts/release-preflight.sh
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

## 3. Rust crates

Publish in dependency order:

1. `aidememo-core`
2. `aidememo-cli`
3. `aidememo-ffi`, `aidememo-napi`, `aidememo-nif`, `aidememo-python`

`aidememo-cli` and all native bindings depend on `aidememo-core`, so their
`cargo package` checks will fail against crates.io until `aidememo-core` is
published at the matching version.

## 4. Python packages

Publish native bindings before composition packages:

1. `aidememo-python`
2. `aidememo-agent-sdk`
3. `hermes-aidememo`

Local Python payload checks:

```bash
mise run python-pack-smoke
mise run python-publish-dry-run
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

Use the trusted-publisher workflow with the exact version input. The default
workflow mode is dry-run.

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
