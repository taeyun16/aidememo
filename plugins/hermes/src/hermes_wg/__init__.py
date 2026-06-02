"""Hermes Agent plugin for wg (Wiki-Graph).

Entry point: :func:`hermes_wg.plugin.register`.

Hermes calls ``register(ctx)`` once on plugin enable. We wire up tools,
slash commands, lifecycle hooks, and a ``hermes wg`` CLI subtree —
all backed by either the in-process ``wg-python`` binding (when
installed) or a subprocess to the ``wg`` CLI binary.
"""

from .client import WgClient
from .plugin import register
from .sdk import WgMemorySDK

__all__ = ["WgClient", "WgMemorySDK", "register"]
__version__ = "1.0.0"
