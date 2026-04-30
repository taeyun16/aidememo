#!/usr/bin/env python3
"""LongMemEval-S end-to-end LLM evaluator on top of wg's retrieval JSONL.

Pipeline:
  wg `--emit-retrievals` JSONL  →  reader LLM (gpt-4o-mini / gpt-4o / etc)
                                →  hypothesis JSONL
                                →  judge LLM (gpt-4o-mini default)
                                →  per-category accuracy

This script does both the reader and the judge in one pass — it
mirrors the official LongMemEval evaluator's prompts but runs in
Python without cloning the reference repo. Each stage is checkpointed
to disk so a partial run can resume; cost is bounded by the row count.

Usage:
  python3 scripts/longmemeval_e2e.py \\
      --retrievals /tmp/wg_retrievals_500_decay90.jsonl \\
      --gold /tmp/longmemeval/longmemeval_s_cleaned.json \\
      --reader gpt-4o-mini \\
      --judge gpt-4o-mini \\
      --out /tmp/wg_e2e_results \\
      --limit 20

Costs (rough, per 500-q full run, top_k=10 retrievals):
  reader gpt-4o-mini:  ~$0.30
  reader gpt-4o:       ~$5-7
  judge  gpt-4o-mini:  ~$0.20
  judge  gpt-4o:       ~$3-4
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path
from typing import Any

try:
    import urllib.request
    import urllib.error
except ImportError:
    print("error: urllib stdlib missing??", file=sys.stderr)
    sys.exit(2)


# Models that reject `max_tokens` and want `max_completion_tokens` instead
# (gpt-5.x and o-series — same as scripts/openai_check.sh).
def _token_field(model: str) -> str:
    if model.startswith(("gpt-5", "o1", "o3", "o4")):
        return "max_completion_tokens"
    return "max_tokens"


def _call_openai(
    api_key: str,
    model: str,
    messages: list[dict],
    max_tokens: int,
    timeout: int = 60,
) -> dict:
    """Single chat completion call with simple retry on 429/5xx."""
    url = "https://api.openai.com/v1/chat/completions"
    body = {
        "model": model,
        "messages": messages,
        _token_field(model): max_tokens,
    }
    # gpt-5 reasoning models: lower temperature is fine; default works.
    last_err: Exception | None = None
    for attempt in range(4):
        req = urllib.request.Request(
            url,
            data=json.dumps(body).encode(),
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.loads(resp.read().decode())
        except urllib.error.HTTPError as e:
            body_txt = e.read().decode("utf-8", errors="replace")
            if e.code in (429, 500, 502, 503, 504):
                # Exponential backoff: 2s, 5s, 12s, 30s
                wait = [2, 5, 12, 30][attempt]
                print(
                    f"  [retry {attempt+1}/4] HTTP {e.code} — sleeping {wait}s",
                    file=sys.stderr,
                )
                time.sleep(wait)
                last_err = RuntimeError(f"HTTP {e.code}: {body_txt[:200]}")
                continue
            raise RuntimeError(f"HTTP {e.code}: {body_txt[:200]}")
        except (urllib.error.URLError, TimeoutError) as e:
            wait = [2, 5, 12, 30][attempt]
            print(f"  [retry {attempt+1}/4] {e} — sleeping {wait}s", file=sys.stderr)
            time.sleep(wait)
            last_err = e
            continue
    raise RuntimeError(f"all retries failed: {last_err}")


def _extract_text(resp: dict) -> str:
    return resp["choices"][0]["message"]["content"].strip()


# ---- Reader prompts (matching LongMemEval official prompt structure) -----


READER_SYSTEM = """You are a helpful assistant. The user has a long history with you, and you have access to summarised retrieved snippets from past conversations. Use them to answer the question concisely and factually. If the retrieved context does not contain enough information, reply with "I don't know."""


def _reader_messages(question: str, retrievals: list[dict]) -> list[dict]:
    """Format retrievals as numbered context blocks."""
    blocks = []
    for r in retrievals:
        sid = r.get("session_id") or "unknown"
        blocks.append(f"[snippet {r['rank']} | session {sid}] {r['content']}")
    user_prompt = (
        "Retrieved snippets from past conversations:\n\n"
        + "\n".join(blocks)
        + f"\n\nQuestion: {question}\nAnswer concisely. If you cannot determine "
        "the answer from the snippets, reply exactly with: I don't know."
    )
    return [
        {"role": "system", "content": READER_SYSTEM},
        {"role": "user", "content": user_prompt},
    ]


# ---- Judge prompt (LLM-as-judge, ~LongMemEval official semantics) --------


JUDGE_SYSTEM = """You are an objective grader for a memory benchmark. You will be shown a question, the gold answer, and the model's hypothesis. Decide if the hypothesis answers the question correctly given the gold. Respond with exactly one of: CORRECT, INCORRECT. Do not explain."""


def _judge_messages(question: str, gold: Any, hypothesis: str) -> list[dict]:
    gold_str = str(gold)
    user_prompt = (
        f"Question: {question}\n"
        f"Gold answer: {gold_str}\n"
        f"Model hypothesis: {hypothesis}\n\n"
        "Verdict (CORRECT or INCORRECT):"
    )
    return [
        {"role": "system", "content": JUDGE_SYSTEM},
        {"role": "user", "content": user_prompt},
    ]


def _parse_verdict(s: str) -> bool | None:
    s_upper = s.strip().upper()
    if "CORRECT" in s_upper and "INCORRECT" not in s_upper:
        return True
    if "INCORRECT" in s_upper:
        return False
    return None


