"""Code-first memory SDK for agents using aidememo."""

from .client import CLIENT_ERRORS, AideMemoClient, AideMemoUnavailable, parse_window_ms
from .sdk import AideMemoMemorySDK

Memory = AideMemoMemorySDK

__all__ = [
    "CLIENT_ERRORS",
    "Memory",
    "AideMemoClient",
    "AideMemoMemorySDK",
    "AideMemoUnavailable",
    "parse_window_ms",
]
__version__ = "0.1.0"
