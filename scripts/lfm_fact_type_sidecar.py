#!/usr/bin/env python3
"""Return LFM/MLX fact_type hints without writing to AideMemo.

This is the intentionally thin sidecar path for early rollout:

* AideMemo core and MCP writes remain deterministic.
* The sidecar receives text, scores the fixed fact_type labels, and returns a
  hint with confidence/margin metadata.
* Callers decide whether to surface the hint in pending review or ignore it.

Example:

  /private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
    --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
    --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora-20260707-240i \
    --text "Tried daemon prewarm, but the real issue was a stale HNSW sidecar."
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

from lfm_mlx_fact_type_eval import (
    classify_case,
    patched_model_dir,
)


def load_input_rows(args: argparse.Namespace) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for idx, text in enumerate(args.text or [], start=1):
        if text.strip():
            rows.append({"id": f"text-{idx}", "text": text.strip()})

    for path in args.input_jsonl or []:
        with path.open(encoding="utf-8") as f:
            for line_no, line in enumerate(f, start=1):
                if not line.strip():
                    continue
                raw = json.loads(line)
                text = str(raw.get("text") or raw.get("content") or "").strip()
                if not text:
                    raise ValueError(f"{path}:{line_no}: missing text/content")
                row = {
                    "id": str(raw.get("id") or f"{path.stem}-{line_no}"),
                    "text": text,
                }
                if raw.get("fact_type"):
                    row["fact_type"] = str(raw["fact_type"])
                if raw.get("split"):
                    row["split"] = str(raw["split"])
                if args.input_split != "all" and row.get("split") != args.input_split:
                    continue
                rows.append(row)

    if not rows and not sys.stdin.isatty():
        text = sys.stdin.read().strip()
        if text:
            rows.append({"id": "stdin", "text": text})

    return rows


def hint_from_row(
    row: dict[str, str],
    result: dict[str, Any],
    confidence_threshold: float,
    margin_threshold: float,
    include_scores: bool,
) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "id": row["id"],
        "text": row["text"],
        "suggested_fact_type": result["predicted_fact_type"],
        "confidence": result["confidence"],
        "score_margin": result["score_margin"],
        "runner_up": result["runner_up"],
        "baseline_fact_type": result["baseline_fact_type"],
        "accepted": (
            result["confidence"] >= confidence_threshold
            and result["score_margin"] >= margin_threshold
        ),
        "latency_ms": result["latency_ms"],
    }
    if "fact_type" in row:
        expected = row["fact_type"].strip().lower().replace("-", "_")
        payload["expected_fact_type"] = expected
        payload["correct"] = result["predicted_fact_type"] == expected
    if include_scores:
        payload["scores"] = result["scores"]
    return payload


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument("--adapter-path", type=Path)
    parser.add_argument("--text", action="append")
    parser.add_argument("--input-jsonl", action="append", type=Path)
    parser.add_argument(
        "--input-split",
        choices=["train", "valid", "test", "all"],
        default="all",
        help="Filter --input-jsonl rows by split before classification.",
    )
    parser.add_argument("--jsonl", action="store_true", help="Emit one hint per line")
    parser.add_argument(
        "--output-jsonl",
        type=Path,
        help="Write one hint per line to this path instead of stdout.",
    )
    parser.add_argument("--prompt-style", choices=["fewshot", "compact"], default="compact")
    parser.add_argument("--template", choices=["chat", "plain"], default="chat")
    parser.add_argument("--normalize", choices=["mean", "sum"], default="mean")
    parser.add_argument("--confidence-threshold", type=float, default=0.8)
    parser.add_argument("--margin-threshold", type=float, default=0.0)
    parser.add_argument("--include-scores", action="store_true")
    args = parser.parse_args()

    rows = load_input_rows(args)
    if not rows:
        raise SystemExit("no input text; pass --text, --input-jsonl, or stdin")

    try:
        from mlx_lm import load
    except ImportError as exc:
        raise SystemExit("missing dependency: pip install -U mlx-lm") from exc

    with patched_model_dir(args.model_dir) as (load_dir, tokenizer_config_patched):
        started = time.perf_counter()
        model, tokenizer = load(str(load_dir))
        if args.adapter_path is not None:
            if not args.adapter_path.is_dir():
                raise SystemExit("--adapter-path must point to an MLX adapter directory")
            from mlx_lm.tuner.utils import load_adapters

            model = load_adapters(model, str(args.adapter_path))
        load_ms = (time.perf_counter() - started) * 1000

        hints = []
        for row in rows:
            case = {
                "id": row["id"],
                "text": row["text"],
                # classify_case expects an expected label for eval accounting.
                # Sidecar callers without labels get a dummy value that is not
                # surfaced unless the input row had fact_type.
                "fact_type": row.get("fact_type", "note"),
            }
            result = classify_case(
                model,
                tokenizer,
                case,
                args.prompt_style,
                args.template,
                args.normalize,
            )
            hints.append(
                hint_from_row(
                    row,
                    result,
                    args.confidence_threshold,
                    args.margin_threshold,
                    args.include_scores,
                )
            )

    if args.jsonl or args.output_jsonl is not None:
        lines = [json.dumps(hint, ensure_ascii=False) for hint in hints]
        if args.output_jsonl is not None:
            args.output_jsonl.write_text("\n".join(lines) + "\n", encoding="utf-8")
            return
        for line in lines:
            print(line)
        return

    print(
        json.dumps(
            {
                "backend": "mlx-lm-label-likelihood-sidecar",
                "model_dir": str(args.model_dir),
                "adapter_path": None if args.adapter_path is None else str(args.adapter_path),
                "tokenizer_config_patched": tokenizer_config_patched,
                "model_load_ms": round(load_ms, 2),
                "confidence_threshold": args.confidence_threshold,
                "margin_threshold": args.margin_threshold,
                "count": len(hints),
                "hints": hints,
            },
            indent=2,
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
