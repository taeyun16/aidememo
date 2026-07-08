#!/usr/bin/env python3
"""Evaluate LFM MLX dense retrieval on the real AideMemo docs corpus.

This is the larger follow-up to ``lfm_mlx_dense_eval.py``. It builds a temporary
AideMemo store from the repository's actual Markdown docs, validates a curated
gold-query set against those chunks, then compares:

* ``aidememo search`` BM25
* ``aidememo search --hybrid`` with the current AideMemo semantic provider
  (model2vec by default, skipped if the local model/cache is unavailable)
* ``mlx-community/LFM2.5-Embedding-350M-4bit`` dense all-document ranking
* a simulated BM25-confidence gate that uses LFM only when BM25 looks weak

Usage:

  /private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_docs_recall_eval.py \
      --aidememo target/debug/aidememo \
      --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
      --summary-only
"""

from __future__ import annotations

import argparse
import json
import os
import re
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


DEFAULT_DOCS = [
    "README.md",
    "AGENTS.md",
    "docs/BRANCHES.md",
    "docs/CLI.md",
    "docs/FEATURES.md",
    "docs/INSTALLATION.md",
    "docs/INTRODUCTION.md",
    "docs/MCP.md",
    "docs/MEASUREMENTS.md",
    "docs/OPERATIONS.md",
    "docs/QUICKSTART.md",
    "docs/RELEASE.md",
    "docs/SDK.md",
    "docs/SDK_POSITIONING.md",
    "docs/SKILLOPT_LITE.md",
    "scripts/README.md",
    "packages/aidememo-agent-sdk/README.md",
]


DEFAULT_CASES: list[dict[str, str]] = [
    {
        "id": "surface-agent-sdk-memory",
        "scenario": "surface-overlap",
        "query": "agent friendly SDK memory Memory.open search_rows coverage_by",
        "path": "README.md",
        "gold": "Memory.open",
    },
    {
        "id": "surface-workflow-start",
        "scenario": "surface-overlap",
        "query": "sparse ticket workflow start decisions lessons errors",
        "path": "README.md",
        "gold": "Creates a tracked session",
    },
    {
        "id": "surface-source-id",
        "scenario": "surface-overlap",
        "query": "source_id isolate shared store",
        "path": "docs/OPERATIONS.md",
        "gold": "neighbouring project",
    },
    {
        "id": "surface-vector-rebuild",
        "scenario": "surface-overlap",
        "query": "rebuild HNSW vector sidecar after model changes",
        "path": "docs/FEATURES.md",
        "gold": "Rebuild the HNSW vector sidecar",
    },
    {
        "id": "surface-mcp-tool-schema",
        "scenario": "surface-overlap",
        "query": "where do MCP tool schemas live list_tools",
        "path": "AGENTS.md",
        "gold": "cmd/mcp_tools.rs::list_tools()",
    },
    {
        "id": "surface-bpaf-usage-bug",
        "scenario": "surface-overlap",
        "query": "bpaf usage BUG positional fields rightmost",
        "path": "AGENTS.md",
        "gold": "bpaf usage BUG",
    },
    {
        "id": "surface-redb-single-writer",
        "scenario": "surface-overlap",
        "query": "redb daemon single writer cannot open store",
        "path": "AGENTS.md",
        "gold": "redb is single-writer",
    },
    {
        "id": "surface-shadow-log",
        "scenario": "surface-overlap",
        "query": "AIDEMEMO_FACT_TYPE_SHADOW_LOG label_source fact_type_hint",
        "path": "docs/OPERATIONS.md",
        "gold": "AIDEMEMO_FACT_TYPE_SHADOW_LOG",
    },
    {
        "id": "surface-lfm-4bit-placement",
        "scenario": "surface-overlap",
        "query": "LFM2.5 Embedding 350M 4bit first-stage semantic retrieval",
        "path": "docs/MEASUREMENTS.md",
        "gold": "mlx-community/LFM2.5-Embedding-350M-4bit",
    },
    {
        "id": "surface-longmemeval-r5",
        "scenario": "surface-overlap",
        "query": "LongMemEval-S R@5 improved from 96.2 to 98.0",
        "path": "docs/MEASUREMENTS.md",
        "gold": "LongMemEval-S R@5 improved",
    },
    {
        "id": "surface-miracl-ko",
        "scenario": "surface-overlap",
        "query": "MIRACL ko improved MRR nDCG rerank",
        "path": "docs/MEASUREMENTS.md",
        "gold": "MIRACL/ko improved",
    },
    {
        "id": "surface-branch-push-base",
        "scenario": "surface-overlap",
        "query": "branch push base backup exports records written after backup",
        "path": "docs/BRANCHES.md",
        "gold": "branch push --base <BACKUP>",
    },
    {
        "id": "surface-backup-sync-cursor",
        "scenario": "surface-overlap",
        "query": "backup manifest records sync cursor branch push base",
        "path": "docs/OPERATIONS.md",
        "gold": "backup manifest records a sync cursor",
    },
    {
        "id": "surface-aggregate-sum-currency",
        "scenario": "surface-overlap",
        "query": "aidememo aggregate sum_currency sum_duration count distinct dates",
        "path": "AGENTS.md",
        "gold": "sum_currency",
    },
    {
        "id": "paraphrase-no-hosted-memory",
        "scenario": "paraphrase",
        "query": "what makes this local instead of a managed vector database service",
        "path": "README.md",
        "gold": "not a hosted memory SaaS",
    },
    {
        "id": "paraphrase-history-replacement",
        "scenario": "paraphrase",
        "query": "how does the system remember that an old fact was replaced",
        "path": "README.md",
        "gold": "validity",
    },
    {
        "id": "paraphrase-code-agent-sdk",
        "scenario": "paraphrase",
        "query": "which interface should an agent use when it needs fanout retrieval in code",
        "path": "README.md",
        "gold": "Use MCP tools for one-off",
    },
    {
        "id": "paraphrase-exact-arithmetic",
        "scenario": "paraphrase",
        "query": "what pulls the reader out of doing totals in its own head",
        "path": "AGENTS.md",
        "gold": "Pulls agent out of in-head arithmetic",
    },
    {
        "id": "paraphrase-auto-hybrid-gate",
        "scenario": "paraphrase",
        "query": "how do we avoid paying semantic search cost unless lexical recall is weak",
        "path": "docs/OPERATIONS.md",
        "gold": "AideMemo first runs a BM25 probe",
    },
    {
        "id": "paraphrase-daemon-warmup",
        "scenario": "paraphrase",
        "query": "which mode moves embedding startup cost before the first user query",
        "path": "docs/MEASUREMENTS.md",
        "gold": "prewarms the semantic provider",
    },
    {
        "id": "paraphrase-pending-review",
        "scenario": "paraphrase",
        "query": "where should uncertain extracted memories wait before becoming durable",
        "path": "README.md",
        "gold": "pending review queue",
    },
    {
        "id": "paraphrase-no-lfm-router",
        "scenario": "paraphrase",
        "query": "why should route selection stay rules based instead of spending a small model call",
        "path": "docs/MEASUREMENTS.md",
        "gold": "Do not use the tested LFM text-generation models as AideMemo's query router",
    },
    {
        "id": "paraphrase-corpus-lora",
        "scenario": "paraphrase",
        "query": "which trained adapter is the current best fact type sidecar placement",
        "path": "docs/MEASUREMENTS.md",
        "gold": "corpus-only adapter is the best current placement",
    },
    {
        "id": "paraphrase-reader-bound-rerank",
        "scenario": "paraphrase",
        "query": "when should cross encoder reranking stay disabled despite better ordering",
        "path": "docs/MEASUREMENTS.md",
        "gold": "Reader-bound agent loops",
    },
    {
        "id": "paraphrase-source-default-install",
        "scenario": "paraphrase",
        "query": "how can MCP clients avoid passing the namespace on every call",
        "path": "README.md",
        "gold": "AIDEMEMO_SOURCE_ID",
    },
    {
        "id": "ko-shared-store-isolation",
        "scenario": "cross-lingual-query",
        "query": "여러 에이전트가 같은 저장소를 쓸 때 서로의 메모리가 섞이지 않게 하는 옵션은?",
        "path": "docs/OPERATIONS.md",
        "gold": "neighbouring project",
    },
    {
        "id": "ko-auto-hybrid-prewarm",
        "scenario": "cross-lingual-query",
        "query": "자동 하이브리드 검색에서 첫 사용자 쿼리 전에 모델을 미리 켜는 건 어디서 설명해?",
        "path": "docs/MEASUREMENTS.md",
        "gold": "prewarms the semantic provider",
    },
    {
        "id": "ko-shadow-fact-type",
        "scenario": "cross-lingual-query",
        "query": "팩트 타입을 나중에 학습하려고 성공한 쓰기 로그를 따로 남기는 환경 변수는?",
        "path": "docs/OPERATIONS.md",
        "gold": "AIDEMEMO_FACT_TYPE_SHADOW_LOG",
    },
    {
        "id": "ko-lfm-router-warning",
        "scenario": "cross-lingual-query",
        "query": "작은 LFM 생성 모델을 쿼리 라우터로 바로 쓰지 말라는 근거는?",
        "path": "docs/MEASUREMENTS.md",
        "gold": "Do not use the tested LFM text-generation models as AideMemo's query router",
    },
    {
        "id": "ko-branch-sync",
        "scenario": "cross-lingual-query",
        "query": "백업 이후 바뀐 브랜치 세그먼트만 내보내는 명령은?",
        "path": "docs/BRANCHES.md",
        "gold": "branch push --base <BACKUP>",
    },
    {
        "id": "ko-sdk-code-path",
        "scenario": "cross-lingual-query",
        "query": "코드를 실행할 수 있는 에이전트가 중간 검색 결과를 직접 다룰 때 쓰는 패키지는?",
        "path": "docs/SDK_POSITIONING.md",
        "gold": "aidememo-agent-sdk",
    },
    {
        "id": "ko-vector-rebuild",
        "scenario": "cross-lingual-query",
        "query": "임베딩 모델을 바꾼 뒤 HNSW를 다시 만드는 명령은?",
        "path": "AGENTS.md",
        "gold": "aidememo vector-rebuild",
    },
]


