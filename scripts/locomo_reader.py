#!/usr/bin/env python3
"""End-to-end reader+judge harness for LoCoMo retrievals.

LoCoMo (Maharana et al., ICLR 2024) grades long-conversational QA
with both EM/F1 (when a single-span gold answer exists) and an
LLM judge for semantic equivalence (paraphrase-tolerant). We compute
all three.

Cat 5 (open-domain adversarial) carries `adversarial_answer` instead
of `answer` — gold extraction handled bench-side; we just receive
`gold_answer` from the JSONL.

Usage:
  python3 scripts/locomo_reader.py \
      --retrievals /tmp/wg_locomo_full.jsonl \
      --reader MiniMax-M2.7-highspeed --judge MiniMax-M2.7-highspeed \
      --reader-base-url https://api.minimax.io/v1 --reader-api-key-env MINIMAX_API_KEY \
      --judge-base-url https://api.minimax.io/v1 --judge-api-key-env MINIMAX_API_KEY \
      --workers 6 --limit 0 --out /tmp/wg_locomo_eval
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
You are answering a question about a long, multi-session conversation \
between two friends. Each retrieved snippet is one turn from the \
conversation, tagged with its dialog id (Dn:k) and the speaker.

Question: {question}

Retrieved turns (top {n}):
{snippets}

Answer concisely. For factual questions ("when / who / where / how \
many") give just the value. For reasoning questions, give the answer \
in a short sentence quoting the relevant turns. If the snippets \
clearly do not contain the answer, reply "Not enough information." \
(only as a last resort — try to infer from what's given first)."""


JUDGE_PROMPT = """\
Question: {question}
Gold answer: {gold}
Model answer: {hypothesis}

Is the model answer factually correct? Treat semantic equivalence as \
correct (different wording, paraphrase, equivalent dates, equivalent \
numbers, equivalent entity references). For LoCoMo's open-domain \
category-5 questions the gold may be one of many acceptable \
phrasings — the model answer is correct if it captures the same \
underlying fact even when worded differently.

Reply with exactly one word: CORRECT or INCORRECT."""


def normalize_answer(s):
    s = str(s).lower()
    s = re.sub(r"\b(a|an|the)\b", " ", s)
    s = "".join(ch for ch in s if ch not in set(string.punctuation))
    s = " ".join(s.split())
    return s


def f1_score(pred, gold):
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


def em_score(pred, gold):
    return int(normalize_answer(pred) == normalize_answer(gold))


def format_snippets(retrievals, max_chars=400):
    out = []
    for h in retrievals:
        out.append(
            f"[{h.get('dia_id','?')} | session {h.get('session','?')} | "
            f"{h.get('speaker','?')}] {h['content'][:max_chars].strip()}"
        )
    return "\n".join(out)


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
    # Stable id from sample_id + qa_index
    for r in rows:
        r["question_id"] = f"{r['sample_id']}#{r['qa_index']}"
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
        if "</think>" in hypothesis:
            hypothesis = hypothesis.split("</think>", 1)[-1].strip()
        return {
            "question_id": row["question_id"],
            "question": row["question"],
            "category": row["category"],
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
        gold = h["gold_answer"]
        gold_str = json.dumps(gold) if not isinstance(gold, str) else gold
        em = em_score(h["hypothesis"], gold_str)
        f1 = f1_score(h["hypothesis"], gold_str)
        prompt = JUDGE_PROMPT.format(
            question=h["question"],
            gold=gold_str,
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
            "category": h["category"],
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
    by_c = defaultdict(lambda: [0, 0, 0, 0])
    for j in js:
        d = by_c[j["category"]]
        d[0] += j["em"]; d[1] += j["f1"]; d[2] += int(j["llm_correct"]); d[3] += 1
    print("\n  By category (EM / F1 / LLM):")
    for c in sorted(by_c):
        em, f1, ll, t = by_c[c]
        print(f"    cat {c} EM {em/t*100:.1f}%  F1 {f1/t*100:.1f}%  LLM {ll/t*100:.1f}%  ({t})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
