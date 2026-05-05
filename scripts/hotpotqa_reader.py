#!/usr/bin/env python3
"""End-to-end reader+judge harness for HotpotQA distractor retrievals.

Takes the JSONL output of `target/release/hotpotqa --emit-retrievals`
and grades reader answers against gold using both:
  * Exact-match (paper standard)
  * LLM-judge for semantic equivalence (yes/no, entity rephrasing)

Usage:
  python3 scripts/hotpotqa_reader.py \
      --retrievals /tmp/wg_hotpotqa_full.jsonl \
      --reader MiniMax-M2.7-highspeed --judge MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 --reader-api-key-env MINIMAX_API_KEY \
      --judge-base-url https://api.minimax.io/v1 --judge-api-key-env MINIMAX_API_KEY \
      --workers 6 --limit 1000 --out /tmp/wg_hotpotqa_eval
"""
from __future__ import annotations

import argparse
import json
import os
import re
import string
import sys
import threading
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from longmemeval_omega_style import _call_openai, _extract_text  # noqa: E402


READER_PROMPT = """\
You are answering a multi-hop question that requires combining \
information from TWO Wikipedia paragraphs. Read every retrieved \
sentence — the answer is rarely in just one snippet; you usually \
have to bridge a fact across two of them.

Question: {question}

Retrieved sentences (top {n}, each from one Wikipedia paragraph):
{snippets}

Output ONLY the answer — a short span (entity name, year, "yes"/"no"). \
Do not explain. If the answer is "yes" or "no", reply exactly that. \
For entity answers, give the canonical name without quotes or extra \
punctuation. If the snippets clearly don't contain the answer, reply \
"unanswerable" (only as a last resort)."""


JUDGE_PROMPT = """\
Question: {question}
Gold answer: {gold}
Model answer: {hypothesis}

Is the model answer factually correct? Treat semantic equivalence as \
correct (e.g. "Steven Spielberg" matches "Spielberg"; "yes" matches \
"Yes."; "1972" matches "in 1972"). Substring matches that change the \
meaning are INCORRECT.

Reply with exactly one word: CORRECT or INCORRECT."""


# Standard HotpotQA F1/EM normalization (from official eval script).
def normalize_answer(s: str) -> str:
    s = str(s).lower()
    s = re.sub(r"\b(a|an|the)\b", " ", s)
    s = "".join(ch for ch in s if ch not in set(string.punctuation))
    s = " ".join(s.split())
    return s


def f1_score(pred: str, gold: str) -> float:
    pred_t = normalize_answer(pred).split()
    gold_t = normalize_answer(gold).split()
    if not pred_t or not gold_t:
        return float(pred_t == gold_t)
    common = set(pred_t) & set(gold_t)
    if not common:
        return 0.0
    num_same = sum(min(pred_t.count(t), gold_t.count(t)) for t in common)
    p = num_same / len(pred_t)
    r = num_same / len(gold_t)
    return 2 * p * r / (p + r) if (p + r) > 0 else 0.0


def em_score(pred: str, gold: str) -> int:
    return int(normalize_answer(pred) == normalize_answer(gold))


