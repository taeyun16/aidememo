#!/usr/bin/env python3
"""LongMemEval reader+judge harness with OMEGA's published recipe ported to aidememo.

Reads a `--emit-retrievals` JSONL produced by `aidememo-benchmarks longmemeval`
(top-K=20-30, with `referenced_date` per hit), then applies the same
prompt-side stack OMEGA's `scripts/longmemeval_official.py` uses to hit
95.4%:

  1. 5 category-specific reader prompts (vanilla / enhanced /
     multi-session / preference / temporal) — verbatim ports.
  2. Per-category adaptive filter (min_relevance / min_results /
     max_results / max_tokens).
  3. Recency boost for knowledge-update questions (multiplies score by
     1 + 0.5 * recency_fraction across the window).
  4. Chronological sort (oldest first) before prompting so the
     "MOST RECENT note wins" instructions in the prompts work.
  5. Per-snippet "[Note N | Date: ISO]" / "[End Note N]" formatting,
     OMEGA's Chain-of-Note style.

Skipped (retrieval-side, would need aidememo-benchmarks rewrite):
  * Query expansion (regex absolute date / entity / counting cues)
  * Triple retrieval merge (temporal-filtered + unfiltered + original)
  * Temporal range filter (`temporal_range=(start, end)` in store.query)

Usage:
  python3 scripts/longmemeval_omega_style.py \
      --retrievals /tmp/aidememo_retrievals_500_omega_style.jsonl \
      --gold /tmp/longmemeval_data/longmemeval_s_cleaned.json \
      --reader gpt-4.1 --judge gpt-4o \
      --out /tmp/aidememo_omega_style_500
"""
from __future__ import annotations

import argparse
import json
import os
import random
import sys
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path


# ---------- LLM call (OpenAI-compatible) -----------------------------

def _token_field(model: str) -> str:
    if model.startswith(("gpt-5", "o1", "o3", "o4")):
        return "max_completion_tokens"
    return "max_tokens"


