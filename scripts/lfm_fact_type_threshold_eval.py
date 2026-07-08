#!/usr/bin/env python3
"""Evaluate fact_type sidecar thresholds against reviewed shadow labels.

Inputs:

* labels JSONL: AideMemo shadow rows or curated corpus rows with
  ``id``, ``text`` and ``fact_type``.
* predictions JSONL: output from ``lfm_fact_type_sidecar.py --jsonl`` with
  ``id``, ``suggested_fact_type``, ``confidence``, ``score_margin`` and
  optional ``baseline_fact_type``.

The report is designed for the safety question that matters before automatic
promotion: at a proposed confidence/margin threshold, how many baseline-correct
labels would the adapter harm, and how many baseline-missed residuals would it
rescue?
"""

from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path
from typing import Any


def normalize_label(value: Any) -> str:
    return str(value or "").strip().lower().replace("-", "_")


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    rows = []
    with path.open(encoding="utf-8") as f:
        for line_no, line in enumerate(f, start=1):
            if not line.strip():
                continue
            row = json.loads(line)
            row.setdefault("id", f"{path.stem}-{line_no}")
            rows.append(row)
    return rows


def eligible_label_rows(
    rows: list[dict[str, Any]],
    include_inferred: bool,
    label_split: str,
) -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    skipped = Counter()
    for row in rows:
        if label_split != "all" and normalize_label(row.get("split")) != label_split:
            skipped["split"] += 1
            continue
        fact_type = normalize_label(row.get("fact_type") or row.get("expected_fact_type"))
        if not fact_type:
            skipped["missing_fact_type"] += 1
            continue
        label_source = normalize_label(row.get("label_source") or row.get("fact_type_source"))
        if label_source in {"default", "inferred"} and not include_inferred:
            skipped["inferred_label"] += 1
            continue
        out[str(row["id"])] = {
            **row,
            "fact_type": fact_type,
            "label_source": label_source,
        }
    return out


