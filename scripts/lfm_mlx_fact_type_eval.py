#!/usr/bin/env python3
"""Evaluate MLX LFM models as closed-label AideMemo fact_type classifiers.

The older ``lfm_mlx_lm_eval.py`` script asks the model to generate JSON, which
mixes two questions: "can it classify?" and "can it obey structured output?".
This harness isolates the classification question by scoring the likelihood of
each allowed fact_type label and choosing the best-scoring label.

Usage:
  /private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_fact_type_eval.py \
    --model-dir /private/tmp/lfm25-230m-mlx-4bit \
    --summary-only
"""

from __future__ import annotations

import argparse
import contextlib
import json
import math
import os
import statistics
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


CASES: list[dict[str, str]] = [
    {
        "id": "preference-dark-mode",
        "fact_type": "preference",
        "text": "I prefer dark mode in editor tools because it is easier during long sessions.",
    },
    {
        "id": "preference-camera",
        "fact_type": "preference",
        "text": "My favorite camera setup is the Sony A7R IV with the 35mm prime lens.",
    },
    {
        "id": "preference-review-style",
        "fact_type": "preference",
        "text": "For review comments, I like concise risk-first summaries before implementation notes.",
    },
    {
        "id": "preference-korean-code-review",
        "fact_type": "preference",
        "text": "나는 코드 리뷰에서 먼저 리스크를 보는 방식을 선호한다.",
    },
    {
        "id": "preference-team-pnpm",
        "fact_type": "preference",
        "text": "Our team prefers pnpm over npm for this repository.",
    },
    {
        "id": "decision-sqlite-default",
        "fact_type": "decision",
        "text": "We decided to keep SQLite as the default AideMemo store and leave redb optional.",
    },
    {
        "id": "decision-auto-hybrid",
        "fact_type": "decision",
        "text": "Going with LFM dense retrieval only behind the --auto search gate.",
    },
    {
        "id": "decision-rust-edition",
        "fact_type": "decision",
        "text": "I chose Rust edition 2024 for the workspace migration.",
    },
    {
        "id": "decision-korean-redb",
        "fact_type": "decision",
        "text": "이번 릴리스에서는 redb를 옵션으로 남기기로 결정했다.",
    },
    {
        "id": "decision-webhook-signatures",
        "fact_type": "decision",
        "text": "Let's go with webhook signatures enforced by default.",
    },
    {
        "id": "lesson-redis-dns",
        "fact_type": "lesson",
        "text": "Tried increasing the Redis pool size, but the timeout root cause was DNS resolver churn.",
    },
    {
        "id": "lesson-docs-e2e",
        "fact_type": "lesson",
        "text": "We learned that the Docusaurus E2E gate fails when the sidebar config is missing.",
    },
    {
        "id": "lesson-benchmark",
        "fact_type": "lesson",
        "text": "The failed benchmark taught us to reject runs that cannot access the model cache.",
    },
    {
        "id": "lesson-korean-mlx",
        "fact_type": "lesson",
        "text": "MLX는 샌드박스에서 Metal 장치가 없어 실패한다는 걸 배웠다.",
    },
    {
        "id": "lesson-root-cause",
        "fact_type": "lesson",
        "text": "Root cause was the search daemon starting cold before semantic prewarm finished.",
    },
    {
        "id": "error-trust-remote-code",
        "fact_type": "error",
        "text": "Avoid enabling trust_remote_code for Hugging Face models until the repo code is audited.",
    },
    {
        "id": "error-publish-before-license",
        "fact_type": "error",
        "text": "Do not run cargo publish before the license audit is complete.",
    },
    {
        "id": "error-vector-shape-only",
        "fact_type": "error",
        "text": "Never again trust an embedding smoke that only checks vector shape.",
    },
    {
        "id": "error-korean-gradle",
        "fact_type": "error",
        "text": "샌드박스 에러가 난 Gradle 벤치를 성능 근거로 쓰면 안 된다.",
    },
    {
        "id": "error-failure-mode",
        "fact_type": "error",
        "text": "Failure mode: reranker outages must not block plain BM25 search.",
    },
    {
        "id": "convention-source-id",
        "fact_type": "convention",
        "text": "Always attach AIDEMEMO_SOURCE_ID when several agents write to the same shared store.",
    },
    {
        "id": "convention-git",
        "fact_type": "convention",
        "text": "House rule: no destructive git commands unless the user explicitly asks for them.",
    },
    {
        "id": "convention-merge",
        "fact_type": "convention",
        "text": "We never auto-merge branch memory without reviewing the diff.",
    },
    {
        "id": "convention-korean-pr",
        "fact_type": "convention",
        "text": "모든 PR 요약에는 테스트 결과를 포함한다.",
    },
    {
        "id": "convention-format",
        "fact_type": "convention",
        "text": "Convention: fact ids stay in ULID format in every exported segment.",
    },
    {
        "id": "pattern-sqlite-store",
        "fact_type": "pattern",
        "text": "AideMemo uses SQLite for the default hot store.",
    },
    {
        "id": "pattern-daemon",
        "fact_type": "pattern",
        "text": "The daemon pattern keeps the embedding model warm for repeated CLI calls.",
    },
    {
        "id": "pattern-auto-hybrid",
        "fact_type": "pattern",
        "text": "Search auto-hybrid uses BM25 confidence to promote semantic retrieval.",
    },
    {
        "id": "pattern-korean-source-id",
        "fact_type": "pattern",
        "text": "프로젝트별 에이전트 공유 저장소는 source_id로 테넌트를 구분한다.",
    },
    {
        "id": "pattern-jsonl-pending",
        "fact_type": "pattern",
        "text": "The pending queue is implemented with JSONL files so hooks can append cheaply.",
    },
    {
        "id": "claim-colbert",
        "fact_type": "claim",
        "text": "LFM2.5-ColBERT-350M uses per-token vectors and MaxSim scoring.",
    },
    {
        "id": "claim-facts-table",
        "fact_type": "claim",
        "text": "The SQLite store has a facts table with a fact_type column.",
    },
    {
        "id": "claim-latency",
        "fact_type": "claim",
        "text": "The warm daemon hybrid search latency was about 45 ms in the local smoke.",
    },
    {
        "id": "claim-korean-model2vec",
        "fact_type": "claim",
        "text": "model2vec 기본 모델은 warm query에서 약 3ms였다.",
    },
    {
        "id": "claim-belief",
        "fact_type": "claim",
        "text": "We believe the first-stage recall gap is caused by lexical mismatch.",
    },
    {
        "id": "note-staging",
        "fact_type": "note",
        "text": "The deploy finished on staging without incidents.",
    },
    {
        "id": "note-meeting",
        "fact_type": "note",
        "text": "Meeting notes mention Redis and DNS in the same paragraph.",
    },
    {
        "id": "note-docs-edited",
        "fact_type": "note",
        "text": "The docs page was edited after lunch.",
    },
    {
        "id": "note-korean-meeting",
        "fact_type": "note",
        "text": "오늘 미팅에서 검색 데몬 이야기가 나왔다.",
    },
    {
        "id": "note-prototype-folder",
        "fact_type": "note",
        "text": "The prototype folder contains screenshots and logs.",
    },
    {
        "id": "question-larger-slice",
        "fact_type": "question",
        "text": "Should we add a larger LongMemEval slice before claiming dense LFM improves recall?",
    },
    {
        "id": "question-cache",
        "fact_type": "question",
        "text": "Why did the cache miss after restart?",
    },
    {
        "id": "question-sidecar",
        "fact_type": "question",
        "text": "Open question: can the sidecar share a warmed process?",
    },
    {
        "id": "question-korean-default",
        "fact_type": "question",
        "text": "LFM 분류기를 기본값으로 켜도 될까?",
    },
    {
        "id": "question-investigate-rerank",
        "fact_type": "question",
        "text": "Investigate whether rerank helps when recall is already saturated.",
    },
]


