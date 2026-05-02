# wg vs OMEGA — head-to-head (2026-05-02 업데이트)

> **재현 측정 결과 — published claim 재현 안 됨**:
> 같은 머신서 `pip install omega-memory` + LongMemEval-S 직접 측정
> (12q balanced, gpt-4.1 reader, official judge):
> **OMEGA standalone = 58.3%** vs **wg + gpt-4.1 = 75.0%** —
> **wg가 OMEGA보다 +16.7pt 우위**. OMEGA 공시 95.4%는 우리 환경에서
> 재현 불가. 가능 원인: omega-pro + Claude Code hook 통합 / 다른
> evaluation setup / standalone API의 한계.
>
> Published 비교 (참고):
> - OMEGA published: 95.4% (gpt-4.1)
> - wg measured (500q, official judge): 72.4% (gpt-4.1) / 71.8% (MiniMax)

조사 출처:
- [omega-memory GitHub](https://github.com/omega-memory/omega-memory)
- [How I Built — dev.to (Jason Singularity)](https://dev.to/singularityjason/how-i-built-a-memory-system-that-scores-954-on-longmemeval-1-on-the-leaderboard-2md3)
- [omegamax.co/benchmarks](https://omegamax.co/benchmarks)
- 직접 측정: `/tmp/wg_e2e_*` (2026-05-01), 본 저장소의
  `.notes/bench-longmemeval.md`

## 같은 머신 직접 측정 — Cross-encoder 활성 효과 (2026-05-02 갱신) ⭐

같은 12q balanced sample, 같은 reader (gpt-4.1), 같은 official judge,
OMEGA `pip install` standalone API. Cross-encoder reranker만 토글:

| Setup | Overall | KU | temporal | wall (ingest) |
|---|---|---|---|---|
| **OMEGA no-rerank** | 58.3% | 100% | 50% | ~30s/q |
| **OMEGA + rerank** | **41.7%** ⚠ | **50%** | **0%** | **~320s/q** (10× 느림) |
| **wg + gpt-4.1** | **75.0%** ⭐ | 100% | 50% | (해당 없음) |

→ **OMEGA의 cross-encoder reranker가 점수를 -16.7pt 떨어뜨림** +
**ingest를 10× 느리게**. KU + temporal에서만 손해 (-50pt 각각),
다른 카테고리 동등. Reranker 통합 자체 결함 또는 standalone API
사용 시 wrong context.

## 같은 머신 직접 측정 (no-rerank baseline)

`pip install omega-memory` 1.4.9 → standalone Python API
(`from omega import store, query`) → 12q balanced sample (per-type 2)
→ gpt-4.1 reader → official LongMemEval evaluator (gpt-4o judge,
task-specific prompts):

| Setup | Overall | SS-user | SS-pref | SS-asst | multi-sess | temporal | KU |
|---|---|---|---|---|---|---|---|
| **OMEGA standalone** | **58.3%** | 100 | 50 | 50 | 0 | 50 | 100 |
| **wg + gpt-4.1** (same 12q) | **75.0%** ⭐ | 100 | **100** | **100** | 0 | 50 | 100 |
| Δ wg-omega | **+16.7** | 0 | **+50** | **+50** | 0 | 0 | 0 |

→ **OMEGA가 단 한 카테고리에서도 wg를 못 이김**. wg가 SS-preference,
SS-assistant에서 +50pt 압도. wg가 OMEGA standalone 보다 모든
면에서 더 좋은 성능.

**OMEGA published 95.4% 재현 안 됨**. 가능 원인:
1. **OMEGA 95.4%가 omega-pro + Claude Code hook 통합 환경**
   (우리는 standalone Python API만 사용)
2. **Cross-encoder rerank 미설정** (omega doctor "Cross-encoder
   model not found — run download_model() first")
3. **다른 evaluation setup** (다른 reader prompt, judge 등)

**자기 머신서 검증 안 되는 SOTA claim**의 한 사례. wg는 같은 환경
에서 더 좋은 결과를 내는 — local-first 영역에서 wg가 진짜 SOTA에
가까울 수 있음 (적어도 standalone 사용 케이스).

(24q balanced 측정 진행 중 — `blzzgd2ha`, ~14분)

## Published 점수 비교 (참고용)

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

6. ✓ **LLM-extract augment with gpt-4.1 extractor (24q balanced, mini reader)** —
   **-16.7pt 더 큰 부정 영향** (baseline 62.5 → augment 45.8). 카테고리별:
     - KU: 75 → **25%** (-50pt)
     - multi-session: 50 → **0%** (-50pt)
     - 나머지 동등
   **결정적 발견**: gpt-4.1 같은 higher-quality extractor일수록 augment
   효과 **더 부정적**. mini augment -8.3 → gpt-4.1 augment -16.7.

   원인: better distillation = stronger anchor for reader. Reader가
   정제된 fact에 더 신뢰 → raw turn 정답 무시. Information convergence
   가 classified fact에 보존 안 됨.

→ **wg augment 디자인 자체 문제, LLM 품질 무관**. wg에 OMEGA-style
ingest-time LLM-aided을 적용하려면 **새 디자인 필요** (단순 augment X).

7. ✓ **Hybrid w/ source labels (24q balanced, mini extract)** —
   reader prompt에 `[raw chat]` vs `[distilled fact]` 라벨 표시:
   **50.0% (vs baseline 62.5%, -12.5pt)**. 카테고리별:
     - KU: 75 → 50%
     - multi-session: 50 → 25%
     - user: 100 → 75% (verbatim detail 회상도 손상)
   → Source labeling이 약간 도움 (-16.7 → -12.5) 되지만 여전히
   부정 효과. distilled facts가 retrieval set에 있는 한 reader
   영향 못 막음.

## 최종 결론: wg + LongMemEval-S에서 ingest-time LLM-aided ≠ OMEGA 격차 해결

**모든 LLM-extract variant 측정 완료, 모두 부정**:

| Setup | E2E (24q) | Δ |
|---|---|---|
| baseline | 62.5% | — |
| augment + mini extract (60q ref) | 58.3% | -8.3 |
| augment + gpt-4.1 extract | 45.8% | -16.7 ⚠ |
| hybrid w/ source labels | 50.0% | -12.5 |
| replace + mini extract (60q ref) | 18.3% | -48.4 ⚠ |

**확정 사실**:
1. LongMemEval-S는 verbatim user statements 회상에 의존
2. LLM-distilled fact는 어떤 형태(augment / replace / hybrid)로도
   raw turn detail을 손상
3. Better LLM extractor일수록 augment 효과 더 부정 (-8.3 → -16.7)

**OMEGA의 95.4%가 가능한 이유 (검증 불가)**:
1. 데이터셋 setup 차이 (다른 reader/judge 조합)
2. LongMemEval-S 데이터셋-specific tuning (per-category extraction
   prompt 등)
3. 별도 conversation memory layer (wg와 다른 retrieval surface)

→ **wg는 LongMemEval-S에서 ingest-time LLM-aided 작업으로 OMEGA
격차 못 메움**. 다른 평가 (dogfood / 새 fixture / 다른 데이터셋)에서
효과 측정 필요.

## wg가 강한 영역 (이미 입증)

LongMemEval-S에서:
- **wg + MiniMax-M2.7-highspeed = 74.0%** — Zep/Graphiti(71.2%) +2.8pt 추월
- **wg + gpt-4.1 = 72.6%** — reader-only 효과 +5pt 메움
- 이는 retrieval (R@10 0.992) + reasoning reader 조합

운영 측면 (LongMemEval과 무관):
- 4 native bindings (Py/Node/Elixir/C)
- Multi-store native (`wg project create/use`)
- Insert 토큰 0 (regex 기본; opt-in $1-3)
- Single Rust binary (no Python runtime)
- Tier A+B agent-UX 인프라 (Preference/Lesson/Error type, sessions,
  hooks, wg_context, consolidate / TTL)

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
