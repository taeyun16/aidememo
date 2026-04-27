"""Heuristic detector for "we just decided X" moments in chat.

Used by the ``on_session_end`` hook to auto-record facts back to wg.
Bias is towards *precision* — false positives clutter the wiki and
erode trust in the auto-recorder, so we only catch unambiguous
phrasings. Operators who want recall over precision can lower the
threshold via ``confidence_floor`` in the plugin config.

Patterns are anchored to short imperative-style claims (≤ 200 chars)
because longer paragraphs are usually exposition, not commitments.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

# Common chat-transcript role prefixes we strip off before pattern
# matching. Without this, "Assistant: Decision: …" wouldn't fire
# because the marker is no longer at the start of the line.
_ROLE_PREFIX = re.compile(
    r"^\s*(?:user|assistant|system|tool|hermes|claude|gpt|model|호출자|어시스턴트)\s*[:：]\s*",
    re.IGNORECASE,
)

# Each pattern carries a confidence weight and a fact_type to suggest.
# Heuristics here are deliberately *paranoid* — we'd rather miss a
# decision than drop noise into the wiki. Patterns use word
# boundaries (`\b`) rather than `^` so role prefixes don't block them
# even if `_ROLE_PREFIX` misses an exotic format.
_PATTERNS: list[tuple[re.Pattern[str], float, str]] = [
    # "Decision: …" / "[decision] …" — explicit markers, very high.
    (re.compile(r"\b(?:decision|결정|결론)\s*[:：]\s*(.+)", re.IGNORECASE), 0.95, "decision"),
    # "[decided] …" / "✅ decided …"
    (re.compile(r"(?:✅|☑️|☑)?\s*\[?decided\]?\s*[:：-]\s*(.+)", re.IGNORECASE), 0.9, "decision"),
    # "We decided to …" — highest-confidence English phrasing.
    (re.compile(r"\bwe\s+(?:decided|agreed|settled\s+on)\s+(?:to\s+|that\s+)?(.+)", re.IGNORECASE), 0.85, "decision"),
    # "Convention: …" / "[convention] …"
    (re.compile(r"\b(?:convention|규칙|규약)\s*[:：]\s*(.+)", re.IGNORECASE), 0.9, "convention"),
    # Korean: "~~ 하기로 했다 / 하기로 정했다 / 합의했다"
    (re.compile(r"(.+?)\s*(?:하기로\s*(?:했|정했|결정했|합의했)|로\s*결정했|로\s*확정)", re.IGNORECASE), 0.85, "decision"),
    # "Always do X" — convention-style imperative.
    (re.compile(r"^\s*(?:always|never)\s+(.+)", re.IGNORECASE), 0.7, "convention"),
]

_MIN_LEN = 12
_MAX_LEN = 200


@dataclass(frozen=True)
class DetectedFact:
    content: str
    fact_type: str
    confidence: float
    source_line: str


def detect(text: str, *, confidence_floor: float = 0.8) -> list[DetectedFact]:
    """Scan ``text`` (a single message body or a stitched transcript)
    for fact-worthy lines. Returns deduplicated detections at or above
    the floor.

    Lines must be short (12–200 chars after match) to qualify — long
    paragraphs almost always explain rather than commit.
    """
    seen: set[str] = set()
    out: list[DetectedFact] = []

    for raw in text.splitlines():
        line = raw.strip().lstrip("-*•").strip()
        # Drop a leading role label so "Assistant: Decision: foo"
        # behaves the same as "Decision: foo".
        line = _ROLE_PREFIX.sub("", line, count=1).strip()
        if len(line) < _MIN_LEN:
            continue
        for pattern, weight, fact_type in _PATTERNS:
            m = pattern.search(line)
            if not m:
                continue
            payload = (m.group(1) if m.groups() else line).strip(" .,;:\"'")
            if not (_MIN_LEN <= len(payload) <= _MAX_LEN):
                continue
            if weight < confidence_floor:
                continue
            key = payload.lower()
            if key in seen:
                continue
            seen.add(key)
            out.append(
                DetectedFact(
                    content=payload,
                    fact_type=fact_type,
                    confidence=weight,
                    source_line=line,
                )
            )
            break  # one detection per line is plenty
    return out
