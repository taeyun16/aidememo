"""Hermes compatibility wrapper around the shared wg agent SDK."""

from __future__ import annotations

from pathlib import Path

from wg_agent.client import CLIENT_ERRORS, WgClient, WgUnavailable, parse_window_ms


HERMES_API_ERRORS: tuple[type[BaseException], ...] = (
    AttributeError,
    TypeError,
    ValueError,
    FileNotFoundError,
)


def default_skills_path() -> Path:
    """Where the bundled SKILL.md lives inside the installed Hermes wheel."""

    return Path(__file__).parent / "skills" / "wg"


__all__ = [
    "CLIENT_ERRORS",
    "HERMES_API_ERRORS",
    "WgClient",
    "WgUnavailable",
    "default_skills_path",
    "parse_window_ms",
]
