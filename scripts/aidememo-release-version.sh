#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$#" -gt 1 ]]; then
    echo "usage: $0 [semver]" >&2
    echo "example: $0 0.1.1" >&2
    exit 1
fi

VERSION="${1:-}"
if [[ -n "$VERSION" ]]; then
    "$ROOT_DIR/scripts/aidememo-python-version.sh" "$VERSION"
    "$ROOT_DIR/scripts/aidememo-napi-version.sh" "$VERSION"
    "$ROOT_DIR/scripts/aidememo-nif-version.sh" "$VERSION"
else
    "$ROOT_DIR/scripts/aidememo-python-version.sh"
    "$ROOT_DIR/scripts/aidememo-napi-version.sh"
    "$ROOT_DIR/scripts/aidememo-nif-version.sh"
fi

python3 - "$ROOT_DIR" "$VERSION" <<'PY'
import re
import sys
import tomllib
from pathlib import Path

SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$")
VERSION_LINE = re.compile(r'^(\s*version\s*=\s*)"([^"]+)"(\s*)$')
DUNDER_VERSION = re.compile(r'^(__version__\s*=\s*)"([^"]+)"(\s*)$')
YAML_VERSION = re.compile(r'^(\s*version:\s*)"([^"]+)"(\s*)$')


def read_toml(path: Path) -> dict:
    with path.open("rb") as f:
        return tomllib.load(f)


def replace_section_version(path: Path, section: str, version: str) -> None:
    lines = path.read_text().splitlines()
    in_section = False
    replaced = False
    out = []
    for line in lines:
        stripped = line.strip()
        if stripped == f"[{section}]":
            in_section = True
            out.append(line)
            continue
        if in_section and stripped.startswith("[") and stripped.endswith("]"):
            if not replaced:
                out.append(f'version = "{version}"')
                replaced = True
            in_section = False
        if in_section and VERSION_LINE.match(line):
            out.append(VERSION_LINE.sub(rf'\g<1>"{version}"\3', line))
            replaced = True
            continue
        out.append(line)
    if in_section and not replaced:
        out.append(f'version = "{version}"')
        replaced = True
    if not replaced:
        raise SystemExit(f"{path}: missing [{section}] version")
    path.write_text("\n".join(out) + "\n")


def replace_regex_line(path: Path, pattern: re.Pattern[str], version: str, label: str) -> None:
    lines = path.read_text().splitlines()
    replaced = False
    out = []
    for line in lines:
        if pattern.match(line):
            out.append(pattern.sub(rf'\g<1>"{version}"\3', line))
            replaced = True
        else:
            out.append(line)
    if not replaced:
        raise SystemExit(f"{path}: missing {label}")
    path.write_text("\n".join(out) + "\n")


root = Path(sys.argv[1]).resolve()
next_version = sys.argv[2]
if next_version and not SEMVER.match(next_version):
    raise SystemExit(f"invalid semver: {next_version}")

managed = [
    {
        "name": "aidememo-agent-sdk",
        "pyproject": root / "packages" / "aidememo-agent-sdk" / "pyproject.toml",
        "init": root
        / "packages"
        / "aidememo-agent-sdk"
        / "src"
        / "aidememo_agent"
        / "__init__.py",
        "plugin_yaml": None,
    },
    {
        "name": "hermes-aidememo",
        "pyproject": root / "plugins" / "hermes" / "pyproject.toml",
        "init": root
        / "plugins"
        / "hermes"
        / "src"
        / "hermes_aidememo"
        / "__init__.py",
        "plugin_yaml": root
        / "plugins"
        / "hermes"
        / "src"
        / "hermes_aidememo"
        / "plugin.yaml",
    },
]

if next_version:
    for package in managed:
        replace_section_version(package["pyproject"], "project", next_version)
        replace_regex_line(package["init"], DUNDER_VERSION, next_version, "__version__")
        if package["plugin_yaml"] is not None:
            replace_regex_line(package["plugin_yaml"], YAML_VERSION, next_version, "version")

with (root / "Cargo.toml").open("rb") as f:
    version = tomllib.load(f)["workspace"]["package"]["version"]

for package in managed:
    pyproject = read_toml(package["pyproject"])
    project = pyproject["project"]
    if project["name"] != package["name"]:
        raise SystemExit(
            f"{package['pyproject']}: unexpected package name {project['name']!r}"
        )
    if project["version"] != version:
        raise SystemExit(
            f"{package['name']} version drift: "
            f"Cargo workspace={version} pyproject={project['version']}"
        )
    init_text = package["init"].read_text()
    expected = f'__version__ = "{version}"'
    if expected not in init_text:
        raise SystemExit(
            f"{package['init']}: runtime __version__ does not match {version}"
        )
    if package["plugin_yaml"] is not None:
        yaml_expected = f'version: "{version}"'
        if yaml_expected not in package["plugin_yaml"].read_text():
            raise SystemExit(
                f"{package['plugin_yaml']}: plugin version does not match {version}"
            )

print(
    "OK: aidememo release version pinned at "
    f"{version} across Cargo, Python, npm, NIF, agent SDK, and Hermes packages "
    "(aidememo-ffi uses Cargo metadata)"
)
PY
