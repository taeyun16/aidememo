#!/usr/bin/env python3
"""Build retrieval fixtures from Hugging Face agent-trace probes.

The fact-type HF probe creates compact candidate-memory rows. This script turns
those rows into a BEIR/MIRACL-style retrieval fixture:

* corpus JSONL with ``doc_id`` + ``text``
* queries JSONL with deterministic surface/paraphrase/CJK-style queries
* qrels TSV with one positive document per query

The labels and queries are weak, deterministic fixtures grounded in public
agent traces. They are useful for relative candidate-recall and latency tests;
they are not a reviewed human relevance set.
"""

from __future__ import annotations

import argparse
import json
import re
from collections import Counter
from pathlib import Path
from typing import Any


STOPWORDS = {
    "about",
    "above",
    "after",
    "agent",
    "arguments",
    "assistant",
    "because",
    "before",
    "call",
    "content",
    "could",
    "dataset",
    "default",
    "during",
    "false",
    "from",
    "have",
    "hermes",
    "into",
    "message",
    "metadata",
    "response",
    "should",
    "status",
    "system",
    "taubench",
    "task",
    "tool",
    "trace",
    "true",
    "used",
    "user",
    "with",
}

LABEL_PROMPTS = {
    "preference": "What preference was expressed",
    "decision": "Which action did the agent choose",
    "lesson": "What did the assistant learn or confirm",
    "error": "What failure happened in the agent run",
    "convention": "Which policy or rule constrained the agent",
    "pattern": "What recurring workflow pattern appears",
    "claim": "What outcome or metadata was recorded",
    "note": "What observation came back from the tool",
    "question": "What task or request was the agent given",
}

KO_LABEL_PROMPTS = {
    "preference": "어떤 선호가 표현됐나",
    "decision": "에이전트가 어떤 행동을 선택했나",
    "lesson": "어시스턴트가 무엇을 배웠거나 확인했나",
    "error": "에이전트 실행에서 어떤 실패가 있었나",
    "convention": "어떤 정책이나 규칙이 에이전트를 제한했나",
    "pattern": "반복되는 워크플로 패턴은 무엇인가",
    "claim": "어떤 결과나 메타데이터가 기록됐나",
    "note": "도구에서 어떤 관찰이 돌아왔나",
    "question": "에이전트가 받은 작업이나 요청은 무엇인가",
}


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows = []
    with path.open(encoding="utf-8") as f:
        for line in f:
            if line.strip():
                rows.append(json.loads(line))
    return rows


def write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")


def token_anchors(text: str, limit: int) -> list[str]:
    tokens = re.findall(r"[A-Za-z][A-Za-z0-9_./:-]{2,}|\d{3,}", text)
    anchors: list[str] = []
    seen: set[str] = set()
    for token in sorted(tokens, key=lambda t: (-len(t), t.lower())):
        lowered = token.lower().strip("._:-/")
        if lowered in STOPWORDS or lowered in seen:
            continue
        seen.add(lowered)
        anchors.append(token.strip(".,;:()[]{}<>"))
        if len(anchors) >= limit:
            break
    return anchors


def compact_text(text: str, limit: int) -> str:
    text = re.sub(r"\s+", " ", text).strip()
    if len(text) <= limit:
        return text
    return text[: limit - 3].rstrip() + "..."


def make_queries(row: dict[str, Any], variants: list[str], max_query_chars: int) -> list[dict[str, Any]]:
    doc_id = str(row["id"])
    text = str(row["text"])
    fact_type = str(row.get("fact_type") or "note")
    scenario = str(row.get("scenario") or "trace")
    source = str(row.get("source") or "huggingface")
    anchors = token_anchors(text, 5)
    anchor_short = " ".join(anchors[:3]) or scenario
    anchor_long = " ".join(anchors[:5]) or compact_text(text, 80)
    prompts = {
        "surface": f"{fact_type} {scenario} {anchor_long}",
        "paraphrase": f"{LABEL_PROMPTS.get(fact_type, LABEL_PROMPTS['note'])} in {source} around {anchor_short}?",
        "cjk": f"{KO_LABEL_PROMPTS.get(fact_type, KO_LABEL_PROMPTS['note'])}? 관련 단서: {anchor_short}",
    }
    out = []
    for variant in variants:
        query = compact_text(prompts[variant], max_query_chars)
        out.append(
            {
                "query_id": f"{doc_id}:{variant}",
                "query": query,
                "source_doc_id": doc_id,
                "scenario": variant,
                "fact_type": fact_type,
                "trace_scenario": scenario,
            }
        )
    return out


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe-jsonl", action="append", required=True, type=Path)
    parser.add_argument("--out-dir", required=True, type=Path)
    parser.add_argument("--prefix", default="hf_agent_trace")
    parser.add_argument("--max-docs", type=int)
    parser.add_argument(
        "--max-docs-per-source",
        type=int,
        help="Keep at most N probe rows per source field before query generation.",
    )
    parser.add_argument("--max-query-chars", type=int, default=180)
    parser.add_argument(
        "--variants",
        default="surface,paraphrase,cjk",
        help="Comma-separated query variants: surface,paraphrase,cjk.",
    )
    args = parser.parse_args()

    variants = [part.strip() for part in args.variants.split(",") if part.strip()]
    unknown = sorted(set(variants) - {"surface", "paraphrase", "cjk"})
    if unknown:
        raise SystemExit(f"unknown variants: {unknown}")

    rows: list[dict[str, Any]] = []
    seen: set[str] = set()
    by_source: Counter[str] = Counter()
    for path in args.probe_jsonl:
        for row in read_jsonl(path):
            key = str(row.get("id") or row.get("text"))
            if key in seen:
                continue
            source = str(row.get("source") or "huggingface")
            if (
                args.max_docs_per_source is not None
                and by_source[source] >= args.max_docs_per_source
            ):
                continue
            seen.add(key)
            rows.append(row)
            by_source[source] += 1
            if args.max_docs is not None and len(rows) >= args.max_docs:
                break
        if args.max_docs is not None and len(rows) >= args.max_docs:
            break

    corpus = [
        {
            "doc_id": str(row["id"]),
            "title": f"{row.get('source', 'huggingface')} {row.get('fact_type', 'note')}",
            "text": str(row["text"]),
            "path": str(row.get("source") or "huggingface"),
            "fact_type": row.get("fact_type"),
            "scenario": row.get("scenario"),
        }
        for row in rows
    ]
    queries: list[dict[str, Any]] = []
    for row in rows:
        queries.extend(make_queries(row, variants, args.max_query_chars))

    corpus_path = args.out_dir / f"{args.prefix}_corpus.jsonl"
    queries_path = args.out_dir / f"{args.prefix}_queries.jsonl"
    qrels_path = args.out_dir / f"{args.prefix}_qrels.tsv"
    write_jsonl(corpus_path, corpus)
    write_jsonl(queries_path, queries)
    qrels_path.write_text(
        "".join(f"{query['query_id']} 0 {query['source_doc_id']} 1\n" for query in queries),
        encoding="utf-8",
    )

    summary = {
        "corpus": str(corpus_path),
        "queries": str(queries_path),
        "qrels": str(qrels_path),
        "documents": len(corpus),
        "queries_count": len(queries),
        "variants": variants,
        "by_source": dict(Counter(str(row.get("source") or "huggingface") for row in rows)),
        "by_fact_type": dict(Counter(str(row.get("fact_type") or "note") for row in rows)),
        "by_query_scenario": dict(Counter(str(row["scenario"]) for row in queries)),
    }
    print(json.dumps(summary, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
