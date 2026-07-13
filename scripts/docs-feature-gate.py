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
README = ROOT / "README.md"
README_KO = ROOT / "README.ko.md"
COMPARE = ROOT / "COMPARE.md"
FEATURES_DOC = ROOT / "docs" / "FEATURES.md"
ARCHITECTURE_DOC = ROOT / "docs" / "ARCHITECTURE.md"
AGENT_WORKFLOWS_DOC = ROOT / "docs" / "AGENT_WORKFLOWS.md"
CODING_AGENTS_DOC = ROOT / "docs" / "CODING_AGENTS.md"
CODEX_MULTI_PROFILE_DOC = ROOT / "docs" / "CODEX_MULTI_PROFILE.md"
INSTALLATION_DOC = ROOT / "docs" / "INSTALLATION.md"
QUICKSTART_DOC = ROOT / "docs" / "QUICKSTART.md"
EVIDENCE_DOC = ROOT / "docs" / "EVIDENCE.md"
MEASUREMENTS_DOC = ROOT / "docs" / "MEASUREMENTS.md"
LFM_EXPERIMENTS_DOC = ROOT / "docs" / "LFM_EXPERIMENTS.md"
RELEASE_DOC = ROOT / "docs" / "RELEASE.md"
SCRIPTS_README = ROOT / "scripts" / "README.md"
INSTALL_SCRIPT = ROOT / "scripts" / "install.sh"
CONTRIBUTING = ROOT / "CONTRIBUTING.md"
SECURITY = ROOT / "SECURITY.md"
PR_TEMPLATE = ROOT / ".github" / "PULL_REQUEST_TEMPLATE.md"
BUG_TEMPLATE = ROOT / ".github" / "ISSUE_TEMPLATE" / "bug_report.yml"
FEATURE_TEMPLATE = ROOT / ".github" / "ISSUE_TEMPLATE" / "feature_request.yml"
ISSUE_TEMPLATE_CONFIG = ROOT / ".github" / "ISSUE_TEMPLATE" / "config.yml"
MCP_TOOLS_RS = ROOT / "crates" / "aidememo-cli" / "src" / "cmd" / "mcp_tools.rs"
SIDEBAR_JS = ROOT / "website" / "sidebars.js"
DOCUSAURUS_CONFIG = ROOT / "website" / "docusaurus.config.js"
WEBSITE_PACKAGE = ROOT / "website" / "package.json"
HOMEPAGE_TSX = ROOT / "website" / "src" / "pages" / "index.tsx"
SEARCH_BAR_TSX = ROOT / "website" / "src" / "theme" / "SearchBar" / "index.tsx"
PAGEFIND_LOADER = ROOT / "website" / "static" / "pagefind-loader.js"
DOCS_I18N_STATUS = ROOT / "scripts" / "docs-i18n-status.py"
KO_CODE_JSON = ROOT / "website" / "i18n" / "ko" / "code.json"
KO_DOCS_JSON = ROOT / "website" / "i18n" / "ko" / "docusaurus-plugin-content-docs" / "current.json"
KO_NAVBAR_JSON = ROOT / "website" / "i18n" / "ko" / "docusaurus-theme-classic" / "navbar.json"
PAGES_WORKFLOW = ROOT / ".github" / "workflows" / "pages.yml"
ROBOTS_TXT = ROOT / "website" / "static" / "robots.txt"
SOCIAL_CARD = ROOT / "website" / "static" / "img" / "aidememo-social-card.png"
KO_ARCHITECTURE_DOC = (
    ROOT
    / "website"
    / "i18n"
    / "ko"
    / "docusaurus-plugin-content-docs"
    / "current"
    / "ARCHITECTURE.md"
)

REQUIRED_SIDEBAR_DOCS = [
    "INTRODUCTION",
    "ARCHITECTURE",
    "INSTALLATION",
    "QUICKSTART",
    "CLI",
    "MCP",
    "SHARED_MEMORY",
    "CODING_AGENTS",
    "CODEX_MULTI_PROFILE",
    "AGENT_WORKFLOWS",
    "SDK",
    "FEATURES",
    "OPERATIONS",
    "BRANCHES",
    "EVIDENCE",
    "MEASUREMENTS",
    "LFM_EXPERIMENTS",
    "RELEASE",
]

REQUIRED_HOMEPAGE_DOCS = [
    "INTRODUCTION",
    "QUICKSTART",
    "ARCHITECTURE",
    "MCP",
    "SHARED_MEMORY",
    "CODING_AGENTS",
    "CODEX_MULTI_PROFILE",
    "AGENT_WORKFLOWS",
    "EVIDENCE",
]

