#!/usr/bin/env python3
"""Classify every chat turn in the 60q balanced sample into a fact_type
using the calling agent's LLM (MiniMax). Writes a JSON map
`{question_id: {dia_id: fact_type}}` consumed by the bench's
`--classify-from FILE` flag.

Why this exists: an earlier `--llm-extract` measurement (where the
extractor *rewrote* facts as paraphrased summaries) regressed accuracy
~13pt because the rewritten text no longer matched the reader's raw
turns. This script tests the alternative — the extractor only **labels
fact_type**, leaving content untouched. The hypothesis: classification-
only carries the in-pipeline weighting benefit (decay-exempt + 2× boost
on personalisation tiers) without the abstraction-mismatch penalty.

The dia_id we tag is constructed inline as "Q{question_id}_S{session_idx}_T{turn_idx}"
since LongMemEval doesn't carry one natively. The bench builds the same
key when it ingests, so they match up by position.

Usage:
  set -a; source ~/.hermes/.env; set +a
  python3 scripts/longmemeval_classify_sessions.py \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --balanced-sample 10 \
      --reader MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 \
      --reader-api-key-env MINIMAX_API_KEY \
      --workers 6 \
      --out /tmp/longmemeval_60bal_classification.json
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import threading
from collections import OrderedDict, defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import _call_openai, _extract_text  # noqa: E402


VALID_TYPES = {
    "preference",
    "decision",
    "lesson",
    "error",
    "convention",
    "pattern",
    "claim",
    "note",
}

PROMPT = """\
Classify each chat turn below into ONE wg fact_type. Return ONE \
valid JSON object: a flat map from `idx` (the turn's position, \
0-based) to fact_type string. No explanation, no extra keys.

fact_type menu (use the most specific match; default `note`):
  preference  — user expresses a like / dislike / habit ("I prefer X", \
                "my favourite is Y", "I usually use Z")
  decision    — user commits to a choice ("I'll go with X", \
                "I bought Y", "I signed up for Z")
  lesson      — outcome of an attempt ("tried X but Y", \
                "turns out", "wish I had", "ended up doing Z")
  error       — explicit warning to avoid ("never again", \
                "was a mistake", "I'll avoid X")
  convention  — universal habit ("I always X", "every time I", \
                "I never X")
  pattern     — architectural / structural assertion \
                ("X uses Y for Z")
  claim       — neutral factual assertion (no opinion)
  note        — assistant turns, generic dialogue, anything else

Turns:
{turns}

Output ONLY the JSON map (e.g. {{"0":"preference","1":"note",...}}).
"""


def classify_session(api_key, model, base_url, session, max_chars_per_turn=300):
    """LLM-classify every turn in `session`. Returns dict[int, str]
    keyed by turn index. Falls back to `note` for any unparsable
    output."""
    if not session:
        return {}
    lines = []
    for i, t in enumerate(session):
        speaker = t.get("role", "user")
        text = t.get("content", "")[:max_chars_per_turn]
        lines.append(f"[{i}] {speaker}: {text}")
    prompt = PROMPT.format(turns="\n".join(lines))
    fallback = {i: "note" for i in range(len(session))}
    try:
        resp = _call_openai(
            api_key, model,
            [{"role": "user", "content": prompt}],
            4096, base_url, temperature=0.0,
        )
        raw = _extract_text(resp)
    except Exception as e:
        print(f"  ! classify fail: {e}", file=sys.stderr)
        return fallback
    text = raw.split("</think>", 1)[-1].strip() if "</think>" in raw else raw.strip()
    # Pull the first {...} block.
    m = re.search(r"\{.*?\}", text, re.DOTALL)
    if not m:
        return fallback
    try:
        parsed = json.loads(m.group(0))
    except json.JSONDecodeError:
        return fallback
    out = {}
    for k, v in parsed.items():
        try:
            i = int(k)
        except (TypeError, ValueError):
            continue
        v_norm = str(v).strip().lower()
        if v_norm not in VALID_TYPES:
            v_norm = "note"
        out[i] = v_norm
    # Fill any missing indices with note.
    for i in range(len(session)):
        out.setdefault(i, "note")
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--balanced-sample", type=int, default=10)
    ap.add_argument("--reader", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--reader-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--reader-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    api_key = os.environ.get(args.reader_api_key_env, "")
    if not api_key:
        print("error: API key not set", file=sys.stderr)
        return 2

    gold = json.load(open(args.gold))
    by_type: dict = OrderedDict()
    for q in gold:
        by_type.setdefault(q["question_type"], []).append(q)
    sample = []
    for _t, bucket in by_type.items():
        sample.extend(bucket[: args.balanced_sample])
    print(f"sample: {len(sample)} questions")
    total_sessions = sum(len(q["haystack_sessions"]) for q in sample)
    print(f"sessions to classify: {total_sessions}")

    # Resume support: if --out already exists, skip already-classified
    # (qid, sess_idx) pairs.
    existing = {}
    if args.out.exists():
        existing = json.loads(args.out.read_text())
        print(f"  resuming: {sum(len(s) for s in existing.values())} sessions on disk")

    # Build job list
    jobs = []
    for q in sample:
        qid = q["question_id"]
        for sess_idx, sess in enumerate(q["haystack_sessions"]):
            key = str(sess_idx)
            if existing.get(qid, {}).get(key) is not None:
                continue
            jobs.append((qid, sess_idx, sess))
    print(f"new sessions to classify: {len(jobs)}")

    # Parallel classify, periodic checkpoint
    results: dict = defaultdict(dict)
    for qid, sess_map in existing.items():
        results[qid] = dict(sess_map)
    write_lock = threading.Lock()

    def _one(job):
        qid, sess_idx, sess = job
        labels = classify_session(api_key, args.reader, args.reader_base_url, sess)
        return qid, sess_idx, labels

    save_every = 100
    done_count = 0
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(_one, j): j for j in jobs}
        for fut in as_completed(futs):
            try:
                qid, sess_idx, labels = fut.result()
            except Exception as e:
                print(f"  ! job fail: {e}", file=sys.stderr)
                continue
            with write_lock:
                results[qid][str(sess_idx)] = labels
                done_count += 1
                if done_count % save_every == 0:
                    args.out.write_text(json.dumps(results))
                    print(f"    [{done_count:>4}/{len(jobs)}] checkpointed", file=sys.stderr)

    # Final write
    args.out.write_text(json.dumps(results))
    print(f"wrote: {args.out}")

    # Distribution stats
    all_labels = [v for q in results.values() for sess in q.values() for v in sess.values()]
    from collections import Counter
    c = Counter(all_labels)
    total = sum(c.values())
    print(f"\nClassification distribution ({total} turns):")
    for t, n in c.most_common():
        print(f"  {t:<12} {n:>6} ({n/total:.1%})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
