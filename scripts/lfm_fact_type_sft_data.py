#!/usr/bin/env python3
"""Build an AideMemo fact_type SFT dataset for MLX LoRA experiments.

The default input is a curated coding-agent shadow corpus:
``fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl``. Rows use a simple
append-friendly JSONL shape so future agent-labelled captures can be appended
after review:

  {"id":"...","text":"...","fact_type":"lesson","split":"train",...}

The generated files match ``mlx_lm.lora --data`` local format:
``train.jsonl``, ``valid.jsonl``, and ``test.jsonl`` with OpenAI-style
``messages`` rows. Use ``--mask-prompt`` during training so loss applies only
to the assistant label.
"""

from __future__ import annotations

import argparse
import json
import random
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Iterable

from lfm_mlx_fact_type_eval import CASES, FACT_TYPES, build_prompt


DEFAULT_CORPUS = Path("fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl")

TOPICS = [
    "SQLite store",
    "semantic prewarm",
    "Redis timeout",
    "Docusaurus docs",
    "Hermes plugin",
    "branch merge",
    "ColBERT reranker",
    "search daemon",
    "pending queue",
    "release checklist",
]

TOOLS = [
    "pnpm",
    "uv",
    "cargo nextest",
    "model2vec",
    "BGE",
    "LFM dense",
    "HNSW",
    "GitHub Actions",
    "S3 backup",
    "SQLite WAL",
]

ISSUES = [
    "DNS resolver churn",
    "cold model load",
    "missing sidebar config",
    "stale HNSW sidecar",
    "sandboxed Metal access",
    "unreviewed remote code",
    "duplicate facts",
    "source_id leakage",
    "slow branch import",
    "weak BM25 overlap",
]

ENTITIES = [
    "AideMemo",
    "Redis",
    "Hermes",
    "Claude hook",
    "LFM2.5",
    "SQLite",
    "redb",
    "LongMemEval",
    "MCP server",
    "agent SDK",
]

TEMPLATES: dict[str, list[str]] = {
    "preference": [
        "I prefer {tool} for {topic} work.",
        "My favorite setup for {topic} is {tool}.",
        "For {topic}, I like short risk-first summaries.",
        "The team prefers {tool} over ad hoc scripts for {topic}.",
        "나는 {topic} 작업에서 {tool} 방식을 선호한다.",
        "우리 팀은 {topic}에는 {tool}을 더 좋아한다.",
    ],
    "decision": [
        "We decided to use {tool} for {topic}.",
        "Going with {tool} as the default for {topic}.",
        "I chose {tool} after comparing alternatives for {topic}.",
        "Decision: keep {topic} on {tool} for now.",
        "{topic}에는 {tool}을 쓰기로 결정했다.",
        "이번 변경에서는 {topic} 기본값을 {tool}로 간다.",
    ],
    "lesson": [
        "Tried {tool} for {topic}, but the root cause was {issue}.",
        "We learned that {issue} breaks {topic} when {tool} is cold.",
        "Root cause was {issue}, not the {tool} setting.",
        "The {topic} run taught us to check {issue} first.",
        "{topic}에서 {issue} 때문에 실패한다는 걸 배웠다.",
        "{tool}을 조정했지만 실제 원인은 {issue}였다.",
    ],
    "error": [
        "Avoid using {tool} for {topic} until {issue} is audited.",
        "Do not ignore {issue} in the {topic} workflow.",
        "Never again treat {issue} as valid {topic} evidence.",
        "Failure mode: {issue} can silently corrupt {topic}.",
        "{topic}에서 {issue}를 성능 근거로 쓰면 안 된다.",
        "{tool}을 켜기 전에 {issue}를 반드시 확인해야 한다.",
    ],
    "convention": [
        "Always run {tool} before changing {topic}.",
        "House rule: {topic} changes must mention {issue}.",
        "Convention: {topic} facts include the responsible {entity}.",
        "We never merge {topic} without checking {issue}.",
        "모든 {topic} 변경에는 {tool} 결과를 포함한다.",
        "{topic}에서는 항상 {entity} 기준으로 source_id를 나눈다.",
    ],
    "pattern": [
        "{entity} uses {tool} for {topic}.",
        "{topic} is implemented with {tool} and reviewed through {entity}.",
        "The {topic} pattern keeps {tool} warm before user queries.",
        "{entity} stores {topic} state in {tool}.",
        "{topic} 구조는 {entity}가 {tool}을 통해 처리한다.",
        "{entity}는 {topic}에 {tool}을 사용하는 구조다.",
    ],
    "claim": [
        "{entity} currently uses {tool} in the {topic} path.",
        "The {topic} benchmark reported {tool} latency near 45 ms.",
        "{entity} has a stored fact_type column for {topic}.",
        "We believe {issue} explains the {topic} regression.",
        "{topic} 측정에서 {tool}은 약 45ms였다.",
        "{entity}에는 {topic} 상태가 저장되어 있다.",
    ],
    "note": [
        "The {topic} notes mention {entity} and {tool}.",
        "{entity} appeared in the {topic} meeting notes.",
        "The {topic} folder contains logs from {tool}.",
        "{topic} was edited after the {entity} discussion.",
        "오늘 {topic} 미팅에서 {entity} 이야기가 나왔다.",
        "{tool} 로그가 {topic} 폴더에 남아 있다.",
    ],
    "question": [
        "Should we use {tool} for {topic}?",
        "Why did {issue} affect {topic}?",
        "Open question: can {entity} share {tool} across {topic}?",
        "Investigate whether {tool} improves {topic} when {issue} is present.",
        "{topic}에 {tool}을 기본값으로 켜도 될까?",
        "{issue}가 {topic}에 왜 영향을 줬지?",
    ],
}


