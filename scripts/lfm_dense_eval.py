#!/usr/bin/env python3
"""Micro-evaluate LFM dense embeddings for AideMemo retrieval scenarios.

This script separates three questions:

1. Does the local SentenceTransformer load produce non-degenerate vectors?
2. If document vectors are precomputed, how does dense ranking behave by scenario?
3. If BM25 already found candidates, does dense reranking improve head order?

Usage:
  /private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_dense_eval.py \
      --aidememo target/debug/aidememo \
      --model LiquidAI/LFM2.5-Embedding-350M

Use `--trust-remote-code` only after auditing and approving the model repo code.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

import numpy as np


FACTS = [
    (
        "Redis timeout root cause was DNS resolver churn, not pool size.",
        "Redis,DNS",
        "lesson",
    ),
    (
        "Pool size tuning reduced connection pressure but did not fix the timeout incident.",
        "Redis,Pool",
        "note",
    ),
    (
        "Authentication outage was resolved by rotating the JWKS cache after stale signing keys were served.",
        "Auth,JWKS",
        "lesson",
    ),
    (
        "Login token failures increased after password reset emails were delayed.",
        "Auth,Login",
        "note",
    ),
    (
        "Stale CDN cache headers caused avatar images to stay outdated after uploads.",
        "CDN,Images",
        "claim",
    ),
    (
        "The payment double-charge bug was fixed by adding an idempotency key to checkout retries.",
        "Payments,Checkout",
        "lesson",
    ),
    (
        "Checkout retry logs showed duplicate requests from mobile clients during poor network handoff.",
        "Checkout,Mobile",
        "claim",
    ),
    (
        "Postgres index migration completed in 14 minutes after batching writes.",
        "Postgres,Migration",
        "claim",
    ),
    (
        "Database migration planning notes still need rollback drills before release.",
        "Database,Migration",
        "question",
    ),
    (
        "Frontend palette work should avoid purple gradients and marketing-style hero sections.",
        "Frontend,Design",
        "convention",
    ),
    (
        "Claude hook preview should put candidate facts in pending review instead of saving ambiguous notes.",
        "Hooks,Pending",
        "pattern",
    ),
    (
        "Ambiguous notes often need agent-level classification before they become durable facts.",
        "Agents,Classification",
        "pattern",
    ),
]


CASES = [
    {
        "id": "surface-redis-root-cause",
        "scenario": "surface-overlap",
        "query": "redis timeout root cause",
        "gold": "DNS resolver churn",
    },
    {
        "id": "paraphrase-jwks-stale-keys",
        "scenario": "paraphrase",
        "query": "why did login keep using old signing keys",
        "gold": "JWKS cache",
    },
    {
        "id": "paraphrase-payment-duplicate-charge",
        "scenario": "paraphrase",
        "query": "how did we stop duplicate checkout charges",
        "gold": "idempotency key",
    },
    {
        "id": "lexical-distractor-postgres-speed",
        "scenario": "lexical-distractor",
        "query": "what made the database migration finish faster",
        "gold": "batching writes",
    },
    {
        "id": "workflow-pending-review",
        "scenario": "workflow-memory",
        "query": "where should uncertain extracted memories go before saving",
        "gold": "pending review",
    },
    {
        "id": "style-frontend",
        "scenario": "preference-convention",
        "query": "what visual style should the frontend avoid",
        "gold": "purple gradients",
    },
    {
        "id": "ko-redis-root-cause",
        "scenario": "cross-lingual-query",
        "query": "레디스 타임아웃의 원인이 뭐였지",
        "gold": "DNS resolver churn",
    },
]


def run_json(cmd: list[str]) -> Any:
    proc = subprocess.run(cmd, check=True, text=True, capture_output=True)
    return json.loads(proc.stdout)


def run(cmd: list[str]) -> None:
    subprocess.run(cmd, check=True, text=True, capture_output=True)


def seed_store(aidememo: str, store: Path) -> None:
    for content, entities, fact_type in FACTS:
        run(
            [
                aidememo,
                "--store",
                str(store),
                "fact",
                "add",
                content,
                "--entities",
                entities,
                "--type",
                fact_type,
            ]
        )


def gold_rank_contents(contents: list[str], gold: str) -> int | None:
    needle = gold.lower()
    for idx, content in enumerate(contents, start=1):
        if needle in content.lower():
            return idx
    return None


def gold_rank_hits(hits: list[dict[str, Any]], gold: str) -> int | None:
    return gold_rank_contents([str(hit.get("content", "")) for hit in hits], gold)


def reciprocal(rank: int | None) -> float:
    if rank is None:
        return 0.0
    return 1.0 / rank


def rank_dense(query_embedding: np.ndarray, doc_embeddings: np.ndarray) -> list[int]:
    scores = doc_embeddings @ query_embedding
    return list(np.argsort(-scores))


def embedding_health(doc_embeddings: np.ndarray) -> dict[str, Any]:
    if len(doc_embeddings) < 2:
        return {"valid": False, "reason": "not enough documents"}
    diffs = []
    for idx in range(1, len(doc_embeddings)):
        diffs.append(float(np.max(np.abs(doc_embeddings[0] - doc_embeddings[idx]))))
    max_diff = max(diffs) if diffs else 0.0
    return {
        "valid": max_diff > 1e-6,
        "max_pairwise_diff_from_first": max_diff,
        "doc_norm_min": float(np.min(np.linalg.norm(doc_embeddings, axis=1))),
        "doc_norm_max": float(np.max(np.linalg.norm(doc_embeddings, axis=1))),
    }


def mean_by_scenario(rows: list[dict[str, Any]]) -> dict[str, Any]:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        grouped.setdefault(str(row["scenario"]), []).append(row)
    out = {}
    for scenario, values in grouped.items():
        out[scenario] = {
            "cases": len(values),
            "bm25_hit1": sum(v["bm25_rank"] == 1 for v in values) / len(values),
            "dense_hit1": sum(v["dense_rank"] == 1 for v in values) / len(values),
            "dense_rerank_hit1": sum(v["dense_rerank_rank"] == 1 for v in values)
            / len(values),
            "bm25_mrr": sum(reciprocal(v["bm25_rank"]) for v in values) / len(values),
            "dense_mrr": sum(reciprocal(v["dense_rank"]) for v in values) / len(values),
            "dense_rerank_mrr": sum(reciprocal(v["dense_rerank_rank"]) for v in values)
            / len(values),
        }
    return out


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aidememo", default="target/debug/aidememo")
    parser.add_argument("--model", default="LiquidAI/LFM2.5-Embedding-350M")
    parser.add_argument("--candidate-limit", type=int, default=8)
    parser.add_argument("--batch-size", type=int, default=32)
    parser.add_argument("--store", type=Path)
    parser.add_argument(
        "--trust-remote-code",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Enable only after auditing the model repo; default avoids executing Hub code.",
    )
    args = parser.parse_args()

    try:
        from sentence_transformers import SentenceTransformer
    except ImportError as exc:
        raise SystemExit("missing dependency: pip install -U sentence-transformers") from exc

    model_started = time.perf_counter()
    model = SentenceTransformer(args.model, trust_remote_code=args.trust_remote_code)
    model_ms = (time.perf_counter() - model_started) * 1000

    documents = [content for content, _entities, _fact_type in FACTS]
    doc_started = time.perf_counter()
    doc_embeddings = model.encode(
        documents,
        prompt_name="document",
        normalize_embeddings=True,
        batch_size=args.batch_size,
        show_progress_bar=False,
    )
    doc_embeddings = np.asarray(doc_embeddings)
    doc_ms = (time.perf_counter() - doc_started) * 1000
    health = embedding_health(doc_embeddings)

    with tempfile.TemporaryDirectory(prefix="aidememo-lfm-dense-eval-") as tmp:
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
        rerank_ms_total = 0.0

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
            query_embedding = model.encode(
                [case["query"]],
                prompt_name="query",
                normalize_embeddings=True,
                batch_size=args.batch_size,
                show_progress_bar=False,
            )
            query_embedding = np.asarray(query_embedding)[0]
            query_ms = (time.perf_counter() - query_started) * 1000
            query_ms_total += query_ms

            bm25_rank = gold_rank_hits(hits, case["gold"])
            if bm25_rank is not None:
                totals["candidate_recall"] += 1

            dense_started = time.perf_counter()
            dense_order = rank_dense(query_embedding, doc_embeddings)
            dense_ms = (time.perf_counter() - dense_started) * 1000
            rerank_ms_total += dense_ms
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
            "model": args.model,
            "trust_remote_code": args.trust_remote_code,
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
            "mean_dense_score_ms": round(rerank_ms_total / n, 4),
        }
        print(
            json.dumps(
                {
                    "summary": summary,
                    "by_scenario": mean_by_scenario(rows),
                    "rows": rows,
                },
                indent=2,
                ensure_ascii=False,
            )
        )


if __name__ == "__main__":
    main()