DOC_CONTENT_REQUIREMENTS = [
    (
        README,
        [
            "Local SLM Extensions (Opt In)",
            "mlx-community/LFM2.5-Embedding-350M-4bit",
            "mlx-community/LFM2.5-ColBERT-350M-4bit",
            "LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit",
            "fact_type_hint",
            "no external LLM API",
        ],
    ),
    (
        README_KO,
        [
            "로컬 SLM 확장(선택형)",
            "mlx-community/LFM2.5-Embedding-350M-4bit",
            "mlx-community/LFM2.5-ColBERT-350M-4bit",
            "LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit",
            "fact_type_hint",
            "외부 LLM API",
        ],
    ),
    (
        EVIDENCE_DOC,
        [
            "does not require an external LLM call",
            "search.auto_hybrid=true",
            "LFM is not the global default embedding replacement",
            "39/155 hints",
            "Measurement Ledger",
            "Release Checklist",
        ],
    ),
    (
        ARCHITECTURE_DOC,
        [
            "aidememo-core",
            "AideMemo API",
            "StoreKind",
            "SQLite default",
            "redb optional",
            "BM25",
            "semantic HNSW",
            "aidememo-cli",
            "aidememo mcp",
            "aidememo mcp-serve",
            "aidememo-agent-sdk",
            "aidememo-python",
            "aidememo-napi",
            "aidememo-nif",
            "aidememo-ffi",
            "mlx-community/LFM2.5-Embedding-350M-4bit",
            "mlx-community/LFM2.5-ColBERT-350M-4bit",
            "LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit",
            "OpenAI Privacy Filter MLX mxfp4",
            "model.provider=lfm-sidecar",
            "scripts/docs-feature-gate.py",
            "scripts/docs-i18n-status.py",
            "scripts/docs-site-e2e.py",
        ],
    ),
    (
        CODING_AGENTS_DOC,
        [
            "Claude Code",
            "CLAUDE_CONFIG_DIR",
            "claude plugin validate ./plugins/claude",
            "--target codex",
            "--codex-home",
            "Hermes Agent",
            "HERMES_HOME",
            "hermes-aidememo",
            "pi coding agent",
            "PI_CODING_AGENT_DIR",
            "pi does not accept MCP",
            "--target cursor",
            "--target openclaw",
            "--target opencode",
        ],
    ),
    (
        CODEX_MULTI_PROFILE_DOC,
        [
            "Two Codex accounts. One project memory.",
            "--codex-home",
            "--actor-id",
            "AIDEMEMO_ACTOR_ID",
            "parent_session_id",
            "continued_from",
            "BEGIN IMMEDIATE",
            "Hermes Agent session storage",
            "does not copy",
        ],
    ),
    (
        AGENT_WORKFLOWS_DOC,
        [
            "aidememo_workflow_start",
            "aidememo_context",
            "aidememo_query",
            "aidememo_search",
            "aidememo_aggregate",
            "aidememo_fact_add",
            "aidememo_fact_add_many",
            "aidememo_session_canvas",
            "aidememo_profile_export",
            "Memory.open",
            "search_rows",
            "coverage_by",
            "aggregate_many",
            "remember",
        ],
    ),
    (
        MEASUREMENTS_DOC,
        [
            "Workflow trigger Scenario F",
            "SDK workflow parity Scenario K",
            "Self-extraction Scenario L",
            "Branch Log Push / Merge",
            "docs-feature-gate",
            "docs-site-e2e",
            "cargo-package-readiness",
            "public-registry-smoke",
            "public-portability-check.py",
            ".github/workflows/fresh-checkout-smoke.yml",
        ],
    ),
    (
        LFM_EXPERIMENTS_DOC,
        [
            "not bundled with AideMemo",
            "not the global default",
            "aidememo config set model.provider lfm-sidecar",
            "scripts/lfm_mlx_docs_recall_eval.py",
            "scripts/lfm_fact_type_threshold_eval.py",
            "AIDEMEMO_FACT_TYPE_SHADOW_LOG",
        ],
    ),
    (
        RELEASE_DOC,
        [
            ".github/workflows/release-preflight.yml",
            "scripts/cargo-package-readiness.sh",
            "AIDEMEMO_CARGO_PACKAGE_CHECK_DEPENDENTS=1",
            "scripts/public-registry-smoke.sh",
            ".github/workflows/public-registry-smoke.yml",
            ".github/workflows/fresh-checkout-smoke.yml",
            "scripts/public-portability-check.py",
            "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE=verify",
            "v0.1.0",
            "aidememo-core",
        ],
    ),
]

