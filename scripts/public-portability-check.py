#!/usr/bin/env python3
"""Reject public-repository portability and trust-boundary regressions."""

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
COMMUNITY_FILES = (
    "CODE_OF_CONDUCT.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "SUPPORT.md",
    ".github/CODEOWNERS",
    ".github/dependabot.yml",
    ".github/ISSUE_TEMPLATE/bug_report.yml",
    ".github/ISSUE_TEMPLATE/feature_request.yml",
)
OIDC_WORKFLOWS = (
    "aidememo-agent-sdk-publish.yml",
    "aidememo-napi-publish.yml",
    "aidememo-python-publish.yml",
    "crates-publish.yml",
    "hermes-aidememo-publish.yml",
    "pages.yml",
)
ACTION_REF_RE = re.compile(r"uses:\s+[^\s@]+@([^\s#]+)")
COMMIT_SHA_RE = re.compile(r"[0-9a-f]{40}")


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


def validate_repository_contracts() -> list[str]:
    failures: list[str] = []
    for rel in COMMUNITY_FILES:
        if not (ROOT / rel).is_file():
            failures.append(f"missing public community file: {rel}")

    workflows_dir = ROOT / ".github" / "workflows"
    workflow_texts = {
        path.name: path.read_text(encoding="utf-8")
        for path in sorted(workflows_dir.glob("*.yml"))
    }
    for name, text in workflow_texts.items():
        if "pull_request_target:" in text:
            failures.append(f"{name}: pull_request_target is not allowed")

    self_hosted = workflow_texts.get("e2e-natural-prompt.yml", "")
    for required in (
        "github.event.pull_request.head.repo.full_name == github.repository",
        "github.ref == 'refs/heads/main'",
        "persist-credentials: false",
    ):
        if required not in self_hosted:
            failures.append(f"e2e-natural-prompt.yml missing trust guard: {required}")

    npm_workflow = workflow_texts.get("aidememo-napi-publish.yml", "")
    npm_script = (ROOT / "scripts" / "aidememo-napi-publish.sh").read_text(
        encoding="utf-8"
    )
    for forbidden in (
        "NPM_TOKEN",
        "NODE_AUTH_TOKEN",
        "AIDEMEMO_NAPI_BOOTSTRAP",
        "inputs.bootstrap",
    ):
        if forbidden in npm_workflow or forbidden in npm_script:
            failures.append(f"npm OIDC path still contains bootstrap token marker: {forbidden}")

    for name in OIDC_WORKFLOWS:
        text = workflow_texts.get(name)
        if text is None:
            failures.append(f"missing OIDC workflow: {name}")
            continue
        for lineno, line in enumerate(text.splitlines(), start=1):
            match = ACTION_REF_RE.search(line)
            if match and not COMMIT_SHA_RE.fullmatch(match.group(1)):
                failures.append(
                    f"{name}:{lineno}: OIDC workflow action is not pinned to a commit SHA"
                )
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
    failures.extend(validate_repository_contracts())
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
