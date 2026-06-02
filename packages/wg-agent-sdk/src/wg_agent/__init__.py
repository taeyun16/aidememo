"""Code-first memory SDK for agents using wg."""

from .client import CLIENT_ERRORS, WgClient, WgUnavailable, parse_window_ms
from .sdk import WgMemorySDK

Memory = WgMemorySDK

__all__ = [
    "CLIENT_ERRORS",
    "Memory",
    "WgClient",
    "WgMemorySDK",
    "WgUnavailable",
    "parse_window_ms",
]
__version__ = "0.1.0"
