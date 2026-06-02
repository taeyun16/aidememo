#!/usr/bin/env python3

import re
import sys
import tomllib
from pathlib import Path


SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$")
MIX_VERSION = re.compile(r'^(\s*)version:\s*"([^"]+)"(,?\s*)$')


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


def read_mix_version(path: Path) -> str:
    for line in path.read_text().splitlines():
        match = MIX_VERSION.match(line)
        if match:
            return match.group(2)
    raise SystemExit(f"{path}: missing project version")


def replace_mix_version(path: Path, version: str) -> None:
    lines = path.read_text().splitlines()
    out = []
    replaced = False

    for line in lines:
        match = MIX_VERSION.match(line)
        if match and not replaced:
            out.append(f'{match.group(1)}version: "{version}"{match.group(3)}')
            replaced = True
            continue
        out.append(line)

    if not replaced:
        raise SystemExit(f"{path}: missing project version")

    path.write_text("\n".join(out) + "\n")


def main() -> None:
    if len(sys.argv) not in (2, 3):
        raise SystemExit("usage: aidememo_nif_version.py <repo-root> [semver]")

    root = Path(sys.argv[1]).resolve()
    next_version = sys.argv[2] if len(sys.argv) == 3 else ""
    if next_version and not SEMVER.match(next_version):
        raise SystemExit(f"invalid semver: {next_version}")

    cargo_path = root / "Cargo.toml"
    mix_path = root / "crates" / "aidememo-nif" / "mix.exs"

    if next_version:
        replace_section_version(cargo_path, "workspace.package", next_version)
        replace_mix_version(mix_path, next_version)

    cargo_version = read_toml(cargo_path)["workspace"]["package"]["version"]
    mix_version = read_mix_version(mix_path)

    if cargo_version != mix_version:
        raise SystemExit(
            "aidememo-nif version drift: "
            f"Cargo workspace={cargo_version} mix.exs={mix_version}"
        )

    print(f"OK: aidememo-nif version pinned at {mix_version}")


if __name__ == "__main__":
    main()