@dataclass(frozen=True)
class Chunk:
    id: str
    path: str
    heading: str
    text: str
    doc_id: str | None = None

    @property
    def content(self) -> str:
        return f"[chunk:{self.id}]\npath: {self.path}\nheading: {self.heading}\n\n{self.text}"


def run_json(cmd: list[str], env: dict[str, str] | None = None) -> Any:
    proc = subprocess.run(cmd, check=True, text=True, capture_output=True, env=env)
    return json.loads(proc.stdout)


def run_capture(
    cmd: list[str],
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, check=True, text=True, capture_output=True, env=env)


def free_loopback_port() -> int:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])
    finally:
        sock.close()


def wait_health(url: str, timeout_s: float) -> None:
    deadline = time.perf_counter() + timeout_s
    health_url = f"{url.rstrip('/')}/health"
    last_error = ""
    while time.perf_counter() < deadline:
        try:
            with urllib.request.urlopen(health_url, timeout=0.5) as resp:
                if resp.status == 200:
                    return
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            last_error = str(exc)
        time.sleep(0.25)
    raise TimeoutError(f"daemon health timeout for {health_url}: {last_error}")


def start_mcp_daemon(
    aidememo: str,
    store: Path,
    timeout_s: float,
    env_base: dict[str, str] | None = None,
) -> tuple[subprocess.Popen[str], str, float]:
    port = free_loopback_port()
    url = f"http://127.0.0.1:{port}"
    env = (env_base or os.environ).copy()
    env["AIDEMEMO_PREWARM_SEMANTIC"] = "1"
    env.setdefault("RUST_LOG", "error")
    started = time.perf_counter()
    proc = subprocess.Popen(
        [aidememo, "--store", str(store), "mcp-serve", "--port", str(port)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
        env=env,
    )
    try:
        wait_health(url, timeout_s)
    except Exception:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        raise
    return proc, url, (time.perf_counter() - started) * 1000


def stop_process(proc: subprocess.Popen[str] | None) -> None:
    if proc is None or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)


