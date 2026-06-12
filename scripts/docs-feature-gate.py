#!/usr/bin/env python3
"""Validate that public docs cover AideMemo's runtime feature surface."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
FEATURES_DOC = ROOT / "docs" / "FEATURES.md"
MCP_TOOLS_RS = ROOT / "crates" / "aidememo-cli" / "src" / "cmd" / "mcp_tools.rs"
SIDEBAR_JS = ROOT / "website" / "sidebars.js"

FORBIDDEN_GLOBS = [
    "README.md",
    "AGENTS.md",
    "CLAUDE.md",
    "docs/*.md",
    "aidememo-skill/*.md",
    "aidememo-skill/hooks/*.py",
    "aidememo-skill/hooks/*.md",
    "packages/aidememo-agent-sdk/**/*.py",
    "packages/aidememo-agent-sdk/**/*.md",
    "plugins/hermes/**/*.py",
    "plugins/hermes/**/*.md",
    "plugins/hermes/pyproject.toml",
    "crates/aidememo-cli/src/**/*.rs",
]

FORBIDDEN_PATTERNS = [
    (re.compile(r"\ba aidememo\b"), "Use 'an AideMemo' or a backticked CLI/package name."),
    (re.compile(r"\bthe aidememo\b"), "Use 'AideMemo' or a precise noun such as 'AideMemo store'."),
    (re.compile(r"\bin the aidememo\b"), "Use 'in AideMemo' or a precise noun such as 'in the AideMemo store'."),
]


def resolve_binary(requested: str | None) -> Path:
    if requested:
        return Path(requested)
    subprocess.check_call(["cargo", "build", "-q", "-p", "aidememo-cli"], cwd=ROOT)
    return ROOT / "target" / "debug" / "aidememo"


def run_help(binary: Path, *args: str) -> str:
    if binary.exists():
        cmd = [str(binary), *args, "--help"]
    else:
        cmd = ["cargo", "run", "-q", "-p", "aidememo-cli", "--", *args, "--help"]
    return subprocess.check_output(cmd, cwd=ROOT, text=True, stderr=subprocess.STDOUT)


def parse_available_commands(help_text: str) -> list[str]:
    commands: list[str] = []
    in_commands = False
    for line in help_text.splitlines():
        if line.strip() == "Available commands:":
            in_commands = True
            continue
        if not in_commands:
            continue
        match = re.match(r"    ([a-z][a-z-]*)(?:,\s*[a-z])?\s{2,}", line)
        if match:
            commands.append(match.group(1))
    return commands


def parse_cli_surface(binary: Path) -> tuple[list[str], dict[str, list[str]]]:
    root_commands = parse_available_commands(run_help(binary))
    subcommands: dict[str, list[str]] = {}
    for command in root_commands:
        help_text = run_help(binary, command)
        nested = parse_available_commands(help_text)
        if nested:
            subcommands[command] = nested
    return root_commands, subcommands


def parse_mcp_tools() -> list[str]:
    source = MCP_TOOLS_RS.read_text(encoding="utf-8")
    tools = re.findall(r'name:\s+"(aidememo_[^"]+)"\.into\(\)', source)
    return sorted(dict.fromkeys(tools))


def public_files_for_wording() -> list[Path]:
    files: list[Path] = []
    for pattern in FORBIDDEN_GLOBS:
        files.extend(ROOT.glob(pattern))
    return sorted({p for p in files if p.is_file() and ".git" not in p.parts and "target" not in p.parts})


def check_feature_inventory(binary: Path) -> list[str]:
    errors: list[str] = []
    features = FEATURES_DOC.read_text(encoding="utf-8")

    cli_commands, cli_subcommands = parse_cli_surface(binary)
    if not cli_commands:
        errors.append("could not parse any CLI commands from aidememo --help")
    for command in cli_commands:
        token = f"`aidememo {command}`"
        if token not in features:
            errors.append(f"docs/FEATURES.md is missing CLI command {token}")
    for command, subcommands in cli_subcommands.items():
        for subcommand in subcommands:
            token = f"`aidememo {command} {subcommand}`"
            if token not in features:
                errors.append(f"docs/FEATURES.md is missing CLI subcommand {token}")

    mcp_tools = parse_mcp_tools()
    if not mcp_tools:
        errors.append("could not parse any MCP tools from cmd/mcp_tools.rs")
    for tool in mcp_tools:
        token = f"`{tool}`"
        if token not in features:
            errors.append(f"docs/FEATURES.md is missing MCP tool {token}")

    sidebar = SIDEBAR_JS.read_text(encoding="utf-8")
    if "FEATURES" not in sidebar:
        errors.append("website/sidebars.js does not expose docs/FEATURES.md")

    return errors


def check_stale_wording() -> list[str]:
    errors: list[str] = []
    for path in public_files_for_wording():
        text = path.read_text(encoding="utf-8", errors="ignore")
        rel = path.relative_to(ROOT)
        for pattern, hint in FORBIDDEN_PATTERNS:
            for match in pattern.finditer(text):
                line_no = text.count("\n", 0, match.start()) + 1
                snippet = text[match.start() : match.end()]
                errors.append(f"{rel}:{line_no}: stale wording '{snippet}'. {hint}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bin",
        default=os.environ.get("AIDEMEMO_BIN"),
        help="Path to the aidememo binary. Defaults to building target/debug/aidememo from current sources.",
    )
    args = parser.parse_args()

    binary = resolve_binary(args.bin)
    errors = []
    errors.extend(check_feature_inventory(binary))
    errors.extend(check_stale_wording())

    if errors:
        print("docs feature gate failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    cli_commands, cli_subcommands = parse_cli_surface(binary)
    cli_count = len(cli_commands)
    cli_subcommand_count = sum(len(items) for items in cli_subcommands.values())
    tool_count = len(parse_mcp_tools())
    print(
        "docs feature gate passed: "
        f"{cli_count} CLI commands, {cli_subcommand_count} CLI subcommands, {tool_count} MCP tools covered"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