def load_predictions(rows: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    for row in rows:
        pred = normalize_label(row.get("suggested_fact_type") or row.get("predicted_fact_type"))
        if not pred:
            continue
        out[str(row["id"])] = {
            **row,
            "suggested_fact_type": pred,
            "baseline_fact_type": normalize_label(row.get("baseline_fact_type")),
            "confidence": float(row.get("confidence", 0.0)),
            "score_margin": float(row.get("score_margin", row.get("margin", 0.0))),
        }
    return out


def joined_rows(
    labels: dict[str, dict[str, Any]],
    predictions: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    rows = []
    for row_id, label in labels.items():
        pred = predictions.get(row_id)
        if pred is None:
            continue
        expected = label["fact_type"]
        suggested = pred["suggested_fact_type"]
        baseline = pred.get("baseline_fact_type") or normalize_label(label.get("baseline_fact_type"))
        rows.append(
            {
                "id": row_id,
                "expected": expected,
                "suggested": suggested,
                "baseline": baseline,
                "confidence": pred["confidence"],
                "score_margin": pred["score_margin"],
                "correct": suggested == expected,
                "baseline_correct": bool(baseline) and baseline == expected,
                "baseline_wrong": bool(baseline) and baseline != expected,
                "label_source": label.get("label_source", ""),
            }
        )
    return rows


def eval_threshold(rows: list[dict[str, Any]], confidence: float, margin: float) -> dict[str, Any]:
    accepted = [
        row
        for row in rows
        if row["confidence"] >= confidence and row["score_margin"] >= margin
    ]
    correct = [row for row in accepted if row["correct"]]
    incorrect = [row for row in accepted if not row["correct"]]
    harms = [row for row in incorrect if row["baseline_correct"]]
    rescues = [row for row in correct if row["baseline_wrong"]]
    accepted_n = len(accepted)
    total_n = len(rows)
    return {
        "confidence": confidence,
        "margin": margin,
        "accepted": accepted_n,
        "coverage": accepted_n / total_n if total_n else 0.0,
        "correct": len(correct),
        "incorrect": len(incorrect),
        "precision": len(correct) / accepted_n if accepted_n else 0.0,
        "false_memory_rate": len(incorrect) / accepted_n if accepted_n else 0.0,
        "baseline_correct_harms": len(harms),
        "residual_rescues": len(rescues),
        "net_rescue_minus_harm": len(rescues) - len(harms),
    }


def grid_values(raw: str) -> list[float]:
    return [float(part) for part in raw.split(",") if part.strip()]


def demo_rows() -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    labels = [
        {"id": "a", "text": "Preference: use local provider", "fact_type": "preference", "label_source": "explicit"},
        {"id": "b", "text": "Tried LFM but BM25 was enough", "fact_type": "lesson", "label_source": "explicit"},
        {"id": "c", "text": "Redis uses HNSW for vectors", "fact_type": "pattern", "label_source": "explicit"},
        {"id": "d", "text": "The meeting mentioned Redis", "fact_type": "note", "label_source": "explicit"},
    ]
    preds = [
        {"id": "a", "suggested_fact_type": "preference", "baseline_fact_type": "note", "confidence": 0.91, "score_margin": 0.30},
        {"id": "b", "suggested_fact_type": "lesson", "baseline_fact_type": "note", "confidence": 0.86, "score_margin": 0.20},
        {"id": "c", "suggested_fact_type": "claim", "baseline_fact_type": "pattern", "confidence": 0.84, "score_margin": 0.10},
        {"id": "d", "suggested_fact_type": "note", "baseline_fact_type": "note", "confidence": 0.72, "score_margin": 0.05},
    ]
    return labels, preds


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--labels-jsonl", type=Path)
    parser.add_argument("--predictions-jsonl", type=Path)
    parser.add_argument(
        "--label-split",
        choices=["train", "valid", "test", "all"],
        default="all",
        help="Filter labels by split before joining predictions.",
    )
    parser.add_argument("--include-inferred-labels", action="store_true")
    parser.add_argument("--confidence-grid", default="0.70,0.75,0.80,0.85,0.90,0.95")
    parser.add_argument("--margin-grid", default="0.0,0.05,0.10,0.20,0.30")
    parser.add_argument("--min-precision", type=float, default=0.95)
    parser.add_argument("--max-baseline-correct-harms", type=int, default=0)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        label_rows, pred_rows = demo_rows()
    else:
        if args.labels_jsonl is None or args.predictions_jsonl is None:
            raise SystemExit("pass --labels-jsonl and --predictions-jsonl, or --self-test")
        label_rows = load_jsonl(args.labels_jsonl)
        pred_rows = load_jsonl(args.predictions_jsonl)

    labels = eligible_label_rows(label_rows, args.include_inferred_labels, args.label_split)
    predictions = load_predictions(pred_rows)
    rows = joined_rows(labels, predictions)
    if not rows:
        raise SystemExit("no joined label/prediction rows")

    thresholds = []
    for confidence in grid_values(args.confidence_grid):
        for margin in grid_values(args.margin_grid):
            thresholds.append(eval_threshold(rows, confidence, margin))
    thresholds.sort(
        key=lambda row: (
            row["precision"],
            -row["baseline_correct_harms"],
            row["accepted"],
            row["net_rescue_minus_harm"],
        ),
        reverse=True,
    )
    viable = [
        row
        for row in thresholds
        if row["accepted"] > 0
        and row["precision"] >= args.min_precision
        and row["baseline_correct_harms"] <= args.max_baseline_correct_harms
    ]
    viable.sort(key=lambda row: (row["accepted"], row["net_rescue_minus_harm"]), reverse=True)

    payload = {
        "joined_rows": len(rows),
        "labels": len(labels),
        "predictions": len(predictions),
        "label_distribution": dict(Counter(row["expected"] for row in rows)),
        "baseline_accuracy": (
            sum(row["baseline_correct"] for row in rows) / len(rows) if rows else 0.0
        ),
        "raw_prediction_accuracy": (
            sum(row["correct"] for row in rows) / len(rows) if rows else 0.0
        ),
        "recommended_threshold": viable[0] if viable else None,
        "top_thresholds": thresholds[:10],
        "viable_thresholds": viable[:10],
    }
    print(json.dumps(payload, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
