"""Hermes compatibility wrapper around the shared aidememo agent SDK."""

from __future__ import annotations

from pathlib import Path

from aidememo_agent.client import CLIENT_ERRORS, AideMemoClient, AideMemoUnavailable, parse_window_ms


HERMES_API_ERRORS: tuple[type[BaseException], ...] = (
    AttributeError,
    TypeError,
    ValueError,
    FileNotFoundError,
)


def default_skills_path() -> Path:
    """Where the bundled SKILL.md lives inside the installed Hermes wheel."""

    return Path(__file__).parent / "skills" / "aidememo"


__all__ = [
    "CLIENT_ERRORS",
    "HERMES_API_ERRORS",
    "AideMemoClient",
    "AideMemoUnavailable",
    "default_skills_path",
    "parse_window_ms",
]