# ---- Pipeline driver -----------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", required=True, type=Path)
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--reader", default="gpt-4o-mini")
    ap.add_argument("--judge", default="gpt-4o-mini")
    ap.add_argument("--out", default=Path("/tmp/wg_e2e"), type=Path)
    ap.add_argument("--limit", type=int, default=0, help="0 = all")
    ap.add_argument("--reader-max-tokens", type=int, default=200)
    ap.add_argument("--judge-max-tokens", type=int, default=10)
    args = ap.parse_args()

    api_key = os.environ.get("OPENAI_API_KEY")
    if not api_key:
        print("error: OPENAI_API_KEY not set", file=sys.stderr)
        return 2

    args.out.mkdir(parents=True, exist_ok=True)
    hyp_path = args.out / f"hypotheses_{args.reader}.jsonl"
    judg_path = args.out / f"judgements_{args.reader}_judge_{args.judge}.jsonl"

    # Load retrievals + gold-answer index.
    retrievals_rows = [json.loads(line) for line in open(args.retrievals)]
    gold_index = {q["question_id"]: q["answer"] for q in json.load(open(args.gold))}
    if args.limit:
        retrievals_rows = retrievals_rows[: args.limit]

    print(
        f"E2E pipeline: {len(retrievals_rows)} questions"
        f" | reader={args.reader} | judge={args.judge}"
    )

    # ---- Stage A: reader (skip rows already in hypothesis file) ----------
    done_ids: set[str] = set()
    if hyp_path.exists():
        with open(hyp_path) as f:
            for line in f:
                row = json.loads(line)
                done_ids.add(row["question_id"])
        print(f"  reader: resuming, {len(done_ids)} hypotheses already on disk")

    todo = [r for r in retrievals_rows if r["question_id"] not in done_ids]
    print(f"  reader: {len(todo)} new questions to call ({args.reader})")
    with open(hyp_path, "a") as fout:
        for i, row in enumerate(todo, 1):
            messages = _reader_messages(row["question"], row["retrievals"])
            try:
                resp = _call_openai(
                    api_key, args.reader, messages, args.reader_max_tokens
                )
                hypothesis = _extract_text(resp)
            except Exception as e:
                print(f"  ! reader failed on {row['question_id']}: {e}", file=sys.stderr)
                hypothesis = ""
            fout.write(
                json.dumps(
                    {
                        "question_id": row["question_id"],
                        "question_type": row["question_type"],
                        "question": row["question"],
                        "hypothesis": hypothesis,
                        "first_evidence_rank": row.get("first_evidence_rank"),
                    }
                )
                + "\n"
            )
            fout.flush()
            if i % 10 == 0 or i == len(todo):
                print(f"    [{i:>4}/{len(todo)}] {row['question_id']}", file=sys.stderr)

    # ---- Stage B: judge --------------------------------------------------
    judged_ids: set[str] = set()
    if judg_path.exists():
        with open(judg_path) as f:
            for line in f:
                judged_ids.add(json.loads(line)["question_id"])
        print(f"  judge:  resuming, {len(judged_ids)} verdicts already on disk")

    hypotheses = [json.loads(line) for line in open(hyp_path)]
    todo_j = [h for h in hypotheses if h["question_id"] not in judged_ids]
    print(f"  judge:  {len(todo_j)} new judgements to call ({args.judge})")
    with open(judg_path, "a") as fout:
        for i, hyp in enumerate(todo_j, 1):
            qid = hyp["question_id"]
            gold = gold_index.get(qid, "")
            messages = _judge_messages(hyp["question"], gold, hyp["hypothesis"])
            try:
                resp = _call_openai(
                    api_key, args.judge, messages, args.judge_max_tokens
                )
                raw = _extract_text(resp)
            except Exception as e:
                print(f"  ! judge failed on {qid}: {e}", file=sys.stderr)
                raw = ""
            verdict = _parse_verdict(raw)
            fout.write(
                json.dumps(
                    {
                        "question_id": qid,
                        "question_type": hyp["question_type"],
                        "verdict_raw": raw,
                        "correct": verdict,
                        "first_evidence_rank": hyp.get("first_evidence_rank"),
                    }
                )
                + "\n"
            )
            fout.flush()
            if i % 10 == 0 or i == len(todo_j):
                print(f"    [{i:>4}/{len(todo_j)}] {qid}", file=sys.stderr)

    # ---- Stage C: aggregate ---------------------------------------------
    judgements = [json.loads(line) for line in open(judg_path)]
    by_type: dict[str, list[bool | None]] = {}
    overall: list[bool | None] = []
    for j in judgements:
        overall.append(j["correct"])
        by_type.setdefault(j["question_type"], []).append(j["correct"])

    def _acc(rows: list[bool | None]) -> tuple[int, int, int]:
        ok = sum(1 for r in rows if r is True)
        bad = sum(1 for r in rows if r is False)
        unk = sum(1 for r in rows if r is None)
        return ok, bad, unk

    ok, bad, unk = _acc(overall)
    total = ok + bad + unk
    print()
    print(f"E2E result: reader={args.reader}, judge={args.judge}")
    print(f"  total questions: {total}")
    print(f"  CORRECT:    {ok:>4}  ({ok/total:.3f})")
    print(f"  INCORRECT:  {bad:>4}  ({bad/total:.3f})")
    print(f"  unparseable {unk:>4}  ({unk/total:.3f})")
    print()
    print("  By question_type:")
    for qt in sorted(by_type):
        ok2, bad2, unk2 = _acc(by_type[qt])
        n = ok2 + bad2 + unk2
        print(f"    {qt:30}  {ok2/n:.3f}  ({ok2}/{n})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
