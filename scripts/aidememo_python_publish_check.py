#!/usr/bin/env python3

import sys
import tarfile
import zipfile
from email.parser import Parser
from pathlib import Path


def fail(message: str) -> None:
    raise SystemExit(message)


def check_wheel(path: Path, version: str) -> None:
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
        if metadata.get("Name") != "aidememo-python":
            fail(f"{path.name}: unexpected Name {metadata.get('Name')!r}")
        if metadata.get("Version") != version:
            fail(f"{path.name}: unexpected Version {metadata.get('Version')!r}")

        required_suffixes = {
            ".dist-info/WHEEL",
            ".dist-info/RECORD",
        }
        for suffix in required_suffixes:
            if not any(name.endswith(suffix) for name in names):
                fail(f"{path.name}: missing {suffix}")

        native_modules = [
            name
            for name in names
            if name.startswith("aidememo_python/") and name.endswith((".so", ".pyd"))
        ]
        if len(native_modules) != 1:
            fail(f"{path.name}: expected one native module, got {native_modules}")

        forbidden = [
            name
            for name in names
            if name.startswith(("target/", ".git/", "tests/")) or name.endswith((".redb", ".tgz"))
        ]
        if forbidden:
            fail(f"{path.name}: forbidden payload files: {forbidden[:5]}")


def check_sdist(path: Path, version: str) -> None:
    expected_root = f"aidememo_python-{version}/"
    with tarfile.open(path, "r:gz") as sdist:
        names = set(sdist.getnames())

    required = {
        f"{expected_root}Cargo.toml",
        f"{expected_root}pyproject.toml",
        f"{expected_root}README.md",
        f"{expected_root}crates/aidememo-python/src/lib.rs",
        f"{expected_root}vendor/tokenizers/Cargo.toml",
    }
    missing = sorted(required - names)
    if missing:
        fail(f"{path.name}: missing required files: {missing}")

    forbidden = [
        name
        for name in names
        if "/target/" in name or "/.git/" in name or name.endswith((".redb", ".tgz", ".whl"))
    ]
    if forbidden:
        fail(f"{path.name}: forbidden payload files: {forbidden[:5]}")


def main() -> None:
    if len(sys.argv) != 3:
        fail("usage: aidememo_python_publish_check.py <dist-dir> <version>")

    dist = Path(sys.argv[1]).resolve()
    version = sys.argv[2]
    wheels = sorted(dist.glob("aidememo_python-*.whl"))
    sdists = sorted(dist.glob("aidememo_python-*.tar.gz"))

    if len(wheels) != 1:
        fail(f"expected exactly one aidememo_python wheel in {dist}, got {len(wheels)}")
    if len(sdists) != 1:
        fail(f"expected exactly one aidememo_python sdist in {dist}, got {len(sdists)}")

    check_wheel(wheels[0], version)
    check_sdist(sdists[0], version)
    print(
        "OK: aidememo-python publish payload "
        f"wheel={wheels[0].name} sdist={sdists[0].name} version={version}"
    )


if __name__ == "__main__":
    main()