MERMAID_CONTENT_REQUIREMENTS = [
    (
        README,
        [
            "default_path",
            "optional_models",
            "lfm_embed",
            "lfm_rerank",
            "lfm_type",
            "type_review",
            "privacy_model",
            "LFM2.5 Embedding 350M",
            "LFM2.5 ColBERT 350M",
            "LFM2.5 1.2B + LoRA",
        ],
    ),
    (
        README_KO,
        [
            "default_path",
            "optional_models",
            "lfm_embed",
            "lfm_rerank",
            "lfm_type",
            "type_review",
            "privacy_model",
            "LFM2.5 Embedding 350M",
            "LFM2.5 ColBERT 350M",
            "LFM2.5 1.2B + LoRA",
        ],
    ),
    (
        ARCHITECTURE_DOC,
        [
            "optional_models",
            "lfm_embed",
            "lfm_rerank",
            "lfm_type",
            "hint",
            "privacy_model",
            "LFM2.5 Embedding 350M 4-bit",
            "LFM2.5 ColBERT 350M 4-bit",
            "LFM2.5 1.2B Instruct + LoRA",
        ],
    ),
    (
        KO_ARCHITECTURE_DOC,
        [
            "optional_models",
            "lfm_embed",
            "lfm_rerank",
            "lfm_type",
            "hint",
            "privacy_model",
            "LFM2.5 Embedding 350M 4-bit",
            "LFM2.5 ColBERT 350M 4-bit",
            "LFM2.5 1.2B Instruct + LoRA",
        ],
    ),
]

FORBIDDEN_GLOBS = [
    "README.md",
    "README.ko.md",
    "COMPARE.md",
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
    "crates/*/README.md",
]

FORBIDDEN_PATTERNS = [
    (re.compile(r"\ba aidememo\b"), "Use 'an AideMemo' or a backticked CLI/package name."),
    (re.compile(r"\bthe aidememo\b"), "Use 'AideMemo' or a precise noun such as 'AideMemo store'."),
    (re.compile(r"\bin the aidememo\b"), "Use 'in AideMemo' or a precise noun such as 'in the AideMemo store'."),
    (
        re.compile(r"\bredb\s+by\s+default\b", re.IGNORECASE),
        "SQLite is the default backend; describe redb as an optional Cargo feature.",
    ),
    (
        re.compile(r"\bredb\s+default\s+(?:store|backend)\b", re.IGNORECASE),
        "SQLite is the default backend; describe redb as an optional Cargo feature.",
    ),
    (
        re.compile(r"\bexperimental\s+SQLite\b", re.IGNORECASE),
        "SQLite is the default backend; describe redb as the optional path.",
    ),
    (
        re.compile(r"\bSQLite\b.{0,40}\bbehind\s+(?:a\s+)?(?:feature|feature flag)\b", re.IGNORECASE),
        "SQLite is in default builds; only redb should be described as feature-gated.",
    ),
]

STORAGE_POSITIONING_REQUIREMENTS = [
    (
        ROOT / "README.md",
        [
            "SQLite default / redb optional",
            "Build with `--features redb` to opt into redb",
        ],
    ),
    (
        ROOT / "README.ko.md",
        [
            "SQLite가 기본 로컬 백엔드",
            "redb를 선택하려면 `--features redb`",
        ],
    ),
    (
        ROOT / "COMPARE.md",
        [
            "one embedded SQLite file by default",
            "optional redb Cargo feature",
        ],
    ),
    (
        ROOT / "AGENTS.md",
        [
            "SQLite by default, redb as an optional Cargo feature",
            "language bindings (full API; SQLite default, optional `redb` Cargo feature)",
        ],
    ),
    (
        ROOT / "docs" / "SDK.md",
        [
            "Default builds include SQLite",
            "Build with Cargo `redb` when you",
        ],
    ),
    (
        ROOT / "docs" / "OPERATIONS.md",
        [
            "SQLite is the default backend",
            "The optional redb backend",
        ],
    ),
]

