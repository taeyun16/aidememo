#!/usr/bin/env python3
"""Micro-evaluate an MLX LFM ColBERT model for AideMemo retrieval.

This uses a local/downloaded MLX ColBERT repository such as:

  mlx-community/LFM2.5-ColBERT-350M-4bit

It imports the repository's `lfm2_bidirectional.py` model definition from the
model directory and runs local MaxSim scoring. Inspect that file before running
against a new repository.

Usage:
  hf download mlx-community/LFM2.5-ColBERT-350M-4bit \
      --local-dir /private/tmp/lfm25-colbert-mlx-4bit

  /private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_colbert_eval.py \
      --aidememo target/debug/aidememo \
      --model-dir /private/tmp/lfm25-colbert-mlx-4bit
"""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

import numpy as np

from lfm_dense_eval import (
    CASES,
    FACTS,
    gold_rank_contents,
    gold_rank_hits,
    mean_by_scenario,
    reciprocal,
    run_json,
    seed_store,
)
from lfm_mlx_dense_eval import load_model_code


class MlxColbert:
    def __init__(self, model_dir: Path):
        try:
            import mlx.core as mx
            import mlx.nn as nn
            from transformers import AutoTokenizer
        except ImportError as exc:
            raise SystemExit("missing dependency: pip install -U mlx transformers") from exc

        self.mx = mx
        self.nn = nn
        self.model_dir = model_dir
        module = load_model_code(model_dir)

        config = json.loads((model_dir / "config.json").read_text())
        model = module.ColbertModel(module.ModelArgs.from_dict(config))
        quantization = config.get("quantization") or {}
        if quantization:
            nn.quantize(
                model,
                group_size=quantization.get("group_size"),
                bits=quantization.get("bits"),
                mode=quantization.get("mode", "affine"),
            )
        model.load_weights(str(model_dir / "model.safetensors"), strict=True)
        mx.eval(model.parameters())

        st_config = json.loads((model_dir / "config_sentence_transformers.json").read_text())
        self.model = model
        self.tokenizer = AutoTokenizer.from_pretrained(str(model_dir), local_files_only=True)
        self.query_prefix = st_config.get("query_prefix", "[Q] ")
        self.document_prefix = st_config.get("document_prefix", "[D] ")
        self.query_length = int(st_config.get("query_length", 32))
        self.document_length = int(st_config.get("document_length", 512))

    def encode(
        self,
        texts: list[str],
        role: str,
        batch_size: int,
    ) -> tuple[np.ndarray, np.ndarray]:
        prefix = self.query_prefix if role == "query" else self.document_prefix
        max_length = self.query_length if role == "query" else self.document_length
        embeddings = []
        masks = []
        for start in range(0, len(texts), batch_size):
            chunk = [prefix + text for text in texts[start : start + batch_size]]
            encoded = self.tokenizer(
                chunk,
                padding="max_length",
                truncation=True,
                max_length=max_length,
                return_tensors="np",
            )
            input_ids = self.mx.array(encoded["input_ids"])
            attention_mask = self.mx.array(encoded["attention_mask"])
            tok = self.model.encode(input_ids, attention_mask, normalize=True)
            tok = tok.astype(self.mx.float32)
            self.mx.eval(tok)
            embeddings.append(np.array(tok))
            masks.append(np.asarray(encoded["attention_mask"], dtype=np.float32))
        return np.concatenate(embeddings, axis=0), np.concatenate(masks, axis=0)


def colbert_score(
    query_tokens: np.ndarray,
    query_mask: np.ndarray,
    doc_tokens: np.ndarray,
    doc_mask: np.ndarray,
) -> float:
    sims = query_tokens @ doc_tokens.T
    sims[:, doc_mask == 0] = -1e9
    maxsim = sims.max(axis=1)
    return float((maxsim * query_mask).sum())