def mcp_search(
    base_url: str,
    query: str,
    limit: int,
    *,
    bm25_only: bool,
    auto_hybrid: bool = False,
) -> list[dict[str, Any]]:
    body = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "aidememo_search",
            "arguments": {
                "query": query,
                "limit": limit,
                "bm25_only": bm25_only,
                "auto_hybrid": auto_hybrid,
                "current_only": False,
                "format": "full",
            },
        },
    }
    request = urllib.request.Request(
        f"{base_url.rstrip('/')}/mcp",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
    if "error" in payload:
        raise RuntimeError(f"daemon error: {payload['error']}")
    text = payload["result"]["content"][0]["text"]
    return json.loads(text)["results"]


def strip_frontmatter(text: str) -> str:
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return text
    for idx in range(1, len(lines)):
        if lines[idx].strip() == "---":
            return "\n".join(lines[idx + 1 :])
    return text


def split_long_block(block: str, max_chars: int) -> list[str]:
    if len(block) <= max_chars:
        return [block]
    pieces: list[str] = []
    current: list[str] = []
    current_len = 0
    for line in block.splitlines():
        next_len = current_len + len(line) + 1
        if current and next_len > max_chars:
            pieces.append("\n".join(current).strip())
            current = []
            current_len = 0
        current.append(line)
        current_len += len(line) + 1
    if current:
        pieces.append("\n".join(current).strip())
    return [piece for piece in pieces if piece]


def stable_chunk_id(raw: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9_.:/=-]+", "_", raw.strip())
    return cleaned.strip("_") or "doc"


def chunk_section(
    path: str,
    heading: str,
    body: str,
    start_idx: int,
    max_chars: int,
    *,
    doc_id: str | None = None,
) -> list[Chunk]:
    paragraphs = [p.strip() for p in re.split(r"\n\s*\n", body.strip()) if p.strip()]
    if not paragraphs:
        return []
    chunks: list[Chunk] = []
    current: list[str] = []
    for para in paragraphs:
        para_parts = split_long_block(para, max_chars)
        for part in para_parts:
            candidate = "\n\n".join([*current, part]).strip()
            if current and len(candidate) > max_chars:
                idx = start_idx + len(chunks)
                chunks.append(
                    Chunk(
                        id=f"{stable_chunk_id(doc_id or path)}#{idx:04d}",
                        path=path,
                        heading=heading,
                        text="\n\n".join(current).strip(),
                        doc_id=doc_id,
                    )
                )
                current = [part]
            else:
                current = [*current, part]
    if current:
        idx = start_idx + len(chunks)
        chunks.append(
            Chunk(
                id=f"{stable_chunk_id(doc_id or path)}#{idx:04d}",
                path=path,
                heading=heading,
                text="\n\n".join(current).strip(),
                doc_id=doc_id,
            )
        )
    return chunks


def chunk_markdown(path: Path, root: Path, max_chars: int) -> list[Chunk]:
    rel = path.relative_to(root).as_posix()
    text = strip_frontmatter(path.read_text(encoding="utf-8"))
    chunks: list[Chunk] = []
    heading = "(preamble)"
    body: list[str] = []
    for line in text.splitlines():
        match = re.match(r"^(#{1,6})\s+(.+?)\s*$", line)
        if match:
            chunks.extend(chunk_section(rel, heading, "\n".join(body), len(chunks), max_chars))
            heading = match.group(2).strip()
            body = [line]
        else:
            body.append(line)
    chunks.extend(chunk_section(rel, heading, "\n".join(body), len(chunks), max_chars))
    return chunks


def collect_docs(root: Path, patterns: list[str], max_chars: int) -> list[Chunk]:
    seen: set[Path] = set()
    chunks: list[Chunk] = []
    for pattern in patterns:
        for path in sorted(root.glob(pattern)):
            if path in seen or not path.is_file():
                continue
            seen.add(path)
            chunks.extend(chunk_markdown(path, root, max_chars))
    return chunks


def first_str(row: dict[str, Any], keys: list[str]) -> str:
    for key in keys:
        value = row.get(key)
        if value is not None and str(value).strip():
            return str(value).strip()
    return ""


def load_corpus_jsonl(path: Path, max_chars: int) -> list[Chunk]:
    chunks: list[Chunk] = []
    with path.open(encoding="utf-8") as f:
        for line_no, line in enumerate(f, start=1):
            if not line.strip():
                continue
            row = json.loads(line)
            doc_id = first_str(row, ["doc_id", "docid", "id", "pid", "_id"])
            text = first_str(row, ["text", "content", "contents", "passage", "body"])
            title = first_str(row, ["title", "heading"]) or "(jsonl)"
            source_path = first_str(row, ["path", "source", "url"]) or doc_id or f"{path}:{line_no}"
            if not doc_id:
                doc_id = f"{path.stem}-{line_no}"
            if not text:
                raise SystemExit(f"{path}:{line_no}: corpus row missing text/content")
            body = f"# {title}\n\n{text}" if title and title != "(jsonl)" else text
            chunks.extend(
                chunk_section(
                    source_path,
                    title,
                    body,
                    len(chunks),
                    max_chars,
                    doc_id=doc_id,
                )
            )
    return chunks


def load_queries_jsonl(path: Path) -> dict[str, dict[str, str]]:
    queries: dict[str, dict[str, str]] = {}
    with path.open(encoding="utf-8") as f:
        for line_no, line in enumerate(f, start=1):
            if not line.strip():
                continue
            row = json.loads(line)
            query_id = first_str(row, ["query_id", "qid", "id", "_id"])
            query = first_str(row, ["query", "question", "text"])
            if not query_id or not query:
                raise SystemExit(f"{path}:{line_no}: query row needs id/query fields")
            queries[query_id] = {
                "query": query,
                "scenario": first_str(row, ["scenario"]) or "",
            }
    return queries


def load_qrels_tsv(
    path: Path,
    *,
    min_score: float,
) -> dict[str, list[str]]:
    qrels: dict[str, list[str]] = {}
    with path.open(encoding="utf-8") as f:
        for line_no, line in enumerate(f, start=1):
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            parts = stripped.split()
            if len(parts) < 2:
                raise SystemExit(f"{path}:{line_no}: qrels row needs at least query_id doc_id")
            if len(parts) >= 4:
                query_id, doc_id, score_s = parts[0], parts[2], parts[3]
            elif len(parts) == 3:
                query_id, doc_id, score_s = parts[0], parts[1], parts[2]
            else:
                query_id, doc_id, score_s = parts[0], parts[1], "1"
            try:
                score = float(score_s)
            except ValueError as exc:
                raise SystemExit(f"{path}:{line_no}: invalid qrel score {score_s!r}") from exc
            if score >= min_score:
                qrels.setdefault(query_id, []).append(doc_id)
    return qrels


def cases_from_qrels(
    queries_path: Path,
    qrels_path: Path,
    *,
    min_score: float,
    max_cases: int | None,
    scenario: str,
) -> list[dict[str, Any]]:
    queries = load_queries_jsonl(queries_path)
    qrels = load_qrels_tsv(qrels_path, min_score=min_score)
    cases: list[dict[str, Any]] = []
    for query_id in sorted(qrels):
        query_row = queries.get(query_id)
        if query_row is None:
            continue
        cases.append(
            {
                "id": query_id,
                "scenario": query_row.get("scenario") or scenario,
                "query": query_row["query"],
                "gold_doc_ids": sorted(set(qrels[query_id])),
            }
        )
        if max_cases is not None and len(cases) >= max_cases:
            break
    if not cases:
        raise SystemExit("no cases matched queries/qrels")
    return cases


def load_cases(path: Path | None) -> list[dict[str, Any]]:
    if path is None:
        return DEFAULT_CASES
    if path.suffix == ".json":
        data = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(data, list):
            raise SystemExit("case JSON must be a list")
        return data
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            rows.append(json.loads(line))
    return rows


def as_list(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        return [str(v) for v in value]
    return [str(value)]


def chunk_matches_doc_id(chunk: Chunk, doc_id: str) -> bool:
    safe = stable_chunk_id(doc_id)
    return (
        chunk.doc_id == doc_id
        or chunk.path == doc_id
        or chunk.id == doc_id
        or chunk.id.startswith(f"{safe}#")
    )


def validate_cases(chunks: list[Chunk], cases: list[dict[str, Any]]) -> dict[str, list[str]]:
    gold_by_case: dict[str, list[str]] = {}
    missing = []
    for case in cases:
        path = case.get("path")
        gold_doc_ids = as_list(case.get("gold_doc_ids") or case.get("gold_doc_id"))
        gold_ids = as_list(case.get("gold_ids") or case.get("gold_id"))
        matches: list[str] = []
        if gold_doc_ids or gold_ids:
            gold_id_set = {stable_chunk_id(gold_id) for gold_id in gold_ids}
            for chunk in chunks:
                if chunk.id in gold_ids or stable_chunk_id(chunk.id) in gold_id_set:
                    matches.append(chunk.id)
                    continue
                if any(chunk_matches_doc_id(chunk, doc_id) for doc_id in gold_doc_ids):
                    matches.append(chunk.id)
        elif "gold" in case:
            gold = str(case["gold"]).lower()
            matches = [
                chunk.id
                for chunk in chunks
                if (not path or chunk.path == path) and gold in chunk.content.lower()
            ]
        else:
            raise SystemExit(f"case {case.get('id')}: expected gold/gold_id/gold_doc_id")
        if not matches:
            missing.append(
                f"{case['id']} path={path!r} gold={case.get('gold')!r} "
                f"gold_ids={gold_ids!r} gold_doc_ids={gold_doc_ids!r}"
            )
        gold_by_case[case["id"]] = matches
    if missing:
        raise SystemExit("gold text not found in docs chunks:\n" + "\n".join(missing))
    return gold_by_case


ULID_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def stable_ulid(timestamp_ms: int, counter: int) -> str:
    value = ((timestamp_ms & ((1 << 48) - 1)) << 80) | (counter & ((1 << 80) - 1))
    chars = []
    for shift in range(125, -1, -5):
        chars.append(ULID_ALPHABET[(value >> shift) & 31])
    return "".join(chars)


def write_import_jsonl(path: Path, chunks: list[Chunk]) -> None:
    now_ms = int(time.time() * 1000)
    entity_id = stable_ulid(now_ms, 0)
    lines = [
        json.dumps({"schema_version": 1, "exported_by": "aidememo docs recall eval"}),
        json.dumps(
            {
                "type": "entity",
                "data": {
                    "id": entity_id,
                    "name": "AideMemoDocs",
                    "name_lower": "aidememodocs",
                    "entity_type": "unknown",
                    "aliases": [],
                    "tags": [],
                    "source_page": None,
                    "summary": None,
                    "summary_updated_at": None,
                    "created_at": now_ms,
                    "updated_at": now_ms,
                },
            },
            ensure_ascii=False,
        ),
    ]
    for offset, chunk in enumerate(chunks, start=1):
        lines.append(
            json.dumps(
                {
                    "type": "fact",
                    "data": {
                        "id": stable_ulid(now_ms, offset),
                        "content": chunk.content,
                        "fact_type": "note",
                        "entity_ids": [entity_id],
                        "tags": ["docs-recall-eval"],
                        "source": chunk.path,
                        "source_id": "docs-recall-eval",
                        "source_confidence": 1.0,
                        "relevance_score": 0.5,
                        "created_at": now_ms,
                        "updated_at": now_ms,
                        "observed_at": None,
                        "superseded_at": None,
                        "superseded_by": None,
                        "access_count": 0,
                        "last_accessed_at": now_ms,
                        "pinned": False,
                    },
                },
                ensure_ascii=False,
            )
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def seed_store(aidememo: str, store: Path, chunks: list[Chunk]) -> float:
    started = time.perf_counter()
    import_path = store.with_suffix(".jsonl")
    write_import_jsonl(import_path, chunks)
    run_capture([aidememo, "--store", str(store), "import", str(import_path)])
    return (time.perf_counter() - started) * 1000


def method_slug(prefix: str, model: str) -> str:
    slug = re.sub(r"[^A-Za-z0-9]+", "_", model.lower()).strip("_")
    return f"{prefix}_{slug}" if slug else prefix


def temp_home_env(home: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    env.setdefault("RUST_LOG", "error")
    return env


def configure_embedding_profile(
    aidememo: str,
    env: dict[str, str],
    *,
    provider: str,
    model: str,
    cache_dir: Path | None,
) -> None:
    settings = [
        ("model.provider", provider),
        ("model.name", model),
        ("search.semantic_index", "hnsw"),
        ("search.auto_hybrid", "false"),
    ]
    if cache_dir is not None:
        settings.append(("model.cache_dir", str(cache_dir)))
        settings.append(("model.download_dir", str(cache_dir / "downloads")))
    for key, value in settings:
        run_capture([aidememo, "config", "set", key, value], env=env)


def evaluate_semantic_profile(
    *,
    aidememo: str,
    store: Path,
    rows: list[dict[str, Any]],
    cases: list[dict[str, Any]],
    gold_by_case: dict[str, list[str]],
    method: str,
    env: dict[str, str],
    limit: int,
    daemon_start_timeout: float,
) -> dict[str, Any]:
    stats: dict[str, Any] = {"available": False}
    rebuild_started = time.perf_counter()
    try:
        proc = run_capture(
            [
                aidememo,
                "--store",
                str(store),
                "vector-rebuild",
                "--current-only",
                "--json",
            ],
            env=env,
        )
        stats.update(
            {
                "available": True,
                "rebuild_ms": round((time.perf_counter() - rebuild_started) * 1000, 2),
                "stdout": proc.stdout.strip(),
            }
        )
    except subprocess.CalledProcessError as exc:
        stats.update(
            {
                "rebuild_ms": round((time.perf_counter() - rebuild_started) * 1000, 2),
                "stderr": exc.stderr.strip()[-2000:],
            }
        )
        return stats

    daemon: subprocess.Popen[str] | None = None
    query_ms: list[float] = []
    try:
        daemon, url, daemon_ms = start_mcp_daemon(
            aidememo,
            store,
            daemon_start_timeout,
            env_base=env,
        )
        stats["daemon_start_ms"] = round(daemon_ms, 2)
        rows_by_id = {str(row["id"]): row for row in rows}
        for case in cases:
            started = time.perf_counter()
            hits = mcp_search(url, case["query"], limit, bm25_only=False)
            query_ms.append((time.perf_counter() - started) * 1000)
            rank = rank_hits(hits, set(gold_by_case[case["id"]]))
            row = rows_by_id[str(case["id"])]
            row[f"{method}_rank"] = rank
            row[f"{method}_top"] = (
                chunk_id_from_content(str(hits[0].get("content", ""))) if hits else None
            )
        stats["search_ms_mean"] = round(mean(query_ms), 2)
    except (TimeoutError, OSError, RuntimeError, urllib.error.URLError) as exc:
        stats["search_error"] = str(exc)[-2000:]
    finally:
        stop_process(daemon)
    return stats


CHUNK_RE = re.compile(r"\[chunk:([^\]]+)\]")


def chunk_id_from_content(content: str) -> str | None:
    match = CHUNK_RE.search(content)
    return match.group(1) if match else None


def rank_gold_ids(ids: list[str], gold_ids: set[str]) -> int | None:
    for idx, chunk_id in enumerate(ids, start=1):
        if chunk_id in gold_ids:
            return idx
    return None


def rank_hits(hits: list[dict[str, Any]], gold_ids: set[str]) -> int | None:
    ids = [
        chunk_id
        for chunk_id in (chunk_id_from_content(str(hit.get("content", ""))) for hit in hits)
        if chunk_id is not None
    ]
    return rank_gold_ids(ids, gold_ids)


def metric_row(rank: int | None) -> dict[str, float | int]:
    return {
        "recall": int(rank is not None),
        "hit1": int(rank == 1),
        "mrr": reciprocal(rank),
    }


def summarize_method(rows: list[dict[str, Any]], method: str) -> dict[str, Any]:
    ranks = [row.get(f"{method}_rank") for row in rows]
    ranks = [rank if isinstance(rank, int) else None for rank in ranks]
    n = len(ranks)
    return {
        "recall": sum(rank is not None for rank in ranks) / n,
        "hit1": sum(rank == 1 for rank in ranks) / n,
        "mrr": sum(reciprocal(rank) for rank in ranks) / n,
    }


def summarize(rows: list[dict[str, Any]], methods: list[str]) -> dict[str, Any]:
    summary: dict[str, Any] = {}
    for method in methods:
        summary[method] = summarize_method(rows, method)

    by_scenario: dict[str, Any] = {}
    for scenario in sorted({str(row["scenario"]) for row in rows}):
        scenario_rows = [row for row in rows if row["scenario"] == scenario]
        by_scenario[scenario] = {
            method: summarize_method(scenario_rows, method) for method in methods
        }
    summary["by_scenario"] = by_scenario
    return summary


def contains_cjk(text: str) -> bool:
    return any(
        "\u3040" <= ch <= "\u30ff"
        or "\u3400" <= ch <= "\u9fff"
        or "\uac00" <= ch <= "\ud7af"
        for ch in text
    )


def auto_promote(
    query: str,
    hits: list[dict[str, Any]],
    min_hits: int,
    min_top_score: float,
) -> tuple[bool, str]:
    if len(hits) < min_hits:
        return True, "few_hits"
    top_score = 0.0
    if hits:
        try:
            top_score = float(hits[0].get("score", 0.0))
        except (TypeError, ValueError):
            top_score = 0.0
    if top_score < min_top_score:
        return True, "weak_top_score"
    if contains_cjk(query) and top_score < min_top_score * 2.0:
        return True, "cjk_weak_top_score"
    return False, "bm25_confident"


def mean(values: list[float]) -> float:
    return sum(values) / len(values) if values else 0.0


def reciprocal(rank: int | None) -> float:
    if rank is None:
        return 0.0
    return 1.0 / rank


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aidememo", default="target/debug/aidememo")
    parser.add_argument("--repo-root", type=Path, default=Path("."))
    parser.add_argument("--model-dir", type=Path)
    parser.add_argument("--candidate-limit", type=int, default=8)
    parser.add_argument("--batch-size", type=int, default=8)
    parser.add_argument("--max-chars", type=int, default=1800)
    parser.add_argument("--case-file", type=Path)
    parser.add_argument("--doc", action="append", dest="docs")
    parser.add_argument(
        "--no-default-docs",
        action="store_true",
        help="Do not load the built-in AideMemo Markdown corpus.",
    )
    parser.add_argument(
        "--corpus-jsonl",
        action="append",
        type=Path,
        help=(
            "External corpus JSONL. Rows accept doc_id/docid/id plus "
            "text/content/contents/passage and optional title/path."
        ),
    )
    parser.add_argument(
        "--queries-jsonl",
        type=Path,
        help="MIRACL/BEIR-style query JSONL with query_id/id and query/text fields.",
    )
    parser.add_argument(
        "--qrels-tsv",
        type=Path,
        help="Whitespace qrels: qid docid score or qid 0 docid score.",
    )
    parser.add_argument("--qrels-min-score", type=float, default=1.0)
    parser.add_argument("--max-cases", type=int)
    parser.add_argument("--external-scenario", default="external-candidate-recall")
    parser.add_argument(
        "--self-test-external",
        action="store_true",
        help="Run a tiny JSONL+qrels fixture through the external-corpus loader.",
    )
    parser.add_argument("--skip-model2vec", action="store_true")
    parser.add_argument(
        "--model2vec-mode",
        choices=["daemon", "cli"],
        default="daemon",
        help="How to run AideMemo's current semantic provider after vector-rebuild.",
    )
    parser.add_argument(
        "--fastembed-model",
        action="append",
        default=[],
        help=(
            "Also evaluate an AideMemo fastembed semantic profile, e.g. "
            "bge-small-en-v1.5 or all-mini-lm-l6-v2. Requires an aidememo "
            "binary built with --features fastembed."
        ),
    )
    parser.add_argument(
        "--fastembed-cache-dir",
        type=Path,
        default=Path.home() / ".aidememo" / "models",
        help="Absolute cache dir shared by temporary fastembed HOME configs.",
    )
    parser.add_argument("--daemon-start-timeout", type=float, default=90.0)
    parser.add_argument("--skip-lfm", action="store_true")
    parser.add_argument("--auto-min-bm25-hits", type=int, default=1)
    parser.add_argument("--auto-min-top-score", type=float, default=1.0)
    parser.add_argument("--summary-only", action="store_true")
    parser.add_argument("--output-json", type=Path, help="Also write the result payload here.")
    args = parser.parse_args()

    self_test_tmp: tempfile.TemporaryDirectory[str] | None = None
    if args.self_test_external:
        self_test_tmp = tempfile.TemporaryDirectory(prefix="aidememo-lfm-docs-external-")
        tmp_root = Path(self_test_tmp.name)
        corpus_path = tmp_root / "corpus.jsonl"
        queries_path = tmp_root / "queries.jsonl"
        qrels_path = tmp_root / "qrels.tsv"
        corpus_path.write_text(
            "\n".join(
                [
                    json.dumps(
                        {
                            "doc_id": "doc-redis",
                            "title": "Redis outage notes",
                            "text": "Redis timeout policy uses exponential backoff.",
                        }
                    ),
                    json.dumps(
                        {
                            "doc_id": "doc-hnsw",
                            "title": "Vector rebuild notes",
                            "text": "Run vector-rebuild after changing embedding models.",
                        }
                    ),
                ]
            )
            + "\n",
            encoding="utf-8",
        )
        queries_path.write_text(
            "\n".join(
                [
                    json.dumps({"query_id": "q1", "query": "redis timeout backoff"}),
                    json.dumps({"query_id": "q2", "query": "rebuild vectors after model swap"}),
                ]
            )
            + "\n",
            encoding="utf-8",
        )
        qrels_path.write_text("q1 0 doc-redis 1\nq2 0 doc-hnsw 1\n", encoding="utf-8")
        args.no_default_docs = True
        args.corpus_jsonl = [corpus_path]
        args.queries_jsonl = queries_path
        args.qrels_tsv = qrels_path
        args.skip_lfm = True
        args.skip_model2vec = True
        args.max_cases = args.max_cases or 2
        args.summary_only = True

    root = args.repo_root.resolve()
    doc_patterns: list[str] = []
    if args.docs:
        doc_patterns.extend(args.docs)
    elif not args.no_default_docs:
        doc_patterns.extend(DEFAULT_DOCS)
    chunks = collect_docs(root, doc_patterns, args.max_chars) if doc_patterns else []
    for corpus_path in args.corpus_jsonl or []:
        chunks.extend(load_corpus_jsonl(corpus_path, args.max_chars))
    if not chunks:
        raise SystemExit("no corpus chunks found")
    if args.queries_jsonl or args.qrels_tsv:
        if not args.queries_jsonl or not args.qrels_tsv:
            raise SystemExit("--queries-jsonl and --qrels-tsv must be passed together")
        cases = cases_from_qrels(
            args.queries_jsonl,
            args.qrels_tsv,
            min_score=args.qrels_min_score,
            max_cases=args.max_cases,
            scenario=args.external_scenario,
        )
    else:
        cases = load_cases(args.case_file)
        if args.max_cases is not None:
            cases = cases[: args.max_cases]
    gold_by_case = validate_cases(chunks, cases)
    chunk_by_id = {chunk.id: chunk for chunk in chunks}
    documents = [chunk.content for chunk in chunks]

    model_stats: dict[str, Any] = {"lfm": None, "model2vec": None, "fastembed": {}}
    lfm_doc_embeddings = None
    rank_dense = None
    if not args.skip_lfm:
        if args.model_dir is None:
            raise SystemExit("--model-dir is required unless --skip-lfm is set")
        from lfm_dense_eval import embedding_health
        from lfm_mlx_dense_eval import MlxEmbedder, rank_dense as rank_dense_impl

        rank_dense = rank_dense_impl
        model_started = time.perf_counter()
        embedder = MlxEmbedder(args.model_dir)
        model_stats["lfm"] = {
            "model_dir": str(args.model_dir),
            "model_load_ms": round((time.perf_counter() - model_started) * 1000, 2),
        }
        doc_started = time.perf_counter()
        lfm_doc_embeddings = embedder.encode(documents, role="document", batch_size=args.batch_size)
        model_stats["lfm"]["document_encode_ms"] = round(
            (time.perf_counter() - doc_started) * 1000, 2
        )
        model_stats["lfm"]["embedding_health"] = embedding_health(lfm_doc_embeddings)
    else:
        embedder = None

    with tempfile.TemporaryDirectory(prefix="aidememo-lfm-docs-recall-") as tmp:
        store = Path(tmp) / "docs.sqlite"
        seed_ms = seed_store(args.aidememo, store, chunks)
        model2vec_daemon: subprocess.Popen[str] | None = None
        model2vec_url: str | None = None

        try:
            if not args.skip_model2vec:
                rebuild_started = time.perf_counter()
                try:
                    proc = run_capture(
                        [
                            args.aidememo,
                            "--store",
                            str(store),
                            "vector-rebuild",
                            "--current-only",
                            "--json",
                        ]
                    )
                    model_stats["model2vec"] = {
                        "available": True,
                        "mode": args.model2vec_mode,
                        "rebuild_ms": round((time.perf_counter() - rebuild_started) * 1000, 2),
                        "stdout": proc.stdout.strip(),
                    }
                    if args.model2vec_mode == "daemon":
                        model2vec_daemon, model2vec_url, daemon_ms = start_mcp_daemon(
                            args.aidememo,
                            store,
                            args.daemon_start_timeout,
                        )
                        model_stats["model2vec"]["daemon_start_ms"] = round(daemon_ms, 2)
                except subprocess.CalledProcessError as exc:
                    model_stats["model2vec"] = {
                        "available": False,
                        "rebuild_ms": round((time.perf_counter() - rebuild_started) * 1000, 2),
                        "stderr": exc.stderr.strip()[-2000:],
                    }
                except (TimeoutError, OSError, RuntimeError) as exc:
                    model_stats["model2vec"] = {
                        "available": False,
                        "mode": args.model2vec_mode,
                        "error": str(exc),
                    }
            else:
                model_stats["model2vec"] = {"available": False, "skipped": True}

            rows: list[dict[str, Any]] = []
            bm25_ms: list[float] = []
            model2vec_ms: list[float] = []
            lfm_query_ms: list[float] = []
            lfm_score_ms: list[float] = []

            for case in cases:
                query = case["query"]
                gold_ids = set(gold_by_case[case["id"]])

                bm25_started = time.perf_counter()
                bm25_hits = run_json(
                    [
                        args.aidememo,
                        "--store",
                        str(store),
                        "search",
                        query,
                        "--json",
                        "--bm25-only",
                        "-l",
                        str(args.candidate_limit),
                    ]
                )
                bm25_ms.append((time.perf_counter() - bm25_started) * 1000)
                bm25_rank = rank_hits(bm25_hits, gold_ids)

                model2vec_rank = None
                model2vec_top = None
                if model_stats["model2vec"] and model_stats["model2vec"].get("available"):
                    hybrid_started = time.perf_counter()
                    try:
                        if model2vec_url is not None:
                            model2vec_hits = mcp_search(
                                model2vec_url,
                                query,
                                args.candidate_limit,
                                bm25_only=False,
                            )
                        else:
                            model2vec_hits = run_json(
                                [
                                    args.aidememo,
                                    "--store",
                                    str(store),
                                    "search",
                                    query,
                                    "--json",
                                    "--hybrid",
                                    "-l",
                                    str(args.candidate_limit),
                                ]
                            )
                        model2vec_ms.append((time.perf_counter() - hybrid_started) * 1000)
                        model2vec_rank = rank_hits(model2vec_hits, gold_ids)
                        model2vec_top = (
                            chunk_id_from_content(str(model2vec_hits[0].get("content", "")))
                            if model2vec_hits
                            else None
                        )
                    except (subprocess.CalledProcessError, RuntimeError, urllib.error.URLError) as exc:
                        model_stats["model2vec"]["search_error"] = str(exc)[-2000:]

                lfm_dense_rank = None
                lfm_dense_full_rank = None
                lfm_rerank_rank = None
                lfm_auto_rank = None
                lfm_dense_top = None
                lfm_auto_reason = None
                lfm_promoted = False
                if (
                    embedder is not None
                    and lfm_doc_embeddings is not None
                    and rank_dense is not None
                ):
                    query_started = time.perf_counter()
                    query_embedding = embedder.encode([query], role="query", batch_size=1)[0]
                    lfm_query_ms.append((time.perf_counter() - query_started) * 1000)

                    score_started = time.perf_counter()
                    dense_order = rank_dense(query_embedding, lfm_doc_embeddings)
                    lfm_score_ms.append((time.perf_counter() - score_started) * 1000)
                    dense_ids = [chunks[idx].id for idx in dense_order]
                    lfm_dense_full_rank = rank_gold_ids(dense_ids, gold_ids)
                    lfm_dense_rank = rank_gold_ids(dense_ids[: args.candidate_limit], gold_ids)
                    lfm_dense_top = dense_ids[0] if dense_ids else None

                    id_to_index = {chunk.id: idx for idx, chunk in enumerate(chunks)}
                    bm25_candidate_ids = [
                        chunk_id
                        for chunk_id in (
                            chunk_id_from_content(str(hit.get("content", ""))) for hit in bm25_hits
                        )
                        if chunk_id is not None and chunk_id in id_to_index
                    ]
                    candidate_scores = [
                        (
                            chunk_id,
                            float(lfm_doc_embeddings[id_to_index[chunk_id]] @ query_embedding),
                        )
                        for chunk_id in bm25_candidate_ids
                    ]
                    candidate_scores.sort(key=lambda item: item[1], reverse=True)
                    lfm_rerank_rank = rank_gold_ids(
                        [chunk_id for chunk_id, _score in candidate_scores],
                        gold_ids,
                    )

                    lfm_promoted, lfm_auto_reason = auto_promote(
                        query,
                        bm25_hits,
                        args.auto_min_bm25_hits,
                        args.auto_min_top_score,
                    )
                    auto_ids = dense_ids if lfm_promoted else bm25_candidate_ids
                    lfm_auto_rank = rank_gold_ids(auto_ids[: args.candidate_limit], gold_ids)

                row = {
                    "id": case["id"],
                    "scenario": case["scenario"],
                    "query": query,
                    "gold_ids": sorted(gold_ids),
                    "gold_paths": sorted({chunk_by_id[chunk_id].path for chunk_id in gold_ids}),
                    "bm25_rank": bm25_rank,
                    "model2vec_rank": model2vec_rank,
                    "lfm_dense_rank": lfm_dense_rank,
                    "lfm_dense_full_rank": lfm_dense_full_rank,
                    "lfm_rerank_rank": lfm_rerank_rank,
                    "lfm_auto_rank": lfm_auto_rank,
                    "lfm_auto_promoted": lfm_promoted,
                    "lfm_auto_reason": lfm_auto_reason,
                    "bm25_top": chunk_id_from_content(str(bm25_hits[0].get("content", "")))
                    if bm25_hits
                    else None,
                    "model2vec_top": model2vec_top,
                    "lfm_dense_top": lfm_dense_top,
                }
                rows.append(row)

            if args.fastembed_model:
                stop_process(model2vec_daemon)
                model2vec_daemon = None
                for fastembed_model in args.fastembed_model:
                    method = method_slug("fastembed", fastembed_model)
                    profile_home = Path(tmp) / f"home-{method}"
                    (profile_home / ".aidememo").mkdir(parents=True, exist_ok=True)
                    env = temp_home_env(profile_home)
                    try:
                        configure_embedding_profile(
                            args.aidememo,
                            env,
                            provider="fastembed",
                            model=fastembed_model,
                            cache_dir=args.fastembed_cache_dir.expanduser().resolve(),
                        )
                        stats = evaluate_semantic_profile(
                            aidememo=args.aidememo,
                            store=store,
                            rows=rows,
                            cases=cases,
                            gold_by_case=gold_by_case,
                            method=method,
                            env=env,
                            limit=args.candidate_limit,
                            daemon_start_timeout=args.daemon_start_timeout,
                        )
                    except subprocess.CalledProcessError as exc:
                        stats = {
                            "available": False,
                            "config_error": exc.stderr.strip()[-2000:],
                        }
                    model_stats["fastembed"][method] = {
                        "model": fastembed_model,
                        **stats,
                    }
        finally:
            stop_process(model2vec_daemon)

    methods = ["bm25"]
    if model_stats["model2vec"] and model_stats["model2vec"].get("available"):
        methods.append("model2vec")
    if not args.skip_lfm:
        methods.extend(["lfm_dense", "lfm_rerank", "lfm_auto"])
    for method, stats in model_stats.get("fastembed", {}).items():
        if stats.get("available"):
            methods.append(method)

    summary = summarize(rows, methods)
    if not args.skip_lfm:
        bm25 = summary["bm25"]
        lfm = summary["lfm_dense"]
        auto_lfm = summary["lfm_auto"]
        summary["lfm_dense_vs_bm25"] = {
            "recall_delta": round(lfm["recall"] - bm25["recall"], 6),
            "hit1_delta": round(lfm["hit1"] - bm25["hit1"], 6),
            "mrr_delta": round(lfm["mrr"] - bm25["mrr"], 6),
            "rescues": sum(
                row["bm25_rank"] is None and row["lfm_dense_rank"] is not None for row in rows
            ),
            "harms": sum(
                row["bm25_rank"] is not None and row["lfm_dense_rank"] is None for row in rows
            ),
        }
        summary["lfm_auto_vs_bm25"] = {
            "recall_delta": round(auto_lfm["recall"] - bm25["recall"], 6),
            "hit1_delta": round(auto_lfm["hit1"] - bm25["hit1"], 6),
            "mrr_delta": round(auto_lfm["mrr"] - bm25["mrr"], 6),
            "promoted": sum(row["lfm_auto_promoted"] for row in rows),
            "promotion_reasons": {
                reason: sum(row["lfm_auto_reason"] == reason for row in rows)
                for reason in sorted({str(row["lfm_auto_reason"]) for row in rows})
            },
        }

    payload: dict[str, Any] = {
        "summary": summary,
        "corpus": {
            "doc_patterns": doc_patterns,
            "external_corpus_jsonl": [str(path) for path in args.corpus_jsonl or []],
            "chunks": len(chunks),
            "cases": len(cases),
            "paths": sorted({chunk.path for chunk in chunks}),
            "docs_loaded": len(doc_patterns),
            "seed_ms": round(seed_ms, 2),
        },
        "latency": {
            "bm25_search_ms_mean": round(mean(bm25_ms), 2),
            "model2vec_search_ms_mean": round(mean(model2vec_ms), 2)
            if model2vec_ms
            else None,
            "lfm_query_embed_ms_mean": round(mean(lfm_query_ms), 2) if lfm_query_ms else None,
            "lfm_dense_score_ms_mean": round(mean(lfm_score_ms), 4) if lfm_score_ms else None,
        },
        "models": model_stats,
    }
    if not args.summary_only:
        payload["rows"] = rows

    if args.output_json is not None:
        args.output_json.parent.mkdir(parents=True, exist_ok=True)
        args.output_json.write_text(
            json.dumps(payload, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )

    print(json.dumps(payload, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
