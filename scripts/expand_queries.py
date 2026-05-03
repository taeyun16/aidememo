#!/usr/bin/env python3
"""Expand LongMemEval questions with OMEGA-style temporal/entity/counting cues.

Verbatim port of OMEGA `scripts/longmemeval_official.py::_expand_query` and
`_resolve_relative_dates`. Reads the original LongMemEval-S JSON, rewrites
each question's `question` field with appended expansion keywords, writes
a new JSON file. The wg-benchmarks bench then ingests/queries against the
expanded text so BM25/semantic retrieval picks up the explicit date and
entity tokens.

Usage:
  python3 scripts/expand_queries.py \
      --in /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --out /tmp/longmemeval_data/longmemeval_s_expanded.json
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timedelta
from pathlib import Path


_WORD_TO_NUM = {
    "one": 1, "a": 1, "two": 2, "three": 3, "four": 4, "five": 5,
    "six": 6, "seven": 7, "eight": 8, "nine": 9, "ten": 10,
    "eleven": 11, "twelve": 12, "thirteen": 13, "fourteen": 14,
    "fifteen": 15, "twenty": 20, "thirty": 30,
}
_DAY_NAMES = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
_MONTH_NAMES = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
]
_COMMON = {
    "I", "The", "A", "An", "My", "What", "When", "Where", "Who", "How",
    "Which", "Why", "Do", "Does", "Did", "Is", "Are", "Was", "Were",
    "Have", "Has", "Had", "Can", "Could", "Would", "Should", "Will",
    "If", "In", "On", "At", "To", "For", "Of", "And", "Or", "But",
    "Not", "That", "This", "It", "He", "She", "They", "We", "You",
    "Please", "Tell", "Me", "About",
}


def _parse_question_anchor(question_date):
    if not question_date:
        return None
    cleaned = re.sub(r"\s*\([A-Za-z]+\)\s*", " ", question_date).strip()
    try:
        return datetime.strptime(cleaned, "%Y/%m/%d %H:%M")
    except ValueError:
        try:
            return datetime.fromisoformat(question_date)
        except ValueError:
            return None


def _resolve_relative_dates(query, anchor):
    """Verbatim port of OMEGA's `_resolve_relative_dates`. Resolves ALL
    relative time references in a query to absolute date keywords."""
    q_lower = query.lower()
    resolved = []

    # 1. "last (Monday|Tuesday|...|Sunday)"
    day_match = re.search(
        r"last\s+(Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)",
        query, re.IGNORECASE,
    )
    if day_match:
        day_name = day_match.group(1).capitalize()
        day_map = {d: i for i, d in enumerate(_DAY_NAMES)}
        target_weekday = day_map[day_name]
        days_back = (anchor.weekday() - target_weekday) % 7
        if days_back == 0:
            days_back = 7
        target_date = anchor - timedelta(days=days_back)
        resolved.append(f"{day_name} {target_date.strftime('%Y-%m-%d')} {target_date.strftime('%B %d')}")

    # 2. "last weekend"
    if "last weekend" in q_lower:
        sat = anchor - timedelta(days=(anchor.weekday() + 2) % 7 or 7)
        sun = sat + timedelta(days=1)
        resolved.append(f"Saturday Sunday {sat.strftime('%Y-%m-%d')} {sun.strftime('%Y-%m-%d')}")

    # 3. "yesterday"
    if "yesterday" in q_lower:
        yest = anchor - timedelta(days=1)
        resolved.append(f"{yest.strftime('%Y-%m-%d')} {yest.strftime('%B %d')}")

    # 4. "last week" (excluding "last weekend")
    if "last week" in q_lower and "weekend" not in q_lower:
        start = anchor - timedelta(days=anchor.weekday() + 7)
        end = start + timedelta(days=6)
        resolved.append(f"{start.strftime('%Y-%m-%d')} {end.strftime('%Y-%m-%d')}")

    # 5. "N days/weeks/months/years ago"
    m = re.search(r"(\d+|[a-z]+)\s+(day|week|month|year)s?\s+ago", query, re.IGNORECASE)
    if m:
        raw_n = m.group(1).lower()
        n = int(raw_n) if raw_n.isdigit() else _WORD_TO_NUM.get(raw_n)
        if n is not None:
            unit = m.group(2).lower()
            delta = None
            if unit == "day":
                delta = timedelta(days=n)
            elif unit == "week":
                delta = timedelta(weeks=n)
            elif unit == "month":
                delta = timedelta(days=n * 30)
            elif unit == "year":
                delta = timedelta(days=n * 365)
            if delta:
                center = anchor - delta
                resolved.append(
                    f"{center.strftime('%Y-%m-%d')} {center.strftime('%B')} {center.strftime('%d')}"
                )

    # 6. "last/past/previous N days/weeks/months/years"
    m = re.search(r"(?:last|past|previous)\s+(\d+|[a-z]+)\s+(day|week|month|year)s?", query, re.IGNORECASE)
    if m:
        raw_n = m.group(1).lower()
        n = int(raw_n) if raw_n.isdigit() else _WORD_TO_NUM.get(raw_n)
        if n is not None:
            unit = m.group(2).lower()
            delta = None
            if unit == "day":
                delta = timedelta(days=n)
            elif unit == "week":
                delta = timedelta(weeks=n)
            elif unit == "month":
                delta = timedelta(days=n * 30)
            elif unit == "year":
                delta = timedelta(days=n * 365)
            if delta:
                start = anchor - delta
                resolved.append(
                    f"{start.strftime('%Y-%m-%d')} {anchor.strftime('%Y-%m-%d')} "
                    f"{start.strftime('%B')} {anchor.strftime('%B')}"
                )

    # 7. "in [Month]" without year
    m = re.search(
        r"in\s+(January|February|March|April|May|June|July|August|September|October|November|December)\b",
        query, re.IGNORECASE,
    )
    if m and not re.search(r"in\s+" + m.group(1) + r"\s+\d{4}", query, re.IGNORECASE):
        month_name = m.group(1).capitalize()
        month_num = _MONTH_NAMES.index(month_name) + 1
        year = anchor.year if month_num <= anchor.month else anchor.year - 1
        resolved.append(f"{month_name} {year} {year}-{month_num:02d}")

    # 8. "in [Month] [Year]"
    m = re.search(
        r"in\s+(January|February|March|April|May|June|July|August|September|October|November|December)\s+(\d{4})",
        query, re.IGNORECASE,
    )
    if m:
        month_name = m.group(1).capitalize()
        year = int(m.group(2))
        month_num = _MONTH_NAMES.index(month_name) + 1
        resolved.append(f"{month_name} {year} {year}-{month_num:02d}")

    return resolved


def expand_query(query, question_date):
    """Verbatim port of OMEGA's `_expand_query`."""
    expansions = []

    # 1. Counting signals
    q_lower = query.lower()
    if any(s in q_lower for s in ("how many", "how much", "how often", "total number", "count")):
        expansions.append("every instance all occurrences each time")

    # 2. Temporal expansion via anchor
    anchor = _parse_question_anchor(question_date)
    if anchor:
        expansions.extend(_resolve_relative_dates(query, anchor))

    # 3. Entity extraction
    words = re.findall(r"\b[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\b", query)
    entities = [w for w in words if w not in _COMMON and len(w) > 1]
    if entities:
        expansions.append(" ".join(entities))

    if not expansions:
        return query
    return query + " " + " ".join(expansions)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inp", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    questions = json.load(open(args.inp))
    n_modified = 0
    for q in questions:
        original = q["question"]
        expanded = expand_query(original, q.get("question_date"))
        if expanded != original:
            n_modified += 1
        q["question"] = expanded

    args.out.write_text(json.dumps(questions))
    print(f"  loaded:    {len(questions)} questions from {args.inp}")
    print(f"  modified:  {n_modified} questions ({n_modified/len(questions)*100:.1f}%)")
    print(f"  wrote:     {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
