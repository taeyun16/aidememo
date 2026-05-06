#!/usr/bin/env python3
"""LLM-judge re-grading for agent_eval_run.py output.

The keyword scorer in the runner is monolingual (English) and
under-counts Korean abstentions / paraphrased answers. This pass
asks an LLM (MiniMax) to grade each {prompt, gold_keywords,
hypothesis} triple semantically — bilingual aware, treats correct
abstention as CORRECT when gold expects abstention, etc.

Usage:
  set -a; source ~/.hermes/.env; set +a
  python3 scripts/agent_eval_judge.py \
      --transcripts /tmp/wg_agent_eval/claude.jsonl \
      --scenarios scripts/agent_eval_scenarios.json \
      --judge MiniMax-M2.7-highspeed \
      --judge-base-url https://api.minimax.io/v1 \
      --judge-api-key-env MINIMAX_API_KEY \
      --workers 6 \
      --out /tmp/wg_agent_eval/claude.judged.jsonl
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


JUDGE_PROMPT = """\
You are grading an agent's answer to a question about the wg \
(Wiki-Graph) codebase. The agent had access to the wiki via MCP \
tools.

Question: {prompt}

Expected answer cues (keywords / concepts that should appear, in \
ANY language): {gold_keywords}

Notes about correct answer: {notes}

Agent's hypothesis (may be in Korean or English):
{hypothesis}

Grade the hypothesis as one of:
* CORRECT — the answer captures the expected concept (semantic \
match), or — when the question expected abstention — the agent \
correctly says it can't answer / the data isn't present.
* PARTIAL — partially right, missing significant elements.
* INCORRECT — wrong answer or hallucinated content.

Reply with EXACTLY one word: CORRECT, PARTIAL, or INCORRECT."""


def grade_one(api_key, model, base_url, scenario, hyp_row):
    prompt = JUDGE_PROMPT.format(
        prompt=scenario["prompt"],
        gold_keywords=", ".join(scenario.get("gold_keywords", [])),
        notes=scenario.get("notes", ""),
        hypothesis=hyp_row["hypothesis"][:2000],
    )
    try:
        resp = _call_openai(
            api_key, model,
            [{"role": "user", "content": prompt}],
            4096, base_url, temperature=0.0,
        )
        raw = _extract_text(resp)
    except Exception as e:
        print(f"  ! judge fail {scenario['id']}: {e}", file=sys.stderr)
        return {"verdict": "ERROR", "raw": ""}
    text = raw.split("</think>", 1)[-1].strip().upper() if "</think>" in raw else raw.strip().upper()
    if not text:
        text = raw.upper()
    if "INCORRECT" in text:
        v = "INCORRECT"
    elif "PARTIAL" in text:
        v = "PARTIAL"
    elif "CORRECT" in text:
        v = "CORRECT"
    else:
        v = "UNKNOWN"
    return {"verdict": v, "raw": raw[:500]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--transcripts", required=True, type=Path)
    ap.add_argument("--scenarios", required=True, type=Path)
    ap.add_argument("--judge", default="MiniMax-M2.7-highspeed")
    ap.add_argument("--judge-base-url", default="https://api.minimax.io/v1")
    ap.add_argument("--judge-api-key-env", default="MINIMAX_API_KEY")
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    api_key = os.environ.get(args.judge_api_key_env, "")
    if not api_key:
        print("error: API key not set", file=sys.stderr)
        return 2

    scenarios = {s["id"]: s for s in json.loads(args.scenarios.read_text())["scenarios"]}
    transcripts = [json.loads(l) for l in open(args.transcripts)]
    print(f"Grading {len(transcripts)} transcripts via {args.judge}")

    args.out.parent.mkdir(parents=True, exist_ok=True)

    def _one(t):
        sc = scenarios.get(t["scenario_id"])
        if not sc:
            return {**t, "judge": {"verdict": "MISSING_SCENARIO", "raw": ""}}
        v = grade_one(api_key, args.judge, args.judge_base_url, sc, t)
        return {**t, "judge": v}

    write_lock = threading.Lock()
    rows = []
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(_one, t): t for t in transcripts}
        for fut in as_completed(futs):
            r = fut.result()
            with write_lock:
                rows.append(r)

    rows.sort(key=lambda r: r["scenario_id"])
    args.out.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\n")

    from collections import Counter
    verdicts = Counter(r["judge"]["verdict"] for r in rows)
    n = len(rows)
    correct = verdicts.get("CORRECT", 0)
    partial = verdicts.get("PARTIAL", 0)
    incorrect = verdicts.get("INCORRECT", 0)
    print()
    print(f"Result ({n} scenarios):")
    print(f"  CORRECT:    {correct}/{n}  ({correct/n:.1%})")
    print(f"  PARTIAL:    {partial}/{n}  ({partial/n:.1%})")
    print(f"  INCORRECT:  {incorrect}/{n}  ({incorrect/n:.1%})")
    print()
    print("By shape:")
    by_shape: dict = defaultdict(lambda: defaultdict(int))
    for r in rows:
        by_shape[r["shape"]][r["judge"]["verdict"]] += 1
    for shape in sorted(by_shape):
        d = by_shape[shape]
        total = sum(d.values())
        c = d.get("CORRECT", 0); p = d.get("PARTIAL", 0); i = d.get("INCORRECT", 0)
        print(f"  {shape:<24} C={c} P={p} I={i}  ({c}/{total} correct)")

    return 0


if __name__ == "__main__":
    sys.exit(main())
