#!/usr/bin/env python3
"""Rerank AideMemo JSON search hits with a LiquidAI LFM ColBERT model.

This is a sidecar experiment path, not a production in-process provider.
It lets us test late-interaction retrieval quality before changing
AideMemo's single-vector HNSW storage model.

Usage:
  aidememo search "redis timeout root cause" --json -l 50 \
    | python3 scripts/lfm_colbert_rerank.py \
        --query "redis timeout root cause" \
        --top-k 10

Requires:
  pip install -U pylate transformers
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


def read_json(path: str) -> Any:
    if path == "-":
        return json.load(sys.stdin)
    with Path(path).open() as fh:
        return json.load(fh)


def extract_hits(payload: Any) -> list[dict[str, Any]]:
    if isinstance(payload, list):
        return [row for row in payload if isinstance(row, dict)]
    if isinstance(payload, dict):
        for key in ("results", "search", "hits"):
            value = payload.get(key)
            if isinstance(value, list):
                return [row for row in value if isinstance(row, dict)]
    raise SystemExit("expected an AideMemo search JSON list or object with results/search/hits")


def normalize_rerank_rows(raw: Any) -> list[tuple[int, float]]:
    rows = raw
    if isinstance(rows, list) and rows and isinstance(rows[0], list):
        rows = rows[0]

    out: list[tuple[int, float]] = []
    for row in rows:
        if isinstance(row, dict):
            idx = (
                row.get("id")
                if row.get("id") is not None
                else row.get("document_id", row.get("corpus_id", row.get("index")))
            )
            score = row.get("score", row.get("similarity", row.get("maxsim")))
        elif isinstance(row, (list, tuple)) and len(row) >= 2:
            idx, score = row[0], row[1]
        else:
            continue
        try:
            out.append((int(idx), float(score)))
        except (TypeError, ValueError):
            continue
    return out


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--query", required=True)
    parser.add_argument("--hits", default="-", help="AideMemo search JSON file, or '-' for stdin")
    parser.add_argument("--top-k", type=int, default=10)
    parser.add_argument(
        "--model",
        default="LiquidAI/LFM2.5-ColBERT-350M",
        help="PyLate ColBERT model; use LiquidAI/LFM2-ColBERT-350M to reproduce the original LFM2 run.",
    )
    parser.add_argument("--batch-size", type=int, default=32)
    parser.add_argument(
        "--trust-remote-code",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Enable only after auditing the model repo; default avoids executing Hub code.",
    )
    args = parser.parse_args()

    payload = read_json(args.hits)
    hits = extract_hits(payload)
    if not hits:
        print("[]")
        return

    try:
        from pylate import models, rank
    except ImportError as exc:
        raise SystemExit("missing dependency: pip install -U pylate transformers") from exc

    documents = [str(hit.get("content", "")) for hit in hits]
    document_ids = list(range(len(documents)))

    model_kwargs: dict[str, Any] = {"model_name_or_path": args.model}
    if args.trust_remote_code:
        model_kwargs["trust_remote_code"] = True
    model = models.ColBERT(**model_kwargs)

    query_embeddings = model.encode(
        [args.query],
        batch_size=args.batch_size,
        is_query=True,
        show_progress_bar=False,
    )
    document_embeddings = model.encode(
        [documents],
        batch_size=args.batch_size,
        is_query=False,
        show_progress_bar=False,
    )
    reranked = rank.rerank(
        documents_ids=[document_ids],
        queries_embeddings=query_embeddings,
        documents_embeddings=document_embeddings,
    )

    scored = normalize_rerank_rows(reranked)
    if not scored:
        raise SystemExit("PyLate returned no rerank scores")

    output = []
    for colbert_rank, (idx, score) in enumerate(scored[: args.top_k], start=1):
        if idx < 0 or idx >= len(hits):
            continue
        row = dict(hits[idx])
        row["original_rank"] = row.get("rank")
        row["original_score"] = row.get("score")
        row["colbert_rank"] = colbert_rank
        row["colbert_score"] = score
        output.append(row)

    print(json.dumps(output, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
