"""Plugin entry point.

Hermes calls :func:`register` once per plugin enable. We:

1. Build a :class:`WgClient` (wg-python in-process if available, CLI
   subprocess fallback otherwise).
2. Register 7 tools (the same surface the wg MCP server exposes).
3. Register 3 slash commands (``/wg``, ``/wg-add``, ``/wg-recent``).
4. Register 2 lifecycle hooks (``on_session_start`` auto-context,
   ``on_session_end`` auto-fact-record).
5. Register the ``hermes wg`` CLI subtree.
6. Register the bundled wg skill so the agent gets free-form
   instructions on top of the structured tools.

Configuration is read from ``~/.hermes/config.yaml`` under
``plugins.wg``; absent values fall back to sensible defaults.
"""

from __future__ import annotations

import logging
import os
from pathlib import Path
from typing import Any

from . import cli, hooks, slash, tools
from .client import (
    HERMES_API_ERRORS,
    WgClient,
    WgUnavailable,
    default_skills_path,
)

log = logging.getLogger("hermes_wg")


def _load_config(ctx: Any) -> dict:
    """Pull the plugin config from Hermes if exposed; fall back to a
    direct read of ``~/.hermes/config.yaml`` otherwise."""
    cfg_attr = getattr(ctx, "config", None)
    if isinstance(cfg_attr, dict):
        return cfg_attr.get("plugins", {}).get("wg", {}) or {}

    home = Path(os.environ.get("HOME") or os.path.expanduser("~"))
    path = home / ".hermes" / "config.yaml"
    if not path.exists():
        return {}
    try:
        import yaml  # type: ignore[import-untyped]
    except ImportError:
        return {}
    try:
        with path.open(encoding="utf-8") as fh:
            doc = yaml.safe_load(fh) or {}
    except (OSError, yaml.YAMLError) as exc:
        log.warning("could not read %s: %s", path, exc)
        return {}
    if not isinstance(doc, dict):
        return {}
    return (doc.get("plugins") or {}).get("wg", {}) or {}


def register(ctx: Any) -> None:
    """The entry point Hermes invokes (see ``pyproject.toml`` →
    ``[project.entry-points."hermes.plugins"]``).

    No return value — registration mutates ``ctx`` in place.
    """
    config = _load_config(ctx)

    store_path = config.get("store_path") or os.environ.get("WG_STORE")
    try:
        client = WgClient(store_path=store_path)
    except WgUnavailable as exc:
        log.error("wg plugin disabled: %s", exc)
        return

    log.info("hermes_wg: backend=%s store=%s", client.backend, store_path or "<default>")

    tools.register_all(ctx, client)
    slash.register_all(ctx, client)
    hooks.register_all(ctx, client, config)
    cli.register(ctx, client)

    skill_dir = default_skills_path()
    if skill_dir.exists():
        try:
            ctx.register_skill(
                name="wg",
                path=skill_dir,
                description="Wiki-Graph: local knowledge graph for persistent context across sessions.",
            )
        except HERMES_API_ERRORS as exc:
            log.warning("register_skill failed (non-fatal): %s", exc)
