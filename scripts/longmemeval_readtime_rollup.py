#!/usr/bin/env python3
"""Verify the READ-TIME session rollup hypothesis.

Hypothesis: rolling up turn-level snippets into session blocks at READ
time gives the same lift as bench-time --hybrid-ingest, without the
2x storage cost. If true, real wg agents can write granular facts +
read coherent session blocks for free, just by tagging facts with the
session entity (which wg already does via WG_SESSION_ID).

Setup:
  * Input: turn-level retrieval JSONL (wg without --hybrid-ingest).
  * For each question, group hits by session_id, concat content
    chronologically into a session block. Each session block becomes
    one "snippet" the reader sees.
  * Compare reader accuracy to:
      - turn-level flat (baseline, no rollup) ~ 73.3% on 60q MiniMax
      - bench --hybrid-ingest (write-time aggregation) ~ 81.7%

If readtime_rollup ≈ 81.7%, the architectural choice for real wg
deployment becomes: don't pay 2x storage; aggregate at read time.

Usage:
  python3 scripts/longmemeval_readtime_rollup.py \
      --in-retrievals /tmp/wg_retrievals_60bal_turn.jsonl \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --reader MiniMax-M2.7-highspeed --judge MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 \
      --reader-api-key-env MINIMAX_API_KEY \
      --judge-base-url https://api.minimax.io/v1 \
      --judge-api-key-env MINIMAX_API_KEY \
      --workers 6 \
      --out /tmp/wg_readtime_rollup_60bal
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import OrderedDict
from pathlib import Path


def rollup_to_sessions(retrievals: list, max_sessions: int = 20) -> list:
    """Group turn-level hits into session blocks at READ time.

    The bench's --hybrid-ingest writes session-summary records at
    INGEST time (2x storage). This function does the same thing
    purely in the read path: take hits as they came back from
    hybrid_search, group by session_id, concat content within group,
    return as a synthetic "session-block" snippet list.

    Each block carries the strongest score from any of its turns
    (so chronological sort + relevance ordering still work
    downstream). Block count caps at max_sessions to mirror OMEGA's
    max_res filter.
    """
    by_session: OrderedDict = OrderedDict()
    for hit in retrievals:
        sid = hit.get("session_id") or "(no-session)"
        if sid not in by_session:
            by_session[sid] = {
                "session_id": sid,
                "turns": [],
                "max_score": 0.0,
                "earliest_date": None,
            }
        bucket = by_session[sid]
        bucket["turns"].append(hit)
        score = hit.get("score") or 0.0
        if score > bucket["max_score"]:
            bucket["max_score"] = score
        d = hit.get("referenced_date")
        if d is not None and (bucket["earliest_date"] is None or d < bucket["earliest_date"]):
            bucket["earliest_date"] = d

    # Build session blocks. Concat turns in retrieval order (which is
    # rank order; not perfectly chronological but good enough — the
    # bench's session_text is also rank-grouped not strictly time-
    # sorted).
    blocks = []
    for bucket in by_session.values():
        content = "\n".join(t["content"] for t in bucket["turns"])
        # Pick the lowest-rank fact_id of the group so the reader's
        # follow-up wg_fact_get still resolves to a real fact.
        sample = min(bucket["turns"], key=lambda t: t.get("rank") or 999)
        blocks.append({
            "rank": 0,  # set below
            "fact_id": sample["fact_id"],
            "content": content,
            "score": bucket["max_score"],
            "session_id": bucket["session_id"],
            "source": "session-rollup",
            "referenced_date": bucket["earliest_date"],
            "n_turns_in_block": len(bucket["turns"]),
        })

    # Order blocks by max-score desc (best matching session first).
    blocks.sort(key=lambda b: b["score"], reverse=True)
    blocks = blocks[:max_sessions]
    for i, b in enumerate(blocks, 1):
        b["rank"] = i
    return blocks


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--in-retrievals", required=True, type=Path)
    ap.add_argument("--out-retrievals", default=Path("/tmp/wg_retrievals_60bal_readtime_rollup.jsonl"), type=Path)
    ap.add_argument("--max-sessions", type=int, default=20)
    args = ap.parse_args()

    rows = [json.loads(l) for l in open(args.in_retrievals)]
    print(f"input: {len(rows)} questions, {sum(len(r['retrievals']) for r in rows)} total retrievals")

    rolled = []
    for r in rows:
        new_retrievals = rollup_to_sessions(r["retrievals"], max_sessions=args.max_sessions)
        rolled.append({**r, "retrievals": new_retrievals})

    args.out_retrievals.write_text("\n".join(json.dumps(r) for r in rolled) + "\n")
    n_blocks = sum(len(r["retrievals"]) for r in rolled)
    avg_turns = sum(b.get("n_turns_in_block", 0) for r in rolled for b in r["retrievals"]) / max(1, n_blocks)
    print(f"output: {len(rolled)} questions, {n_blocks} session blocks (avg {avg_turns:.1f} turns/block)")
    print(f"wrote: {args.out_retrievals}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
