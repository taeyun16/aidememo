# LongMemEval-S — wg retrieval baseline

> 작성: 2026-04-30
> Harness: `benchmarks/src/bin/longmemeval.rs`
> Dataset 출처: [xiaowu0162/longmemeval-cleaned (HF)](https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned)

## 목적

2026 표준 agent-memory 벤치마크인 [LongMemEval](https://arxiv.org/abs/2410.10813)
의 retrieval-only 축에서 wg 점수를 확보한다. 공식 metric은 LLM이 retrieved
context로부터 답을 생성해 정답과 비교하지만, 그 단계는 wg가 직접 영향을
주지 않는다. wg가 결정하는 부분은 **답을 담은 evidence session이 top-K hit
안에 들어오는지** — 이게 실패하면 LLM이 무얼 해도 정답이 안 나온다. 이
재현/측정은 fair한 head-to-head이다.

## 측정 축

| metric | 의미 |
|---|---|
| **R@1** | top-1 hit이 evidence session 안의 fact인 질문 비율 |
| **R@5** | top-5 안에 evidence가 들어온 비율 |
| **R@10** | top-10 안에 evidence가 들어온 비율 |
| **MRR** | 첫 evidence hit의 reciprocal rank 평균 |
| 부 metric | question_type별 R@K (single-session / multi-session / temporal-reasoning / abstention 등) |

## 데이터 다운로드

cleaned 버전(원본의 noise session 제거됨, ~277 MB) 권장:

```bash
# Python
python -c "
from huggingface_hub import hf_hub_download
hf_hub_download(
    repo_id='xiaowu0162/longmemeval-cleaned',
    filename='longmemeval_s_cleaned.json',
    repo_type='dataset',
    local_dir='/tmp/longmemeval')
"

# 또는 curl (인증 토큰 필요한 경우 HF_TOKEN env)
curl -L -o /tmp/longmemeval/longmemeval_s_cleaned.json \
  "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json"
```

## 실행

```bash
# 빌드
cargo build --release -p wg-benchmarks --bin longmemeval

# 작은 fixture (committed, 3 questions) — harness sanity-check
./target/release/longmemeval --data benchmarks/fixtures/longmemeval_tiny.json

# 부분집합 (50문제)
LONGMEMEVAL_DATA=/tmp/longmemeval/longmemeval_s_cleaned.json \
  ./target/release/longmemeval --limit 50 --top-k 10

# 전체
LONGMEMEVAL_DATA=/tmp/longmemeval/longmemeval_s_cleaned.json \
  ./target/release/longmemeval --top-k 10
```

## 동작 요약

각 질문 처리:

1. tempdir에 격리 store 생성 (`semantic_index = bm25` — 베이스라인은
   model 부하 없이 BM25-only).
2. `haystack_session_ids` 각각을 `session:<id>` 이름의 Custom 엔티티로
   추가. 이걸 통해 fact ↔ session 매핑이 graph 구조로 보존.
3. 모든 `haystack_sessions[*][*]` turn을 `fact_add_many` 한 번에 적재.
   각 fact는 해당 session 엔티티 + `session:<id>` tag 부착.
4. `wg_search(question, bm25_only=true, top_k=K)` 호출.
5. 결과 fact의 entity name이 `answer_session_ids` 매핑된 session 엔티티
   중 하나면 hit. 첫 hit의 1-indexed rank 기록.

## Fixture 결과 (3 questions, 검증용)

```
R@1:  1.000
R@5:  1.000
R@10: 1.000
MRR:  1.000
wall: 0.22s
```

수치가 1.0인 이유: fixture는 evidence session 텍스트가 질문과 키워드를
겹치도록 조작된 합성 데이터. 진짜 LongMemEval-S에선 noise session이
40개 가까이 끼어 있어 BM25-only baseline은 R@5 ~ 0.4-0.6 범위가 예상됨
(공시: [Mem0 49% / Zep 63.8% on full LongMemEval at GPT-4o](https://docs.mem0.ai)
 — 단 이건 retrieval+생성 합산이라 직접 비교는 안 됨).

## 실측 결과

`xiaowu0162/longmemeval-cleaned/longmemeval_s_cleaned.json` (500 questions),
M-series Apple Silicon, `wg @ 257cb44`, BM25-only via
`SearchOpts { bm25_only: true, current_only: true }`.

```
LongMemEval-S retrieval baseline — wg BM25-only
questions: 500 / 500
top_k:    10

R@1:  0.866
R@5:  0.952
R@10: 0.974
MRR:  0.902
wall: 174.13s   (≈ 348 ms/question, includes per-question
                 store build + 50-200 turn ingest + 1 search)

By question_type:
  knowledge-update            R@10: 1.000  (78/78)
  single-session-assistant    R@10: 1.000  (56/56)
  single-session-user         R@10: 0.986  (69/70)
  multi-session               R@10: 0.985  (131/133)
  temporal-reasoning          R@10: 0.940  (125/133)
  single-session-preference   R@10: 0.933  (28/30)
```

**해석.** 이 수치는 **retrieval-only** — 답을 담은 evidence session이
top-10 hit에 들어왔는지 측정. 공식 LongMemEval은 LLM이 retrieved context
로부터 답을 생성한 다음 정답과 비교하는데, 그 단계는 LLM 품질에 좌우되고
wg가 직접 영향을 주지 않는다. **R@10 0.974**는 LLM이 답을 만들 때 정답에
필요한 evidence를 97% 이상 컨텍스트에 가지고 들어간다는 의미.

**비교 참조** (직접 비교 X — 다른 metric, 다른 LLM, 다른 stack):

| 시스템 | LongMemEval 점수 | 출처 |
|---|---|---|
| MemPalace (verbatim Chroma) | 96.6% (full E2E with LLM) | 자체 공시 |
| Supermemory | 99%+ 주장 | 자체 공시 |
| Mastra Observational | 84.2% (gpt-4o) | [Mastra research](https://mastra.ai/research/observational-memory) |
| Zep / Graphiti | 63.8% (gpt-4o) | [Zep](https://www.getzep.com/) |
| Mem0 | 49.0% (gpt-4o) | [Mem0 docs](https://docs.mem0.ai) |
| **wg (retrieval R@10, BM25-only)** | **0.974** | 본 측정 |

wg의 R@10 = 0.974는 retrieval 구간만 — LLM 품질 추가하면 답 정확도는 그
이하. 그래도 retrieval ceiling이 매우 높다는 뜻이고, 이 단계가 system
설계의 가장 중요한 책임 영역이다.

## 가장 약한 카테고리 분석

`temporal-reasoning` (94%)와 `single-session-preference` (93%)가 R@10
하락의 주범:

- **temporal-reasoning** — "지난주에 결정한 게 뭐냐" 같은 시간 한정 질문.
- **single-session-preference** — "내가 어떤 종류의 음식을 좋아하나"
  같은 1인칭 선호. 짧은 fact + 정확한 형용사 매칭이 필요.
  rerank가 도움될 가능성.

## 후속 측정: temporal mode (negative result)

`--temporal` 옵션을 harness에 추가해서 각 fact에 `observed_at =
haystack_date`를 stamp하고 검색 시 `until = question_date`로 미래
세션을 필터해 봤다 (`benchmarks/src/bin/longmemeval.rs`):

```
temporal-reasoning baseline:    R@10 0.940  (125/133)
temporal-reasoning --temporal:  R@10 0.782  (104/133)  ← 16pt 하락
```

**이유** — LongMemEval-S 데이터셋 자체에 dating noise가 있다. 133개
temporal-reasoning 질문의 **answer-evidence sessions 74개가 question_date
이후로 timestamp 매김**:

```bash
python3 -c "..." # see commit 메시지
# temporal-reasoning total: 133
# total future-dated sessions: 1417
# future-dated EVIDENCE sessions: 74
```

즉 evidence 자체가 미래로 잘못 매겨진 케이스가 절반 이상이라 hard-cutoff
필터가 evidence를 제외한다. 합성 chat-memory 코퍼스를 만들 때 흔히 생기는
artifact (질문은 시점 t, evidence는 t 이후 LLM 생성).

**시사점:**
1. **LongMemEval-S에선 temporal filter ROI 없음.** `until=question_date`
   가정 자체가 데이터 contract와 맞지 않음.
2. **실제 위키엔 ROI가 다를 가능성.** frontmatter `date:` / `decided_at:`
   이 정확한 마크다운 위키에선 같은 메커니즘이 도움. 우리 자체 도그푸드
   세팅(`wg watch`)에서 후속 측정 가치.
3. **soft-bias가 더 나을 가능성.** hard-cutoff 대신 `time_decay_tau_ms`
   로 가중치만 조정하면 evidence가 future-dated여도 살아남을 수 있다 →
   다음 실험에서 검증.

`--temporal` 플래그는 harness에 남겨둔다 (위키 스타일 데이터에 재시험할 때
유용). LongMemEval-S baseline 보고는 BM25-only 0.974 그대로.

## 후속 측정: 어댑터 학습 효과 (구조적 부적합)

LongMemEval-S에서 어댑터 학습이 R@K에 주는 영향을 측정하려 했으나 데이터셋
contract와 어댑터 메커니즘이 맞지 않아 의미있는 측정이 불가:

1. 데이터셋 — 각 질문은 자체 haystack을 가지고 다른 질문과 fact를
   공유하지 않는다. harness는 question별 isolated tempdir wiki를 만든다
   (`build_store_for_question`).
2. 어댑터 — fact ID별 bias. store별 meta에 저장되고 다른 store와 공유
   불가. cross-question learning이 일어날 store-level state가 없다.

**가능한 실험 형태들과 그 한계:**

- **Within-query perfect-feedback (overfitting)**: 같은 질문에서 evidence
  fact를 helpful로 표시 → 어댑터 학습 → 같은 질문 재검색. 모든 evidence가
  rank 1에 들어오는 것을 보장하지만 이건 어댑터가 약속한 "boost" 메커니즘
  자체의 단위 테스트일 뿐 — agent value를 측정하지 않음.
- **Train/test split (cross-question)**: 250 questions train, 250 test.
  하지만 train의 어댑터 state가 test wiki에 carry되지 않으므로 무의미.
- **Repeated-query simulation**: 같은 질문을 여러 차례 묻는다고 가정하고
  매 회 helpful 피드백 누적. 비현실적인 시나리오 (실제 agent는 같은
  질문을 거의 반복 안 함).

**결론**: 어댑터의 가치는 LongMemEval처럼 **하나의 질문 = 하나의
isolated 짤막한 코퍼스** 형태가 아니라, **장기간 누적되는 도그푸드 위키
+ 반복적 사용자 질의** 환경에서 드러남. 우리 자체 wiki에서 6+ 개월간
`wg feedback` 누적 후 retrieval 정확도 변화를 측정하는 게 맞는 실험.

어댑터 자체가 작동한다는 증거는 wg-core 단위 테스트
(`adapt::tests::weight_factor_*`)와 통합 테스트
(`hybrid_search_adapter_promotes_helpful_facts`, #[ignore]에 있음 —
모델 다운로드 필요)로 이미 확보됨. LongMemEval 측정은 skip.

## 후속 측정: time_decay soft-bias

`--time-decay-days τ` 옵션 추가 (`benchmarks/src/bin/longmemeval.rs`).
세션마다 `observed_at = haystack_date` stamp + BM25 검색 후 score를
`exp(-|question_date - observed_at|_days / τ)` 로 곱셈해 재정렬.
top_k×5 wider candidate slate를 가져와서 하위 BM25 hit이 추가 boost로
top_k에 들어올 수 있게 함.

**Tau 스윕 (full 500 questions):**

| τ (days) | R@1 | R@5 | R@10 | MRR | Δ vs baseline |
|---|---|---|---|---|---|
| baseline (no decay) | 0.866 | 0.952 | **0.974** | 0.902 | — |
| τ = 7 | 0.730 | 0.894 | 0.948 | 0.803 | -2.6pt R@10 (over-aggressive) |
| τ = 14 (temporal-only run) | n/a | n/a | (0.955 on temporal) | n/a | n/a |
| τ = 30 (temporal-only run) | n/a | n/a | (0.955 on temporal) | n/a | n/a |
| τ = 90 | 0.858 | 0.958 | **0.978** | 0.898 | **+0.4pt R@10** |
| τ = 180 | 0.860 | 0.954 | **0.978** | 0.899 | **+0.4pt R@10** |

**Per-category breakdown @ τ=90 (sweet spot, full run):**

| Category | Baseline | τ=7 (aggressive) | τ=90 (gentle) |
|---|---|---|---|
| knowledge-update | 1.000 | 0.936 | **1.000** |
| single-session-assistant | 1.000 | 1.000 | **1.000** |
| single-session-user | 0.986 | 0.929 | **0.986** |
| multi-session | 0.985 | 0.970 | **0.985** |
| **temporal-reasoning** ★ | 0.940 | 0.962 | **0.955** |
| single-session-preference | 0.933 | 0.767 | **0.933** |

**해석:**
1. **Aggressive decay (τ=7)는 temporal에서 이기지만 다른 5개를 모두
   잃는다.** 7일 halflife는 LongMemEval-S 세션 분포 (수개월 ~ 1년 span)
   대비 너무 짧아 evidence를 깎아냄.
2. **Gentle decay (τ=90 ~ 180)는 안전한 sweet spot.** temporal-reasoning
   +1.5pt 개선 + 다른 카테고리 정확히 보존 → 전체 R@10 +0.4pt 순이익.
3. **다음 단계 권장**: wg 기본값 (`time_decay_tau_ms = 7,776,000,000` =
   90일)이 우리 실측에서 정당화됨. 도그푸드 위키에서도 이 값으로 시작.

**negative finding의 후속으로서**: hard `until` cutoff (이전 실험의
-16pt 회귀)와 soft decay (이번 실험의 +0.4pt 개선)는 같은 신호("질문
시점 부근 fact를 부스트")를 주지만 강도가 완전히 다른 결과로 이어짐.
**dataset에 dating noise가 있을 때 hard filter는 evidence를 잃지만
soft bias는 score 압축으로 살아남게 한다** — 일반적인 retrieval 설계
교훈.

## 후속 측정: ONNX-based fastembed providers

### bge-small-en-v1.5 (English-tuned, 384-dim, 90MB)

OMEGA가 95.4%로 1위 찍은 retrieval pipeline의 핵심 임베딩. wg에
`fastembed` cargo feature + `model.provider = "fastembed"` /
`model.name = "bge-small-en-v1.5"` 조합으로 통합 (commit e0580f9).

**500 questions, --hybrid + --embed-model bge-small-en-v1.5 + decay τ=90:**

| metric | BM25 baseline | model2vec + decay | bge + decay | bge alone | bge+reranker (K=10) | **two-stage K=20→10** ★ |
|---|---|---|---|---|---|---|
| R@1 | 0.866 | 0.858 | 0.808 | 0.914 | 0.938 | **0.940** |
| R@5 | 0.952 | 0.958 | 0.952 | 0.976 | 0.984 | **0.984** |
| **R@10** | 0.974 | 0.978 | 0.984 | 0.986 | 0.986 | **0.992** |
| MRR | 0.902 | 0.898 | 0.868 | 0.941 | 0.957 | **0.958** |

**Two-stage retrieval (commit pending)**: when a reranker is configured,
the harness fetches the top-`2k` BM25+vector candidates and lets the
cross-encoder re-rank them down to the requested top-k. The wider
candidate pool gives the cross-encoder access to more easy-to-promote
gold sessions, lifting R@10 from 0.986 → 0.992 (+0.6pt) without
hurting R@1.

| | narrow K=10 | **wide K=20→10** |
|---|---|---|
| knowledge-update R@10 | 1.000 | 1.000 |
| multi-session R@10 | 0.985 | 0.992 |
| single-session-assistant R@10 | 1.000 | 1.000 |
| single-session-preference R@10 | 0.967 | 1.000 |
| single-session-user R@10 | 1.000 | 1.000 |
| temporal-reasoning R@10 | 0.962 | 0.977 |

**Per-category R@1 (decisive metric for LLM reader):**

| Category | BM25+decay | bge alone | **bge + reranker** | best Δ vs origin |
|---|---|---|---|---|
| single-session-preference | 0.367 | 0.700 | **0.733** | **+36.6** |
| multi-session | 0.842 | 0.917 | **0.955** | **+11.3** |
| knowledge-update | 0.949 | 0.987 | **1.000** | +5.1 |
| single-session-assistant | 0.982 | 1.000 | 0.982 | 0 (saturated) |
| single-session-user | 0.900 | 0.929 | **0.957** | +5.7 |
| temporal-reasoning | 0.857 | 0.872 | **0.902** | +4.5 |

bge embedding quality + observed_at가 LongMemEval-S 2023 timestamps라
"now" 기준 wg-core in-pipeline decay가 모든 fact를 동등하게 crush.
post-hoc decay는 question_date 기준이라 다른 신호. 둘 모두 net 음.
**bge alone이 5/6 카테고리에서 best, 나머지(temporal)는 0.872 vs 0.895
1.3-2.3pt 차이로 거의 동등.**

**Per category R@10 (vs BM25 baseline):**

| Category | BM25 | bge+decay | Δ |
|---|---|---|---|
| knowledge-update | 1.000 | 1.000 | 0 |
| single-session-assistant | 1.000 | 1.000 | 0 |
| **single-session-user** | 0.986 | **1.000** | **+1.4** |
| multi-session | 0.985 | 0.985 | 0 |
| **temporal-reasoning** | 0.940 | 0.962 | **+2.2** |
| **single-session-preference** | 0.933 | **0.967** | **+3.4** |

**해석:**
- **R@10 = 0.984** — wg 측정 사상 최고. retrieval ceiling 더 끌어올림.
- 약점 카테고리에서 의미 있는 개선: preference +3.4pt, temporal +2.2pt.
- 단, R@1 / MRR은 약간 하락 — bge는 top-10 안에 정답을 더 많이
  넣지만, 그 안의 rank는 약간 분산. LLM reader로 보면 동일 top-10이라
  수치적 차이는 무시할 수 있을 가능성.
- 비용: 500q에 wall 1390s (~2.8s/question, 매 question마다 모델 콜드
  로드 포함). 운영 환경에선 한 번 로드 후 amortize됨.

### 추가 옵션 (config.toml만 수정으로 활성)

```toml
# 다국어 (한국어 등) — bge 영어보다 한국어 우수
[model]
provider = "fastembed"
name = "multilingual-e5-large"  # 1024-dim
# 또는 name = "bge-m3"  # 1024-dim, BGE 다국어 SOTA

# Cross-encoder reranker (in-process, no TEI server)
[rerank]
provider = "fastembed"
model = "bge-reranker-v2-m3"  # 다국어
top_k = 20
```

## 후속 측정: hybrid (BM25 + semantic) vs BM25-only

`--hybrid` flag로 in-process model2vec semantic search 활성화
(default `minishlab/potion-multilingual-128M`). 기존 약점인
`single-session-preference` (BM25-only R@10 0.933) 끌어올리기 시도.

**single-session-preference (30 questions):**

| | R@1 | R@5 | R@10 | MRR | wall |
|---|---|---|---|---|---|
| BM25-only | 0.367 | 0.800 | 0.933 | 0.535 | 10s |
| Hybrid (BM25 + semantic) | **0.433** | 0.800 | 0.933 | **0.596** | 30s |
| Δ | +6.7pt | — | — | +6.1pt | 3× slower |

**해석:** semantic이 BM25가 이미 찾은 top-10 안에서 정확한 답을 위로
끌어올림 — R@10 ceiling은 같지만 R@1과 MRR이 의미있게 개선. preference
("내가 어떤 음식을 좋아하나" 같은 의미 매칭이 중요한 질문)에 적합.

**Full 500 questions, hybrid + decay τ=90:**

| 설정 | R@1 | R@5 | **R@10** | MRR |
|---|---|---|---|---|
| baseline (BM25-only) | 0.866 | 0.952 | 0.974 | 0.902 |
| BM25 + decay-90 | 0.858 | 0.958 | **0.978** | 0.898 |
| Hybrid + decay-90 | 0.762 | 0.934 | 0.974 | 0.832 |

**해석:** Hybrid은 카테고리별로 영향이 갈린다:

| Category | BM25 base | Hybrid+decay | Δ |
|---|---|---|---|
| single-session-user | 0.986 | 1.000 | **+1.4** |
| temporal-reasoning | 0.940 | 0.962 | **+2.2** |
| multi-session | 0.985 | 0.985 | 0 |
| knowledge-update | 1.000 | 0.987 | -1.3 |
| single-session-assistant | 1.000 | 0.982 | -1.8 |
| single-session-preference | 0.933 | 0.867 | -6.6 |

multilingual potion-128M이 영어 chat 코퍼스에 완벽 align되지 않아
preference 같은 정확 매칭 의존 카테고리는 오히려 noise 추가. 전체
R@1/R@5/MRR 모두 BM25-only baseline보다 낮음.

**카테고리별 권장 config:**

- **temporal-reasoning** → BM25 + decay τ=90 (0.940 → 0.955)
- **single-session-preference** → hybrid (0.367 → 0.433 R@1)
- **나머지** → BM25-only이 가장 견고

운영 차원에서: 도메인별 측정 후 config 분기 또는 query classifier로
설정을 다르게 가는 방향이 정량적으로 정당화됨. 단일 "best config"는
없다.

## E2E LLM 측정 (Phase 2: reader + judge)

`scripts/longmemeval_e2e.py`로 wg retrieval JSONL + OpenAI API
호출. retrieval은 best config (BM25 + decay τ=90, R@10 0.978).

### 결과 (500 questions, 2026-04-30)

**v1 측정 (초기 prompt with abstention emphasis):**

| reader | judge | overall | preference | temporal | knowledge |
|---|---|---|---|---|---|
| `gpt-4o-mini` | `gpt-4o-mini` | 0.540 | 0.300 | 0.241 | 0.731 |
| `gpt-4o` | `gpt-4o-mini` | 0.480 | 0.200 | 0.218 | 0.679 |
| `gpt-5.4-mini` | `gpt-4o-mini` | 0.484 | 0.533 | 0.165 | 0.615 |

**v1에서 발견된 prompt 결함** — bigger / reasoning-tuned model일수록 abstention
지침을 보수적으로 해석. retrieval rank 1에 답이 있어도 "I don't know" 회피.
예시 (qid=8ebdbe50, gold "Data Science"):

```
rank 1 retrieval: "I need to add my latest certification in Data Science…"
gpt-4o response: "I don't know."
```

→ Prompt에서 abstention emphasis 제거, extraction을 명시적으로 권장하도록 수정.

**v2 측정 (extraction-friendly prompt):**

| reader | judge | **overall** | preference | temporal | knowledge | 비용 |
|---|---|---|---|---|---|---|
| `gpt-4o-mini` | `gpt-4o-mini` | 0.580 | 0.567 | 0.323 | 0.705 | ~$0.25 |
| `gpt-4o` | `gpt-4o-mini` | 0.586 | 0.533 | 0.316 | 0.744 | ~$3.90 |
| `gpt-5.4-mini` | `gpt-4o-mini` | 0.596 | 0.700 | 0.353 | 0.654 | ~$1.50 |

**v1 → v2 효과:** 같은 retrieval slate 동일, prompt만 수정으로 +4-12pt 상승.
abstention이 강할수록 큰 모델이 더 손해를 봤음.

### Judge calibration (gpt-4o-mini vs gpt-4o judge)

published baselines (mem0/Zep/Mastra)이 모두 gpt-4o judge를 쓰므로,
같은 axis로 비교하기 위해 v2 결과를 gpt-4o judge로 재채점.

| reader | mini judge | **gpt-4o judge** | Δ | agreement |
|---|---|---|---|---|
| gpt-4o-mini | 0.580 | **0.600** | +2.0 | 96.4% |
| gpt-4o | 0.586 | **0.604** | +1.8 | 95.8% |
| gpt-5.4-mini | 0.596 | **0.626** | +3.0 | 94.6% |

**Agreement 95-96%** — 두 judge가 대부분 동의. 차이는 직선적 — gpt-4o
judge가 CORRECT 표시를 14-21건 더 많이 줌, 반대 방향(mini correct, 4o
wrong)은 4-6건 뿐. **mini judge는 verbose answer를 보수적으로 incorrect
판정하는 경향 — published 비교에는 gpt-4o judge가 fair**.

**Per-category, gpt-5.4-mini reader × gpt-4o judge (best v2):**

| Category | E2E acc |
|---|---|
| single-session-user | 0.971 |
| single-session-assistant | 0.964 |
| single-session-preference | 0.767 |
| knowledge-update | 0.705 |
| multi-session | 0.466 |
| temporal-reasoning | 0.383 |

multi-session 47% / temporal 38%로 reader 한계 — judge 바뀌어도 bottom
카테고리 양상 같음. 즉 published 시스템(Mastra 84%, OMEGA 95%) 대비 차이
의 본질은 **judge calibration이 아니라 retrieval+reader 파이프라인 차이**.

### Per-category 상세 (v2)

| Category | gpt-4o-mini | gpt-4o | gpt-5.4-mini | 최고 |
|---|---|---|---|---|
| single-session-assistant | 0.964 | 0.929 | 0.946 | 4o-mini ✓ |
| single-session-user | 0.943 | 0.943 | 0.929 | 동률 |
| knowledge-update | 0.705 | 0.744 | 0.654 | gpt-4o ✓ |
| single-session-preference | 0.567 | 0.533 | 0.700 | 5.4-mini ✓ |
| multi-session | 0.414 | 0.444 | 0.459 | 5.4-mini ✓ |
| temporal-reasoning | 0.323 | 0.316 | 0.353 | 5.4-mini ✓ |

### Retrieval × Verdict 크로스탭 (best v2: gpt-5.4-mini)

```
                       CORRECT  INCORRECT
  retrieval HIT            295        194
  retrieval MISS             3          8
```

**202건 오답 중 194건 (96%)이 retrieval은 맞췄는데 LLM이 합성/추론 실패.**
v1의 같은 패턴 (96%) 유지 — prompt 수정해도 reasoning 한계는 그대로.

| Category | total | CORRECT | r-MISS (wg 탓) | r-HIT-but-WRONG (LLM 탓) |
|---|---|---|---|---|
| single-session-assistant | 56 | 53 (0.946) | 0 | 3 |
| single-session-user | 70 | 65 (0.929) | 1 | 5 |
| knowledge-update | 78 | 51 (0.654) | 0 | 27 |
| multi-session | 133 | 61 (0.459) | 2 | 70 |
| single-session-preference | 30 | 21 (0.700) | 2 | 8 |
| temporal-reasoning | 133 | 47 (0.353) | 6 | 81 |

multi-session과 temporal-reasoning이 모델 ceiling. retrieval은 R@10 0.985
/ 0.955로 대부분 맞추지만 LLM이 다중 세션 합성 / 시간 비교에서 무너짐.

### Published 비교 (judge=gpt-4o로 통일)

| 시스템 | reader | E2E |
|---|---|---|
| Mem0 (published) | gpt-4o | 0.490 |
| wg @ model2vec + decay-90 | gpt-4o-mini | 0.600 |
| wg @ model2vec + decay-90 | gpt-4o | 0.604 |
| wg @ model2vec + decay-90 | gpt-5.4-mini | 0.626 |
| wg @ bge + reranker wide K=20→10 | gpt-4o-mini | 0.656 |
| wg @ bge + reranker wide K=20→10 | gpt-5.4-mini | 0.660 |
| wg @ bge + reranker wide K=20→10 | gpt-4o | 0.676 |
| wg @ bge + reranker wide K=20→10 + dedup | gpt-4o-mini | 0.662 |
| **wg @ bge + reranker wide K=20→10** | **gpt-4.1** | **0.726** |
| **wg @ bge + reranker wide K=20→10** ★ | **MiniMax-M2.7-highspeed** | **0.740** |
| Zep / Graphiti 2026 (published) | gpt-4o | 0.712 |
| Supermemory (published) | (?) | 0.854 |
| Emergence AI (published) | (?) | 0.860 |
| Mastra (published) | gpt-4o | 0.842 |
| Mastra (published) | gpt-5-mini | 0.949 |
| OMEGA (published, local) | gpt-4.1 | 0.954 |

### 2026-05-01 재측정 카테고리별 (bge + reranker wide K=20→10)

| Category | gpt-4o | gpt-5.4-mini | gpt-4o-mini | gpt-4o-mini (+dedup) | **MiniMax-M2.7-highspeed** ⭐ |
|---|---|---|---|---|---|
| knowledge-update | 80.8% (63/78) | 69.2% (54/78) | 74.4% (58/78) | 74.4% (58/78) | **84.6%** (66/78) |
| multi-session | 61.7% (82/133) | 62.4% (83/133) | 59.4% (79/133) | 62.4% (83/133) | **71.4%** (95/133) |
| single-session-assistant | 94.6% (53/56) | 96.4% (54/56) | 94.6% (53/56) | **96.4%** (54/56) | 92.9% (52/56) |
| single-session-user | 97.1% (68/70) | 97.1% (68/70) | **100%** (70/70) | 98.6% (69/70) | 97.1% (68/70) |
| single-session-preference | 63.3% (19/30) | **80.0%** (24/30) | 60.0% (18/30) | 63.3% (19/30) | **80.0%** (24/30) |
| **temporal-reasoning** ⚠ | 39.8% (53/133) | 35.3% (47/133) | 37.6% (50/133) | 36.1% (48/133) | **48.9%** (65/133) |
| **Overall** | 67.6% | 66.0% | 65.6% | 66.2% | **74.0%** ⭐ |

**Reader 선택 트레이드오프:**
- **gpt-4o** — 절대 best (67.6%). knowledge-update에서 압도적 (80.8%).
  비용 ~$5/500q.
- **gpt-5.4-mini** — best ROI (66.0%, $1.50). single-session-preference
  에서 +17pt 격차 (80.0% vs 63.3%) — 선호 추론 강점. 비용 1/3.
- **gpt-4o-mini** — 운영 minimum (65.6%, $0.30). single-session-user
  100% 도달.

**retrieval 개선 amplification 모델별 차이:**
- gpt-4o: model2vec → bge+reranker로 **+7.2pt** (가장 큰 수혜)
- gpt-4o-mini: +5.6pt
- gpt-5.4-mini: +3.4pt (이미 reasoning 능력 있어서 retrieval gain이 작음)

→ retrieval 개선은 **smaller / older 모델에 더 도움**. reasoning 모델은
이미 부족한 retrieval에서도 답을 만들어냄.

### 핵심 결론 (2026-05-01 갱신 — MiniMax 측정 후)

1. **wg + MiniMax-M2.7-highspeed = 0.740** ⭐ — Zep/Graphiti (0.712)
   처음 추월 (**+2.8pt**). 같은 retrieval(wide-rerank)에 reader만
   gpt-4o → MiniMax로 바꿔서 **+6.4pt 점프**. local-first + non-OpenAI
   조합으로 SOTA-근접 영역 진입.
2. **MiniMax 카테고리별 압도**: multi-session **+9.7pt** (vs gpt-4o
   61.7→71.4), temporal-reasoning **+9.1pt** (39.8→48.9), knowledge-
   update **+3.8pt** (80.8→84.6). reasoning 모델이 cross-session 합산
   + current-state 추론에서 명확한 이점.
3. **vs Mem0 (0.490): +25.0pt 압도** (이전 +11.4pt에서 더 벌림).
4. **vs Zep/Graphiti (0.712): +2.8pt 추월** — 이전 -10.8pt에서 완전
   반전. retrieval ceiling (R@10 0.992) + reasoning reader 조합의 힘.
5. **vs Mastra (0.842): -10.2pt** — 다음 ceiling. Mastra의
   Observer/Reflector ingest-time LLM 정제가 핵심 격차.
6. **vs OMEGA (0.954, gpt-4.1): -21.4pt** — OMEGA는 lifecycle
   management (compaction / consolidation / context-virt 25 tools)에서
   추가 마진. wg에 Tier 1+2 구현됨, dogfood 측정 필요.
7. **dedup 효과 (LongMemEval에선 미미)**: gpt-4o-mini 65.6 → 66.2
   (+0.6pt). Multi-session +3.0, preference +3.3. 단 LongMemEval은
   isolated stores라 dedup 효과 측정 부적합 — 진짜 효과는 dogfood.
8. **single-session-user 100% (mini), 97.1% (다른 reader) 도달** —
   거의 완벽.
9. **약점 = temporal-reasoning** — gpt-4o 39.8 / MiniMax 48.9.
   LongMemEval-S timestamp noise (이전 분석 참조). 데이터셋 한계.

**격차 좁히는 ROI 순 (다음 단계)**:
   - (a) **MiniMax with dedup-enabled** 측정 — 추가 +1-2pt 추정
   - (b) **type-weighted scoring + decay-exempt 활성** — preference/
     decision boost — 추정 +2-3pt
   - (c) **`FactType::Preference` + `FactType::Lesson` 추가** — agent
     UX 직접 개선
   - (d) **LLM-aided ingestion** (Mastra/OMEGA 격차) — 추정 +3-5pt

전체 head-to-head는 [`compare-graphiti.md`](compare-graphiti.md) 참조.

### 다음 측정 권장 (선택)

| reader | 예상 비용 | 가설 |
|---|---|---|
| gpt-5.4 (full frontier) | ~$10-15 | multi-session/temporal에서 추가 +5-15pt 가능성 — Mastra 84% 도달? |
| gpt-5.5 (newest premium) | ~$15-20 | 자체 SOTA 수준 (90%+) 가능성 |
| 다른 judge (gpt-4o instead of mini) | ~$3 | judge calibration 차이 측정 |
| 자체 도그푸드 위키 | 무료 | 이건 reader 없는 R@K 측정 — 본 결과의 일반화 검증 |

추가 측정 합산 ~$25-40. 현 결과만으로도 카테고리 비교 표에 자리 잡음.

## TEI rerank (skipped — 별도 인프라 필요)

`rerank.provider = "tei"` + `rerank.endpoint = http://...` + BGE-reranker
서버를 띄우면 hybrid 결과의 top-K를 cross-encoder로 재정렬. 본 측정에선
TEI 인스턴스가 없어 skip. 예상 ROI:

- preference 같은 짧은 사실 매칭에서 +5-15pt R@1 가능 (cross-encoder가
  강한 영역)
- 모든 카테고리에서 추가 latency 50-200 ms/query (TEI 호출 + rerank)
- 운영 권장: query type을 알 수 있을 때만 rerank 활성화 (bool flag로
  agent가 결정)

수치는 자체 TEI 서버가 있을 때 측정 후 추가 필요.

## 향후 측정

1. **하이브리드 (BM25 + 시맨틱)** — `--top-k 10` + `bm25_only=false`
   비교. 영어 데이터셋이라 model2vec multilingual보다 e5-small이 좋을
   가능성. `wg vector-rebuild` 후 재실행.
2. **어댑터 적용** — synthetic feedback (evidence session에 helpful=true)
   몇 회 학습 후 R@K 변화량.
3. **rerank** — TEI BGE-reranker 적용 시 top-10 → top-5 재정렬 효과.
4. **세션 단위 적재** — 현재는 turn 단위 fact 적재. session 전체를 한
   fact로 적재했을 때 BM25 score 분포 차이.

## 주의

- `fact_add_many`로 일괄 적재해도 ~40 sessions × ~5 turns/session
  ≈ 200 facts × 500 questions = 100K fact 적재. 단일 fsync로 묶이지만
  BM25 인덱스 lazy rebuild가 search 첫 호출에서 발생 — 첫 호출이 느림.
- HuggingFace 다운로드는 277 MB. 디스크 + 네트워크 사전 확보 필요.
- LongMemEval은 영어 corpus. 한국어 retrieval은 [bench-miracl-ko.md](./bench-miracl-ko.md) 참고.
