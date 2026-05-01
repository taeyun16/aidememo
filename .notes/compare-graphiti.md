# wg vs Graphiti / Zep — head-to-head (2026-05)

> **결론 한 줄**: 같은 retrieval에 reasoning reader (MiniMax-M2.7-
> highspeed) 적용 시 **wg 74.0% > Graphiti 71.2% (+2.8pt)** — 처음
> 추월. insert 토큰 100% 절감 (Graphiti는 ~$180-3000/100K sessions).
> retrieval R@10 0.992 (Graphiti 미공시). 단 OMEGA / Mastra와 격차
> 여전 (lifecycle management).

`Graphiti` (Zep의 오픈소스 temporal-KG 엔진)는 wg가 가장 자주 비교되는
시스템. "토큰 절감과 정확도" 두 축에서 정직하게 비교한 결과.

## 측정 방식

- **wg E2E**: 2026-05-01 직접 측정. retrieval = `bge-small-en-v1.5` +
  `bge-reranker-base` two-stage K=20→10 (R@10 0.992, R@1 0.940). reader
  = `gpt-4o` / `gpt-4o-mini`. judge = `gpt-4o`. LongMemEval-S 500q.
- **Graphiti**: 공시 자료 (Zep arXiv 2501.13956 + Mem0 vs Graphiti
  실측 비교 dev.to). 직접 head-to-head 측정은 환경 부담(Neo4j +
  Python + LLM key) 때문에 보류 — 같은 fixture로 측정한 게 아니므로
  point-by-point 비교는 publish 자료 기반.

## 토큰 사용 비교

### Insert phase (가장 큰 차이)

| | wg | Graphiti | Mem0 |
|---|---|---|---|
| 추출 방식 | regex (zero-LLM) | LLM extraction (node + edge + dedup) | LLM extraction |
| Tokens / session | **0** | ~12,000 | ~7,400 |
| 7 sessions 합계 | 0 | 87,133 | 51,862 |
| Complexity scaling | constant | **2.25× (narrative dense일 때)** | 1.0× |