def rank_scores(scores: list[tuple[int, float]]) -> list[int]:
    return [idx for idx, _score in sorted(scores, key=lambda item: item[1], reverse=True)]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aidememo", default="target/debug/aidememo")
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument("--candidate-limit", type=int, default=8)
    parser.add_argument("--batch-size", type=int, default=8)
    parser.add_argument("--store", type=Path)
    parser.add_argument("--summary-only", action="store_true")
    args = parser.parse_args()

    model_started = time.perf_counter()
    colbert = MlxColbert(args.model_dir)
    model_ms = (time.perf_counter() - model_started) * 1000

    documents = [content for content, _entities, _fact_type in FACTS]
    doc_started = time.perf_counter()
    doc_embeddings, doc_masks = colbert.encode(
        documents,
        role="document",
        batch_size=args.batch_size,
    )
    doc_ms = (time.perf_counter() - doc_started) * 1000

    with tempfile.TemporaryDirectory(prefix="aidememo-lfm-mlx-colbert-eval-") as tmp:
        store = args.store or Path(tmp) / "wiki.sqlite"
        if args.store is None:
            seed_store(args.aidememo, store)

        rows = []
        totals = {
            "bm25_mrr": 0.0,
            "colbert_rerank_mrr": 0.0,
            "colbert_all_mrr": 0.0,
            "bm25_hit1": 0,
            "colbert_rerank_hit1": 0,
            "colbert_all_hit1": 0,
            "candidate_recall": 0,
        }
        search_ms_total = 0.0
        query_encode_ms_total = 0.0
        rerank_score_ms_total = 0.0
        all_score_ms_total = 0.0

        for case in CASES:
            search_started = time.perf_counter()
            hits = run_json(
                [
                    args.aidememo,
                    "--store",
                    str(store),
                    "search",
                    case["query"],
                    "--json",
                    "-l",
                    str(args.candidate_limit),
                ]
            )
            search_ms = (time.perf_counter() - search_started) * 1000
            search_ms_total += search_ms

            query_started = time.perf_counter()
            query_embeddings, query_masks = colbert.encode(
                [case["query"]],
                role="query",
                batch_size=1,
            )
            query_ms = (time.perf_counter() - query_started) * 1000
            query_encode_ms_total += query_ms
            query_embedding = query_embeddings[0]
            query_mask = query_masks[0]

            bm25_rank = gold_rank_hits(hits, case["gold"])
            if bm25_rank is not None:
                totals["candidate_recall"] += 1

            hit_to_doc = {content: idx for idx, content in enumerate(documents)}
            candidate_indices = [
                hit_to_doc[str(hit.get("content", ""))]
                for hit in hits
                if str(hit.get("content", "")) in hit_to_doc
            ]

            rerank_started = time.perf_counter()
            rerank_scores = [
                (
                    idx,
                    colbert_score(
                        query_embedding,
                        query_mask,
                        doc_embeddings[idx],
                        doc_masks[idx],
                    ),
                )
                for idx in candidate_indices
            ]
            rerank_ms = (time.perf_counter() - rerank_started) * 1000
            rerank_score_ms_total += rerank_ms

            all_started = time.perf_counter()
            all_scores = [
                (
                    idx,
                    colbert_score(
                        query_embedding,
                        query_mask,
                        doc_embeddings[idx],
                        doc_masks[idx],
                    ),
                )
                for idx in range(len(documents))
            ]
            all_ms = (time.perf_counter() - all_started) * 1000
            all_score_ms_total += all_ms

            rerank_contents = [documents[idx] for idx in rank_scores(rerank_scores)]
            all_contents = [documents[idx] for idx in rank_scores(all_scores)]
            rerank_rank = gold_rank_contents(rerank_contents, case["gold"])
            all_rank = gold_rank_contents(all_contents, case["gold"])

            totals["bm25_mrr"] += reciprocal(bm25_rank)
            totals["colbert_rerank_mrr"] += reciprocal(rerank_rank)
            totals["colbert_all_mrr"] += reciprocal(all_rank)
            totals["bm25_hit1"] += int(bm25_rank == 1)
            totals["colbert_rerank_hit1"] += int(rerank_rank == 1)
            totals["colbert_all_hit1"] += int(all_rank == 1)

            rows.append(
                {
                    "id": case["id"],
                    "scenario": case["scenario"],
                    "query": case["query"],
                    "gold": case["gold"],
                    "candidate_count": len(hits),
                    "bm25_rank": bm25_rank,
                    "colbert_rerank_rank": rerank_rank,
                    "colbert_all_rank": all_rank,
                    "search_ms": round(search_ms, 2),
                    "query_encode_ms": round(query_ms, 2),
                    "rerank_score_ms": round(rerank_ms, 4),
                    "all_score_ms": round(all_ms, 4),
                    "bm25_top": hits[0].get("content") if hits else None,
                    "colbert_rerank_top": rerank_contents[0] if rerank_contents else None,
                    "colbert_all_top": all_contents[0] if all_contents else None,
                }
            )

        n = len(CASES)
        summary = {
            "backend": "mlx-colbert",
            "model_dir": str(args.model_dir),
            "cases": n,
            "candidate_limit": args.candidate_limit,
            "candidate_recall": totals["candidate_recall"] / n,
            "bm25_hit1": totals["bm25_hit1"] / n,
            "colbert_rerank_hit1": totals["colbert_rerank_hit1"] / n,
            "colbert_all_hit1": totals["colbert_all_hit1"] / n,
            "bm25_mrr": totals["bm25_mrr"] / n,
            "colbert_rerank_mrr": totals["colbert_rerank_mrr"] / n,
            "colbert_all_mrr": totals["colbert_all_mrr"] / n,
            "model_load_ms": round(model_ms, 2),
            "document_encode_ms": round(doc_ms, 2),
            "mean_search_ms": round(search_ms_total / n, 2),
            "mean_query_encode_ms": round(query_encode_ms_total / n, 2),
            "mean_rerank_score_ms": round(rerank_score_ms_total / n, 4),
            "mean_all_score_ms": round(all_score_ms_total / n, 4),
        }
        payload: dict[str, Any] = {"summary": summary}
        if not args.summary_only:
            payload["by_scenario"] = mean_by_scenario(rows)
            payload["rows"] = rows
        print(json.dumps(payload, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
