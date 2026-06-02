from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
for path in [
    ROOT / "packages" / "aidememo-agent-sdk" / "src",
    ROOT / "plugins" / "hermes" / "src",
]:
    text = str(path)
    if text not in sys.path:
        sys.path.insert(0, text)