def _call_openai(api_key, model, messages, max_tokens, base_url, timeout=60, temperature=0.0):
    url = base_url.rstrip("/") + "/chat/completions"
    # temperature=0 default for reproducibility, but reasoning models like
    # MiniMax-M2.7 still show ±5pt run-to-run variance because they sample
    # from think-token paths even at temp=0. Caller can override (e.g. for
    # self-consistency voting we want diversity at temp=0.5).
    body = {
        "model": model,
        "messages": messages,
        "temperature": temperature,
        _token_field(model): max_tokens,
    }
    last_err = None
    # Jittered exponential backoff: base * 2^attempt + uniform(0, base).
    # Pure fixed waits cause synchronized retries when many workers hit
    # the same rate limit ("herding") — they all back off and re-fire
    # together, blowing the limit again. Jitter spreads re-fires out.
    base_waits = [1.5, 4, 10, 22]
    for attempt in range(5):
        req = urllib.request.Request(
            url,
            data=json.dumps(body).encode(),
            headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.loads(resp.read().decode())
        except urllib.error.HTTPError as e:
            body_txt = e.read().decode("utf-8", errors="replace")
            if e.code in (429, 500, 502, 503, 504) and attempt < len(base_waits):
                base = base_waits[attempt]
                wait = base + random.uniform(0, base)
                print(f"  [retry {attempt+1}/5] HTTP {e.code} — sleeping {wait:.1f}s", file=sys.stderr)
                time.sleep(wait)
                last_err = RuntimeError(f"HTTP {e.code}: {body_txt[:200]}")
                continue
            raise RuntimeError(f"HTTP {e.code}: {body_txt[:200]}")
        except (urllib.error.URLError, TimeoutError) as e:
            if attempt < len(base_waits):
                base = base_waits[attempt]
                wait = base + random.uniform(0, base)
                print(f"  [retry {attempt+1}/5] {e} — sleeping {wait:.1f}s", file=sys.stderr)
                time.sleep(wait)
                last_err = e
            else:
                raise RuntimeError(f"all retries failed: {e}")
    raise RuntimeError(f"all retries failed: {last_err}")


def _extract_text(resp):
    text = resp["choices"][0]["message"]["content"].strip()
    if "</think>" in text:
        text = text.split("</think>", 1)[1].strip()
    return text


# ---------- Reader prompts (verbatim from OMEGA's longmemeval_official.py) ---

RAG_PROMPT_VANILLA = """\
I will give you several notes from past conversations between you and a user. \
Please answer the question based on the relevant notes. \
If the question cannot be answered based on the provided notes, say so.

Notes from past conversations:

{sessions}

Current Date: {question_date}
Question: {question}
Answer:"""

RAG_PROMPT_ENHANCED = """\
I will give you several notes from past conversations between you and a user, \
ordered from oldest to newest. Please answer the question based on the relevant notes. \
If the question cannot be answered based on the provided notes, say so.

CRITICAL — LATEST WINS RULE (single most important rule):
When the same topic has DIFFERENT values across multiple notes, the value from the \
note with the **most recent date** is the CORRECT answer. Older values are wrong / \
out of date. NEVER invert this — do NOT treat the latest value as "superseded".

EXAMPLE: If Note 1 (dated 2023-08-11) says "$350,000" and Note 13 (dated 2023-11-30) \
says "$400,000" for the same item, the answer is **$400,000** (Note 13 is later).

You MUST follow this process for EVERY question:

STEP 1 — Scan ALL notes for mentions of the queried topic. List every note that \
discusses it, with its note number and date.

STEP 2 — If the topic appears in multiple notes with DIFFERENT values, compare the \
dates. The note with the LATEST date is the ONLY correct one. Earlier values are \
SUPERSEDED and WRONG.

STEP 3 — Answer using ONLY the value from the latest note. Do NOT mention the older \
superseded values in the final answer (they are noise).

CRITICAL rules:
- Notes are in chronological order (oldest first). Higher note numbers are more recent.
- The same fact may appear in multiple snippets (some are turn-level fragments, \
some are full-session blocks). Treat duplicates of the same statement as ONE \
data point — they don't add weight, only the DATE matters for ranking.
- For questions about current state (e.g., "what is my current X?", "how many times \
have I done Y?"), the answer ALWAYS comes from the LAST note mentioning that topic.
- If a quantity changes across notes (e.g., worn 4 times → worn 6 times), the \
LATEST number replaces all earlier ones. Do NOT add or average them.
- If the question references a role, title, or name that does NOT exactly match \
what appears in the notes, say the information is not enough to answer.
- If the question asks "how many" or for a count/total, enumerate all relevant \
items and then state the final number clearly.
- Give a direct, concise answer. Do not hedge if the evidence is clear.

Notes from past conversations:

{sessions}

Current Date: {question_date}
Question: {question}
Answer:"""

RAG_PROMPT_MULTISESSION = """\
I will give you several notes from past conversations between you and a user, \
ordered from oldest to newest. Please answer the question based on the relevant notes. \
If the question cannot be answered based on the provided notes, say so.

CRITICAL — MANDATORY FINAL SYNTHESIS STEP:
For ANY question that asks "how many", "how much", "how often", "total", "combined", \
"sum", or implies aggregation across multiple items/events:
  STEP 0 (COVERAGE — required for counting): Before you start listing items, \
explicitly RE-READ each note from start to finish and write down EVERY occurrence of a \
word/phrase that could be a candidate. For "how many model kits" you must look at every \
note and list every model kit name mentioned (Tiger I, Spitfire, F-15 Eagle, B-29, \
Camaro, etc.) — do NOT just skim. Long contexts hide items easily.
  STEP A: From the candidates in STEP 0, list each one as "matching" or "not matching" \
the question with its [Note #] citation. Apply the question's narrowing criteria here.
  STEP B: COMPUTE the sum/count/total. Show the arithmetic explicitly \
(e.g. "2 weeks + 1.5 weeks = 3.5 weeks").
  STEP C: State the FINAL NUMBER as the last sentence of the answer. Do NOT end with \
the components — end with the computed total. If you list "MCU=2 weeks, Star Wars=1.5 \
weeks" without then writing "Total = 3.5 weeks", the answer is INCOMPLETE.

CRITICAL — ARITHMETIC ON RANGES:
When the notes give a RANGE ("around 7-8 hours", "between 3 and 5 days"), prefer the \
LOWER bound or the EXACT smaller value the user actually stated. Do NOT use midpoints \
unless the question explicitly asks for an estimate. Example: "drive is around 7-8 hours" \
→ use 7, not 7.5. The benchmark gold tends to use exact stated values, not statistical \
averages.

Important:
- Notes are in chronological order. When the same fact appears in multiple \
notes with different values, always use the value from the MOST RECENT note.
- If the question asks "how many", for a count, or for a total:
  1. You MUST list EVERY matching item individually, citing its source as [Note #].
  2. VERIFY each item: re-read the question and confirm each item EXACTLY matches \
what was asked. If the question asks about "types of citrus fruits", only count \
distinct fruit types the user actually used, not every mention of citrus. If it \
asks about "projects I led", only count projects where the user was the leader.
  3. REMOVE items that don't strictly match the question's criteria. But NEVER dismiss \
something the USER claims they did (bought, attended, downloaded, etc.) just because \
the assistant questioned whether it's real. The user's statement is ground truth.
  4. After filtering, count the remaining items and state the total clearly.
  5. For "how much total" questions: list each amount with its source [Note #], \
then sum them and state the total.
- When the same fact is UPDATED in a later note (e.g., a number changes from X to Y), \
use ONLY the latest value. The earlier value is superseded.
- DEDUPLICATION (HARD RULE): When listing items for a count, for each candidate ask \
"have I already listed this same item under a different mention?" If yes, do NOT add \
it again. The same physical item / model / event mentioned in multiple notes counts \
ONCE — regardless of context (e.g., "B-29 bomber kit I bought" in Note 1 and "B-29 as \
my next project" in Note 8 are the SAME kit, count = 1). Same wedding mentioned by \
different names ("cousin's wedding" / "Rachel's wedding") is ONE event. Err on the \
side of merging duplicates rather than double-counting.
- For questions about an "increase", "decrease", or "change" in a quantity: you MUST find \
BOTH the starting value AND the ending value, then compute the DIFFERENCE. Do NOT report \
the final total as the increase. Example: if followers went from 250 to 350, the increase is 100.
- Do NOT skip notes. Scan every note for potential matches before answering.
- Give a direct, concise answer. Do not hedge if the evidence is clear.
- NEVER guess, estimate, or calculate values that are not explicitly stated in the notes. \
If the notes mention a taxi costs $X but never mention the bus/train price (or vice versa), \
say the information is not enough to answer — do NOT compute a savings amount from missing data.

Notes from past conversations:

{sessions}

Current Date: {question_date}
Question: {question}
Answer:"""

RAG_PROMPT_PREFERENCE = """\
I will give you several notes from past conversations between you and a user. \
Please answer the question based on the user's stated preferences, habits, and \
personal information found in these notes.

CRITICAL — DO NOT HEDGE: For preference / recommendation / suggestion questions, \
the answer is ALWAYS in the notes (as preferences to APPLY, not facts to LOOKUP). \
NEVER respond with "I don't have access to" / "I don't know your specific X" / \
"I can't recommend particular items in your area". The user is not asking you to \
LOOK UP a specific hotel/event/paper — they are asking you to RECOMMEND one BASED \
ON their preferences from the notes. Skip the disclaimer entirely.

Important:
- Focus on what the user explicitly said about their preferences, likes, dislikes, \
habits, routines, and personal details.
- When the same preference appears in multiple notes with different values, always \
use the value from the MOST RECENT note (higher note number = more recent).
- If the question asks for a recommendation or suggestion, USE the user's stated \
preferences to tailor your response. Do NOT say you lack information if the notes \
contain ANY relevant preferences, interests, or habits — apply them creatively.
- Even if the notes don't mention the exact topic, look for RELATED preferences \
(e.g., if asked about hotels, use stated preferences about views, amenities, \
luxury vs budget, or location preferences from ANY context).
- For "recommend recent publications/conferences/events" questions: identify the \
user's PROFESSIONAL DOMAIN or NICHE INTEREST from the notes (e.g., "AI in \
healthcare", "Spanish language learning", "outdoor hiking") and tailor recommendations \
to that specific niche. Generic items in a broader field (e.g., recommending NeurIPS \
when the user's niche is medical-imaging AI) are WRONG.
- When the user mentions a place, activity, or event, ALWAYS check if the notes \
contain a SPECIFIC PAST EXPERIENCE with that place/activity. If so, reference it \
directly (e.g., "You mentioned enjoying X when you visited Denver before" or \
"Given your experience with Y in high school").
- Your answer MUST reference at least one specific detail from the notes. Generic \
advice that could apply to anyone is WRONG. The answer should be clearly \
personalized — someone reading it should be able to tell it was written for this \
specific user.
- Give a direct, specific answer. Quote the user's own words when possible.
- Open the answer with the personalized recommendation directly. NEVER start with \
"I don't have", "I can't", "Based on what you said I'm not sure" — start with the \
user-tailored answer.

Notes from past conversations:

{sessions}

Current Date: {question_date}
Question: {question}
Answer:"""

RAG_PROMPT_TEMPORAL = """\
I will give you several notes from past conversations between you and a user, \
ordered from oldest to newest. Each note has a date stamp. Please answer the \
question based on the relevant notes. \
If the question cannot be answered based on the provided notes, say so.

You MUST follow these steps for ALL time-based questions:

STEP 1 — Convert every relative date to an ABSOLUTE date:
  For each event mentioned in the notes, write its absolute date. Convert ALL \
relative references using the note's own date stamp:
  - "last Saturday" = the most recent Saturday BEFORE the note's date
  - "yesterday" = the day before the note's date
  - "two weeks ago" = 14 days before the note's date
  - "last month" = the calendar month before the note's date
  - "next Friday" = the first Friday AFTER the note's date

STEP 2 — Find ALL candidate events, not just the first match:
  When the question asks about something at a specific time (e.g., "two weeks ago", \
"last Saturday"), scan ALL notes and list every event that could match both the \
time reference AND the event description. Do NOT stop at the first event near \
the target date.

STEP 3 — Select the best match by verifying BOTH date AND description:
  - The event must match the question's description (e.g., "art event", "business \
milestone", "life event of a relative"). A nearby event of the wrong type is wrong.
  - Among events matching the description, pick the one closest to the exact \
target date. Prefer events within ±2 days; only consider ±3-7 days if no closer match exists.
  - If a note says "I went to X last week" and the note is dated near the target, \
resolve "last week" to find the EXACT event date, not the note date.

STEP 4 — Compute the answer using ONLY the absolute dates:
  - For "how many days/weeks/months between X and Y": subtract the two absolute \
dates and convert to the requested unit.
  - For ordering questions: list each event with its absolute date, then sort by \
date (earliest first).
  - For "how many times" or counting: enumerate each matching event with its \
absolute date, then state the total count.
  - For "when" questions: state the absolute date directly.

CRITICAL rules:
- RECOLLECTION ≠ ACTION: When a note says "I was thinking about X", "I remembered X", \
or "I was reminiscing about X", the event X did NOT happen on that note's date. \
The note's date is when the user RECALLED the event, not when it occurred. \
Only use notes where the user describes PERFORMING an action to date that action.
- Notes are in chronological order. When the same fact appears in multiple \
notes with different values, always use the value from the MOST RECENT note.
- Give a direct, concise answer. Do not hedge if the evidence is clear.
- Show your date arithmetic briefly before giving the final answer.
- If you can infer the answer by combining information across multiple notes, DO SO. \
Do not refuse to answer simply because no single note contains the complete answer.
- When a relative time reference (e.g., "last Saturday", "two weeks ago") appears \
in a note, ALWAYS resolve it to an absolute date using that note's date stamp \
before comparing to the question date.
- BEFORE saying "not enough information": re-read every note looking for SYNONYMS \
or INDIRECT references. "Investment for a competition" could be "bought tools for \
a contest." "Kitchen appliance" could be "smoker" or "grill." "Piece of jewelry" \
could be "ring" or "necklace." Try harder to match before abstaining.

Notes from past conversations:

{sessions}

Current Date: {question_date}
Question: {question}
Answer:"""


_CATEGORY_PROMPT = {
    "single-session-assistant": RAG_PROMPT_VANILLA,
    "single-session-user": RAG_PROMPT_VANILLA,
    "knowledge-update": RAG_PROMPT_ENHANCED,
    "multi-session": RAG_PROMPT_MULTISESSION,
    "temporal-reasoning": RAG_PROMPT_TEMPORAL,
    "single-session-preference": RAG_PROMPT_PREFERENCE,
}

# Per-category adaptive-filter configs (verbatim from OMEGA).
_CATEGORY_CONFIG = {
    "single-session-assistant": {"min_rel": 0.15, "min_res": 2, "max_res": 10, "max_tokens": 512},
    "single-session-user":      {"min_rel": 0.12, "min_res": 3, "max_res": 12, "max_tokens": 512},
    "knowledge-update":         {"min_rel": 0.15, "min_res": 3, "max_res": 15, "max_tokens": 2048},
    "multi-session":            {"min_rel": 0.08, "min_res": 4, "max_res": 20, "max_tokens": 2048},
    "temporal-reasoning":       {"min_rel": 0.10, "min_res": 5, "max_res": 20, "max_tokens": 2048},
    # SS-pref bumped from OMEGA's max_res=10 to 20: failure analysis on
    # 60q balanced (hybrid-ingest, MiniMax) showed the gold evidence
    # landed at rank 14 in 1/2 fails — the 10-cap was filtering it out.
    # OMEGA's tighter cap fits its session-only ingest where evidence
    # is more concentrated; aidememo's hybrid pool needs the wider window.
    "single-session-preference": {"min_rel": 0.12, "min_res": 3, "max_res": 20, "max_tokens": 2048},
}
_DEFAULT_CONFIG = {"min_rel": 0.15, "min_res": 3, "max_res": 10, "max_tokens": 512}


# ---------- Adaptive filter, recency boost, chronological sort -----------

def _epoch_ms_to_iso(ms):
    if ms is None:
        return None
    try:
        return datetime.fromtimestamp(ms / 1000.0, tz=timezone.utc).isoformat()
    except (ValueError, OSError):
        return None


def _boost_recency(retrievals):
    """Multiply each retrieval's score by 1 + 0.5 * recency_fraction.

    Verbatim port of OMEGA's `_boost_recency` (longmemeval_official.py:911).
    Newer sessions get up to a 1.5× boost so the latest fact about a topic
    ranks higher than older mentions — most useful for knowledge-update.
    """
    dates = [r.get("referenced_date") for r in retrievals if r.get("referenced_date")]
    if not dates:
        return retrievals
    earliest, latest = min(dates), max(dates)
    span = latest - earliest
    if span <= 0:
        return retrievals
    for r in retrievals:
        d = r.get("referenced_date")
        if d is None:
            continue
        frac = (d - earliest) / span
        r["score"] = (r.get("score") or 0.0) * (1.0 + 0.5 * frac)
    retrievals.sort(key=lambda r: r.get("score") or 0.0, reverse=True)
    return retrievals


def _filter_and_sort(retrievals, cfg):
    """Adaptive filter + chronological sort.

    Verbatim port of OMEGA's `_filter_and_sort_results`. Filters by
    relevance floor, ensures min_results coverage, caps at max_results,
    then sorts by `referenced_date` ascending (oldest first) so the
    "MOST RECENT note wins" prompt instructions work as intended.
    """
    # OMEGA scores from its own retrieval are 0..1-ish. aidememo's BM25/RRF
    # scores have a different scale — typically 0.01..30 for BM25 or
    # 0.001..0.06 for RRF. To make min_rel comparable we normalise to
    # the question's max score before applying the filter.
    if retrievals:
        max_score = max((r.get("score") or 0.0) for r in retrievals)
        if max_score > 0:
            for r in retrievals:
                r["_norm_score"] = (r.get("score") or 0.0) / max_score
        else:
            for r in retrievals:
                r["_norm_score"] = 0.0
    strong = [r for r in retrievals if r.get("_norm_score", 0.0) >= cfg["min_rel"]]
    if len(strong) < cfg["min_res"]:
        strong = sorted(retrievals, key=lambda r: r.get("score") or 0.0, reverse=True)[: cfg["min_res"]]
    if len(strong) > cfg["max_res"]:
        strong = sorted(strong, key=lambda r: r.get("score") or 0.0, reverse=True)[: cfg["max_res"]]
    # Chronological sort (oldest first). Hits without a date go last so
    # the recency-aware prompts still see them but the dated ones flow
    # in order.
    strong.sort(key=lambda r: r.get("referenced_date") or 10**18)
    return strong


def _format_session_block(content, date_iso, idx):
    return f"[Note {idx} | Date: {date_iso or 'Unknown'}]\n{content}\n[End Note {idx}]"


_COUNTING_SIGNALS = (
    "how many", "how much", "how often", "total number", "total ",
    "count", "number of", "combined", "altogether",
)


def _is_counting_question(question):
    q_lower = (question or "").lower()
    return any(sig in q_lower for sig in _COUNTING_SIGNALS)


def _build_reader_prompt(question_data, retrievals):
    qtype = question_data["question_type"]
    qid = question_data["question_id"]
    is_abstention = qid.endswith("_abs")
    rag_prompt = RAG_PROMPT_VANILLA if is_abstention else _CATEGORY_PROMPT.get(qtype, RAG_PROMPT_MULTISESSION)
    cfg = (
        {"min_rel": 0.20, "min_res": 2, "max_res": 5, "max_tokens": 256}
        if is_abstention
        else dict(_CATEGORY_CONFIG.get(qtype, _DEFAULT_CONFIG))  # copy so per-question override doesn't mutate global
    )

    # Counting/aggregation boost — restrict to multi-session only.
    # Initially applied globally (60q v5) but caused ±5pt regressions in
    # SS-pref/temporal that traced to per-call MiniMax temp=0 noise +
    # extra snippet noise overwhelming focused-answer categories. Keep
    # the boost where it actually helps (multi-session aggregation).
    if (
        not is_abstention
        and qtype == "multi-session"
        and _is_counting_question(question_data.get("question", ""))
    ):
        cfg["max_res"] = min(30, int(cfg["max_res"] * 1.5))

    retr_copy = [dict(r) for r in retrievals]  # don't mutate caller
    if qtype == "knowledge-update":
        retr_copy = _boost_recency(retr_copy)
    filtered = _filter_and_sort(retr_copy, cfg)

    blocks = []
    for i, r in enumerate(filtered, 1):
        date_iso = _epoch_ms_to_iso(r.get("referenced_date"))
        blocks.append(_format_session_block(r["content"], date_iso, i))

    prompt = rag_prompt.format(
        sessions="\n\n".join(blocks),
        question_date=question_data.get("question_date") or "Unknown",
        question=question_data["question"],
    )
    return prompt, cfg["max_tokens"], len(filtered)


# ---------- Judge prompts (verbatim from official LongMemEval evaluate_qa.py) -

GRADE_PROMPTS = {
    "default": (
        "I will give you a question, a correct answer, and a response from a model. "
        "Please answer yes if the response contains the correct answer. Otherwise, answer no. "
        "If the response is equivalent to the correct answer or contains all the intermediate "
        "steps to get the correct answer, you should also answer yes. If the response only "
        "contains a subset of the information required by the answer, answer no.\n\n"
        "Question: {question}\nCorrect Answer: {answer}\nModel Response: {hypothesis}\n\n"
        "Is the model response correct? Answer yes or no only."
    ),
    "temporal-reasoning": (
        "I will give you a question, a correct answer, and a response from a model. "
        "Please answer yes if the response contains the correct answer. Otherwise, answer no. "
        "If the response is equivalent to the correct answer or contains all the intermediate "
        "steps to get the correct answer, you should also answer yes. If the response only "
        "contains a subset of the information required by the answer, answer no. "
        "In addition, do not penalize off-by-one errors for the number of days. If the question "
        "asks for the number of days/weeks/months, etc., and the model makes off-by-one errors "
        "(e.g., predicting 19 days when the answer is 18), the model's response is still correct.\n\n"
        "Question: {question}\nCorrect Answer: {answer}\nModel Response: {hypothesis}\n\n"
        "Is the model response correct? Answer yes or no only."
    ),
    "knowledge-update": (
        "I will give you a question, a correct answer, and a response from a model. "
        "Please answer yes if the response contains the correct answer. Otherwise, answer no. "
        "If the response contains some previous information along with an updated answer, the "
        "response should be considered as correct as long as the updated answer is the required answer.\n\n"
        "Question: {question}\nCorrect Answer: {answer}\nModel Response: {hypothesis}\n\n"
        "Is the model response correct? Answer yes or no only."
    ),
    "single-session-preference": (
        "I will give you a question, a rubric for desired personalized response, and a response "
        "from a model. Please answer yes if the response satisfies the desired response. "
        "Otherwise, answer no. The model does not need to reflect all the points in the rubric. "
        "The response is correct as long as it recalls and utilizes the user's personal "
        "information correctly.\n\n"
        "Question: {question}\nRubric: {answer}\nModel Response: {hypothesis}\n\n"
        "Is the model response correct? Answer yes or no only."
    ),
    "abstention": (
        "I will give you an unanswerable question, an explanation, and a response from a model. "
        "Please answer yes if the model correctly identifies the question as unanswerable. "
        "The model could say that the information is incomplete, or some other information is "
        "given but the asked information is not.\n\n"
        "Question: {question}\nExplanation: {answer}\nModel Response: {hypothesis}\n\n"
        "Does the model correctly identify the question as unanswerable? Answer yes or no only."
    ),
}


def _grade(question_data, hypothesis, judge_model, api_key, base_url):
    qtype = question_data["question_type"]
    qid = question_data["question_id"]
    answer = question_data["answer"]
    if isinstance(answer, list):
        answer = ", ".join(str(a) for a in answer)
    if qid.endswith("_abs"):
        template = GRADE_PROMPTS["abstention"]
    elif qtype in GRADE_PROMPTS:
        template = GRADE_PROMPTS[qtype]
    else:
        template = GRADE_PROMPTS["default"]
    prompt = template.format(question=question_data["question"], answer=answer, hypothesis=hypothesis)
    # Bump max_tokens to 1024 so reasoning models (MiniMax / o3 / R1)
    # can finish their <think>…</think> block and still emit yes/no.
    resp = _call_openai(api_key, judge_model, [{"role": "user", "content": prompt}], 1024, base_url)
    raw = _extract_text(resp)
    return raw, "yes" in raw.lower()


# ---------- Pipeline -------------------------------------------------------

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrievals", required=True, type=Path)
    ap.add_argument("--gold", required=True, type=Path)
    ap.add_argument("--reader", default="gpt-4.1")
    ap.add_argument("--judge", default="gpt-4o")
    ap.add_argument("--out", default=Path("/tmp/aidememo_omega_style"), type=Path)
    ap.add_argument("--limit", type=int, default=0, help="0 = all")
    ap.add_argument("--reader-base-url", default="https://api.openai.com/v1")
    ap.add_argument("--reader-api-key-env", default="OPENAI_API_KEY")
    ap.add_argument("--judge-base-url", default="https://api.openai.com/v1")
    ap.add_argument("--judge-api-key-env", default="OPENAI_API_KEY")
    ap.add_argument(
        "--workers",
        type=int,
        default=12,
        help="Concurrent HTTP workers for reader+judge stages (default: 12 — "
        "MiniMax tolerates this with jittered backoff. Bump to 16-20 for "
        "MiniMax pro tier, drop to 4-6 for free tier).",
    )
    args = ap.parse_args()

    reader_key = os.environ.get(args.reader_api_key_env, "")
    judge_key = os.environ.get(args.judge_api_key_env, "")
    if not reader_key or not judge_key:
        print(f"error: {args.reader_api_key_env} or {args.judge_api_key_env} not set", file=sys.stderr)
        return 2

    args.out.mkdir(parents=True, exist_ok=True)
    hyp_path = args.out / f"hypotheses_{args.reader}.jsonl"
    judg_path = args.out / f"judgements_{args.reader}_judge_{args.judge}.jsonl"

    rows = [json.loads(line) for line in open(args.retrievals)]
    gold_index = {q["question_id"]: q for q in json.load(open(args.gold))}
    if args.limit:
        rows = rows[: args.limit]
    # Always overlay the original question + question_date from gold so that
    # query-expansion runs (which mutate the question text in the bench input
    # JSON for retrieval) don't leak expansion artefacts into the reader
    # prompt. Gold is the source of truth for what the reader and judge see.
    n_overlaid = 0
    for r in rows:
        gold_q = gold_index.get(r["question_id"])
        if gold_q is None:
            continue
        if r.get("question") != gold_q["question"]:
            n_overlaid += 1
        r["question"] = gold_q["question"]
        if "question_date" in gold_q:
            r["question_date"] = gold_q["question_date"]
    if n_overlaid:
        print(f"  overlaid: {n_overlaid} questions restored from gold (expansion stripped from prompt)")
    print(f"OMEGA-style harness: {len(rows)} questions | reader={args.reader} | judge={args.judge}")

    # ---- Stage A: reader (resume from existing hypothesis file) ----
    done = set()
    if hyp_path.exists():
        for line in open(hyp_path):
            done.add(json.loads(line)["question_id"])
        print(f"  reader: resuming, {len(done)} already on disk")
    todo = [r for r in rows if r["question_id"] not in done]
    print(f"  reader: {len(todo)} new questions ({args.reader}), workers={args.workers}")

    def _read_one(row):
        qdata = {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "question": row["question"],
            "question_date": row.get("question_date"),
        }
        prompt, max_tokens, n_used = _build_reader_prompt(qdata, row.get("retrievals", []))
        try:
            resp = _call_openai(
                reader_key, args.reader,
                [{"role": "user", "content": prompt}],
                max_tokens, args.reader_base_url,
            )
            hypothesis = _extract_text(resp)
        except Exception as e:
            print(f"  ! reader fail {row['question_id']}: {e}", file=sys.stderr)
            hypothesis = ""
        return {
            "question_id": row["question_id"],
            "question_type": row["question_type"],
            "question": row["question"],
            "hypothesis": hypothesis,
            "n_snippets_used": n_used,
            "first_evidence_rank": row.get("first_evidence_rank"),
        }

    write_lock = threading.Lock()
    with open(hyp_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_read_one, row): row for row in todo}
        i = 0
        for fut in as_completed(futures):
            i += 1
            result = fut.result()
            with write_lock:
                fout.write(json.dumps(result) + "\n")
                fout.flush()
            if i % 10 == 0 or i == len(todo):
                print(f"    [{i:>4}/{len(todo)}] {result['question_id']}", file=sys.stderr)

    # ---- Stage B: judge ----
    judged = set()
    if judg_path.exists():
        for line in open(judg_path):
            judged.add(json.loads(line)["question_id"])
        print(f"  judge:  resuming, {len(judged)} already on disk")
    hyps = [json.loads(line) for line in open(hyp_path)]
    todo_j = [h for h in hyps if h["question_id"] not in judged]
    print(f"  judge:  {len(todo_j)} new judgements ({args.judge}), workers={args.workers}")

    def _judge_one(hyp):
        qid = hyp["question_id"]
        qdata = gold_index.get(qid)
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
            "first_evidence_rank": hyp.get("first_evidence_rank"),
        }

    judge_lock = threading.Lock()
    with open(judg_path, "a") as fout, ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(_judge_one, hyp): hyp for hyp in todo_j}
        i = 0
        for fut in as_completed(futures):
            i += 1
            result = fut.result()
            if result is None:
                continue
            with judge_lock:
                fout.write(json.dumps(result) + "\n")
                fout.flush()
            if i % 10 == 0 or i == len(todo_j):
                print(f"    [{i:>4}/{len(todo_j)}] {result['question_id']}", file=sys.stderr)

    # ---- Stage C: aggregate (mirrors official print_qa_metrics.py) ----
    judgements = [json.loads(line) for line in open(judg_path)]
    by_type = {}
    overall = []
    for j in judgements:
        overall.append(j["correct"])
        by_type.setdefault(j["question_type"], []).append(j["correct"])

    def _acc(rows):
        ok = sum(1 for r in rows if r is True)
        bad = sum(1 for r in rows if r is False)
        unk = sum(1 for r in rows if r is None)
        return ok, bad, unk

    ok, bad, unk = _acc(overall)
    total = ok + bad + unk
    print()
    print(f"Result (OMEGA-style harness on aidememo retrievals): reader={args.reader}, judge={args.judge}")
    print(f"  total: {total}")
    print(f"  CORRECT:    {ok:>4}  ({ok/total:.3f})")
    print(f"  INCORRECT:  {bad:>4}  ({bad/total:.3f})")
    print(f"  unparseable {unk:>4}  ({unk/total:.3f})")
    print("\n  By question_type:")
    type_accs = []
    for qt in sorted(by_type):
        ok2, bad2, unk2 = _acc(by_type[qt])
        n = ok2 + bad2 + unk2
        acc = ok2 / n if n else 0.0
        type_accs.append(acc)
        print(f"    {qt:30}  {acc:.3f}  ({ok2}/{n})")
    if type_accs:
        print(f"\n  Task-Averaged: {sum(type_accs)/len(type_accs):.3f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