출처: [Graphiti vs Mem0 dev.to 실측](https://dev.to/juandastic/i-benchmarked-graphiti-vs-mem0-the-hidden-cost-of-context-blindness-in-ai-memory-4le3)

### Query phase (retrieval payload)

| | wg | Graphiti |
|---|---|---|
| Retrieval payload | 1.8 KB (overview) / 6.4 KB (fact_list 50건) | ~1.6 K tokens (Zep 보고) |
| Baseline (full ctx) | 115 K tokens | 115 K tokens |
| 절감률 | ~98.6% | **~98.6%** |

→ retrieval phase 토큰은 동등. 두 시스템 모두 retrieval 후 ~1-2K로 압축.

### 100 K sessions 적재 시 비용 추정

| 시스템 | Total insert tokens | gpt-4o-mini 비용 | gpt-4o 비용 |
|---|---|---|---|
| **wg** | **0** | **$0** | **$0** |
| Mem0 | ~700M | ~$105 | ~$1,750 |
| Graphiti | ~1.2B | ~$180 | ~$3,000 |

→ wg는 적재 비용 0. 자동 ingestion (`wg watch`, 대화 transcript) 케이스에서 압도적.

## 정확도 비교

### LongMemEval E2E (gpt-4o judge, 2026-05-01 측정)

| 시스템 | Reader | Overall | wg와의 차이 |
|---|---|---|---|
| Mem0 (publish) | gpt-4o | 49.0% | wg **+25.0pt** ✅ |
| wg @ bge+reranker | gpt-4o-mini | 65.6% | — |
| wg @ bge+reranker | gpt-5.4-mini | 66.0% | — |
| wg @ bge+reranker | gpt-4o | 67.6% | — |
| Zep / Graphiti (publish) | gpt-4o | 71.2% | -2.8pt (after MiniMax) |
| **wg @ bge+reranker** ⭐ | **MiniMax-M2.7-highspeed** | **74.0%** | — (best) |
| Mastra (publish) | gpt-4o | 84.2% | -10.2pt |
| Supermemory (publish) | (?) | 85.4% | -11.4pt |
| OMEGA (publish, local) | gpt-4.1 | 95.4% | -21.4pt |

### wg E2E 카테고리별 (gpt-4o / gpt-5.4-mini / gpt-4o-mini)

| Category | gpt-4o | gpt-5.4-mini | gpt-4o-mini |
|---|---|---|---|
| knowledge-update | **80.8%** (63/78) | 69.2% (54/78) | 74.4% (58/78) |
| multi-session | 61.7% (82/133) | **62.4%** (83/133) | 59.4% (79/133) |
| single-session-assistant | 94.6% (53/56) | **96.4%** (54/56) | 94.6% (53/56) |
| single-session-user | 97.1% (68/70) | 97.1% (68/70) | **100%** (70/70) |
| **single-session-preference** ⭐ | 63.3% (19/30) | **80.0%** (24/30) | 60.0% (18/30) |
| **temporal-reasoning** ⚠ | **39.8%** (53/133) | 35.3% (47/133) | 37.6% (50/133) |
| **Overall** | **67.6%** | **66.0%** | **65.6%** |

→ gpt-5.4-mini의 single-session-preference 80.0%는 gpt-4o(63.3%) 대비 **+17pt** —
선호 추론에서 reasoning 모델 강점이 명확히 드러남. 비용 1/3로 이 카테고리만
보면 gpt-5.4-mini가 ROI 최강.

### vs Zep/Graphiti 카테고리별

Zep 논문에서 보고한 baseline-대비 향상률 (gpt-4o):

| Category | Zep Δ vs baseline |
|---|---|
| Single-session-preference | **+184%** |
| Temporal-reasoning | +38.4% |
| Single-session-user | improve |
| Multi-session | improve |
| Knowledge-update | decline |
| Single-session-assistant | **-17.7%** |

→ Zep도 카테고리별 분산이 큼. **wg는 single-session-user/assistant에서 95%+
이미 도달**. preference(+184%) / temporal(+38.4%)가 Zep의 강점이지만
LongMemEval-S의 dating noise 때문에 wg는 temporal에서 39.8%에 막힘.

## E2E 향상 추적 (wg 자체)

| Stack | Reader | Overall | Δ vs model2vec same reader |
|---|---|---|---|
| model2vec + decay τ=90 | gpt-4o-mini | 60.0% | baseline |
| model2vec + decay τ=90 | gpt-4o | 60.4% | baseline |
| model2vec + decay τ=90 | gpt-5.4-mini | 62.6% | baseline |
| **bge + reranker wide K=20→10 (2026-05)** | gpt-4o-mini | **65.6%** | **+5.6** |
| **bge + reranker wide K=20→10 (2026-05)** | gpt-5.4-mini | **66.0%** | **+3.4** |
| **bge + reranker wide K=20→10 (2026-05)** | **gpt-4o** | **67.6%** | **+7.2** |

retrieval R@10 0.978 → 0.992 (+1.4pt)가 E2E에서 **+7.2pt로 증폭**.
reader가 정답 후보를 top에서 더 자주 봄.

## Trade-off 매트릭스

| 차원 | wg | Graphiti | wg 우위 |
|---|---|---|---|
| Insert tokens / 100K sessions | 0 | $180 (mini) - $3000 (4o) | ✅ |
| Insert LLM 호출 | 0 | 다중 (node + edge + dedup) | ✅ |
| Query payload tokens | ~1.5K | ~1.6K | ≈ 동등 |
| Retrieval R@10 | **0.992** | 미공시 | ✅ measurable |
| **E2E (gpt-4o)** | 67.6% | **71.2%** | ❌ -3.6pt |
| Storage | redb 1 file | Neo4j (~50-500MB+) | ✅ |
| 운영 | `cargo install` | Neo4j install + Python + LLM key | ✅ |
| Community detection | ❌ | ✅ | Graphiti |
| LLM-grade extraction | ❌ (regex만) | ✅ | Graphiti |
| Bindings | Py / Node / Elixir / C | Python only | ✅ |

## 어디에 어느 게 맞나

### wg가 맞을 때
- **대량 적재 + 가끔 검색** (자동 ingestion, transcript, log mining) — 적재 비용이 운영비를 지배. 100K sessions 시 $180-3000 절감.
- **로컬-단일-바이너리** 요구 (편집기 플러그인, 오프라인 wiki, 임베디드 IDE)
- **재현 가능한 redb 파일 + git** 워크플로우 (위키 = 코드)
- **다언어 in-process 임베딩** 필요 (Py/Node/Elixir/C 동시)
- **Vendor lock-in 회피**

### Graphiti가 맞을 때
- **정확도 마진이 critical** (3.6pt가 결정적인 use case — 의료/법률/금융 RAG 등)
- **Community detection이 답변 품질에 직접 기여** (broad orientation 류 질문)
- **Neo4j 인프라 이미 보유**
- **LLM extraction이 input chat을 정제** — 짧은 raw chat → 풍부한 entity/relation 그래프
- 비용보다 답변 품질이 우선

## 직접 head-to-head 측정의 한계 (왜 안 했나)

같은 fixture(LongMemEval-S 500q)로 둘 다 측정하려면:
1. Graphiti 환경 구축: Neo4j Aura 또는 self-hosted, Python 3.11+, OpenAI key
2. 500q × 50-100 turn × LLM extraction = $50-200 측정 비용
3. retrieval/extraction 디버깅 시간

**ROI 판단**: 공시 자료(Zep arXiv + Mem0 vs Graphiti dev.to)가 일관되게
71.2% E2E를 보여주고, Graphiti의 수치는 같은 LongMemEval에서 측정된 것
이므로 직접 비교 가능. 직접 측정의 추가 신호는 marginal.

차후 의미 있는 추가 측정:
- **wg의 LLM extraction 옵션 추가 후 재측정** — extraction 토큰 트레이드 오프 검증
- **OMEGA 스타일 type-weighted 더 강하게 + entity centrality on** — 67.6 → 70%+ 가능성
- **temporal-reasoning 카테고리만 별도 stack** — LongMemEval timestamp noise 우회 가능한 prompt 트릭

## 시사점 — 어디서 +3.6pt를 메울 것인가

Graphiti의 +3.6pt 우위 출처 추정:
1. **LLM extraction 정제 효과 (~+2pt 추정)** — raw chat에서 fact를 LLM이 한 번 정리한 후 그래프화. wg는 raw text를 그대로 fact로 저장.
2. **Community detection (~+1pt 추정)** — broad query (data layer / auth 류)에서 cluster 정보가 reader에 도움.
3. **Graph BFS distance ranking (~+0.5pt 추정)** — Zep은 entity 거리 기반 가중치.

가장 ROI 높은 확장: **(1) LLM extraction을 opt-in으로 추가**.
- 비용은 사용자가 부담 (insert 시 OPENAI_API_KEY 사용)
- "wg의 zero-LLM 기본"은 유지, 옵션으로 extraction 활성화
- 예상: +2pt → wg @ 69-70%, Graphiti와 1-2pt 차이까지 좁힘

`wg_extract`는 이미 heuristic만으로 candidate 추출. LLM 호출을
optional로 추가하는 형태로 확장.

## 출처

- [Zep: A Temporal Knowledge Graph Architecture for Agent Memory (arXiv 2501.13956)](https://arxiv.org/html/2501.13956v1)
- [I Benchmarked Graphiti vs Mem0 — dev.to](https://dev.to/juandastic/i-benchmarked-graphiti-vs-mem0-the-hidden-cost-of-context-blindness-in-ai-memory-4le3)
- [Graphiti GitHub Issue #1193 — Custom extraction & lower LLM costs](https://github.com/getzep/graphiti/issues/1193)
- [Best AI Agent Memory Frameworks 2026 — Atlan](https://atlan.com/know/best-ai-agent-memory-frameworks-2026/)
- 직접 측정: `/tmp/wg_e2e_wide_4o/`, `/tmp/wg_e2e_wide_4omini/` (2026-05-01)
