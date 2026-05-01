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

## 종합 점수 (LongMemEval E2E, gpt-4o judge, basic prompt)

| 시스템 | reader | Overall | Δ vs OMEGA |
|---|---|---|---|
| **OMEGA** | gpt-4.1 (+omega-tricks prompt) | **95.4%** | — |
| wg + MiniMax | MiniMax-M2.7-highspeed | 74.0% | -21.4 |
| **wg + gpt-4.1** ⭐ | gpt-4.1 (basic prompt) | **72.6%** | **-22.8** |
| wg + gpt-4o | gpt-4o | 67.6% | -27.8 |
| wg + dedup + mini | gpt-4o-mini | 66.2% | -29.2 |
| Mem0 (publish) | gpt-4o | 49.0% | -46.4 |

→ **wg + gpt-4.1 (same reader as OMEGA, no prompt tricks) = 72.6%**.
즉 같은 reader 모델로 비교 시 격차는 **22.8pt**, 그 중:
  - **prompting tricks** (omega-tricks vs basic): 측정 진행 중 (`be260683m`)
  - **lifecycle / hook-based ingestion**: 나머지

reader 모델만으로는 OMEGA 격차의 **5pt만 설명** (gpt-4o → gpt-4.1).
나머지 17.8pt는 prompting + ingestion + lifecycle.

## 카테고리별 (OMEGA / wg+gpt-4.1 / wg+MiniMax)

| Category | OMEGA | wg+gpt-4.1 | wg+MiniMax | Δ (best wg) |
|---|---|---|---|---|
| Single-Session-User | (saturated) | 97.1% | 97.1% | ≈ 동등 |
| Single-Session-Assistant | (saturated) | 94.6% | 92.9% | ≈ 동등 |
| **Knowledge Update** | **96.2%** | 82.1% | 84.6% (MiniMax) | -11.6 |
| **Multi-Session** | **83.5%** | **72.9%** (gpt-4.1) | 71.4% | -10.6 |
| **Preference** | **98.6%** | **83.3%** (gpt-4.1) | 80.0% | -15.3 |
| **Temporal** | **94.0%** | 42.1% | **48.9%** (MiniMax) | **-45.1** ⚠ |
| **Overall** | **95.4%** | 72.6% | **74.0%** (MiniMax) | -21.4 |

→ **gpt-4.1과 MiniMax는 카테고리별 강점 다름**:
  - MiniMax: temporal +6.8pt, KU +2.5pt 우위 (reasoning 모델 강점)
  - gpt-4.1: multi-session +1.5pt, preference +3.3pt 우위

이상적 setup: **카테고리별 reader 라우팅** (temporal → MiniMax, 나머지
gpt-4.1) 시 추정 ~75% — 격차 좁히는 한 옵션.

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

## 격차 좁히는 ROI 순 (실측 갱신, 2026-05-01)

**측정으로 확인됨** (✓ = 직접 측정):

1. ✓ **gpt-4.1 reader 측정** — wg + gpt-4.1 (basic) = **72.6%**, gpt-4o
   대비 +5pt. **reader 모델 변경만으론 5pt만 메움**.
2. ✓ **OMEGA-style reader prompt (omega-tricks)** — 모든 reader 크기에서
   효과 없음 또는 negative. mini -0.8, gpt-4.1 -1.6. **prompt 재현 안 됨**.
3. ✓ **LLM-extract retrieval 60q balanced (concurrent)** — R@K 완전히
   baseline과 동일 (R@1 0.917, R@10 1.000 둘 다). **retrieval 단에선
   효과 0** (단, 60q baseline ceiling 때문).
4. ✓ **LLM-extract augment 모드 reader E2E 60q balanced** —
   **-8.3pt 부정 영향** (66.7 → 58.3). Per-category:
     - knowledge-update -20pt ⚠
     - multi-session -10pt
     - temporal -10pt
     - user/assistant ≈ 동등
   원인: raw turn + classified facts 둘 다 retrieval 노출 → reader
   혼란. wg의 augment 디자인이 OMEGA의 replace 디자인과 다름.
5. ✓ **LLM-extract replace 모드 (60q balanced, gpt-4o-mini extractor)** —
   **-48.4pt 처참** (baseline 66.7 → replace 18.3). 카테고리별:
     - single-session-user 100 → **0.0%** (완전 망함)
     - multi-session 60 → 10
     - temporal 10 → 0
   원인: gpt-4o-mini extract이 raw chat의 verbatim detail을 너무
   많이 drop. R@K도 R@1 0.917 → 0.700, R@10 1.000 → 0.833.

→ **gpt-4o-mini extractor로는 OMEGA 디자인 (replace) 모방 불가**.
정보 손실이 noise보다 큼.

## 큰 결론: OMEGA 격차 = higher-LLM extractor + 다른 ingest 디자인

reader / prompt / retrieval / 우리 ingest 디자인 모든 옵션 측정 후
도달한 결론:

| 격차 출처 | 측정 결과 |
|---|---|
| Reader 모델 (gpt-4.1) | +5pt 메움 ✓ |
| Reader prompt (omega-tricks 우리 구현) | -1.6 ~ -0.8 ✗ |
| LLM-extract augment (raw + classified) | -8.3pt ✗ |
| LLM-extract replace (gpt-4o-mini) | -48pt ✗ |
| **남은 -17pt 격차** | **higher-quality extractor + 새 ingest 디자인 필요** |

**OMEGA가 95% 달성하는 진짜 이유 (가설)**:
1. **gpt-4.1+ 또는 Claude Opus extractor** — 우리 mini로는 불충분
2. **Per-conversation tracking** — raw chat 별도 보관 + classified
   facts는 보조 신호 (hybrid)
3. **Hybrid retrieval** — query 종류에 따라 raw vs classified 선택
4. **Pre-curated extraction prompt** — LongMemEval-S 특성에 tuning

다음 세션 진짜 ROI 작업:
- (a) **gpt-4.1 + replace 모드** — 같은 디자인, 더 좋은 LLM (~$5-10)
- (b) **Hybrid ingest 디자인** — classified facts는 main entity에,
  raw turn은 별도 retrievable layer (큰 인프라)
- (c) **Per-question reader routing** — temporal → MiniMax,
  preference → gpt-4.1, default → gpt-4o

→ **reader + prompt + retrieval 단 작업으로는 OMEGA 격차 못 메움**.

**남은 가능성** (미검증):

4. **카테고리별 reader routing** — temporal → MiniMax (reasoning),
   preference → gpt-4.1 (instruction-following). 추정 +3-5pt overall.
5. **LLM-extract reader E2E 영향** — 같은 retrievals에 reader가 추가된
   classified facts를 활용하나 측정 (`bda7h6raz`).
6. **Hook auto-apply** — `wg-skill/hooks/wg-extract-facts.py` 자동
   fact_add 모드. dogfood-only 효과 — LongMemEval로 측정 불가.
7. **OMEGA의 25 specialized tools 일부 구현** — checkpoint/resume,
   per-conversation context tracking. 큰 인프라.
8. **Few-shot reader prompts** — basic + 카테고리별 in-context exemplars.

**측정 인프라 개선 사항** (✓):

- bench `--llm-extract-base-url / api-key-env / model` — MiniMax /
  Ollama / Kimi 등 OpenAI-호환 endpoint 지원
- bench `--balanced-sample N` — 카테고리별 균등 sample
- bench rayon-concurrent LLM-extract — sequential ~43h →
  concurrent ~5h (8× speedup)
- e2e `--reader-prompt {basic,omega-tricks}` — prompt A/B

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