def deterministic_fact_type(text: str) -> str:
    """Mirror aidememo_core::extract::infer_fact_type for baseline scoring."""

    lowered = text.strip().lower()
    if (
        lowered.endswith("?")
        or lowered.startswith("question")
        or " why " in lowered
        or lowered.startswith("why ")
    ):
        return "question"
    if (
        "decided" in lowered
        or lowered.startswith("decision")
        or "we picked" in lowered
        or "we chose" in lowered
        or "i chose" in lowered
        or "i picked" in lowered
        or "let's go with" in lowered
        or "going with" in lowered
        or "rolled out" in lowered
    ):
        return "decision"
    if (
        lowered.startswith("preference")
        or "i prefer" in lowered
        or "we prefer" in lowered
        or "user prefers" in lowered
        or "team prefers" in lowered
        or "my favorite" in lowered
        or "my favourite" in lowered
        or "i like " in lowered
        or "i dislike " in lowered
    ):
        return "preference"
    if (
        lowered.startswith("lesson")
        or "we learned" in lowered
        or "i learned" in lowered
        or "learned that" in lowered
        or "tried " in lowered
        or "turns out" in lowered
        or "wish i had" in lowered
        or "root cause was" in lowered
        or "caused by" in lowered
    ):
        return "lesson"
    if (
        lowered.startswith("error")
        or lowered.startswith("mistake")
        or "avoid " in lowered
        or "do not " in lowered
        or "don't " in lowered
        or "never again" in lowered
        or "was a mistake" in lowered
        or "failure mode" in lowered
    ):
        return "error"
    if (
        " always " in lowered
        or lowered.startswith("always ")
        or " never " in lowered
        or "convention" in lowered
        or "house rule" in lowered
        or " rule:" in lowered
    ):
        return "convention"
    if (
        "pattern" in lowered
        or "antipattern" in lowered
        or "anti-pattern" in lowered
        or " is implemented with " in lowered
        or " are implemented with " in lowered
        or (" uses " in lowered and " for " in lowered)
    ):
        return "pattern"
    if (
        "we believe" in lowered
        or "we think" in lowered
        or lowered.startswith("claim:")
        or " claim " in lowered
    ):
        return "claim"
    return "note"


