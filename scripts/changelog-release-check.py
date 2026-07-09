#!/usr/bin/env python3
"""Validate that CHANGELOG.md is cut for the current release version."""

from __future__ import annotations

import argparse
import datetime as dt
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHANGELOG = ROOT / "CHANGELOG.md"
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$")
SECTION_RE = re.compile(r"^## \[([^\]]+)\](.*)$", flags=re.MULTILINE)


@dataclass(frozen=True)
class Section:
    name: str
    suffix: str
    heading: str
    body: str


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def load_toml(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def workspace_version() -> str:
    return load_toml(ROOT / "Cargo.toml")["workspace"]["package"]["version"]


def fail(failures: list[str], label: str, detail: str) -> None:
    failures.append(f"{label}: {detail}")


def ok(rows: list[str], label: str) -> None:
    rows.append(f"ok: {label}")


def parse_sections(text: str) -> list[Section]:
    matches = list(SECTION_RE.finditer(text))
    sections: list[Section] = []
    for index, match in enumerate(matches):
        body_start = match.end()
        body_end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        sections.append(
            Section(
                name=match.group(1),
                suffix=match.group(2),
                heading=match.group(0),
                body=text[body_start:body_end],
            )
        )
    return sections


def validate_changelog(version: str) -> tuple[list[str], list[str], str | None]:
    failures: list[str] = []
    rows: list[str] = []
    release_date: str | None = None

    if not SEMVER_RE.fullmatch(version):
        fail(failures, "version", f"{version!r} is not semver")
        return rows, failures, release_date

    text = read(CHANGELOG)
    if not text.startswith("# Changelog\n"):
        fail(failures, "CHANGELOG.md title", "must start with '# Changelog'")
    else:
        ok(rows, "CHANGELOG.md title")

    sections = parse_sections(text)
    if not sections:
        fail(failures, "CHANGELOG.md sections", "no '## [name]' sections found")
        return rows, failures, release_date

    unreleased = [section for section in sections if section.name == "Unreleased"]
    if len(unreleased) != 1:
        fail(failures, "Unreleased section", f"expected exactly one, found {len(unreleased)}")
    elif sections[0].name != "Unreleased":
        fail(failures, "Unreleased section", "must be the first release section")
    elif unreleased[0].suffix.strip():
        fail(failures, "Unreleased section", f"unexpected suffix {unreleased[0].suffix!r}")
    elif unreleased[0].body.strip():
        fail(
            failures,
            "Unreleased section",
            "must be empty after cutting the current release notes",
        )
    else:
        ok(rows, "Unreleased section is empty")

    version_sections = [section for section in sections if section.name == version]
    if len(version_sections) != 1:
        fail(failures, "release section", f"expected one [{version}] section, found {len(version_sections)}")
        return rows, failures, release_date

    release = version_sections[0]
    if len(sections) < 2 or sections[1].name != version:
        fail(failures, "release section order", f"[{version}] must immediately follow [Unreleased]")
    else:
        ok(rows, f"[{version}] follows [Unreleased]")

    heading_match = re.fullmatch(rf"## \[{re.escape(version)}\] - (\d{{4}}-\d{{2}}-\d{{2}})", release.heading)
    if not heading_match:
        fail(
            failures,
            "release heading",
            f"expected '## [{version}] - YYYY-MM-DD', got {release.heading!r}",
        )
    else:
        release_date = heading_match.group(1)
        try:
            parsed = dt.date.fromisoformat(release_date)
        except ValueError:
            fail(failures, "release date", f"{release_date!r} is not a valid ISO date")
        else:
            if parsed > dt.date.today():
                fail(failures, "release date", f"{release_date} is in the future")
            else:
                ok(rows, f"[{version}] release date")

    if not re.search(r"^###\s+\S", release.body, flags=re.MULTILINE):
        fail(failures, "release content", f"[{version}] must include at least one '###' category")
    elif not re.search(r"^\s*-\s+\S", release.body, flags=re.MULTILINE):
        fail(failures, "release content", f"[{version}] must include at least one bullet")
    else:
        ok(rows, f"[{version}] release content")

    return rows, failures, release_date


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "version",
        nargs="?",
        default=workspace_version(),
        help="release version to validate; defaults to Cargo workspace version",
    )
    args = parser.parse_args()

    rows, failures, release_date = validate_changelog(args.version)

    print("changelog release check")
    for row in rows:
        print(row)
    if failures:
        print("\nfailures:", file=sys.stderr)
        for item in failures:
            print(f"- {item}", file=sys.stderr)
        return 1

    print(
        "OK: changelog release check passed "
        f"(version={args.version}, date={release_date})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
