#!/usr/bin/env python3
"""Validate first-release registry/workflow mappings without network access."""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DOC = ROOT / "docs" / "RELEASE.md"
OWNER_REPO = "taeyun16/aidememo"
PYPI_ENV = "pypi-publish"
NPM_ENV = "npm-publish"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def load_toml(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def fail(failures: list[str], label: str, detail: str) -> None:
    failures.append(f"{label}: {detail}")


def ok(rows: list[str], label: str) -> None:
    rows.append(f"ok: {label}")


def require_contains(failures: list[str], text: str, needle: str, label: str) -> None:
    if needle not in text:
        fail(failures, label, f"missing {needle!r}")


def require_regex(failures: list[str], text: str, pattern: str, label: str) -> None:
    if not re.search(pattern, text, flags=re.MULTILINE):
        fail(failures, label, f"missing pattern {pattern!r}")


def project_version(path: Path) -> tuple[str, str]:
    project = load_toml(path)["project"]
    return project["name"], project["version"]


def validate_pypi_package(
    failures: list[str],
    rows: list[str],
    *,
    package: str,
    pyproject: Path,
    workflow: Path,
    env_var: str,
    dry_run_script: str,
    artifact: str,
    workspace_version: str,
    release_doc: str,
) -> None:
    start_failures = len(failures)
    label = f"PyPI {package}"
    name, version = project_version(pyproject)
    if name != package:
        fail(failures, label, f"{pyproject} project.name={name!r}")
    if version != workspace_version:
        fail(
            failures,
            label,
            f"{pyproject} project.version={version!r} != workspace {workspace_version!r}",
        )

    doc_row = (
        f"| `{package}` | `{OWNER_REPO}` | `{workflow.name}` | `{PYPI_ENV}` | Workflow ready |"
    )
    require_contains(failures, release_doc, doc_row, f"{label} docs row")

    text = read(workflow)
    require_contains(failures, text, f"name: {package} publish", f"{label} workflow name")
    require_contains(failures, text, "workflow_dispatch:", f"{label} manual trigger")
    require_regex(failures, text, r"^\s+version:\s*$", f"{label} version input")
    require_regex(failures, text, r"^\s+dry_run:\s*$", f"{label} dry-run input")
    require_regex(failures, text, r"^\s+default:\s+true\s*$", f"{label} dry-run default")
    require_contains(failures, text, f"{env_var}: ${{{{ inputs.version }}}}", f"{label} version env")
    require_contains(failures, text, dry_run_script, f"{label} dry-run script")
    require_contains(failures, text, f"name: {artifact}", f"{label} artifact upload")
    require_contains(failures, text, "if: ${{ ! inputs.dry_run }}", f"{label} publish guard")
    require_contains(failures, text, f"environment: {PYPI_ENV}", f"{label} environment")
    require_contains(failures, text, "id-token: write", f"{label} OIDC permission")
    require_contains(
        failures,
        text,
        "pypa/gh-action-pypi-publish@release/v1",
        f"{label} PyPI action",
    )
    require_contains(failures, text, "packages-dir: dist", f"{label} dist directory")
    for forbidden in ("PYPI_API_TOKEN", "pypi-token", "password:"):
        if forbidden in text:
            fail(failures, label, f"workflow should not require long-lived PyPI token {forbidden!r}")
    if len(failures) == start_failures:
        ok(rows, label)


def validate_npm(failures: list[str], rows: list[str], workspace_version: str, release_doc: str) -> None:
    start_failures = len(failures)
    label = "npm aidememo-napi"
    package_path = ROOT / "crates" / "aidememo-napi" / "package.json"
    root_pkg = json.loads(read(package_path))
    if root_pkg["name"] != "aidememo-napi":
        fail(failures, label, f"root package name={root_pkg['name']!r}")
    if root_pkg["version"] != workspace_version:
        fail(
            failures,
            label,
            f"root package version={root_pkg['version']!r} != workspace {workspace_version!r}",
        )
    if root_pkg.get("publishConfig", {}).get("access") != "public":
        fail(failures, label, "root package publishConfig.access must be public")
    if root_pkg.get("homepage") != "https://aidememo.taeyun.me":
        fail(failures, label, "root package homepage must use the documentation site")
    root_keywords = set(root_pkg.get("keywords", []))
    required_root_keywords = {
        "aidememo",
        "agent-memory",
        "knowledge-graph",
        "local-first",
        "rag",
        "semantic-search",
        "sqlite",
        "napi",
    }
    missing_root_keywords = sorted(required_root_keywords - root_keywords)
    if missing_root_keywords:
        fail(failures, label, f"root package keywords missing {missing_root_keywords}")
    if "README.md" not in root_pkg.get("files", []):
        fail(failures, label, "root package files must include README.md")

    optional = root_pkg.get("optionalDependencies", {})
    platform_dir = ROOT / "crates" / "aidememo-napi" / "npm"
    platform_packages = []
    for path in sorted(platform_dir.glob("*/package.json")):
        data = json.loads(read(path))
        platform_packages.append(data["name"])
        if optional.get(data["name"]) != workspace_version:
            fail(
                failures,
                label,
                f"root optionalDependency for {data['name']} != {workspace_version}",
            )
        if data["version"] != workspace_version:
            fail(failures, label, f"{path} version={data['version']!r}")
        if data.get("publishConfig", {}).get("access") != "public":
            fail(failures, label, f"{path} publishConfig.access must be public")
        if data.get("homepage") != "https://aidememo.taeyun.me":
            fail(failures, label, f"{path} homepage must use the documentation site")
        if not data.get("description", "").startswith("Prebuilt AideMemo native binding"):
            fail(failures, label, f"{path} description must identify the prebuilt binding")
        if data.get("engines", {}).get("node") != ">= 16":
            fail(failures, label, f"{path} must document the supported Node.js version")
        if "README.md" not in data.get("files", []):
            fail(failures, label, f"{path} files must include README.md")
        platform_readme = path.with_name("README.md")
        if not platform_readme.exists():
            fail(failures, label, f"{platform_readme} is missing")
        platform_keywords = set(data.get("keywords", []))
        missing_platform_keywords = sorted(
            {"aidememo", "aidememo-napi", "agent-memory", "nodejs", "napi"}
            - platform_keywords
        )
        if missing_platform_keywords:
            fail(
                failures,
                label,
                f"{path} keywords missing {missing_platform_keywords}",
            )

    expected_packages = ["aidememo-napi", *platform_packages]
    for package in expected_packages:
        require_regex(
            failures,
            release_doc,
            rf"(?:^|\n)(?:\d+\. |)\`{re.escape(package)}\`",
            f"npm docs package {package}",
        )

    workflow = ROOT / ".github" / "workflows" / "aidememo-napi-publish.yml"
    text = read(workflow)
    require_contains(failures, text, "name: aidememo-napi publish", f"{label} workflow name")
    require_contains(failures, text, "workflow_dispatch:", f"{label} manual trigger")
    require_regex(failures, text, r"^\s+version:\s*$", f"{label} version input")
    require_regex(failures, text, r"^\s+dry_run:\s*$", f"{label} dry-run input")
    require_regex(failures, text, r"^\s+bootstrap:\s*$", f"{label} bootstrap input")
    require_regex(failures, text, r"^\s+default:\s+true\s*$", f"{label} dry-run default")
    require_regex(failures, text, r"^\s+default:\s+false\s*$", f"{label} bootstrap default")
    require_contains(
        failures,
        text,
        "AIDEMEMO_NAPI_EXPECT_VERSION: ${{ inputs.version }}",
        f"{label} version env",
    )
    require_contains(failures, text, "id-token: write", f"{label} OIDC permission")
    require_contains(failures, text, f"environment: {NPM_ENV}", f"{label} environment")
    require_contains(failures, text, 'registry-url: "https://registry.npmjs.org"', f"{label} registry")
    require_contains(failures, text, "AIDEMEMO_NAPI_PUBLISH_SCOPE: platform", f"{label} platform scope")
    require_contains(failures, text, "AIDEMEMO_NAPI_PUBLISH_SCOPE: root", f"{label} root scope")
    require_contains(failures, text, "AIDEMEMO_NAPI_PUBLISH_MODE:", f"{label} publish mode")
    require_contains(failures, text, "npm@11.15.0", f"{label} trusted-publishing npm pin")
    require_contains(
        failures,
        text,
        "AIDEMEMO_NAPI_BOOTSTRAP_TOKEN: ${{ inputs.bootstrap && secrets.NPM_TOKEN || '' }}",
        f"{label} one-time bootstrap token",
    )
    require_contains(
        failures,
        text,
        "AIDEMEMO_NAPI_EXPECT_PLATFORM_PACKAGE: ${{ matrix.package }}",
        f"{label} platform identity guard",
    )
    matrix = {
        "ubuntu-24.04": "aidememo-napi-linux-x64-gnu",
        "ubuntu-24.04-arm": "aidememo-napi-linux-arm64-gnu",
        "macos-15-intel": "aidememo-napi-darwin-x64",
        "macos-15": "aidememo-napi-darwin-arm64",
        "windows-2025": "aidememo-napi-win32-x64-msvc",
    }
    for runner, package in matrix.items():
        require_contains(failures, text, f"runner: {runner}", f"{label} matrix runner {runner}")
        require_contains(failures, text, f"package: {package}", f"{label} matrix package {package}")
    for forbidden in ("_authToken", "npm token"):
        if forbidden in text:
            fail(failures, label, f"workflow should not require long-lived npm token {forbidden!r}")

    if len(failures) == start_failures:
        ok(rows, f"{label} ({len(expected_packages)} packages)")


def validate_non_oidc_registry_notes(failures: list[str], rows: list[str], release_doc: str) -> None:
    start_failures = len(failures)
    require_contains(
        failures,
        release_doc,
        "| `crates-publish` | `.github/workflows/crates-publish.yml` |",
        "Rust crates environment docs",
    )
    require_contains(
        failures,
        release_doc,
        "There is no Hex\npublish workflow or repository `HEX_API_KEY` requirement yet",
        "Hex registry note",
    )
    rust_workflow = ROOT / ".github" / "workflows" / "crates-publish.yml"
    if not rust_workflow.exists():
        fail(failures, "Rust crates registry note", "crates-publish.yml is missing")
    else:
        rust_text = read(rust_workflow)
        require_contains(failures, rust_text, "environment: crates-publish", "Rust crates environment")
        require_contains(failures, rust_text, "id-token: write", "Rust crates OIDC permission")
        require_contains(
            failures,
            rust_text,
            "rust-lang/crates-io-auth-action@v1",
            "Rust crates OIDC authentication action",
        )
    if len(failures) == start_failures:
        ok(rows, "Rust OIDC and non-OIDC registry notes")


def validate_cargo_package_ci(failures: list[str], rows: list[str], release_doc: str) -> None:
    start_failures = len(failures)
    label = "Cargo publish dry-run CI"
    ci_text = read(ROOT / ".github" / "workflows" / "ci.yml")
    scripts_readme = read(ROOT / "scripts" / "README.md")
    measurements = read(ROOT / "docs" / "MEASUREMENTS.md")

    require_contains(failures, ci_text, "cargo-package-readiness:", f"{label} job id")
    require_contains(failures, ci_text, "name: Rust publish dry-run readiness", f"{label} job name")
    require_contains(
        failures,
        ci_text,
        "AIDEMEMO_CARGO_PACKAGE_CHECK_DEPENDENTS: \"0\"",
        f"{label} dependent skip default",
    )
    require_contains(
        failures,
        ci_text,
        "scripts/cargo-package-readiness.sh",
        f"{label} script call",
    )
    require_contains(
        failures,
        release_doc,
        "scripts/cargo-package-readiness.sh",
        f"{label} release docs",
    )
    require_contains(
        failures,
        scripts_readme,
        "cargo-package-readiness.sh",
        f"{label} script inventory",
    )
    require_contains(
        failures,
        measurements,
        "CI `cargo-package-readiness` job",
        f"{label} measurement docs",
    )
    if len(failures) == start_failures:
        ok(rows, label)


def validate_public_registry_smoke(failures: list[str], rows: list[str], release_doc: str) -> None:
    start_failures = len(failures)
    label = "public registry smoke"
    script = ROOT / "scripts" / "public-registry-smoke.sh"
    workflow = ROOT / ".github" / "workflows" / "public-registry-smoke.yml"
    scripts_readme = read(ROOT / "scripts" / "README.md")
    measurements = read(ROOT / "docs" / "MEASUREMENTS.md")
    installation = read(ROOT / "docs" / "INSTALLATION.md")
    mise = read(ROOT / "mise.toml")

    if not script.exists():
        fail(failures, label, "scripts/public-registry-smoke.sh is missing")
    else:
        text = read(script)
        require_contains(
            failures,
            text,
            "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE",
            f"{label} mode env",
        )
        require_contains(failures, text, "cargo install aidememo-cli", f"{label} cargo plan")
        require_contains(failures, text, "aidememo-agent-sdk[binding]", f"{label} binding plan")
        require_contains(failures, text, "npm install aidememo-napi", f"{label} npm plan")

    if not workflow.exists():
        fail(failures, label, ".github/workflows/public-registry-smoke.yml is missing")
    else:
        text = read(workflow)
        require_contains(failures, text, "name: public registry smoke", f"{label} workflow name")
        require_contains(failures, text, "workflow_dispatch:", f"{label} workflow dispatch")
        require_regex(failures, text, r"^\s+version:\s*$", f"{label} version input")
        require_regex(failures, text, r"^\s+mode:\s*$", f"{label} mode input")
        require_contains(failures, text, "type: choice", f"{label} mode choice")
        require_contains(
            failures,
            text,
            "AIDEMEMO_PUBLIC_REGISTRY_VERSION: ${{ inputs.version }}",
            f"{label} version env",
        )
        require_contains(
            failures,
            text,
            "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE: ${{ inputs.mode }}",
            f"{label} mode env",
        )
        require_contains(
            failures,
            text,
            "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_AGENT_SDK_BINDING:",
            f"{label} binding env",
        )
        require_contains(
            failures,
            text,
            'registry-url: "https://registry.npmjs.org"',
            f"{label} npm registry",
        )
        require_contains(
            failures,
            text,
            "scripts/public-registry-smoke.sh",
            f"{label} workflow script",
        )
    require_contains(
        failures,
        release_doc,
        "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE=verify scripts/public-registry-smoke.sh",
        f"{label} release verify command",
    )
    require_contains(
        failures,
        release_doc,
        ".github/workflows/public-registry-smoke.yml",
        f"{label} release workflow docs",
    )
    require_contains(
        failures,
        scripts_readme,
        "public-registry-smoke.sh",
        f"{label} script inventory",
    )
    require_contains(
        failures,
        measurements,
        "scripts/public-registry-smoke.sh",
        f"{label} measurement docs",
    )
    require_contains(
        failures,
        measurements,
        ".github/workflows/public-registry-smoke.yml",
        f"{label} measurement workflow docs",
    )
    require_contains(
        failures,
        installation,
        "mise run public-registry-smoke",
        f"{label} installation docs",
    )
    require_contains(
        failures,
        mise,
        "[tasks.public-registry-smoke]",
        f"{label} mise task",
    )
    if len(failures) == start_failures:
        ok(rows, label)


def validate_release_preflight_workflow(failures: list[str], rows: list[str], release_doc: str) -> None:
    start_failures = len(failures)
    label = "release preflight workflow"
    workflow = ROOT / ".github" / "workflows" / "release-preflight.yml"
    scripts_readme = read(ROOT / "scripts" / "README.md")
    measurements = read(ROOT / "docs" / "MEASUREMENTS.md")
    installation = read(ROOT / "docs" / "INSTALLATION.md")

    if not workflow.exists():
        fail(failures, label, ".github/workflows/release-preflight.yml is missing")
    else:
        text = read(workflow)
        require_contains(failures, text, "name: release preflight", f"{label} workflow name")
        require_contains(failures, text, "workflow_dispatch:", f"{label} workflow dispatch")
        require_regex(failures, text, r"^\s+version:\s*$", f"{label} version input")
        require_regex(failures, text, r"^\s+profile:\s*$", f"{label} profile input")
        require_contains(failures, text, "publish_dry_runs:", f"{label} publish dry-run input")
        require_contains(failures, text, "cargo_package:", f"{label} cargo package input")
        require_contains(failures, text, "optional_bindings:", f"{label} optional binding input")
        require_contains(
            failures,
            text,
            "AIDEMEMO_RELEASE_PREFLIGHT_PROFILE: ${{ inputs.profile }}",
            f"{label} profile env",
        )
        require_contains(
            failures,
            text,
            "AIDEMEMO_RELEASE_PREFLIGHT_CARGO_PACKAGE:",
            f"{label} cargo package env",
        )
        require_contains(
            failures,
            text,
            "AIDEMEMO_RELEASE_PREFLIGHT_PUBLISH:",
            f"{label} publish env",
        )
        require_contains(
            failures,
            text,
            "AIDEMEMO_RELEASE_PREFLIGHT_BINDINGS_OPTIONAL:",
            f"{label} optional binding env",
        )
        require_contains(
            failures,
            text,
            'python -m pip install "uv==0.11.21"',
            f"{label} uv install",
        )
        require_contains(
            failures,
            text,
            "go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.1",
            f"{label} actionlint install",
        )
        require_contains(failures, text, "npm --prefix website ci", f"{label} docs deps")
        require_contains(failures, text, "scripts/release-preflight.sh", f"{label} script call")

    require_contains(
        failures,
        release_doc,
        ".github/workflows/release-preflight.yml",
        f"{label} release docs",
    )
    require_contains(
        failures,
        scripts_readme,
        ".github/workflows/release-preflight.yml",
        f"{label} script inventory",
    )
    require_contains(
        failures,
        measurements,
        ".github/workflows/release-preflight.yml",
        f"{label} measurement docs",
    )
    require_contains(
        failures,
        installation,
        ".github/workflows/release-preflight.yml",
        f"{label} installation docs",
    )
    if len(failures) == start_failures:
        ok(rows, label)


def validate_public_onboarding_gates(
    failures: list[str], rows: list[str], release_doc: str
) -> None:
    start_failures = len(failures)
    label = "public onboarding gates"
    workflow = ROOT / ".github" / "workflows" / "fresh-checkout-smoke.yml"
    portability = ROOT / "scripts" / "public-portability-check.py"
    ci_text = read(ROOT / ".github" / "workflows" / "ci.yml")
    preflight = read(ROOT / "scripts" / "release-preflight.sh")
    scripts_readme = read(ROOT / "scripts" / "README.md")
    measurements = read(ROOT / "docs" / "MEASUREMENTS.md")
    installation = read(ROOT / "docs" / "INSTALLATION.md")

    if not workflow.exists():
        fail(failures, label, ".github/workflows/fresh-checkout-smoke.yml is missing")
    else:
        text = read(workflow)
        require_contains(failures, text, "name: fresh checkout smoke", f"{label} workflow name")
        require_contains(failures, text, "workflow_dispatch:", f"{label} manual trigger")
        require_contains(failures, text, "pull_request:", f"{label} PR trigger")
        require_contains(
            failures,
            text,
            "scripts/fresh-checkout-smoke.sh",
            f"{label} workflow script",
        )
        require_contains(failures, text, "toolchain: 1.96.0", f"{label} Rust toolchain")
        require_contains(failures, text, 'python-version: "3.13"', f"{label} Python toolchain")

    if not portability.exists():
        fail(failures, label, "scripts/public-portability-check.py is missing")
    else:
        text = read(portability)
        require_contains(failures, text, "git", f"{label} tracked-file scan")
        require_contains(failures, text, "macOS user home", f"{label} macOS path rule")
        require_contains(failures, text, "Windows user home", f"{label} Windows path rule")

    require_contains(
        failures,
        ci_text,
        "python3 scripts/public-portability-check.py",
        f"{label} CI portability gate",
    )
    require_contains(
        failures,
        preflight,
        'run "public portability gate" python3 "$ROOT_DIR/scripts/public-portability-check.py"',
        f"{label} release portability gate",
    )
    for text, token, detail in (
        (release_doc, ".github/workflows/fresh-checkout-smoke.yml", "release workflow docs"),
        (release_doc, "scripts/public-portability-check.py", "release portability docs"),
        (scripts_readme, "public-portability-check.py", "script inventory"),
        (measurements, ".github/workflows/fresh-checkout-smoke.yml", "measurement workflow docs"),
        (measurements, "scripts/public-portability-check.py", "measurement portability docs"),
        (installation, ".github/workflows/fresh-checkout-smoke.yml", "installation workflow docs"),
    ):
        require_contains(failures, text, token, f"{label} {detail}")
    if len(failures) == start_failures:
        ok(rows, label)


def main() -> int:
    failures: list[str] = []
    rows: list[str] = []
    release_doc = read(DOC)
    workspace_version = load_toml(ROOT / "Cargo.toml")["workspace"]["package"]["version"]

    validate_pypi_package(
        failures,
        rows,
        package="aidememo-python",
        pyproject=ROOT / "crates" / "aidememo-python" / "pyproject.toml",
        workflow=ROOT / ".github" / "workflows" / "aidememo-python-publish.yml",
        env_var="AIDEMEMO_PYTHON_EXPECT_VERSION",
        dry_run_script="AIDEMEMO_PYTHON_DIST_DIR=dist scripts/aidememo-python-publish-dry-run.sh",
        artifact="aidememo-python-dist",
        workspace_version=workspace_version,
        release_doc=release_doc,
    )
    validate_pypi_package(
        failures,
        rows,
        package="aidememo-agent-sdk",
        pyproject=ROOT / "packages" / "aidememo-agent-sdk" / "pyproject.toml",
        workflow=ROOT / ".github" / "workflows" / "aidememo-agent-sdk-publish.yml",
        env_var="AIDEMEMO_AGENT_SDK_EXPECT_VERSION",
        dry_run_script="AIDEMEMO_AGENT_SDK_DIST_DIR=dist scripts/aidememo-agent-sdk-publish-dry-run.sh",
        artifact="aidememo-agent-sdk-dist",
        workspace_version=workspace_version,
        release_doc=release_doc,
    )
    validate_pypi_package(
        failures,
        rows,
        package="hermes-aidememo",
        pyproject=ROOT / "plugins" / "hermes" / "pyproject.toml",
        workflow=ROOT / ".github" / "workflows" / "hermes-aidememo-publish.yml",
        env_var="HERMES_AIDEMEMO_EXPECT_VERSION",
        dry_run_script="HERMES_AIDEMEMO_DIST_DIR=dist scripts/hermes-aidememo-publish-dry-run.sh",
        artifact="hermes-aidememo-dist",
        workspace_version=workspace_version,
        release_doc=release_doc,
    )
    validate_npm(failures, rows, workspace_version, release_doc)
    validate_non_oidc_registry_notes(failures, rows, release_doc)
    validate_cargo_package_ci(failures, rows, release_doc)
    validate_public_registry_smoke(failures, rows, release_doc)
    validate_release_preflight_workflow(failures, rows, release_doc)
    validate_public_onboarding_gates(failures, rows, release_doc)

    print("registry readiness check")
    for row in rows:
        print(row)
    if failures:
        print("\nfailures:", file=sys.stderr)
        for item in failures:
            print(f"- {item}", file=sys.stderr)
        return 1
    print(
        "OK: registry readiness check passed "
        f"(version={workspace_version}, pypi=3, npm=6, cargo-package-ci, "
        "public-registry-smoke, release-preflight-workflow, public-onboarding-gates, "
        "docs/workflows aligned)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
