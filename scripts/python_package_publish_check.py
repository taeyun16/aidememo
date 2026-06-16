#!/usr/bin/env python3

import sys
import tarfile
import zipfile
from dataclasses import dataclass
from email.parser import Parser
from pathlib import PurePosixPath
from typing import Iterable


@dataclass(frozen=True)
class PackageConfig:
    distribution_name: str
    file_stem: str
    wheel_required: frozenset[str]
    sdist_required: frozenset[str]
    requires_dist: frozenset[str]


CONFIGS: dict[str, PackageConfig] = {
    "aidememo-agent-sdk": PackageConfig(
        distribution_name="aidememo-agent-sdk",
        file_stem="aidememo_agent_sdk",
        wheel_required=frozenset(
            {
                "aidememo_agent/__init__.py",
                "aidememo_agent/client.py",
                "aidememo_agent/sdk.py",
            }
        ),
        sdist_required=frozenset(
            {
                "pyproject.toml",
                "README.md",
                "src/aidememo_agent/__init__.py",
                "src/aidememo_agent/client.py",
                "src/aidememo_agent/sdk.py",
            }
        ),
        requires_dist=frozenset(),
    ),
    "hermes-aidememo": PackageConfig(
        distribution_name="hermes-aidememo",
        file_stem="hermes_aidememo",
        wheel_required=frozenset(
            {
                "hermes_aidememo/__init__.py",
                "hermes_aidememo/capture_adapter.py",
                "hermes_aidememo/client.py",
                "hermes_aidememo/plugin.py",
                "hermes_aidememo/plugin.yaml",
                "hermes_aidememo/skills/aidememo/SKILL.md",
            }
        ),
        sdist_required=frozenset(
            {
                "pyproject.toml",
                "README.md",
                "src/hermes_aidememo/__init__.py",
                "src/hermes_aidememo/capture_adapter.py",
                "src/hermes_aidememo/client.py",
                "src/hermes_aidememo/plugin.py",
                "src/hermes_aidememo/plugin.yaml",
                "src/hermes_aidememo/skills/aidememo/SKILL.md",
            }
        ),
        requires_dist=frozenset({"aidememo-agent-sdk>=0.1", "pyyaml>=6.0"}),
    ),
}

FORBIDDEN_PARTS = {".git", ".pytest_cache", "__pycache__", "target"}
FORBIDDEN_SUFFIXES = {".pyc", ".pyo", ".redb", ".sqlite", ".tgz", ".whl"}


def fail(message: str) -> None:
    raise SystemExit(message)


def normalize_requirement(requirement: str) -> str:
    return requirement.replace(" ", "").lower()


def forbidden_paths(names: Iterable[str]) -> list[str]:
    forbidden = []
    for name in names:
        path = PurePosixPath(name)
        if any(part in FORBIDDEN_PARTS for part in path.parts):
            forbidden.append(name)
            continue
        if any(name.endswith(suffix) for suffix in FORBIDDEN_SUFFIXES):
            forbidden.append(name)
    return forbidden


def check_wheel(path, config: PackageConfig, version: str) -> None:
    with zipfile.ZipFile(path) as wheel:
        names = set(wheel.namelist())
        metadata_paths = [
            name for name in names if name.endswith(".dist-info/METADATA")
        ]
        if len(metadata_paths) != 1:
            fail(f"{path.name}: expected one METADATA file, got {metadata_paths}")

        metadata = Parser().parsestr(
            wheel.read(metadata_paths[0]).decode("utf-8", errors="replace")
        )
        if metadata.get("Name") != config.distribution_name:
            fail(f"{path.name}: unexpected Name {metadata.get('Name')!r}")
        if metadata.get("Version") != version:
            fail(f"{path.name}: unexpected Version {metadata.get('Version')!r}")

        requires = {
            normalize_requirement(value)
            for value in metadata.get_all("Requires-Dist", [])
        }
        for expected in config.requires_dist:
            expected_normalized = normalize_requirement(expected)
            if not any(req.startswith(expected_normalized) for req in requires):
                fail(f"{path.name}: missing Requires-Dist {expected!r}; got {sorted(requires)}")

        required_suffixes = {
            ".dist-info/WHEEL",
            ".dist-info/RECORD",
        }
        for suffix in required_suffixes:
            if not any(name.endswith(suffix) for name in names):
                fail(f"{path.name}: missing {suffix}")

        missing = sorted(config.wheel_required - names)
        if missing:
            fail(f"{path.name}: missing required wheel files: {missing}")

        forbidden = forbidden_paths(names)
        if forbidden:
            fail(f"{path.name}: forbidden payload files: {forbidden[:5]}")


def check_sdist(path, config: PackageConfig, version: str) -> None:
    expected_root = f"{config.file_stem}-{version}/"
    with tarfile.open(path, "r:gz") as sdist:
        names = set(sdist.getnames())

    required = {f"{expected_root}{name}" for name in config.sdist_required}
    missing = sorted(required - names)
    if missing:
        fail(f"{path.name}: missing required sdist files: {missing}")

    unexpected_roots = [
        name for name in names if name and not name.startswith(expected_root)
    ]
    if unexpected_roots:
        fail(f"{path.name}: unexpected sdist roots: {unexpected_roots[:5]}")

    forbidden = forbidden_paths(names)
    if forbidden:
        fail(f"{path.name}: forbidden payload files: {forbidden[:5]}")


def main() -> None:
    if len(sys.argv) != 4:
        fail(
            "usage: python_package_publish_check.py "
            "<aidememo-agent-sdk|hermes-aidememo> <dist-dir> <version>"
        )

    package = sys.argv[1]
    try:
        config = CONFIGS[package]
    except KeyError:
        fail(f"unknown package {package!r}; expected one of {sorted(CONFIGS)}")

    from pathlib import Path

    dist = Path(sys.argv[2]).resolve()
    version = sys.argv[3]
    wheels = sorted(dist.glob(f"{config.file_stem}-*.whl"))
    sdists = sorted(dist.glob(f"{config.file_stem}-*.tar.gz"))

    if len(wheels) != 1:
        fail(f"expected exactly one {config.file_stem} wheel in {dist}, got {len(wheels)}")
    if len(sdists) != 1:
        fail(f"expected exactly one {config.file_stem} sdist in {dist}, got {len(sdists)}")

    check_wheel(wheels[0], config, version)
    check_sdist(sdists[0], config, version)
    print(
        f"OK: {config.distribution_name} publish payload "
        f"wheel={wheels[0].name} sdist={sdists[0].name} version={version}"
    )


if __name__ == "__main__":
    main()
