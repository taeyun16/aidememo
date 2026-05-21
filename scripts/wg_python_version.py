#!/usr/bin/env python3

import re
import sys
import tomllib
from pathlib import Path


SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$")


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
        if in_section and stripped.startswith("version"):
            out.append(f'version = "{version}"')
            replaced = True
            continue
        out.append(line)

    if in_section and not replaced:
        out.append(f'version = "{version}"')
        replaced = True

    if not replaced:
        raise SystemExit(f"{path}: missing [{section}] version")

    path.write_text("\n".join(out) + "\n")


def main() -> None:
    if len(sys.argv) not in (2, 3):
        raise SystemExit("usage: wg_python_version.py <repo-root> [semver]")

    root = Path(sys.argv[1]).resolve()
    next_version = sys.argv[2] if len(sys.argv) == 3 else ""
    if next_version and not SEMVER.match(next_version):
        raise SystemExit(f"invalid semver: {next_version}")

    cargo_path = root / "Cargo.toml"
    pyproject_path = root / "crates" / "wg-python" / "pyproject.toml"

    if next_version:
        replace_section_version(cargo_path, "workspace.package", next_version)
        replace_section_version(pyproject_path, "project", next_version)

    cargo = read_toml(cargo_path)
    pyproject = read_toml(pyproject_path)

    cargo_version = cargo["workspace"]["package"]["version"]
    project = pyproject["project"]
    python_version = project["version"]

    if cargo_version != python_version:
        raise SystemExit(
            "wg-python version drift: "
            f"Cargo workspace={cargo_version} pyproject={python_version}"
        )
    if project["name"] != "wg-python":
        raise SystemExit(f"unexpected Python package name: {project['name']}")
    if pyproject["build-system"]["build-backend"] != "maturin":
        raise SystemExit("wg-python build-backend must be maturin")
    if pyproject["tool"]["maturin"]["module-name"] != "wg_python":
        raise SystemExit("wg-python module-name must be wg_python")

    print(f"OK: wg-python version pinned at {python_version}")


if __name__ == "__main__":
    main()
