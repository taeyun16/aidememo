#!/usr/bin/env python3
"""Merge multiple retrieval JSONL files with file-order precedence + dedup.

Verbatim port of OMEGA's triple-retrieval-merge pattern from
`scripts/longmemeval_official.py::retrieve_context`. Each input file is a
retrieval-emit JSONL produced by `wg-benchmarks longmemeval`. For each
question_id, retrievals are concatenated in input-file order, dedup'd by
fact_id (first occurrence wins), then the rank field is renumbered.

Use cases:
  * Expanded-query + original-query merge (covers cases where expansion
    dilutes the original BM25 match — OMEGA's "tertiary" pass).
  * Temporal-filtered + unfiltered merge (the temporal filter is applied
    via --temporal-window post-hoc to the FIRST input file before merging,
    using the question_date anchor and a ±buffer).

Usage:
  python3 scripts/merge_retrievals.py \
      --in /tmp/wg_retrievals_500_expanded.jsonl \
      --in /tmp/wg_retrievals_500_omega_pattern.jsonl \
      --out /tmp/wg_retrievals_500_merged.jsonl
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timedelta
from pathlib import Path


_WORD_TO_NUM = {
    "one": 1, "two": 2, "three": 3, "four": 4, "five": 5,
    "six": 6, "seven": 7, "eight": 8, "nine": 9, "ten": 10,
    "eleven": 11, "twelve": 12, "thirteen": 13, "fourteen": 14,
    "fifteen": 15, "twenty": 20, "thirty": 30,
}


def _parse_anchor(question_date):
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


def _infer_temporal_range(query, anchor):
    """Verbatim port of OMEGA `_infer_temporal_range_anchored`. Returns
    (start_dt, end_dt) or None if no temporal signal."""
    # "last (Monday|...|Sunday|weekend)"
    m = re.search(
        r"last\s+(Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday|weekend)",
        query, re.IGNORECASE,
    )
    if m:
        day_name = m.group(1).capitalize()
        if day_name == "Weekend":
            target_weekday = 5
        else:
            day_map = {
                "Monday": 0, "Tuesday": 1, "Wednesday": 2, "Thursday": 3,
                "Friday": 4, "Saturday": 5, "Sunday": 6,
            }
            target_weekday = day_map[day_name]
        days_back = (anchor.weekday() - target_weekday) % 7
        if days_back == 0:
            days_back = 7
        target = anchor - timedelta(days=days_back)
        return (target - timedelta(days=2), target + timedelta(days=2))

    # "N days/weeks/months/years ago"
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
                buffer = max(delta * 0.25, timedelta(days=3))
                return (center - buffer, center + buffer)

    # "between DATE and DATE" / "from DATE to DATE"
    m = re.search(
        r"(?:between|from)\s+(\d{4}[/-]\d{1,2}[/-]\d{1,2})\s+(?:and|to)\s+(\d{4}[/-]\d{1,2}[/-]\d{1,2})",
        query, re.IGNORECASE,
    )
    if m:
        try:
            d1 = datetime.strptime(m.group(1).replace("/", "-"), "%Y-%m-%d")
            d2 = datetime.strptime(m.group(2).replace("/", "-"), "%Y-%m-%d")
            return (min(d1, d2) - timedelta(days=1), max(d1, d2) + timedelta(days=1))
        except ValueError:
            pass

    # "last/past/previous N days/weeks/months/years"
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
                return (anchor - delta - timedelta(days=1), anchor)

    # "in [Month] [Year]"
    m = re.search(r"in\s+(January|February|March|April|May|June|July|August|September|October|November|December)\s+(\d{4})", query, re.IGNORECASE)
    if m:
        month_name = m.group(1)
        year = int(m.group(2))
        month_num = datetime.strptime(month_name, "%B").month
        start = datetime(year, month_num, 1) - timedelta(days=1)
        if month_num == 12:
            end = datetime(year + 1, 1, 1) + timedelta(days=1)
        else:
            end = datetime(year, month_num + 1, 1) + timedelta(days=1)
        return (start, end)

    return None


def _apply_temporal_window(rows, gold_index):
    """For each row, filter retrievals by window inferred from question_date.
    Returns a NEW list where the 'retrievals' field of each row is the filtered
    subset (empty if no temporal signal)."""
    out = []
    for r in rows:
        gold_q = gold_index.get(r["question_id"])
        if gold_q is None:
            out.append({**r, "retrievals": []})
            continue
        anchor = _parse_anchor(gold_q.get("question_date"))
        if anchor is None:
            out.append({**r, "retrievals": []})
            continue
        window = _infer_temporal_range(gold_q.get("question", ""), anchor)
        if window is None:
            out.append({**r, "retrievals": []})
            continue
        start_ms = int(window[0].timestamp() * 1000)
        end_ms = int(window[1].timestamp() * 1000)
        kept = [
            hit for hit in r.get("retrievals", [])
            if hit.get("referenced_date") is not None
            and start_ms <= hit["referenced_date"] <= end_ms
        ]
        out.append({**r, "retrievals": kept})
    return out


def _merge_one_question(retrieval_lists):
    """Merge N retrieval lists with file-order precedence. Dedup by fact_id."""
    seen = set()
    out = []
    for retrievals in retrieval_lists:
        for hit in retrievals:
            fid = hit.get("fact_id")
            if fid in seen:
                continue
            seen.add(fid)
            out.append(hit)
    # Renumber rank
    for new_rank, hit in enumerate(out, 1):
        hit["rank"] = new_rank
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inputs", action="append", required=True, type=Path,
                    help="One or more retrieval JSONL files (highest precedence first)")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--gold", type=Path,
                    help="If set with --temporal-window, apply temporal filter to the FIRST input")
    ap.add_argument("--temporal-window", action="store_true",
                    help="Apply temporal range filter to the FIRST input before merging "
                         "(uses --gold for question_date anchor)")
    args = ap.parse_args()

    if len(args.inputs) < 1:
        print("error: need at least one --in", file=sys.stderr)
        return 2

    by_qid = {}
    for path in args.inputs:
        with open(path) as f:
            for line in f:
                row = json.loads(line)
                qid = row["question_id"]
                if qid not in by_qid:
                    by_qid[qid] = {**row, "_lists": []}
                by_qid[qid]["_lists"].append(row.get("retrievals", []))

    if args.temporal_window:
        if not args.gold:
            print("error: --temporal-window requires --gold", file=sys.stderr)
            return 2
        gold_index = {q["question_id"]: q for q in json.load(open(args.gold))}
        # Apply temporal filter only to the first input's retrievals (which is
        # the first list in each row's _lists).
        n_filtered = 0
        for qid, row in by_qid.items():
            if not row["_lists"]:
                continue
            first = row["_lists"][0]
            gold_q = gold_index.get(qid)
            if gold_q is None:
                continue
            anchor = _parse_anchor(gold_q.get("question_date"))
            if anchor is None:
                continue
            window = _infer_temporal_range(gold_q.get("question", ""), anchor)
            if window is None:
                continue
            start_ms = int(window[0].timestamp() * 1000)
            end_ms = int(window[1].timestamp() * 1000)
            kept = [
                hit for hit in first
                if hit.get("referenced_date") is not None
                and start_ms <= hit["referenced_date"] <= end_ms
            ]
            row["_lists"][0] = kept
            if kept:
                n_filtered += 1
        print(f"  temporal-window: {n_filtered} questions had at least one in-window hit")

    n_in = sum(len(lists) for lists in (r["_lists"] for r in by_qid.values()))
    n_out = 0
    with open(args.out, "w") as fout:
        for qid in sorted(by_qid):
            row = by_qid[qid]
            merged = _merge_one_question(row["_lists"])
            n_out += len(merged)
            del row["_lists"]
            row["retrievals"] = merged
            fout.write(json.dumps(row) + "\n")
    print(f"  inputs:    {len(args.inputs)} files, {len(by_qid)} questions")
    print(f"  retrievals (in):  {n_in} (sum across files+questions)")
    print(f"  retrievals (out): {n_out} (after dedup)")
    print(f"  wrote: {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