def build_prompt(case: dict[str, str], prompt_style: str) -> tuple[str, str]:
    system = "You classify AideMemo memory facts. Return only one label."
    labels = ", ".join(FACT_TYPES)
    if prompt_style == "compact":
        return (
            system,
            f"Choose one label from: {labels}.\nText: {case['text']}\nLabel:",
        )

    rules = "\n".join(
        [
            "preference = first-person or team preference, favorite, like/dislike",
            "decision = decided, chose, picked, going with, rollout choice",
            "lesson = tried-X-hit-Y, learned, root cause, what the agent should remember",
            "error = avoid, do not, failure mode, never-again warning",
            "convention = always/never rule or durable workflow convention",
            "pattern = architectural or implementation pattern, X uses Y for Z",
            "claim = factual assertion or belief about current/external state",
            "note = passive observation or low-actionability note",
            "question = open investigation or question",
        ]
    )
    examples = "\n".join(
        [
            "Text: I prefer dark mode. Label: preference",
            "Text: Tried Redis pool tuning, but DNS was the root cause. Label: lesson",
            "Text: Avoid disabling webhook signature validation. Label: error",
            "Text: AideMemo uses SQLite for the default hot store. Label: pattern",
            "Text: The deploy finished on staging. Label: note",
        ]
    )
    user = (
        "Choose the most specific AideMemo fact_type.\n"
        f"Allowed labels: {labels}.\n"
        f"{rules}\n"
        f"{examples}\n"
        f"Text: {case['text']}\n"
        "Label:"
    )
    return system, user


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


def encode_label(tokenizer: Any, label: str) -> list[int]:
    token_ids = tokenizer.encode(f" {label}", add_special_tokens=False)
    if not token_ids:
        token_ids = tokenizer.encode(label, add_special_tokens=False)
    return token_ids


def logsumexp(values: list[float]) -> float:
    high = max(values)
    return high + math.log(sum(math.exp(value - high) for value in values))


def softmax(scores: dict[str, float]) -> dict[str, float]:
    denom = logsumexp(list(scores.values()))
    return {label: math.exp(score - denom) for label, score in scores.items()}


def model_logits(model: Any, tokens: Any) -> Any:
    out = model(tokens)
    if isinstance(out, tuple):
        return out[0]
    return out


def load_cases_file(path: Path, split: str) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    with path.open(encoding="utf-8") as f:
        for line_no, line in enumerate(f, start=1):
            if not line.strip():
                continue
            raw = json.loads(line)
            row_split = str(raw.get("split", "")).strip().lower()
            if split != "all" and row_split and row_split != split:
                continue
            fact_type = normalize_label(raw.get("fact_type"))
            if fact_type not in FACT_TYPES:
                raise ValueError(f"{path}:{line_no}: unsupported fact_type {fact_type!r}")
            text = str(raw.get("text") or raw.get("content") or "").strip()
            if not text:
                raise ValueError(f"{path}:{line_no}: missing text/content")
            rows.append(
                {
                    "id": str(raw.get("id") or f"{path.stem}-{line_no}"),
                    "text": text,
                    "fact_type": fact_type,
                }
            )
    return rows


