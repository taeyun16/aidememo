#!/usr/bin/env python3
"""Micro-evaluate an MLX LFM dense embedding model for AideMemo retrieval.

This uses a local/downloaded MLX embedding repository such as:

  mlx-community/LFM2.5-Embedding-350M-4bit

It imports the repository's `lfm2_bidirectional.py` model definition from the
model directory. Inspect that file before running against a new repository.

Usage:
  hf download mlx-community/LFM2.5-Embedding-350M-4bit \
      --local-dir /private/tmp/lfm25-embedding-mlx-4bit

  /private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_dense_eval.py \
      --aidememo target/debug/aidememo \
      --model-dir /private/tmp/lfm25-embedding-mlx-4bit
"""

from __future__ import annotations

import argparse
import importlib.util
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
    embedding_health,
    gold_rank_contents,
    gold_rank_hits,
    mean_by_scenario,
    reciprocal,
    run_json,
    seed_store,
)


def load_model_code(model_dir: Path) -> Any:
    code_path = model_dir / "lfm2_bidirectional.py"
    if not code_path.exists():
        raise SystemExit(f"missing model definition: {code_path}")
    spec = importlib.util.spec_from_file_location("lfm2_bidirectional_local", code_path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"failed to import model definition: {code_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class MlxEmbedder:
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
        model = module.EmbeddingModel(module.ModelArgs.from_dict(config))
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

        self.model = model
        self.tokenizer = AutoTokenizer.from_pretrained(str(model_dir), local_files_only=True)
        prompts = config.get("mlx", {}).get("prompts", {})
        self.query_prefix = prompts.get("query", "query: ")
        self.document_prefix = prompts.get("document", "document: ")

    def encode(self, texts: list[str], role: str, batch_size: int) -> np.ndarray:
        prefix = self.query_prefix if role == "query" else self.document_prefix
        outputs = []
        for start in range(0, len(texts), batch_size):
            chunk = [prefix + text for text in texts[start : start + batch_size]]
            encoded = self.tokenizer(chunk, padding=True, return_tensors="np")
            input_ids = self.mx.array(encoded["input_ids"])
            attention_mask = self.mx.array(encoded["attention_mask"])
            embeddings = self.model.encode(input_ids, attention_mask, normalize=True)
            embeddings = embeddings.astype(self.mx.float32)
            self.mx.eval(embeddings)
            outputs.append(np.array(embeddings))
        return np.concatenate(outputs, axis=0)


def rank_dense(query_embedding: np.ndarray, doc_embeddings: np.ndarray) -> list[int]:
    scores = doc_embeddings @ query_embedding
    return list(np.argsort(-scores))


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
    embedder = MlxEmbedder(args.model_dir)
    model_ms = (time.perf_counter() - model_started) * 1000

    documents = [content for content, _entities, _fact_type in FACTS]
    doc_started = time.perf_counter()
    doc_embeddings = embedder.encode(documents, role="document", batch_size=args.batch_size)
    doc_ms = (time.perf_counter() - doc_started) * 1000
    health = embedding_health(doc_embeddings)

    with tempfile.TemporaryDirectory(prefix="aidememo-lfm-mlx-eval-") as tmp:
        store = args.store or Path(tmp) / "wiki.sqlite"
        if args.store is None:
            seed_store(args.aidememo, store)

        rows = []
        totals = {
            "bm25_mrr": 0.0,
            "dense_mrr": 0.0,
            "dense_rerank_mrr": 0.0,
            "bm25_hit1": 0,
            "dense_hit1": 0,
            "dense_rerank_hit1": 0,
            "candidate_recall": 0,
        }
        search_ms_total = 0.0
        query_ms_total = 0.0
        dense_score_ms_total = 0.0

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
            query_embedding = embedder.encode([case["query"]], role="query", batch_size=1)[0]
            query_ms = (time.perf_counter() - query_started) * 1000
            query_ms_total += query_ms

            bm25_rank = gold_rank_hits(hits, case["gold"])
            if bm25_rank is not None:
                totals["candidate_recall"] += 1

            dense_started = time.perf_counter()
            dense_order = rank_dense(query_embedding, doc_embeddings)
            dense_ms = (time.perf_counter() - dense_started) * 1000
            dense_score_ms_total += dense_ms
            dense_contents = [documents[idx] for idx in dense_order]
            dense_rank = gold_rank_contents(dense_contents, case["gold"])

            hit_to_doc = {content: idx for idx, content in enumerate(documents)}
            candidate_indices = [
                hit_to_doc[str(hit.get("content", ""))]
                for hit in hits
                if str(hit.get("content", "")) in hit_to_doc
            ]
            candidate_scores = [
                (idx, float(doc_embeddings[idx] @ query_embedding)) for idx in candidate_indices
            ]
            candidate_scores.sort(key=lambda item: item[1], reverse=True)
            dense_rerank_contents = [documents[idx] for idx, _score in candidate_scores]
            dense_rerank_rank = gold_rank_contents(dense_rerank_contents, case["gold"])

            totals["bm25_mrr"] += reciprocal(bm25_rank)
            totals["dense_mrr"] += reciprocal(dense_rank)
            totals["dense_rerank_mrr"] += reciprocal(dense_rerank_rank)
            totals["bm25_hit1"] += int(bm25_rank == 1)
            totals["dense_hit1"] += int(dense_rank == 1)
            totals["dense_rerank_hit1"] += int(dense_rerank_rank == 1)

            rows.append(
                {
                    "id": case["id"],
                    "scenario": case["scenario"],
                    "query": case["query"],
                    "gold": case["gold"],
                    "candidate_count": len(hits),
                    "bm25_rank": bm25_rank,
                    "dense_rank": dense_rank,
                    "dense_rerank_rank": dense_rerank_rank,
                    "bm25_to_dense_delta": None
                    if bm25_rank is None or dense_rank is None
                    else bm25_rank - dense_rank,
                    "bm25_to_dense_rerank_delta": None
                    if bm25_rank is None or dense_rerank_rank is None
                    else bm25_rank - dense_rerank_rank,
                    "search_ms": round(search_ms, 2),
                    "query_embed_ms": round(query_ms, 2),
                    "dense_score_ms": round(dense_ms, 4),
                    "bm25_top": hits[0].get("content") if hits else None,
                    "dense_top": dense_contents[0] if dense_contents else None,
                    "dense_rerank_top": dense_rerank_contents[0]
                    if dense_rerank_contents
                    else None,
                }
            )

        n = len(CASES)
        summary = {
            "backend": "mlx",
            "model_dir": str(args.model_dir),
            "embedding_health": health,
            "cases": n,
            "candidate_limit": args.candidate_limit,
            "candidate_recall": totals["candidate_recall"] / n,
            "bm25_hit1": totals["bm25_hit1"] / n,
            "dense_hit1": totals["dense_hit1"] / n,
            "dense_rerank_hit1": totals["dense_rerank_hit1"] / n,
            "bm25_mrr": totals["bm25_mrr"] / n,
            "dense_mrr": totals["dense_mrr"] / n,
            "dense_rerank_mrr": totals["dense_rerank_mrr"] / n,
            "model_load_ms": round(model_ms, 2),
            "document_encode_ms": round(doc_ms, 2),
            "mean_search_ms": round(search_ms_total / n, 2),
            "mean_query_embed_ms": round(query_ms_total / n, 2),
            "mean_dense_score_ms": round(dense_score_ms_total / n, 4),
        }
        payload: dict[str, Any] = {"summary": summary}
        if not args.summary_only:
            payload["by_scenario"] = mean_by_scenario(rows)
            payload["rows"] = rows
        print(json.dumps(payload, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
