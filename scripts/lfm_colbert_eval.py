#!/usr/bin/env python3
"""Micro-evaluate LFM ColBERT as a late reranker over AideMemo BM25 hits.

This is intentionally small and synthetic. It answers whether the sidecar shape
can improve candidate ordering when BM25 has already surfaced the gold fact.

Usage:
  /private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_colbert_eval.py \
      --aidememo target/debug/aidememo \
      --model LiquidAI/LFM2.5-ColBERT-350M

Requires the current interpreter to have `pylate` and `transformers` installed.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any


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
        "id": "redis-root-cause",
        "query": "what caused redis timeouts",
        "gold": "DNS resolver churn",
    },
    {
        "id": "jwks-stale-keys",
        "query": "why did login keep using old signing keys",
        "gold": "JWKS cache",
    },
    {
        "id": "payment-duplicate-charge",
        "query": "how did we stop duplicate checkout charges",
        "gold": "idempotency key",
    },
    {
        "id": "postgres-speed",
        "query": "what made the database migration finish faster",
        "gold": "batching writes",
    },
    {
        "id": "pending-review",
        "query": "where should uncertain extracted memories go before saving",
        "gold": "pending review",
    },
    {
        "id": "frontend-style",
        "query": "what visual style should the frontend avoid",
        "gold": "purple gradients",
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


def gold_rank(hits: list[dict[str, Any]], gold: str) -> int | None:
    needle = gold.lower()
    for idx, hit in enumerate(hits, start=1):
        if needle in str(hit.get("content", "")).lower():
            return idx
    return None


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


def reciprocal(rank: int | None) -> float:
    if rank is None:
        return 0.0
    return 1.0 / rank


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aidememo", default="target/debug/aidememo")
    parser.add_argument("--model", default="LiquidAI/LFM2.5-ColBERT-350M")
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
        from pylate import models, rank
    except ImportError as exc:
        raise SystemExit("missing dependency: pip install -U pylate transformers") from exc

    model_kwargs: dict[str, Any] = {"model_name_or_path": args.model}
    if args.trust_remote_code:
        model_kwargs["trust_remote_code"] = True

    started = time.perf_counter()
    model = models.ColBERT(**model_kwargs)
    model_ms = (time.perf_counter() - started) * 1000

    with tempfile.TemporaryDirectory(prefix="aidememo-lfm-eval-") as tmp:
        store = args.store or Path(tmp) / "wiki.sqlite"
        if args.store is None:
            seed_store(args.aidememo, store)

        rows = []
        totals = {
            "bm25_mrr": 0.0,
            "colbert_mrr": 0.0,
            "bm25_hit1": 0,
            "colbert_hit1": 0,
            "candidate_recall": 0,
        }
        search_ms_total = 0.0
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

            bm25_rank = gold_rank(hits, case["gold"])
            if bm25_rank is not None:
                totals["candidate_recall"] += 1

            documents = [str(hit.get("content", "")) for hit in hits]
            document_ids = list(range(len(documents)))

            rerank_started = time.perf_counter()
            query_embeddings = model.encode(
                [case["query"]],
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
            rerank_ms = (time.perf_counter() - rerank_started) * 1000
            rerank_ms_total += rerank_ms

            scored = normalize_rerank_rows(reranked)
            reranked_hits = [hits[idx] for idx, _score in scored if 0 <= idx < len(hits)]
            colbert_rank = gold_rank(reranked_hits, case["gold"])

            totals["bm25_mrr"] += reciprocal(bm25_rank)
            totals["colbert_mrr"] += reciprocal(colbert_rank)
            totals["bm25_hit1"] += int(bm25_rank == 1)
            totals["colbert_hit1"] += int(colbert_rank == 1)

            rows.append(
                {
                    "id": case["id"],
                    "query": case["query"],
                    "gold": case["gold"],
                    "candidate_count": len(hits),
                    "bm25_rank": bm25_rank,
                    "colbert_rank": colbert_rank,
                    "delta": None
                    if bm25_rank is None or colbert_rank is None
                    else bm25_rank - colbert_rank,
                    "search_ms": round(search_ms, 2),
                    "colbert_ms": round(rerank_ms, 2),
                    "bm25_top": hits[0].get("content") if hits else None,
                    "colbert_top": reranked_hits[0].get("content") if reranked_hits else None,
                }
            )

        n = len(CASES)
        summary = {
            "model": args.model,
            "trust_remote_code": args.trust_remote_code,
            "cases": n,
            "candidate_limit": args.candidate_limit,
            "candidate_recall": totals["candidate_recall"] / n,
            "bm25_hit1": totals["bm25_hit1"] / n,
            "colbert_hit1": totals["colbert_hit1"] / n,
            "bm25_mrr": totals["bm25_mrr"] / n,
            "colbert_mrr": totals["colbert_mrr"] / n,
            "model_load_ms": round(model_ms, 2),
            "mean_search_ms": round(search_ms_total / n, 2),
            "mean_colbert_ms": round(rerank_ms_total / n, 2),
        }
        print(json.dumps({"summary": summary, "rows": rows}, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