def normalize_label(value: Any) -> str:
    return str(value or "").strip().lower().replace("-", "_")


def classify_case(
    model: Any,
    tokenizer: Any,
    case: dict[str, str],
    prompt_style: str,
    template: str,
    normalize: str,
) -> dict[str, Any]:
    import mlx.core as mx

    system, user = build_prompt(case, prompt_style)
    if template == "plain":
        prompt = f"{system}\n\n{user}"
    else:
        prompt = apply_chat_template(tokenizer, system, user)
    prompt_ids = tokenizer.encode(prompt, add_special_tokens=False)
    labels = {label: encode_label(tokenizer, label) for label in FACT_TYPES}
    max_len = max(len(prompt_ids) + len(label_ids) for label_ids in labels.values())
    pad_id = getattr(tokenizer, "eos_token_id", None) or getattr(
        tokenizer, "pad_token_id", None
    )
    if pad_id is None:
        pad_id = 0

    batch = []
    for label in FACT_TYPES:
        seq = prompt_ids + labels[label]
        batch.append(seq + [pad_id] * (max_len - len(seq)))

    started = time.perf_counter()
    logits = model_logits(model, mx.array(batch))
    mx.eval(logits)
    elapsed_ms = (time.perf_counter() - started) * 1000

    scores: dict[str, float] = {}
    token_logprobs: dict[str, list[float]] = {}
    for row_idx, label in enumerate(FACT_TYPES):
        label_ids = labels[label]
        values = []
        for offset, token_id in enumerate(label_ids):
            pred_pos = len(prompt_ids) + offset - 1
            row = logits[row_idx, pred_pos]
            log_prob = row[token_id] - mx.logsumexp(row, axis=-1)
            mx.eval(log_prob)
            values.append(float(log_prob))
        token_logprobs[label] = values
        if normalize == "sum":
            scores[label] = sum(values)
        else:
            scores[label] = sum(values) / len(values)

    probs = softmax(scores)
    ranked = sorted(scores.items(), key=lambda item: item[1], reverse=True)
    predicted = ranked[0][0]
    runner_up = ranked[1][0]
    baseline = deterministic_fact_type(case["text"])

    return {
        "id": case["id"],
        "text": case["text"],
        "expected_fact_type": case["fact_type"],
        "predicted_fact_type": predicted,
        "correct": predicted == case["fact_type"],
        "baseline_fact_type": baseline,
        "baseline_correct": baseline == case["fact_type"],
        "runner_up": runner_up,
        "score_margin": round(ranked[0][1] - ranked[1][1], 4),
        "confidence": round(probs[predicted], 4),
        "latency_ms": round(elapsed_ms, 2),
        "scores": {label: round(scores[label], 4) for label in FACT_TYPES},
        "label_token_counts": {label: len(labels[label]) for label in FACT_TYPES},
        "prompt_chars": len(prompt),
    }


