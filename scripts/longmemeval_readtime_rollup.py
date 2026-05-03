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


def rollup_to_sessions(
    retrievals: list,
    max_sessions: int = 20,
    full_session_lookup: dict | None = None,
) -> list:
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

    full_session_lookup: optional dict[session_id -> full session
    content]. When provided, the block content is the FULL session
    (every turn) rather than just matched turns. Lets the script
    test the upper bound of read-time rollup as if a real wg
    implementation called fact_list on the session entity for
    every hit. Without it, only matched turns are concatenated.
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
    # sorted). When full_session_lookup is provided, swap matched-only
    # content for the full session text — simulates fact_list per
    # session entity at read time.
    blocks = []
    for bucket in by_session.values():
        if full_session_lookup is not None and bucket["session_id"] in full_session_lookup:
            content = full_session_lookup[bucket["session_id"]]
        else:
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
    ap.add_argument(
        "--gold-for-full-session",
        type=Path,
        help="LongMemEval JSON. When provided, blocks contain FULL session "
        "content (every turn) — simulates a real wg fact_list per session "
        "entity. Without it, blocks include only matched turns.",
    )
    args = ap.parse_args()

    rows = [json.loads(l) for l in open(args.in_retrievals)]
    print(f"input: {len(rows)} questions, {sum(len(r['retrievals']) for r in rows)} total retrievals")

    # Build per-question {session_id -> full session content} map from gold.
    gold_lookup_by_qid: dict = {}
    if args.gold_for_full_session is not None:
        gold = json.load(open(args.gold_for_full_session))
        for q in gold:
            qid = q["question_id"]
            sess_to_text: dict = {}
            for sid, session_turns in zip(q["haystack_session_ids"], q["haystack_sessions"]):
                sess_to_text[sid] = "\n".join(
                    f"{t['role']}: {t['content']}" for t in session_turns
                )
            gold_lookup_by_qid[qid] = sess_to_text
        print(f"  full-session lookup loaded for {len(gold_lookup_by_qid)} questions")

    rolled = []
    for r in rows:
        full_lookup = gold_lookup_by_qid.get(r["question_id"]) if gold_lookup_by_qid else None
        new_retrievals = rollup_to_sessions(
            r["retrievals"],
            max_sessions=args.max_sessions,
            full_session_lookup=full_lookup,
        )
        rolled.append({**r, "retrievals": new_retrievals})

    args.out_retrievals.write_text("\n".join(json.dumps(r) for r in rolled) + "\n")
    n_blocks = sum(len(r["retrievals"]) for r in rolled)
    avg_turns = sum(b.get("n_turns_in_block", 0) for r in rolled for b in r["retrievals"]) / max(1, n_blocks)
    avg_chars = sum(len(b.get("content","")) for r in rolled for b in r["retrievals"]) / max(1, n_blocks)
    print(f"output: {len(rolled)} questions, {n_blocks} session blocks "
          f"(avg {avg_turns:.1f} matched turns/block, {avg_chars:.0f} chars/block)")
    print(f"wrote: {args.out_retrievals}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
