#!/usr/bin/env python3
"""Reject developer-specific home paths from first-party public files."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SKIP_PREFIXES = (
    "crates/aidememo-python/vendor/",
)
FORBIDDEN = (
    ("macOS user home", re.compile("/" + "Users" + r"/[^/\s'\"`]+/")),
    ("Windows user home", re.compile(r"[A-Za-z]:\\Users\\[^\\\s'\"`]+\\")),
    (
        "Linux developer home",
        re.compile(r"/home/[^/\s'\"`]+/(?:dev|src|projects|\.local|\.config|\.cache)/"),
    ),
)


def validate_rules() -> list[str]:
    slash = "\\"
    samples = {
        "macOS user home": "/" + "Users" + "/alice/dev/aidememo",
        "Windows user home": f"C:{slash}Users{slash}alice{slash}dev{slash}aidememo",
        "Linux developer home": "/" + "home" + "/alice/dev/aidememo",
    }
    failures: list[str] = []
    for label, pattern in FORBIDDEN:
        if not pattern.search(samples[label]):
            failures.append(f"internal rule did not match {label}")
    return failures


def tracked_paths() -> list[Path]:
    proc = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=ROOT,
        capture_output=True,
        check=False,
    )
    if proc.returncode != 0:
        print(proc.stderr.decode("utf-8", errors="replace"), file=sys.stderr)
        raise SystemExit("git ls-files failed")
    return [ROOT / item.decode() for item in proc.stdout.split(b"\0") if item]


def main() -> int:
    failures = validate_rules()
    checked = 0
    for path in tracked_paths():
        rel = path.relative_to(ROOT).as_posix()
        if rel.startswith(SKIP_PREFIXES) or not path.is_file():
            continue
        raw = path.read_bytes()
        if b"\0" in raw:
            continue
        checked += 1
        text = raw.decode("utf-8", errors="replace")
        for lineno, line in enumerate(text.splitlines(), start=1):
            for label, pattern in FORBIDDEN:
                match = pattern.search(line)
                if match:
                    failures.append(f"{rel}:{lineno}: {label}: {match.group(0)}")

    if failures:
        print("public portability check failed:", file=sys.stderr)
        for item in failures:
            print(f"- {item}", file=sys.stderr)
        return 1
    print(f"public portability check passed: {checked} first-party text files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