def summarize(
    rows: list[dict[str, Any]],
    model_dir: Path,
    adapter_path: Path | None,
    load_ms: float,
    tokenizer_config_patched: bool,
    prompt_style: str,
    template: str,
    normalize: str,
) -> dict[str, Any]:
    latencies = [float(row["latency_ms"]) for row in rows]
    baseline_default = [row for row in rows if row["baseline_fact_type"] == "note"]
    baseline_default_non_note = [
        row for row in baseline_default if row["expected_fact_type"] != "note"
    ]
    lfm_rescues = [
        row
        for row in baseline_default_non_note
        if row["predicted_fact_type"] == row["expected_fact_type"]
    ]
    lfm_harms = [
        row
        for row in rows
        if row["baseline_correct"] and row["predicted_fact_type"] != row["expected_fact_type"]
    ]

    by_expected: dict[str, dict[str, Any]] = {}
    for label in FACT_TYPES:
        label_rows = [row for row in rows if row["expected_fact_type"] == label]
        by_expected[label] = {
            "cases": len(label_rows),
            "accuracy": sum(row["correct"] for row in label_rows) / len(label_rows),
            "baseline_accuracy": sum(row["baseline_correct"] for row in label_rows)
            / len(label_rows),
        }

    threshold_precision = []
    for threshold in [0.30, 0.40, 0.50, 0.60, 0.70, 0.80]:
        accepted = [row for row in rows if row["confidence"] >= threshold]
        threshold_precision.append(
            {
                "confidence_gte": threshold,
                "accepted": len(accepted),
                "coverage": round(len(accepted) / len(rows), 4),
                "precision": None
                if not accepted
                else round(sum(row["correct"] for row in accepted) / len(accepted), 4),
            }
        )

    confusion: dict[str, dict[str, int]] = {}
    for row in rows:
        confusion.setdefault(row["expected_fact_type"], {})
        confusion[row["expected_fact_type"]][row["predicted_fact_type"]] = (
            confusion[row["expected_fact_type"]].get(row["predicted_fact_type"], 0) + 1
        )

    return {
        "backend": "mlx-lm-label-likelihood",
        "model_dir": str(model_dir),
        "adapter_path": None if adapter_path is None else str(adapter_path),
        "tokenizer_config_patched": tokenizer_config_patched,
        "prompt_style": prompt_style,
        "template": template,
        "score_normalization": normalize,
        "model_load_ms": round(load_ms, 2),
        "cases": len(rows),
        "accuracy": round(sum(row["correct"] for row in rows) / len(rows), 4),
        "baseline_accuracy": round(
            sum(row["baseline_correct"] for row in rows) / len(rows), 4
        ),
        "agreement_with_baseline": round(
            sum(
                row["predicted_fact_type"] == row["baseline_fact_type"] for row in rows
            )
            / len(rows),
            4,
        ),
        "baseline_default_cases": len(baseline_default),
        "baseline_default_non_note_cases": len(baseline_default_non_note),
        "lfm_rescued_baseline_default_non_note": len(lfm_rescues),
        "lfm_harmed_baseline_correct": len(lfm_harms),
        "mean_latency_ms": round(statistics.fmean(latencies), 2),
        "p50_latency_ms": round(statistics.median(latencies), 2),
        "by_expected": by_expected,
        "threshold_precision": threshold_precision,
        "confusion": confusion,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument(
        "--adapter-path",
        type=Path,
        help="Optional MLX LoRA adapter directory or adapters.safetensors file.",
    )
    parser.add_argument("--prompt-style", choices=["fewshot", "compact"], default="fewshot")
    parser.add_argument("--template", choices=["chat", "plain"], default="chat")
    parser.add_argument("--normalize", choices=["mean", "sum"], default="mean")
    parser.add_argument(
        "--cases-file",
        action="append",
        type=Path,
        help="Optional JSONL corpus with id/text/fact_type/split fields.",
    )
    parser.add_argument(
        "--case-split",
        choices=["train", "valid", "test", "all"],
        default="test",
        help="Split to evaluate when --cases-file is used.",
    )
    parser.add_argument("--limit", type=int)
    parser.add_argument("--summary-only", action="store_true")
    args = parser.parse_args()

    try:
        from mlx_lm import load
    except ImportError as exc:
        raise SystemExit("missing dependency: pip install -U mlx-lm") from exc

    if args.cases_file:
        selected = []
        for cases_file in args.cases_file:
            selected.extend(load_cases_file(cases_file, args.case_split))
        if not selected:
            raise SystemExit("no cases selected from --cases-file")
    else:
        selected = CASES
    if args.limit:
        selected = selected[: args.limit]
    with patched_model_dir(args.model_dir) as (load_dir, tokenizer_config_patched):
        load_started = time.perf_counter()
        model, tokenizer = load(str(load_dir))
        if args.adapter_path is not None:
            if not args.adapter_path.is_dir():
                raise SystemExit("--adapter-path must point to an MLX adapter directory")
            from mlx_lm.tuner.utils import load_adapters

            model = load_adapters(model, str(args.adapter_path))
        load_ms = (time.perf_counter() - load_started) * 1000

        rows = [
            classify_case(
                model,
                tokenizer,
                case,
                args.prompt_style,
                args.template,
                args.normalize,
            )
            for case in selected
        ]

    payload: dict[str, Any] = {
        "summary": summarize(
            rows,
            args.model_dir,
            args.adapter_path,
            load_ms,
            tokenizer_config_patched,
            args.prompt_style,
            args.template,
            args.normalize,
        )
    }
    if not args.summary_only:
        payload["rows"] = rows
    print(json.dumps(payload, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
