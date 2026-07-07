#!/usr/bin/env python3
"""Evaluate MLX LFM text-generation models on AideMemo control tasks.

This script does not judge open-ended answer quality. It measures whether a
small local LFM can help AideMemo route and capture memory with structured
outputs:

* extraction: fact_type + entity suggestions for candidate facts
* router: choose bm25 / dense / colbert / aggregate
* consolidation: duplicate / supersede / keep_distinct / review

Usage:
  hf download LiquidAI/LFM2.5-230M-MLX-4bit \
      --local-dir /private/tmp/lfm25-230m-mlx-4bit

  /private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_lm_eval.py \
      --model-dir /private/tmp/lfm25-230m-mlx-4bit \
      --suite all
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import tempfile
import time
from pathlib import Path
from typing import Any, Iterator


FACT_TYPES = [
    "preference",
    "decision",
    "lesson",
    "error",
    "convention",
    "pattern",
    "claim",
    "note",
    "question",
]

ROUTES = ["bm25", "dense", "colbert", "aggregate"]
CONSOLIDATION_ACTIONS = ["duplicate", "supersede", "keep_distinct", "review"]

EXTRACTION_CASES = [
    {
        "id": "preference-dark-mode",
        "text": "I prefer dark mode in editor tools because it is easier during long sessions.",
        "fact_type": "preference",
        "entities": ["Editor"],
    },
    {
        "id": "decision-sqlite",
        "text": "We decided to keep SQLite as the default AideMemo store and leave redb optional.",
        "fact_type": "decision",
        "entities": ["AideMemo", "SQLite", "redb"],
    },
    {
        "id": "lesson-redis",
        "text": "Tried increasing the Redis pool size, but the timeout root cause was DNS resolver churn.",
        "fact_type": "lesson",
        "entities": ["Redis", "DNS"],
    },
    {
        "id": "error-remote-code",
        "text": "Avoid enabling trust_remote_code for Hugging Face models until the repo code is audited.",
        "fact_type": "error",
        "entities": ["Hugging Face"],
    },
    {
        "id": "convention-agent",
        "text": "Always attach AIDEMEMO_SOURCE_ID when several agents write to the same shared store.",
        "fact_type": "convention",
        "entities": ["AIDEMEMO_SOURCE_ID"],
    },
    {
        "id": "pattern-pending",
        "text": "Claude hook preview should put uncertain candidate facts in pending review before durable writes.",
        "fact_type": "pattern",
        "entities": ["Claude", "pending review"],
    },
    {
        "id": "claim-colbert",
        "text": "LFM2.5-ColBERT-350M uses per-token vectors and MaxSim scoring.",
        "fact_type": "claim",
        "entities": ["LFM2.5-ColBERT-350M"],
    },
    {
        "id": "question-benchmark",
        "text": "Should we add a larger LongMemEval slice before claiming dense LFM improves recall?",
        "fact_type": "question",
        "entities": ["LongMemEval", "LFM"],
    },
]

ROUTER_CASES = [
    {
        "id": "surface-redis",
        "query": "redis timeout root cause",
        "route": "bm25",
    },
    {
        "id": "korean-paraphrase",
        "query": "레디스 타임아웃의 원인이 뭐였지",
        "route": "dense",
    },
    {
        "id": "favorite-camera",
        "query": "what is my favorite camera setup",
        "route": "dense",
    },
    {
        "id": "how-many-decisions",
        "query": "how many times did we decide to use SQLite",
        "route": "aggregate",
    },
    {
        "id": "timeline-incidents",
        "query": "timeline of all Redis timeout incidents",
        "route": "aggregate",
    },
    {
        "id": "ambiguous-head-order",
        "query": "which database migration note says it finished faster, not rollback drills",
        "route": "colbert",
    },
    {
        "id": "exact-command",
        "query": "aidememo vector-rebuild --current-only",
        "route": "bm25",
    },
    {
        "id": "sum-money",
        "query": "how much total did I spend on embeddings experiments",
        "route": "aggregate",
    },
]

CONSOLIDATION_CASES = [
    {
        "id": "duplicate-redis",
        "old": "Redis timeout root cause was DNS resolver churn, not pool size.",
        "new": "The Redis timeout incident was caused by DNS resolver churn rather than pool size.",
        "action": "duplicate",
    },
    {
        "id": "supersede-model",
        "old": "Use BGE as the default semantic model for all repositories.",
        "new": "Keep model2vec as default; use BGE only for English paraphrase-heavy memory.",
        "action": "supersede",
    },
    {
        "id": "keep-distinct",
        "old": "AideMemo stores current facts in SQLite by default.",
        "new": "AideMemo can archive old facts to a cold tier.",
        "action": "keep_distinct",
    },
    {
        "id": "review-ambiguous",
        "old": "The LFM dense path is useful for Korean queries.",
        "new": "The LFM dense path may be useful after more benchmarks.",
        "action": "review",
    },
]


def normalize_json_payload(parsed: Any) -> dict[str, Any] | None:
    if isinstance(parsed, list) and parsed:
        parsed = parsed[0]
    if not isinstance(parsed, dict):
        return None
    if isinstance(parsed.get("arguments"), dict):
        return parsed["arguments"]
    return parsed


def parse_json_object(text: str) -> dict[str, Any] | None:
    cleaned = text.strip()
    if cleaned.startswith("```"):
        cleaned = re.sub(r"^```(?:json)?", "", cleaned, flags=re.IGNORECASE).strip()
        cleaned = re.sub(r"```$", "", cleaned).strip()
    try:
        parsed = json.loads(cleaned)
        return normalize_json_payload(parsed)
    except json.JSONDecodeError:
        pass

    match = re.search(r"\{.*\}", cleaned, flags=re.DOTALL)
    if not match:
        return None
    try:
        parsed = json.loads(match.group(0))
    except json.JSONDecodeError:
        return None
    return normalize_json_payload(parsed)


def normalize_string(value: Any) -> str:
    return str(value or "").strip().lower().replace("-", "_")


def normalize_entities(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    return [str(item).strip().lower() for item in value if str(item).strip()]


def apply_chat_template(tokenizer: Any, system: str, user: str) -> str:
    if getattr(tokenizer, "chat_template", None) is None:
        bos = getattr(tokenizer, "bos_token", None) or "<|startoftext|>"
        return (
            f"{bos}<|im_start|>system\n{system}<|im_end|>\n"
            f"<|im_start|>user\n{user}<|im_end|>\n"
            "<|im_start|>assistant\n"
        )
    try:
        return tokenizer.apply_chat_template(
            [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            tokenize=False,
            add_generation_prompt=True,
        )
    except Exception:
        return tokenizer.apply_chat_template(
            [{"role": "user", "content": f"{system}\n\n{user}"}],
            tokenize=False,
            add_generation_prompt=True,
        )


@contextlib.contextmanager
def patched_model_dir(model_dir: Path) -> Iterator[tuple[Path, bool]]:
    """Patch Liquid 230M/350M MLX tokenizer metadata without touching the repo."""

    tokenizer_config = model_dir / "tokenizer_config.json"
    if not tokenizer_config.exists():
        yield model_dir, False
        return

    config = json.loads(tokenizer_config.read_text())
    if config.get("tokenizer_class") != "TokenizersBackend":
        yield model_dir, False
        return

    with tempfile.TemporaryDirectory(prefix="aidememo-lfm-mlx-tokenizer-") as tmp:
        patched = Path(tmp)
        for child in model_dir.iterdir():
            target = patched / child.name
            if child.name == "tokenizer_config.json":
                updated = dict(config)
                updated["tokenizer_class"] = "PreTrainedTokenizerFast"
                target.write_text(json.dumps(updated, indent=2), encoding="utf-8")
            else:
                os.symlink(child, target, target_is_directory=child.is_dir())
        yield patched, True


def build_extraction_prompt(case: dict[str, Any], prompt_style: str) -> tuple[str, str]:
    system = "Return only valid JSON. Do not explain."
    if prompt_style == "compact":
        user = (
            "Choose one fact_type from "
            f"{FACT_TYPES}. Return exactly: "
            '{"fact_type":"...","entities":["..."],"should_save":true}\n'
            f"Text: {case['text']}"
        )
        return system, user

    user = (
        "Classify an AideMemo memory candidate.\n"
        "Allowed fact_type values: "
        f"{FACT_TYPES}.\n"
        'Example text: I prefer dark mode.\n'
        'Example JSON: {"fact_type":"preference","entities":["dark mode"],"should_save":true}\n'
        'Example text: Tried Redis pool tuning, but DNS was the root cause.\n'
        'Example JSON: {"fact_type":"lesson","entities":["Redis","DNS"],"should_save":true}\n'
        'Return exactly JSON with keys fact_type, entities, should_save.\n'
        f"Text: {case['text']}"
    )
    return system, user


def build_router_prompt(case: dict[str, Any], prompt_style: str) -> tuple[str, str]:
    system = "Return only valid JSON. Do not explain."
    if prompt_style == "compact":
        user = (
            "Routes: bm25 for exact lexical/code/doc lookup; dense for "
            "paraphrase, multilingual, or weak lexical overlap; colbert when "
            "candidates likely exist but top ordering is subtle; aggregate for "
            "counts, sums, distinct dates, or timelines. Return exactly: "
            '{"route":"..."}\n'
            f"Query: {case['query']}"
        )
        return system, user

    user = (
        "Choose one AideMemo route.\n"
        "bm25 = exact lexical/code/doc lookup.\n"
        "dense = paraphrase, multilingual, or weak lexical overlap.\n"
        "colbert = candidates likely exist but the top ordering is subtle.\n"
        "aggregate = counts, sums, distinct dates, or timelines.\n"
        'Example query: redis timeout root cause\n'
        'Example JSON: {"route":"bm25"}\n'
        'Example query: 레디스 타임아웃의 원인이 뭐였지\n'
        'Example JSON: {"route":"dense"}\n'
        'Example query: how many decisions mention SQLite\n'
        'Example JSON: {"route":"aggregate"}\n'
        'Example query: which migration note says faster, not rollback drills\n'
        'Example JSON: {"route":"colbert"}\n'
        'Return exactly JSON with key route.\n'
        f"Query: {case['query']}"
    )
    return system, user


def build_consolidation_prompt(case: dict[str, Any], prompt_style: str) -> tuple[str, str]:
    system = "Return only valid JSON. Do not explain."
    if prompt_style == "compact":
        user = (
            "Actions: duplicate for same meaning; supersede when the new fact "
            "replaces the old; keep_distinct when both should remain; review "
            'when ambiguous. Return exactly: {"action":"..."}\n'
            f"Old fact: {case['old']}\nNew fact: {case['new']}"
        )
        return system, user

    user = (
        "Compare two AideMemo facts.\n"
        "duplicate = same meaning.\n"
        "supersede = new fact replaces old fact.\n"
        "keep_distinct = both facts should remain.\n"
        "review = ambiguous or uncertain.\n"
        'Example old: Use BGE for every repository.\n'
        'Example new: Keep model2vec default; use BGE only for English paraphrase memory.\n'
        'Example JSON: {"action":"supersede"}\n'
        'Example old: Redis timeout was DNS churn.\n'
        'Example new: Redis timeout was caused by DNS resolver churn.\n'
        'Example JSON: {"action":"duplicate"}\n'
        'Return exactly JSON with key action.\n'
        f"Old fact: {case['old']}\nNew fact: {case['new']}"
    )
    return system, user


def run_case(
    model: Any,
    tokenizer: Any,
    suite: str,
    case: dict[str, Any],
    max_tokens: int,
    sampler: Any,
    logits_processors: Any,
    prompt_style: str,
) -> dict[str, Any]:
    from mlx_lm import generate

    if suite == "extraction":
        system, user = build_extraction_prompt(case, prompt_style)
    elif suite == "router":
        system, user = build_router_prompt(case, prompt_style)
    elif suite == "consolidation":
        system, user = build_consolidation_prompt(case, prompt_style)
    else:
        raise ValueError(f"unknown suite: {suite}")

    prompt = apply_chat_template(tokenizer, system, user)
    started = time.perf_counter()
    output = generate(
        model,
        tokenizer,
        prompt=prompt,
        max_tokens=max_tokens,
        sampler=sampler,
        logits_processors=logits_processors,
        verbose=False,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    parsed = parse_json_object(output)

    row: dict[str, Any] = {
        "suite": suite,
        "id": case["id"],
        "valid_json": parsed is not None,
        "latency_ms": round(elapsed_ms, 2),
        "raw": output.strip()[:500],
    }

    if suite == "extraction":
        expected_type = case["fact_type"]
        actual_type = normalize_string(parsed.get("fact_type") if parsed else "")
        expected_entities = [entity.lower() for entity in case["entities"]]
        actual_entities = normalize_entities(parsed.get("entities") if parsed else [])
        entity_hits = sum(
            1
            for entity in expected_entities
            if any(entity in actual or actual in entity for actual in actual_entities)
        )
        row.update(
            {
                "expected_fact_type": expected_type,
                "actual_fact_type": actual_type,
                "fact_type_ok": actual_type == expected_type,
                "expected_entities": case["entities"],
                "actual_entities": actual_entities,
                "entity_recall": entity_hits / len(expected_entities),
            }
        )
    elif suite == "router":
        expected_route = case["route"]
        actual_route = normalize_string(parsed.get("route") if parsed else "")
        row.update(
            {
                "expected_route": expected_route,
                "actual_route": actual_route,
                "route_ok": actual_route == expected_route,
            }
        )
    elif suite == "consolidation":
        expected_action = case["action"]
        actual_action = normalize_string(parsed.get("action") if parsed else "")
        row.update(
            {
                "expected_action": expected_action,
                "actual_action": actual_action,
                "action_ok": actual_action == expected_action,
            }
        )
    return row


def summarize(
    rows: list[dict[str, Any]],
    model_dir: Path,
    load_ms: float,
    tokenizer_config_patched: bool,
    prompt_style: str,
) -> dict[str, Any]:
    by_suite: dict[str, dict[str, Any]] = {}
    for suite in ["extraction", "router", "consolidation"]:
        suite_rows = [row for row in rows if row["suite"] == suite]
        if not suite_rows:
            continue
        summary: dict[str, Any] = {
            "cases": len(suite_rows),
            "valid_json_rate": sum(row["valid_json"] for row in suite_rows) / len(suite_rows),
            "mean_latency_ms": round(
                sum(float(row["latency_ms"]) for row in suite_rows) / len(suite_rows),
                2,
            ),
        }
        if suite == "extraction":
            summary["fact_type_accuracy"] = sum(
                row["fact_type_ok"] for row in suite_rows
            ) / len(suite_rows)
            summary["mean_entity_recall"] = round(
                sum(float(row["entity_recall"]) for row in suite_rows)
                / len(suite_rows),
                4,
            )
        elif suite == "router":
            summary["route_accuracy"] = sum(row["route_ok"] for row in suite_rows) / len(
                suite_rows
            )
        elif suite == "consolidation":
            summary["action_accuracy"] = sum(
                row["action_ok"] for row in suite_rows
            ) / len(suite_rows)
        by_suite[suite] = summary

    return {
        "backend": "mlx-lm",
        "model_dir": str(model_dir),
        "tokenizer_config_patched": tokenizer_config_patched,
        "prompt_style": prompt_style,
        "model_load_ms": round(load_ms, 2),
        "cases": len(rows),
        "valid_json_rate": sum(row["valid_json"] for row in rows) / len(rows),
        "mean_latency_ms": round(
            sum(float(row["latency_ms"]) for row in rows) / len(rows),
            2,
        ),
        "by_suite": by_suite,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument(
        "--suite",
        choices=["all", "extraction", "router", "consolidation"],
        default="all",
    )
    parser.add_argument("--max-tokens", type=int, default=96)
    parser.add_argument("--temp", type=float, default=0.0)
    parser.add_argument("--top-p", type=float, default=0.0)
    parser.add_argument("--top-k", type=int, default=0)
    parser.add_argument("--repetition-penalty", type=float, default=1.05)
    parser.add_argument("--prompt-style", choices=["fewshot", "compact"], default="fewshot")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--summary-only", action="store_true")
    args = parser.parse_args()

    try:
        from mlx_lm import load
        from mlx_lm.sample_utils import make_logits_processors, make_sampler
    except ImportError as exc:
        raise SystemExit("missing dependency: pip install -U mlx-lm") from exc

    with patched_model_dir(args.model_dir) as (load_dir, tokenizer_config_patched):
        load_started = time.perf_counter()
        model, tokenizer = load(str(load_dir))
        load_ms = (time.perf_counter() - load_started) * 1000

        sampler = make_sampler(temp=args.temp, top_p=args.top_p, top_k=args.top_k)
        logits_processors = make_logits_processors(repetition_penalty=args.repetition_penalty)

        selected: list[tuple[str, dict[str, Any]]] = []
        if args.suite in ("all", "extraction"):
            selected.extend(("extraction", case) for case in EXTRACTION_CASES)
        if args.suite in ("all", "router"):
            selected.extend(("router", case) for case in ROUTER_CASES)
        if args.suite in ("all", "consolidation"):
            selected.extend(("consolidation", case) for case in CONSOLIDATION_CASES)
        if args.limit is not None:
            selected = selected[: args.limit]

        rows = [
            run_case(
                model,
                tokenizer,
                suite,
                case,
                args.max_tokens,
                sampler,
                logits_processors,
                args.prompt_style,
            )
            for suite, case in selected
        ]

    payload: dict[str, Any] = {
        "summary": summarize(
            rows,
            args.model_dir,
            load_ms,
            tokenizer_config_patched,
            args.prompt_style,
        )
    }
    if not args.summary_only:
        payload["rows"] = rows
    print(json.dumps(payload, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