COUNT_CLAIM_GLOBS = [
    "README.md",
    "README.ko.md",
    "COMPARE.md",
    "AGENTS.md",
    "docs/*.md",
    "scripts/README.md",
    "aidememo-skill/*.md",
    "aidememo-skill/hooks/*.md",
    "packages/*/README.md",
    "plugins/*/README.md",
    "crates/*/README.md",
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


def parse_agents_core_tools() -> list[str]:
    text = (ROOT / "AGENTS.md").read_text(encoding="utf-8")
    start = text.find("**Core (90% of agent traffic)**:")
    end = text.find("**Standard", start)
    if start == -1 or end == -1:
        return []
    section = text[start:end]
    return re.findall(r"\|\s+\*\*`(aidememo_[a-z_]+)`\*\*", section)


def public_files_for_wording() -> list[Path]:
    files: list[Path] = []
    for pattern in FORBIDDEN_GLOBS:
        files.extend(ROOT.glob(pattern))
    return sorted({p for p in files if p.is_file() and ".git" not in p.parts and "target" not in p.parts})


def public_files_for_count_claims() -> list[Path]:
    files: list[Path] = []
    for pattern in COUNT_CLAIM_GLOBS:
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


def check_count_claims(
    *,
    cli_count: int,
    cli_subcommand_count: int,
    mcp_tools: list[str],
    architecture_diagram_count: int,
) -> list[str]:
    errors: list[str] = []
    tool_count = len(mcp_tools)
    core_tools = parse_agents_core_tools()
    core_tool_count = len(core_tools)
    if not core_tools:
        errors.append("AGENTS.md core MCP tool table could not be parsed")
    for tool in core_tools:
        if tool not in mcp_tools:
            errors.append(f"AGENTS.md core MCP tool {tool!r} is not declared in cmd/mcp_tools.rs::list_tools()")

    claim_patterns = count_claim_patterns(
        cli_count=cli_count,
        cli_subcommand_count=cli_subcommand_count,
        tool_count=tool_count,
        architecture_diagram_count=architecture_diagram_count,
        core_tool_count=core_tool_count,
    )

    for path in public_files_for_count_claims():
        text = path.read_text(encoding="utf-8", errors="ignore")
        rel = str(path.relative_to(ROOT))
        errors.extend(check_count_claim_text(text, rel, claim_patterns))
    return errors


def count_claim_patterns(
    *,
    cli_count: int,
    cli_subcommand_count: int,
    tool_count: int,
    architecture_diagram_count: int,
    core_tool_count: int,
) -> list[tuple[re.Pattern[str], int, str]]:
    return [
        (re.compile(r"\b(?P<count>\d+)\s+MCP\s+tools?\b"), tool_count, "MCP tool count"),
        (re.compile(r"\b(?P<count>\d+)-tool dispatch\b"), tool_count, "MCP dispatch count"),
        (re.compile(r"\b(?P<count>\d+)\s+CLI commands?\b"), cli_count, "CLI command count"),
        (re.compile(r"\b(?P<count>\d+)\s+CLI subcommands?\b"), cli_subcommand_count, "CLI subcommand count"),
        (
            re.compile(r"\b(?P<count>\d+)\s+architecture diagrams?\b"),
            architecture_diagram_count,
            "architecture diagram count",
        ),
        (re.compile(r"Tool surface\s+.\s+(?P<count>\d+)\s+core\b"), core_tool_count, "core MCP tool count"),
        (re.compile(r"\blead with the core (?P<count>\d+)\b"), core_tool_count, "core MCP tool count"),
    ]


def check_count_claim_text(
    text: str,
    rel: str,
    claim_patterns: list[tuple[re.Pattern[str], int, str]],
) -> list[str]:
    errors: list[str] = []
    for pattern, expected, label in claim_patterns:
        for match in pattern.finditer(text):
            observed = int(match.group("count"))
            if observed != expected:
                line_no = text.count("\n", 0, match.start()) + 1
                snippet = text[match.start() : match.end()]
                errors.append(
                    f"{rel}:{line_no}: {label} claim {snippet!r} says {observed}, "
                    f"but implementation/docs table currently reports {expected}"
                )
    return errors


def check_count_claim_self_test() -> list[str]:
    errors: list[str] = []
    tool_count = len(parse_mcp_tools())
    core_tool_count = len(parse_agents_core_tools())
    cli_count = 42
    cli_subcommand_count = 61
    architecture_diagram_count = 4
    claim_patterns = count_claim_patterns(
        cli_count=cli_count,
        cli_subcommand_count=cli_subcommand_count,
        tool_count=tool_count,
        architecture_diagram_count=architecture_diagram_count,
        core_tool_count=core_tool_count,
    )
    good = "\n".join(
        [
            f"{tool_count} MCP tools",
            f"{tool_count}-tool dispatch",
            f"{cli_count} CLI commands",
            f"{cli_subcommand_count} CLI subcommands",
            f"{architecture_diagram_count} architecture diagrams",
            f"Tool surface — {core_tool_count} core",
            f"should lead with the core {core_tool_count}",
        ]
    )
    unexpected = check_count_claim_text(good, "count-claim-self-test-good.md", claim_patterns)
    if unexpected:
        errors.append("count claim self-test rejected matching claims: " + "; ".join(unexpected))

    bad = "\n".join(
        [
            f"{tool_count + 1} MCP tools",
            f"{tool_count + 1}-tool dispatch",
            f"{cli_count + 1} CLI commands",
            f"{cli_subcommand_count + 1} CLI subcommands",
            f"{architecture_diagram_count + 1} architecture diagrams",
            f"Tool surface — {core_tool_count + 1} core",
            f"should lead with the core {core_tool_count + 1}",
        ]
    )
    drift_errors = check_count_claim_text(bad, "count-claim-self-test-bad.md", claim_patterns)
    expected_labels = {
        "MCP tool count",
        "MCP dispatch count",
        "CLI command count",
        "CLI subcommand count",
        "architecture diagram count",
        "core MCP tool count",
    }
    found_labels = {label for label in expected_labels if any(label in error for error in drift_errors)}
    missing = sorted(expected_labels - found_labels)
    if missing:
        errors.append(
            "count claim self-test did not detect drift for: "
            + ", ".join(missing)
            + f"; errors={drift_errors!r}"
        )
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


def check_storage_positioning() -> list[str]:
    errors: list[str] = []
    for path, tokens in STORAGE_POSITIONING_REQUIREMENTS:
        text = path.read_text(encoding="utf-8")
        normalized_text = re.sub(r"\s+", " ", text)
        rel = path.relative_to(ROOT)
        for token in tokens:
            normalized_token = re.sub(r"\s+", " ", token)
            if normalized_token not in normalized_text:
                errors.append(
                    f"{rel}: missing storage positioning token {token!r}; "
                    "public docs must keep SQLite as default and redb as optional"
                )
    return errors


def require_tokens(path: Path, tokens: list[str], label: str) -> list[str]:
    errors: list[str] = []
    text = path.read_text(encoding="utf-8")
    normalized_text = re.sub(r"\s+", " ", text)
    rel = path.relative_to(ROOT)
    for token in tokens:
        normalized_token = re.sub(r"\s+", " ", token)
        if normalized_token not in normalized_text:
            errors.append(f"{rel}: missing {label} token {token!r}")
    return errors


def check_onboarding_contract(binary: Path) -> list[str]:
    errors: list[str] = []
    errors.extend(
        require_tokens(
            INSTALL_SCRIPT,
            ["cargo install --git", "--bin \"$BIN_NAME\" aidememo-cli"],
            "installer",
        )
    )
    errors.extend(
        require_tokens(
            README,
            [
                "curl -fsSL https://raw.githubusercontent.com/taeyun16/aidememo/main/scripts/install.sh | bash",
                "cargo install --path crates/aidememo-cli",
                '<a href="./README.ko.md">한국어</a>',
                "docs/CODING_AGENTS.md",
                "skill install --target pi",
            ],
            "public install",
        )
    )
    errors.extend(
        require_tokens(
            README_KO,
            [
                "curl -fsSL https://raw.githubusercontent.com/taeyun16/aidememo/main/scripts/install.sh | bash",
                "cargo install --path crates/aidememo-cli",
                '<a href="./README.md">English</a>',
                "docs/CODING_AGENTS.md",
                "skill install --target pi",
            ],
            "Korean public install",
        )
    )
    errors.extend(
        require_tokens(
            INSTALLATION_DOC,
            [
                "From a checkout",
                "scripts/fresh-checkout-smoke.sh",
                "Coding Agent Setup",
                "aidememo mcp-install --list-targets",
            ],
            "checkout install",
        )
    )
    errors.extend(
        require_tokens(
            QUICKSTART_DOC,
            ['am query "Fix Redis timeout in worker" --bm25-only'],
            "deterministic quickstart",
        )
    )
    errors.extend(
        require_tokens(
            SCRIPTS_README,
            ["fresh-checkout-smoke.sh", "cargo-package-readiness.sh", "public-registry-smoke.sh"],
            "script inventory",
        )
    )
    query_help = run_help(binary, "query")
    if "--bm25-only" not in query_help:
        errors.append("aidememo query --help is missing --bm25-only; quickstart deterministic query would drift")
    return errors


def check_script_inventory() -> list[str]:
    errors: list[str] = []
    inventory = SCRIPTS_README.read_text(encoding="utf-8")
    names = sorted(set(re.findall(r"`([^`/\s]+\.(?:py|sh|mjs))`", inventory)))
    scripts_dir = ROOT / "scripts"
    for name in names:
        if "*" in name:
            if not any(scripts_dir.glob(name)):
                errors.append(f"scripts/README.md inventory pattern has no matches: {name}")
        elif not (scripts_dir / name).is_file():
            errors.append(f"scripts/README.md inventories missing script: scripts/{name}")
    return errors


def check_community_contract() -> list[str]:
    errors: list[str] = []
    errors.extend(
        require_tokens(
            CONTRIBUTING,
            ["Quality gates", "Filing issues", "No AI attribution in commit messages."],
            "contributor guide",
        )
    )
    errors.extend(
        require_tokens(
            SECURITY,
            ["Reporting a Vulnerability", "taeyun16@pm.me", "Do not open a public GitHub issue"],
            "security policy",
        )
    )
    errors.extend(
        require_tokens(
            PR_TEMPLATE,
            ["Summary", "Validation", "registry-release implications"],
            "PR template",
        )
    )
    errors.extend(
        require_tokens(
            BUG_TEMPLATE,
            ["Version or commit", "Package version, Git commit SHA, or install source.", "Redact secrets."],
            "bug template",
        )
    )
    errors.extend(
        require_tokens(
            FEATURE_TEMPLATE,
            ["Use case", "Proposed interface", "Alternatives considered"],
            "feature request template",
        )
    )
    errors.extend(
        require_tokens(
            ISSUE_TEMPLATE_CONFIG,
            ["blank_issues_enabled: false", "security/advisories/new"],
            "issue template config",
        )
    )
    bug_text = BUG_TEMPLATE.read_text(encoding="utf-8")
    if "aidememo --version" in bug_text:
        errors.append(".github/ISSUE_TEMPLATE/bug_report.yml asks for `aidememo --version`, but the CLI does not expose that flag")
    return errors


def contains_doc_id(text: str, doc_id: str) -> bool:
    return (
        f"'{doc_id}'" in text
        or f'"{doc_id}"' in text
        or f"/docs/{doc_id}" in text
        or f"docs/{doc_id}.md" in text
    )


def mermaid_diagram_count(text: str) -> int:
    return len(re.findall(r"```mermaid\b", text))


def mermaid_blocks(text: str) -> list[str]:
    return re.findall(r"```mermaid\s*\n(.*?)```", text, flags=re.DOTALL)


def check_mermaid_content_requirements(path: Path, tokens: list[str]) -> list[str]:
    if not path.exists():
        return [f"{path.relative_to(ROOT)} is missing"]
    blocks = mermaid_blocks(path.read_text(encoding="utf-8"))
    rel = path.relative_to(ROOT)
    if not blocks:
        return [f"{rel}: expected a Mermaid diagram for the local SLM architecture contract"]
    diagram_text = "\n".join(blocks)
    return [
        f"{rel}: Mermaid diagrams are missing local SLM architecture token {token!r}"
        for token in tokens
        if token not in diagram_text
    ]


def check_mermaid_content_self_test() -> list[str]:
    errors: list[str] = []
    if check_mermaid_content_requirements(README, ["lfm_embed"]):
        errors.append("Mermaid content self-test rejected a required README SLM node")
    if check_mermaid_content_requirements(README_KO, ["lfm_embed"]):
        errors.append("Mermaid content self-test rejected a required Korean README SLM node")
    missing = check_mermaid_content_requirements(README, ["missing_slm_contract_node"])
    if not any("missing local SLM architecture token" in error for error in missing):
        errors.append("Mermaid content self-test did not reject a missing SLM node")
    return errors


def check_mermaid_static_lint(path: Path, text: str) -> list[str]:
    errors: list[str] = []
    reserved_node_ids = {
        "class",
        "click",
        "default",
        "end",
        "flowchart",
        "graph",
        "linkstyle",
        "sequenceDiagram",
        "stateDiagram",
        "style",
        "subgraph",
    }
    rel = path.relative_to(ROOT)
    for block_index, block in enumerate(mermaid_blocks(text), start=1):
        if not block.strip():
            errors.append(f"{rel}: Mermaid block {block_index} is empty")
            continue
        for line_no, line in enumerate(block.splitlines(), start=1):
            stripped = line.strip()
            match = re.match(r"([A-Za-z][A-Za-z0-9_]*)\s*[\[\(\{]", stripped)
            if match and match.group(1) in reserved_node_ids:
                errors.append(
                    f"{rel}: Mermaid block {block_index} line {line_no} uses reserved node id "
                    f"{match.group(1)!r}; choose a more specific id"
                )
    return errors


def check_docusaurus_contract() -> list[str]:
    errors: list[str] = []
    sidebar = SIDEBAR_JS.read_text(encoding="utf-8")
    config = DOCUSAURUS_CONFIG.read_text(encoding="utf-8")
    package = WEBSITE_PACKAGE.read_text(encoding="utf-8")
    homepage = HOMEPAGE_TSX.read_text(encoding="utf-8")
    normalized_homepage = re.sub(r"\s+", " ", homepage)

    for doc_id in REQUIRED_SIDEBAR_DOCS:
        if not contains_doc_id(sidebar, doc_id):
            errors.append(f"website/sidebars.js does not expose docs/{doc_id}.md")

    for doc_id in REQUIRED_HOMEPAGE_DOCS:
        if f"/docs/{doc_id}" not in homepage:
            errors.append(f"website/src/pages/index.tsx does not link to /docs/{doc_id}")

    if "mermaid: true" not in config:
        errors.append("website/docusaurus.config.js must enable markdown.mermaid so diagrams render")
    if "@docusaurus/theme-mermaid" not in config:
        errors.append("website/docusaurus.config.js must include @docusaurus/theme-mermaid")
    if '"@docusaurus/theme-mermaid"' not in package:
        errors.append("website/package.json must depend on @docusaurus/theme-mermaid")
    if '"docusaurus-pagefind-search": "0.2.2"' not in package:
        errors.append("website/package.json must pin the Pagefind documentation search plugin")
    if not re.search(r"locales:\s*\[\s*['\"]en['\"]\s*,\s*['\"]ko['\"]\s*\]", config):
        errors.append("website/docusaurus.config.js must build the en and ko locales")
    for token in ("htmlLang: 'en-US'", "htmlLang: 'ko-KR'", "type: 'localeDropdown'"):
        if token not in config:
            errors.append(f"website/docusaurus.config.js is missing i18n contract token {token!r}")
    for token in ('"start:ko"', '"write-translations:ko"'):
        if token not in package:
            errors.append(f"website/package.json is missing i18n script {token}")
    if "onBrokenLinks: 'throw'" not in config and 'onBrokenLinks: "throw"' not in config:
        errors.append("website/docusaurus.config.js must fail on broken links")
    if "onBrokenMarkdownLinks: 'throw'" not in config and 'onBrokenMarkdownLinks: "throw"' not in config:
        errors.append("website/docusaurus.config.js must fail on broken Markdown links")
    for token in (
        "favicon: 'img/aidememo-logo.png'",
        "image: 'img/aidememo-social-card.png'",
        "'docusaurus-pagefind-search'",
        "summary_large_image",
    ):
        if token not in config:
            errors.append(f"website/docusaurus.config.js is missing discovery token {token!r}")
    if "does not require an external LLM call" not in normalized_homepage:
        errors.append("website/src/pages/index.tsx must state the default local no-external-LLM boundary")
    for token in ("homepage.hero.title", "homepage.hero.subtitle", "homepage.card.evidence.title"):
        if token not in homepage:
            errors.append(f"website/src/pages/index.tsx is missing translation id {token!r}")

    i18n_requirements = [
        (
            KO_CODE_JSON,
            [
                '"homepage.hero.title"',
                "코딩 에이전트에 친화적인 SDK 메모리.",
                "기본 로컬 메모리 루프는 외부 LLM 호출이 필요하지 않습니다.",
                '"search.placeholder"',
                "문서 검색",
            ],
            "Korean homepage",
        ),
        (KO_DOCS_JSON, ["시작하기", "AideMemo 사용하기"], "Korean docs navigation"),
        (KO_NAVBAR_JSON, ['"message": "문서"'], "Korean navbar"),
    ]
    for path, tokens, label in i18n_requirements:
        if not path.exists():
            errors.append(f"{path.relative_to(ROOT)} is missing")
            continue
        errors.extend(require_tokens(path, tokens, label))

    if not SOCIAL_CARD.exists() or SOCIAL_CARD.read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
        errors.append("website/static/img/aidememo-social-card.png must be a PNG social card")
    if not SEARCH_BAR_TSX.exists():
        errors.append("website/src/theme/SearchBar/index.tsx is missing")
    else:
        search_bar = SEARCH_BAR_TSX.read_text(encoding="utf-8")
        for token in (
            "pagefind-loader.js",
            "pagefind/pagefind.js",
            "search.placeholder",
            'role="combobox"',
        ):
            if token not in search_bar:
                errors.append(f"website/src/theme/SearchBar/index.tsx is missing search token {token!r}")
    if not PAGEFIND_LOADER.exists() or "aidememoLoadPagefind" not in PAGEFIND_LOADER.read_text(
        encoding="utf-8"
    ):
        errors.append("website/static/pagefind-loader.js must expose the browser-only Pagefind loader")
    if not ROBOTS_TXT.exists() or "https://aidememo.taeyun.me/sitemap.xml" not in ROBOTS_TXT.read_text(
        encoding="utf-8"
    ):
        errors.append("website/static/robots.txt must advertise the deployed sitemap")
    if not PAGES_WORKFLOW.exists():
        errors.append(".github/workflows/pages.yml is missing")
    else:
        pages_workflow = PAGES_WORKFLOW.read_text(encoding="utf-8")
        for token in (
            "actions/configure-pages@",
            "actions/upload-pages-artifact@",
            "actions/deploy-pages@",
            "pages: write",
            "id-token: write",
            "name: github-pages",
            "path: website/build",
        ):
            if token not in pages_workflow:
                errors.append(f".github/workflows/pages.yml is missing deployment token {token!r}")

    if not DOCS_I18N_STATUS.exists():
        errors.append("scripts/docs-i18n-status.py is missing")
    else:
        result = subprocess.run(
            [sys.executable, str(DOCS_I18N_STATUS), "check"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip()
            errors.append(f"Korean translation status failed: {detail}")

    for path, tokens in DOC_CONTENT_REQUIREMENTS:
        if not path.exists():
            errors.append(f"{path.relative_to(ROOT)} is missing")
            continue
        text = path.read_text(encoding="utf-8")
        errors.extend(check_mermaid_static_lint(path, text))
        normalized_text = re.sub(r"\s+", " ", text)
        rel = path.relative_to(ROOT)
        for token in tokens:
            normalized_token = re.sub(r"\s+", " ", token)
            if normalized_token not in normalized_text:
                errors.append(f"{rel}: missing implementation-doc contract token {token!r}")

    for path, tokens in MERMAID_CONTENT_REQUIREMENTS:
        errors.extend(check_mermaid_content_requirements(path, tokens))
    if KO_ARCHITECTURE_DOC.exists():
        errors.extend(
            check_mermaid_static_lint(
                KO_ARCHITECTURE_DOC,
                KO_ARCHITECTURE_DOC.read_text(encoding="utf-8"),
            )
        )

    if ARCHITECTURE_DOC.exists():
        architecture = ARCHITECTURE_DOC.read_text(encoding="utf-8")
        diagram_count = mermaid_diagram_count(architecture)
        if diagram_count < 3:
            errors.append(
                f"docs/ARCHITECTURE.md must include at least 3 Mermaid diagrams; found {diagram_count}"
            )
        if "flowchart" not in architecture or "sequenceDiagram" not in architecture:
            errors.append("docs/ARCHITECTURE.md must include both flowchart and sequenceDiagram Mermaid views")

    if AGENT_WORKFLOWS_DOC.exists():
        workflows = AGENT_WORKFLOWS_DOC.read_text(encoding="utf-8")
        if mermaid_diagram_count(workflows) < 1:
            errors.append("docs/AGENT_WORKFLOWS.md must include a Mermaid decision flow")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bin",
        default=os.environ.get("AIDEMEMO_BIN"),
        help="Path to the aidememo binary. Defaults to building target/debug/aidememo from current sources.",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run the docs drift detector self-tests without building the aidememo binary.",
    )
    args = parser.parse_args()

    if args.self_test:
        errors = [*check_count_claim_self_test(), *check_mermaid_content_self_test()]
        if errors:
            print("docs feature gate self-test failed:", file=sys.stderr)
            for error in errors:
                print(f"- {error}", file=sys.stderr)
            return 1
        print("docs feature gate self-test passed")
        return 0

    binary = resolve_binary(args.bin)
    cli_commands, cli_subcommands = parse_cli_surface(binary)
    cli_count = len(cli_commands)
    cli_subcommand_count = sum(len(items) for items in cli_subcommands.values())
    mcp_tools = parse_mcp_tools()
    diagram_count = mermaid_diagram_count(ARCHITECTURE_DOC.read_text(encoding="utf-8"))

    errors = []
    errors.extend(check_count_claim_self_test())
    errors.extend(check_mermaid_content_self_test())
    errors.extend(check_feature_inventory(binary))
    errors.extend(
        check_count_claims(
            cli_count=cli_count,
            cli_subcommand_count=cli_subcommand_count,
            mcp_tools=mcp_tools,
            architecture_diagram_count=diagram_count,
        )
    )
    errors.extend(check_docusaurus_contract())
    errors.extend(check_stale_wording())
    errors.extend(check_storage_positioning())
    errors.extend(check_onboarding_contract(binary))
    errors.extend(check_script_inventory())
    errors.extend(check_community_contract())

    if errors:
        print("docs feature gate failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    tool_count = len(mcp_tools)
    print(
        "docs feature gate passed: "
        f"{cli_count} CLI commands, {cli_subcommand_count} CLI subcommands, "
        f"{tool_count} MCP tools covered, {diagram_count} architecture diagrams"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
