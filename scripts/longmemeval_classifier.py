#!/usr/bin/env python3
"""Question classifier — does this question need agentic aggregation?

Hypothesis: a single LLM call can decide whether the answer requires
cross-session aggregation (sum / count / timeline across sessions) or
is a simple retrieval question. If so, aidememo can dispatch agentic-loop
mode for "YES" questions and the cheaper omega-style single-call mode
for "NO" questions, capturing the +2pt selective lift without leaking
the bench's question_type label.

Outputs:
  * Per-qid {aggregation_required: bool} JSONL.
  * Confusion matrix vs oracle (multi-session = positive class).
  * Simulated selective-dispatch score: combine classifier-predicted
    YES qids (read from agentic results) + classifier-NO qids (read
    from omega baseline results). Compare to oracle selective (53/60).

Usage:
  python3 scripts/longmemeval_classifier.py \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --qids-from /tmp/aidememo_retrievals_60bal_relevance_full.jsonl \
      --reader MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 --reader-api-key-env MINIMAX_API_KEY \
      --workers 8 \
      --out /tmp/aidememo_classifier_60bal.jsonl \
      --agentic-judg /tmp/aidememo_agentic_60bal_v2/judgements_MiniMax-M2.7-highspeed_judge_MiniMax-M2.7-highspeed.jsonl \
      --baseline-judg /tmp/aidememo_omega_60bal_baseline/judgements_MiniMax-M2.7-highspeed_judge_MiniMax-M2.7-highspeed.jsonl
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import threading
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import _call_openai, _extract_text  # noqa: E402


CLASSIFIER_PROMPT = """\
You are deciding how to answer a user's question about themselves.

Question: {question}

Does answering this question require AGGREGATING information across \
multiple past conversations? Specifically:
  * Summing money / time / counts across separate sessions, OR
  * Counting distinct events that span sessions, OR
  * Building a chronological timeline from multiple sessions, OR
  * Determining how many sessions / weeks / months / instances total.

Reply with EXACTLY one word — YES or NO. No explanation.
"""


def classify_one(api_key: str, model: str, base_url: str, question: str) -> bool:
    """Returns True if classifier says the question needs aggregation."""
    resp = _call_openai(
        api_key, model,
        [{"role": "user", "content": CLASSIFIER_PROMPT.format(question=question)}],
        max_tokens=2048, base_url=base_url, temperature=0.0,
    )
    raw = _extract_text(resp).strip()
    # Strip </think> if present, take last non-empty line.
    if "</think>" in raw:
        raw = raw.split("</think>", 1)[-1]
    raw = raw.strip().upper()
    # Conservative: only YES counts as positive.
    return raw.startswith("YES")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--qids-from", required=True, type=Path,
                    help="JSONL with one row per question (e.g., retrievals file). "
                    "Restricts which qids to classify.")
    ap.add_argument("--reader", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--reader-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--reader-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--out", required=True, type=Path,
                    help="Per-question classification JSONL")
    ap.add_argument("--agentic-judg", type=Path,
                    help="Agentic judgements (used for YES-routed simulation)")
    ap.add_argument("--baseline-judg", type=Path,
                    help="Baseline judgements (used for NO-routed simulation)")
    args = ap.parse_args()

    api_key = os.environ.get(args.reader_api_key_env, "")
    if not api_key:
        print("error: API key not set", file=sys.stderr)
        return 2

    qids_to_classify = [json.loads(l)["question_id"] for l in open(args.qids_from)]
    gold = {q["question_id"]: q for q in json.load(open(args.gold))}
    targets = [(qid, gold[qid]) for qid in qids_to_classify if qid in gold]
    print(f"Classifier: {len(targets)} questions, model={args.reader}, workers={args.workers}")

    done: dict = {}
    if args.out.exists():
        for line in open(args.out):
            r = json.loads(line)
            done[r["question_id"]] = r
        print(f"  resuming: {len(done)} on disk")
    todo = [(qid, q) for qid, q in targets if qid not in done]
    print(f"  new: {len(todo)}")

    def _one(qid_q):
        qid, q = qid_q
        try:
            yes = classify_one(api_key, args.reader, args.reader_base_url, q["question"])
        except Exception as e:
            print(f"  ! classify fail {qid}: {e}", file=sys.stderr)
            yes = None
        return {
            "question_id": qid,
            "question_type": q["question_type"],
            "aggregation_required": yes,
        }

    write_lock = threading.Lock()
    with open(args.out, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_one, x): x for x in todo}
        i = 0
        for fut in as_completed(futures):
            i += 1
            r = fut.result()
            with write_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 10 == 0 or i == len(todo):
                print(f"    [{i:>4}/{len(todo)}] {r['question_id']} → "
                      f"{'YES' if r['aggregation_required'] else 'NO'}",
                      file=sys.stderr)

    # ---- Confusion matrix vs oracle (multi-session = positive) ----
    rows = [json.loads(l) for l in open(args.out)]
    tp = fp = tn = fn = 0
    by_qt: dict = defaultdict(lambda: [0, 0])  # [yes, total]
    for r in rows:
        is_ms = r["question_type"] == "multi-session"
        pred = r["aggregation_required"]
        by_qt[r["question_type"]][1] += 1
        if pred: by_qt[r["question_type"]][0] += 1
        if is_ms and pred: tp += 1
        elif is_ms and not pred: fn += 1
        elif not is_ms and pred: fp += 1
        else: tn += 1
    n = tp + fp + tn + fn
    print()
    print("Classifier vs oracle (multi-session = positive class):")
    print(f"  TP={tp}  FP={fp}  TN={tn}  FN={fn}  total={n}")
    if tp + fp > 0:
        print(f"  precision={tp/(tp+fp):.3f}")
    if tp + fn > 0:
        print(f"  recall={tp/(tp+fn):.3f}")
    print(f"  accuracy={(tp+tn)/n:.3f}")
    print()
    print("YES rate by question_type:")
    for qt in sorted(by_qt):
        y, t = by_qt[qt]
        print(f"  {qt:<28}  {y}/{t}  ({y/t:.0%})")

    # ---- Simulated selective dispatch ----
    if args.agentic_judg and args.baseline_judg:
        ag = {j["question_id"]: j for j in (json.loads(l) for l in open(args.agentic_judg))}
        ba = {j["question_id"]: j for j in (json.loads(l) for l in open(args.baseline_judg))}
        cls = {r["question_id"]: r["aggregation_required"] for r in rows}
        spliced = []
        for qid, c in cls.items():
            src = ag.get(qid) if c else ba.get(qid)
            if src: spliced.append(src)
        ok = sum(1 for r in spliced if r["correct"])
        n = len(spliced)
        print()
        print(f"Simulated selective-dispatch (classifier-routed): {ok}/{n} ({ok/n:.3f})")
        print(f"  oracle selective (multi-session→agentic, rest→base): see prior result")
        cat: dict = defaultdict(lambda: [0, 0])
        for r in spliced:
            cat[r["question_type"]][1] += 1
            if r["correct"]: cat[r["question_type"]][0] += 1
        for qt in sorted(cat):
            o, t = cat[qt]
            print(f"  {qt:<28}  {o}/{t}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
