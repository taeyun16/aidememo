"""Plugin entry point.

Hermes calls :func:`register` once per plugin enable. We:

1. Build a :class:`AideMemoClient` (aidememo-python in-process if available, CLI
   subprocess fallback otherwise).
2. Register 12 native tools for context, retrieval, aggregation, writes, and health.
3. Register 8 slash commands for the same operator workflows.
4. Register 2 lifecycle hooks (``pre_llm_call`` auto-context,
   ``post_llm_call`` opt-in capture adapter).
5. Register the ``hermes aidememo`` CLI subtree.
6. Register the bundled aidememo skill so the agent gets free-form
   instructions on top of the structured tools.

Configuration is read from ``~/.hermes/config.yaml`` under
``plugins.aidememo``; absent values fall back to sensible defaults.
"""

from __future__ import annotations

import logging
import os
from pathlib import Path
from typing import Any

from . import cli, hooks, slash, tools
from .client import (
    HERMES_API_ERRORS,
    AideMemoClient,
    AideMemoUnavailable,
    default_skills_path,
)

log = logging.getLogger("hermes_aidememo")


def _load_config(ctx: Any) -> dict:
    """Pull the plugin config from Hermes if exposed; fall back to a
    direct read of ``$HERMES_HOME/config.yaml`` (honoring isolated
    test profiles) and finally ``~/.hermes/config.yaml`` for stock
    setups."""
    cfg_attr = getattr(ctx, "config", None)
    if isinstance(cfg_attr, dict):
        return cfg_attr.get("plugins", {}).get("aidememo", {}) or {}

    # `HERMES_HOME` is the canonical Hermes state-dir env var; it
    # points at /tmp/aidememo-hermes-test in our isolated profile and at
    # ~/.hermes in a stock setup. Read it before falling back to the
    # default location so auto_capture / dry_run / pending_log keys
    # written into a test profile actually take effect.
    home_env = os.environ.get("HERMES_HOME")
    if home_env:
        path = Path(home_env) / "config.yaml"
    else:
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
    return (doc.get("plugins") or {}).get("aidememo", {}) or {}


def register(ctx: Any) -> None:
    """The entry point Hermes invokes (see ``pyproject.toml`` →
    ``[project.entry-points."hermes.plugins"]``).

    No return value — registration mutates ``ctx`` in place.
    """
    config = _load_config(ctx)

    store_path = config.get("store_path") or os.environ.get("AIDEMEMO_STORE")
    source_id = config.get("source_id") or os.environ.get("AIDEMEMO_SOURCE_ID")
    actor_id = config.get("actor_id") or os.environ.get("AIDEMEMO_ACTOR_ID")
    try:
        lock_retry_ms = int(config.get("lock_retry_ms", 5000))
    except (TypeError, ValueError):
        log.warning("invalid aidememo lock_retry_ms=%r; using 5000", config.get("lock_retry_ms"))
        lock_retry_ms = 5000
    try:
        client = AideMemoClient(
            store_path=store_path,
            lock_retry_ms=lock_retry_ms,
            source_id=source_id,
            actor_id=actor_id,
        )
    except AideMemoUnavailable as exc:
        log.error("aidememo plugin disabled: %s", exc)
        return

    log.info(
        "hermes_aidememo: backend=%s store=%s source_id=%s actor_id=%s",
        client.backend,
        store_path or "<default>",
        source_id or "<none>",
        actor_id or "<none>",
    )

    tools.register_all(ctx, client)
    slash.register_all(ctx, client)
    hooks.register_all(ctx, client, config)
    cli.register(ctx, client)

    skill_dir = default_skills_path()
    if skill_dir.exists():
        try:
            ctx.register_skill(
                name="aidememo",
                path=skill_dir,
                description="AideMemo: local knowledge graph for persistent context across sessions.",
            )
        except HERMES_API_ERRORS as exc:
            log.warning("register_skill failed (non-fatal): %s", exc)