def normalize_label(value: Any) -> str:
    return str(value or "").strip().lower().replace("-", "_")


def render(template: str, rng: random.Random) -> str:
    return template.format(
        topic=rng.choice(TOPICS),
        tool=rng.choice(TOOLS),
        issue=rng.choice(ISSUES),
        entity=rng.choice(ENTITIES),
    )


def as_messages(row: dict[str, Any], prompt_style: str) -> dict[str, object]:
    system, user = build_prompt(
        {
            "id": str(row["id"]),
            "text": str(row["text"]),
            "fact_type": str(row["fact_type"]),
        },
        prompt_style,
    )
    return {
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
            {"role": "assistant", "content": row["fact_type"]},
        ],
        "id": row["id"],
        "fact_type": row["fact_type"],
        "text": row["text"],
        "source": row.get("source", "unknown"),
        "scenario": row.get("scenario", "unknown"),
        "language": row.get("language", "unknown"),
        "label_source": row.get("label_source", ""),
    }


def load_corpus(
    path: Path,
    *,
    include_inferred_labels: bool,
    include_disputed_labels: bool,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open(encoding="utf-8") as f:
        for line_no, line in enumerate(f, start=1):
            if not line.strip():
                continue
            raw = json.loads(line)
            label_source = normalize_label(
                raw.get("label_source") or raw.get("fact_type_source")
            )
            if (
                label_source in {"default", "inferred"}
                and not include_inferred_labels
            ):
                continue
            if raw.get("fact_type_hint") and not include_disputed_labels:
                continue
            text = str(raw.get("text") or raw.get("content") or "").strip()
            fact_type = normalize_label(raw.get("fact_type"))
            if not text:
                raise ValueError(f"{path}:{line_no}: missing text/content")
            if fact_type not in FACT_TYPES:
                raise ValueError(f"{path}:{line_no}: unsupported fact_type {fact_type!r}")
            split = str(raw.get("split", "")).strip().lower()
            if split and split not in {"train", "valid", "test"}:
                raise ValueError(f"{path}:{line_no}: unsupported split {split!r}")
            rows.append(
                {
                    **raw,
                    "id": str(raw.get("id") or f"{path.stem}-{line_no}"),
                    "text": text,
                    "fact_type": fact_type,
                    "split": split,
                    "label_source": label_source,
                    "source_file": str(path),
                }
            )
    return rows


def synthetic_rows(examples_per_label: int, seed: int) -> list[dict[str, Any]]:
    rng = random.Random(seed)
    rows = []
    for fact_type in FACT_TYPES:
        templates = TEMPLATES[fact_type]
        seen = set()
        while len(seen) < examples_per_label:
            text = render(rng.choice(templates), rng)
            seen.add(text)
        for idx, text in enumerate(sorted(seen), start=1):
            rows.append(
                {
                    "id": f"synthetic-{fact_type}-{idx:04d}",
                    "text": text,
                    "fact_type": fact_type,
                    "split": "",
                    "source": "synthetic_template",
                    "scenario": "template_augmentation",
                    "language": "mixed",
                }
            )
    rng.shuffle(rows)
    return rows


def builtin_test_rows() -> list[dict[str, Any]]:
    return [
        {
            "id": case["id"],
            "text": case["text"],
            "fact_type": case["fact_type"],
            "split": "test",
            "source": "builtin_lfm_mlx_fact_type_eval",
            "scenario": "holdout",
            "language": "mixed",
        }
        for case in CASES
    ]


def stratified_split(
    rows: list[dict[str, Any]],
    valid_ratio: float,
    test_ratio: float,
    seed: int,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[dict[str, Any]]]:
    rng = random.Random(seed)
    train: list[dict[str, Any]] = []
    valid: list[dict[str, Any]] = []
    test: list[dict[str, Any]] = []
    by_label: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_label[row["fact_type"]].append(row)
    for label, label_rows in by_label.items():
        rng.shuffle(label_rows)
        n = len(label_rows)
        valid_n = max(1, int(round(n * valid_ratio))) if n >= 3 else 0
        test_n = max(1, int(round(n * test_ratio))) if n >= 3 else 0
        for row in label_rows[:test_n]:
            test.append({**row, "split": "test"})
        for row in label_rows[test_n : test_n + valid_n]:
            valid.append({**row, "split": "valid"})
        for row in label_rows[test_n + valid_n :]:
            train.append({**row, "split": "train"})
    rng.shuffle(train)
    rng.shuffle(valid)
    rng.shuffle(test)
    return train, valid, test


def split_rows(
    rows: list[dict[str, Any]],
    valid_ratio: float,
    test_ratio: float,
    seed: int,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[dict[str, Any]]]:
    explicit = [row for row in rows if row.get("split")]
    unsplit = [row for row in rows if not row.get("split")]
    train = [row for row in explicit if row["split"] == "train"]
    valid = [row for row in explicit if row["split"] == "valid"]
    test = [row for row in explicit if row["split"] == "test"]
    if unsplit:
        more_train, more_valid, more_test = stratified_split(
            unsplit,
            valid_ratio,
            test_ratio,
            seed,
        )
        train.extend(more_train)
        valid.extend(more_valid)
        test.extend(more_test)
    return train, valid, test


def write_jsonl(path: Path, rows: Iterable[dict[str, object]]) -> int:
    count = 0
    with path.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")
            count += 1
    return count


def count_by_label(rows: list[dict[str, Any]]) -> dict[str, int]:
    counts = Counter(row["fact_type"] for row in rows)
    return {label: counts.get(label, 0) for label in FACT_TYPES}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out",
        type=Path,
        default=Path("/private/tmp/aidememo-lfm-fact-type-sft"),
    )
    parser.add_argument(
        "--corpus",
        action="append",
        type=Path,
        help="JSONL corpus path. Can be repeated. Defaults to the coding-agent seed corpus.",
    )
    parser.add_argument("--no-default-corpus", action="store_true")
    parser.add_argument(
        "--examples-per-label",
        type=int,
        default=0,
        help="Optional synthetic template examples per label to add to train/valid.",
    )
    parser.add_argument(
        "--include-builtin-test",
        action="store_true",
        help="Append the original 45-case mixed-language holdout to test.jsonl.",
    )
    parser.add_argument(
        "--include-inferred-labels",
        action="store_true",
        help=(
            "Keep rows whose label_source/fact_type_source is inferred/default. "
            "By default these are skipped so heuristic labels do not train the adapter."
        ),
    )
    parser.add_argument(
        "--include-disputed-labels",
        action="store_true",
        help=(
            "Keep rows with fact_type_hint. By default these are skipped because "
            "the stored label and strong-cue hint disagree."
        ),
    )
    parser.add_argument("--valid-ratio", type=float, default=0.2)
    parser.add_argument("--test-ratio", type=float, default=0.2)
    parser.add_argument("--seed", type=int, default=20260707)
    parser.add_argument("--prompt-style", choices=["compact", "fewshot"], default="compact")
    args = parser.parse_args()

    if not 0.0 <= args.valid_ratio < 0.5:
        raise SystemExit("--valid-ratio must be >= 0.0 and < 0.5")
    if not 0.0 <= args.test_ratio < 0.5:
        raise SystemExit("--test-ratio must be >= 0.0 and < 0.5")

    corpus_paths = [] if args.no_default_corpus else [DEFAULT_CORPUS]
    if args.corpus:
        corpus_paths.extend(args.corpus)

    rows: list[dict[str, Any]] = []
    for corpus_path in corpus_paths:
        rows.extend(
            load_corpus(
                corpus_path,
                include_inferred_labels=args.include_inferred_labels,
                include_disputed_labels=args.include_disputed_labels,
            )
        )
    if args.examples_per_label:
        rows.extend(synthetic_rows(args.examples_per_label, args.seed))
    if not rows:
        raise SystemExit("no corpus rows selected")

    train, valid, test = split_rows(rows, args.valid_ratio, args.test_ratio, args.seed)
    if args.include_builtin_test:
        test.extend(builtin_test_rows())

    args.out.mkdir(parents=True, exist_ok=True)
    counts = {
        "train": write_jsonl(args.out / "train.jsonl", (as_messages(row, args.prompt_style) for row in train)),
        "valid": write_jsonl(args.out / "valid.jsonl", (as_messages(row, args.prompt_style) for row in valid)),
        "test": write_jsonl(args.out / "test.jsonl", (as_messages(row, args.prompt_style) for row in test)),
    }
    metadata = {
        "prompt_style": args.prompt_style,
        "seed": args.seed,
        "labels": FACT_TYPES,
        "corpus_paths": [str(path) for path in corpus_paths],
        "synthetic_examples_per_label": args.examples_per_label,
        "include_builtin_test": args.include_builtin_test,
        "include_inferred_labels": args.include_inferred_labels,
        "include_disputed_labels": args.include_disputed_labels,
        "counts": counts,
        "by_label": {
            "train": count_by_label(train),
            "valid": count_by_label(valid),
            "test": count_by_label(test),
        },
        "training_note": "Use mlx_lm.lora --mask-prompt so only assistant labels train.",
    }
    (args.out / "metadata.json").write_text(
        json.dumps(metadata, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    print(json.dumps({"out": str(args.out), **metadata}, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
