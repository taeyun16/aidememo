#!/usr/bin/env python3
"""Self-consistency wrapper around longmemeval_omega_style.

Hypothesis: MiniMax temp=0 has ±5pt run-to-run variance because reasoning
models sample from think-token paths even at temp=0. The 4-run 120q
analysis showed:
  * 83 questions are 0/4 fails (solid wins)
  * 16 questions are in the 1-3/4 variance band — coin-flip outcomes
  * 7 questions are 4/4 fails (structural ceiling)

Theoretical max if variance band were perfectly recovered: 113/120 = 94.2%.
Current 4-run mean: 83.9%. Self-consistency voting could recover much of
the 10pt lost to variance.

Algorithm (per question):
  1. Run reader N times at temperature=0.5 (diversity).
  2. Run a synthesis call: "Given these N candidate answers, return the
     consensus answer (or pick the most likely correct one if they
     conflict)."
  3. Judge the synthesized answer.

Cost: (N+1) reader calls + 1 judge call per question. For N=3 and 120q,
that's 480 LLM calls — about 5min with workers=16 + jittered backoff.

Usage:
  python3 scripts/longmemeval_self_consistency.py \
      --retrievals /tmp/wg_retrievals_120bal_full.jsonl \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --reader MiniMax-M2.7-highspeed --judge MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 \
      --reader-api-key-env MINIMAX_API_KEY \
      --judge-base-url https://api.minimax.io/v1 \
      --judge-api-key-env MINIMAX_API_KEY \
      --workers 16 --n-votes 3 --vote-temperature 0.5 \
      --out /tmp/wg_self_consistency_120bal
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import (  # noqa: E402
    _build_reader_prompt,
    _call_openai,
    _extract_text,
    _epoch_ms_to_iso,
    _filter_and_sort,
    _format_session_block,
    _grade,
    _CATEGORY_CONFIG,
    _DEFAULT_CONFIG,
    _is_counting_question,
)


SYNTHESIS_PROMPT = """\
A user asked a question about themselves. Three independent answers were \
generated using the same retrieved context. Pick the BEST answer by \
checking each against the retrieved snippets — don't blindly take majority.

# Retrieved snippets (the ground truth context)

{sessions}

# Question
{question}

# Candidate answers

## A:
{answer_a}

## B:
{answer_b}

## C:
{answer_c}

# Selection rules

For "how many X" / "how much X" / counting / aggregation questions:
- Pick the candidate whose count matches the snippets when you re-count
- If candidates differ on count, the count that you can VERIFY by listing \
items in the snippets is correct

For preference / recommendation questions:
- Pick the candidate that references the SPECIFIC user preferences / past \
experiences from the snippets (not generic advice)
- Honour niche preferences (e.g., specific genre / platform / brand the \
user mentioned) over broad themes

For factual lookup / temporal questions:
- Pick the candidate whose dates / values match the snippets exactly
- For "cannot answer" candidates: only pick them if you genuinely can't \
find the info in the snippets

If 2 of 3 candidates agree AND that answer matches the snippets: pick it.
If candidates disagree AND only 1 matches the snippets: pick that one.
If none clearly match: pick the most direct candidate.

