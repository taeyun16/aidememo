"""Hermes Agent plugin for aidememo (AideMemo).

Entry point: :func:`hermes_aidememo.plugin.register`.

Hermes calls ``register(ctx)`` once on plugin enable. We wire up tools,
slash commands, lifecycle hooks, and a ``hermes aidememo`` CLI subtree —
all backed by either the in-process ``aidememo-python`` binding (when
installed) or a subprocess to the ``aidememo`` CLI binary.
"""

from .client import AideMemoClient
from .plugin import register
from .sdk import Memory, AideMemoMemorySDK

__all__ = ["Memory", "AideMemoClient", "AideMemoMemorySDK", "register"]
__version__ = "0.1.0"
