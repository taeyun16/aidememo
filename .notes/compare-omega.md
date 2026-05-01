# wg vs OMEGA — head-to-head (2026-05)

> **결론 한 줄**: OMEGA는 LongMemEval-S에서 **95.4% (gpt-4.1)**, wg는
> **74.0% (MiniMax-M2.7-highspeed)** — 격차 21.4pt. 두 시스템 모두
> local-first + bge-small-en-v1.5 임베딩으로 같은 retrieval base를
> 공유하지만, OMEGA는 reader (gpt-4.1) + 25 specialized lifecycle
> tools + ingest-time LLM 자동 capture에서 마진을 벌고 있음.

조사 출처:
- [omega-memory GitHub](https://github.com/omega-memory/omega-memory)
- [How I Built — dev.to (Jason Singularity)](https://dev.to/singularityjason/how-i-built-a-memory-system-that-scores-954-on-longmemeval-1-on-the-leaderboard-2md3)
- [omegamax.co/benchmarks](https://omegamax.co/benchmarks)
- 직접 측정: `/tmp/wg_e2e_*` (2026-05-01), 본 저장소의
  `.notes/bench-longmemeval.md`

## 종합 점수 (LongMemEval E2E, gpt-4o judge)

| 시스템 | reader | Overall | Δ vs OMEGA |
|---|---|---|---|
| **OMEGA** | gpt-4.1 | **95.4%** | — |
| wg + MiniMax | MiniMax-M2.7-highspeed | 74.0% | -21.4 |
| wg + gpt-4o | gpt-4o | 67.6% | -27.8 |
| wg + dedup + mini | gpt-4o-mini | 66.2% | -29.2 |
| Mem0 (publish) | gpt-4o | 49.0% | -46.4 |

## 카테고리별 (OMEGA / wg+MiniMax)

| Category | OMEGA | wg+MiniMax | Δ |
|---|---|---|---|
| Single-Session-User | (saturated) | **97.1%** | ≈ 동등 |
| Single-Session-Assistant | (saturated) | 92.9% | ≈ 동등 |
| **Knowledge Update** | **96.2%** | 84.6% | -11.6 |
| **Multi-Session** | **83.5%** | 71.4% | -12.1 |
| **Preference** | **98.6%** | 80.0% | -18.6 |
| **Temporal** | **94.0%** | 48.9% | **-45.1** ⚠ |
| **Overall** | **95.4%** | 74.0% | -21.4 |

→ 격차의 대부분이 **temporal-reasoning (45pt)** + **preference (18.6pt)**.
OMEGA의 보고서가 명시한 4 prompting tricks (temporal +5q, KU +4q,
query augmentation +2q, preference personalization +2q) 중 가장 큰
효과는 wg가 temporal 데이터셋 노이즈에 노출돼서 못 따라잡음.

## 인프라 매트릭스

| 차원 | OMEGA | wg |
|---|---|---|
| 임베딩 | bge-small-en-v1.5 ONNX (384-dim) | **bge-small-en-v1.5 ONNX (384-dim)** ← 동일 |
| 스토리지 | SQLite + sqlite-vec | redb single-file |
| 운영 | `pip install omega-memory` | `cargo install wg-cli` |
| Bindings | Python only | **Py / Node / Elixir / C** |
| Multi-store | ❌ | ✅ `wg project create/use` |
| Custom entity types | (확인 안 됨) | ✅ 임의 문자열 (`--type session` 등) |
| Cross-encoder rerank | (확인 안 됨) | ✅ bge-reranker, two-stage K=20→10 |
| **Insert 토큰 / 100K sessions** | LLM 자동 ≈ \$10-30 | **0** (regex 기본; opt-in \$1-3 — `extract.provider`) |
| Encryption at rest | AES-256 | ❌ (redb 평문, OS-level 암호화 가능) |
| MCP tools | 25 core + 29 coord (omega-pro) | 17 (`wg_context` 포함, Tier A+B 적용 후) |
| Background hooks | Hook-based auto-capture (built-in) | wg-skill/hooks/ 3개 (수동 install) |
| Memory hierarchy | core / archival / context-virt | profile / pinned / current / archived (4-tier convention) |
| Forgetting | SHA-256 dedup + 0.85 evolution + TTL + Jaccard compaction (5 mechanisms) | exact dedup + atomic conflict + semantic dedup + TTL (4 mechanisms, 명시 명령) |

## 격차 분해 (총 -21.4pt)

각 요인의 추정 기여 (확실치 X, 정성적 분석):

| 요인 | 추정 | 비고 |
|---|---|---|
| **reader 모델** (gpt-4.1 vs gpt-4o-class) | **-5pt** | gpt-4.1은 LongMemEval에서 reasoning 우월, MiniMax도 +6.4pt로 비슷한 효과 |
| **Hook-based ingestion 깊이 통합** | -3pt | wg-skill/hooks/는 read-only, 명시적 fact_add 안 함 |
| **LLM-aided ingestion 기본 ON** | -8pt | OMEGA는 매 conversation hook에서 자동 분류, wg는 사용자가 `extract.provider` 옵트인 |
| **Temporal prompting tricks** | -3-5pt | wg 영역 밖, reader prompt 개선 필요 |
| **Forgetting at write 통합 (compaction)** | -2pt | wg에 인프라 있지만 LongMemEval-S에선 효과 X (isolated stores) |

→ **3가지 핵심 격차**: reader 모델 + hook-based 자동 capture + LLM-aided
ingestion. wg는 인프라 있지만 default OFF (사용자 비용 부담 회피).

## wg가 더 강한 영역

1. **운영 단순성**: single Rust binary, no Python runtime
2. **4 native bindings**: 같은 wiki를 Py/Node/Elixir/C에서 직접 호출
3. **Insert 비용 0**: regex 기본, LLM-aided는 opt-in. 100K sessions
   적재 시 OMEGA가 \$10-30 발생할 때 wg는 \$0. 자동 ingestion (`wg watch`,
   transcript dump) 케이스에서 압도적 비용 우위.
4. **Multi-store native**: 프로젝트별 격리 (`wg project use work` /
   `personal`)
5. **Custom entity types**: `--type session`, `--type rfc`, `--type
   incident` 임의 문자열, 추가 인프라 없음
6. **Cross-encoder rerank shipped**: two-stage K=20→10 (R@10 0.992)
7. **Local-first 완전성**: TEI / fastembed 모두 in-process,
   외부 service 0개
8. **Bench harness multi-provider**: extract / reader 둘 다 OpenAI /
   MiniMax / Ollama / Kimi / OpenRouter 자유 (기본 인프라)

## 격차 좁히는 ROI 순 (다음 단계)

1. **gpt-4.1 reader 측정** — wg + gpt-4.1 시험. 이전 측정 안 됨.
   가장 빠른 +3-5pt. 비용 \$5.
2. **LLM-aided ingestion 기본 활용** — `wg watch` + `extract.provider`
   를 dogfood. 운영 환경에서 측정.
3. **Temporal reasoning prompting 가이드** — wg-skill에 reader
   prompt 패턴 추가 ("when comparing time periods, …"). 추정 +3pt.
4. **Hook 강화** — wg-skill/hooks/wg-extract-facts.py를 default
   `WG_EXTRACT_LLM=1` 권장으로 변경, 자동 fact_add (현재는 preview only).
   Async 인프라 필요 (rate limit 처리).
5. **Async LLM extract 인프라** — wg-core extract에 tokio + reqwest
   async, 동시 N calls. LongMemEval 500q full 측정 가능 (~30분).

## 측정 시 고려사항

- **OMEGA 점수는 ICL 데이터셋 + gpt-4.1 reader + judge=gpt-4o** 기준.
  wg를 정확히 비교하려면 같은 reader (gpt-4.1) 사용 권장.
- LongMemEval-S baseline ceiling (R@10 0.992)이 너무 높아 retrieval
  쪽 변화는 R@1 / MRR 수준에서만 보임. ingest-time LLM-aided 효과는
  full 500q (또는 random hard sample) 필요.
- **Temporal 카테고리는 데이터셋 자체 노이즈** (74개 evidence sessions
  future-dated, 이전 분석 참조). OMEGA는 prompting trick으로 우회한 것.

## Trade-off 결론

| 케이스 | 추천 |
|---|---|
| Solo coding agent + Claude Code | OMEGA — turnkey + SOTA |
| 다언어 in-process embedding (CLI / IDE plugin) | **wg** (4 bindings) |
| 대량 자동 ingestion (transcript / log mining) | **wg** (insert 비용 0) |
| 멀티 프로젝트 격리 + git-versioned wiki | **wg** (redb 1 file + multi-store) |
| 정확도 마지막 5%까지 critical | OMEGA |
| Vendor-neutral local stack (no Python) | **wg** |