Output ONLY the final selected answer text (concise, single answer):"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", required=True, type=Path)
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--reader", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--judge", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--out", default=Path("/tmp/wg_self_consistency"), type=Path)
    ap.add_argument("--reader-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--reader-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--judge-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--judge-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=16)
    ap.add_argument("--n-votes", type=int, default=3, help="Reader calls per question (3 default).")
    ap.add_argument(
        "--vote-temperature",
        type=float,
        default=0.5,
        help="Reader temperature for vote calls. Higher = more diversity (default 0.5). 0 disables diversity.",
    )
    args = ap.parse_args()

    reader_key = os.environ.get(args.reader_api_key_env, "")
    judge_key = os.environ.get(args.judge_api_key_env, "")
    if not reader_key or not judge_key:
        print(f"error: keys not set", file=sys.stderr)
        return 2

    args.out.mkdir(parents=True, exist_ok=True)
    hyp_path = args.out / f"hypotheses_{args.reader}_n{args.n_votes}.jsonl"
    judg_path = args.out / f"judgements_{args.reader}_judge_{args.judge}_n{args.n_votes}.jsonl"

    rows = [json.loads(l) for l in open(args.retrievals)]
    gold = {q["question_id"]: q for q in json.load(open(args.gold))}
    for r in rows:
        g = gold.get(r["question_id"])
        if g is not None:
            r["question"] = g["question"]
            r["question_date"] = g.get("question_date")
    print(f"Self-consistency harness: {len(rows)} questions, n_votes={args.n_votes}, "
          f"vote_temp={args.vote_temperature}, workers={args.workers}")

    # ---- Stage A: N-vote reader + synthesis ----
    done = set()
    if hyp_path.exists():
        for line in open(hyp_path):
            done.add(json.loads(line)["question_id"])
        print(f"  reader: resuming, {len(done)} on disk")
    todo = [r for r in rows if r["question_id"] not in done]
    print(f"  reader: {len(todo)} new questions, each needs {args.n_votes}+1 calls")

    def _one_question(row):
        qdata = {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "question": row["question"],
            "question_date": row.get("question_date"),
        }
        prompt, max_tokens, n_used = _build_reader_prompt(qdata, row.get("retrievals", []))
        # N parallel reader calls at vote_temperature for diversity.
        candidates = []
        for _ in range(args.n_votes):
            try:
                resp = _call_openai(
                    reader_key, args.reader,
                    [{"role": "user", "content": prompt}],
                    max_tokens, args.reader_base_url,
                    temperature=args.vote_temperature,
                )
                candidates.append(_extract_text(resp))
            except Exception as e:
                print(f"  ! vote fail {row['question_id']}: {e}", file=sys.stderr)
                candidates.append("")

        # Synthesis call: pick consensus from N candidates, with the
        # retrieved snippets passed as context so synthesis can VERIFY
        # against ground truth (not blind majority).
        if len([c for c in candidates if c]) <= 1:
            # Degenerate case — only 1 valid answer, no synthesis needed.
            hypothesis = next((c for c in candidates if c), "")
        else:
            # Pad to 3 for prompt template.
            while len(candidates) < 3:
                candidates.append("(no answer)")
            # Build the snippet block the same way the reader saw it.
            qtype = qdata["question_type"]
            cfg = dict(_CATEGORY_CONFIG.get(qtype, _DEFAULT_CONFIG))
            if qtype == "multi-session" and _is_counting_question(qdata["question"]):
                cfg["max_res"] = min(30, int(cfg["max_res"] * 1.5))
            retr_copy = [dict(r) for r in row.get("retrievals", [])]
            filtered = _filter_and_sort(retr_copy, cfg)
            blocks = []
            for i, r in enumerate(filtered, 1):
                date_iso = _epoch_ms_to_iso(r.get("referenced_date"))
                blocks.append(_format_session_block(r["content"][:600], date_iso, i))
            sessions_text = "\n\n".join(blocks)
            synth_prompt = SYNTHESIS_PROMPT.format(
                sessions=sessions_text,
                question=qdata["question"],
                answer_a=candidates[0][:1500],
                answer_b=candidates[1][:1500],
                answer_c=candidates[2][:1500],
            )
            try:
                # 4096 tokens so reasoning model finishes <think> + emits
                # the answer. 1024 tokens left empty hypotheses on SS-asst
                # (short answers like "Roscioli") because thinking ate the
                # budget. Doesn't hurt cost — actual answers are short,
                # extra tokens only used by reasoning when the prompt is
                # complex.
                resp = _call_openai(
                    reader_key, args.reader,
                    [{"role": "user", "content": synth_prompt}],
                    4096, args.reader_base_url,
                    temperature=0.0,  # synthesis is deterministic
                )
                hypothesis = _extract_text(resp)
                # Empty after <think> strip = thinker hit token limit
                # before emitting. Fall back to first candidate.
                if not hypothesis:
                    print(f"  ! synth empty {row['question_id']} — fallback to vote 0", file=sys.stderr)
                    hypothesis = candidates[0]
            except Exception as e:
                print(f"  ! synth fail {row['question_id']}: {e}", file=sys.stderr)
                hypothesis = candidates[0]  # fallback to first vote

        return {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "question": row["question"],
            "hypothesis": hypothesis,
            "n_votes": args.n_votes,
            "candidates": candidates,
        }

    write_lock = threading.Lock()
    with open(hyp_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_one_question, r): r for r in todo}
        i = 0
        for fut in as_completed(futures):
            i += 1
            r = fut.result()
            with write_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 10 == 0 or i == len(todo):
                print(f"    [{i:>4}/{len(todo)}] {r['question_id']}", file=sys.stderr)

    # ---- Stage B: judge ----
    judged = set()
    if judg_path.exists():
        for line in open(judg_path):
            judged.add(json.loads(line)["question_id"])
    hyps = [json.loads(l) for l in open(hyp_path)]
    todo_j = [h for h in hyps if h["question_id"] not in judged]
    print(f"  judge: {len(todo_j)} new judgements")

    def _judge_one(hyp):
        qid = hyp["question_id"]
        qdata = gold.get(qid)
        if qdata is None:
            return None
        try:
            raw, correct = _grade(qdata, hyp["hypothesis"], args.judge, judge_key, args.judge_base_url)
        except Exception as e:
            print(f"  ! judge fail {qid}: {e}", file=sys.stderr)
            raw, correct = "", None
        return {
            "question_id": qid,
            "question_type": hyp["question_type"],
            "verdict_raw": raw,
            "correct": correct,
        }

    judge_lock = threading.Lock()
    with open(judg_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_judge_one, h): h for h in todo_j}
        i = 0
        for fut in as_completed(futures):
            i += 1
            r = fut.result()
            if r is None:
                continue
            with judge_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 10 == 0 or i == len(todo_j):
                print(f"    [{i:>4}/{len(todo_j)}] {r['question_id']}", file=sys.stderr)

    # ---- Aggregate ----
    js = [json.loads(l) for l in open(judg_path)]
    ok = sum(1 for j in js if j["correct"] is True)
    bad = sum(1 for j in js if j["correct"] is False)
    unk = sum(1 for j in js if j["correct"] is None)
    n = ok + bad + unk
    print()
    print(f"Self-consistency (n={args.n_votes}, vote_temp={args.vote_temperature}): "
          f"reader={args.reader}, judge={args.judge}")
    print(f"  total: {n}")
    print(f"  CORRECT:    {ok}  ({ok/n:.3f})")
    print(f"  INCORRECT:  {bad}  ({bad/n:.3f})")
    print(f"  unparseable {unk}  ({unk/n:.3f})")
    by_type = {}
    for j in js:
        by_type.setdefault(j["question_type"], []).append(j["correct"])
    print("\n  By question_type:")
    for qt in sorted(by_type):
        vs = by_type[qt]
        okc = sum(1 for v in vs if v)
        print(f"    {qt:30}  {okc/len(vs):.3f}  ({okc}/{len(vs)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