def format_snippets(retrievals, max_chars=400):
    out = []
    for h in retrievals:
        title = h.get("paragraph_title", "")
        sent_idx = h.get("sentence_idx", "?")
        snippet = h["content"][:max_chars].strip()
        out.append(f"[{title} | sent {sent_idx}]\n{snippet}")
    return "\n\n".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", required=True, type=Path)
    ap.add_argument("--reader", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--judge", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--reader-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--reader-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--judge-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--judge-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    reader_key = os.environ.get(args.reader_api_key_env, "")
    judge_key = os.environ.get(args.judge_api_key_env, "")
    if not reader_key or not judge_key:
        print("error: API keys not set", file=sys.stderr)
        return 2

    args.out.mkdir(parents=True, exist_ok=True)
    hyp_path = args.out / f"hypotheses_{args.reader}.jsonl"
    judg_path = args.out / f"judgements_{args.reader}_judge_{args.judge}.jsonl"

    rows = [json.loads(l) for l in open(args.retrievals)]
    if args.limit:
        rows = rows[:args.limit]
    print(f"Reader: {len(rows)} q, model={args.reader}, workers={args.workers}")

    done = set()
    if hyp_path.exists():
        for line in open(hyp_path):
            done.add(json.loads(line)["question_id"])
    todo = [r for r in rows if r["question_id"] not in done]

    def _read_one(row):
        snippets = format_snippets(row.get("retrievals", []))
        prompt = READER_PROMPT.format(
            question=row["question"],
            n=len(row.get("retrievals", [])),
            snippets=snippets,
        )
        try:
            resp = _call_openai(
                reader_key, args.reader,
                [{"role": "user", "content": prompt}],
                4096, args.reader_base_url, temperature=0.0,
            )
            hypothesis = _extract_text(resp)
        except Exception as e:
            print(f"  ! reader fail q={row['question_id']}: {e}", file=sys.stderr)
            hypothesis = ""
        # Strip </think> if reasoning model leaked it through
        if "</think>" in hypothesis:
            hypothesis = hypothesis.split("</think>", 1)[-1].strip()
        return {
            "question_id": row["question_id"],
            "question": row["question"],
            "qtype": row["qtype"],
            "gold_answer": row["gold_answer"],
            "hypothesis": hypothesis,
        }

    write_lock = threading.Lock()
    with open(hyp_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(_read_one, r): r for r in todo}
        i = 0
        for fut in as_completed(futs):
            i += 1
            r = fut.result()
            with write_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 100 == 0 or i == len(todo):
                print(f"    [{i:>4}/{len(todo)}] reader", file=sys.stderr)

    judged = set()
    if judg_path.exists():
        for line in open(judg_path):
            judged.add(json.loads(line)["question_id"])
    hyps = [json.loads(l) for l in open(hyp_path)]
    todo_j = [h for h in hyps if h["question_id"] not in judged]
    print(f"  judge: {len(todo_j)} new")

    def _judge_one(h):
        gold = str(h["gold_answer"])
        em = em_score(h["hypothesis"], gold)
        f1 = f1_score(h["hypothesis"], gold)
        # LLM judge for semantic equivalence (yes/no rephrases, "the X" → "X")
        prompt = JUDGE_PROMPT.format(
            question=h["question"],
            gold=gold,
            hypothesis=h["hypothesis"],
        )
        try:
            resp = _call_openai(
                judge_key, args.judge,
                [{"role": "user", "content": prompt}],
                4096, args.judge_base_url, temperature=0.0,
            )
            raw = _extract_text(resp)
        except Exception as e:
            print(f"  ! judge fail q={h['question_id']}: {e}", file=sys.stderr)
            raw = ""
        text = raw.split("</think>", 1)[-1].strip().upper()
        if not text:
            text = raw.upper()
        has_correct = bool(re.search(r"(?<!IN)CORRECT", text))
        has_incorrect = "INCORRECT" in text
        if has_correct and has_incorrect:
            llm_correct = text.rfind("CORRECT") > text.rfind("INCORRECT") + 2
        else:
            llm_correct = has_correct and not has_incorrect
        return {
            "question_id": h["question_id"],
            "qtype": h["qtype"],
            "em": em,
            "f1": f1,
            "llm_correct": llm_correct,
        }

    judge_lock = threading.Lock()
    with open(judg_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(_judge_one, h): h for h in todo_j}
        i = 0
        for fut in as_completed(futs):
            i += 1
            r = fut.result()
            with judge_lock:
                fout.write(json.dumps(r) + "\n")
                fout.flush()
            if i % 100 == 0 or i == len(todo_j):
                print(f"    [{i:>4}/{len(todo_j)}] judge", file=sys.stderr)

    js = [json.loads(l) for l in open(judg_path)]
    n = len(js)
    em_avg = sum(j["em"] for j in js) / n * 100
    f1_avg = sum(j["f1"] for j in js) / n * 100
    llm_avg = sum(int(j["llm_correct"]) for j in js) / n * 100
    print()
    print(f"Result: reader={args.reader}, judge={args.judge}, total={n}")
    print(f"  EM:          {em_avg:.2f}%")
    print(f"  F1:          {f1_avg:.2f}%")
    print(f"  LLM-judge:   {llm_avg:.2f}%")
    by_t = defaultdict(lambda: [0,0,0,0])
    for j in js:
        d = by_t[j["qtype"]]
        d[0] += j["em"]; d[1] += j["f1"]; d[2] += int(j["llm_correct"]); d[3] += 1
    print("\n  By type (EM / F1 / LLM):")
    for qt in sorted(by_t):
        em, f1, ll, t = by_t[qt]
        print(f"    {qt:<14} EM {em/t*100:.1f}%  F1 {f1/t*100:.1f}%  LLM {ll/t*100:.1f}%  ({t})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
