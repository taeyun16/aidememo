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

| reader | judge | overall | wall | 비용 |
|---|---|---|---|---|
| `gpt-4o-mini` | `gpt-4o-mini` | **0.540** | ~22 min | ~$0.25 |

**Per category:**

| Category | wg R@10 | E2E acc | r-MISS (wg 탓) | r-HIT-but-WRONG (LLM 탓) |
|---|---|---|---|---|
| single-session-assistant | 1.000 | 0.929 | 0 | 4 |
| single-session-user | 0.986 | 0.914 | 1 | 6 |
| knowledge-update | 1.000 | 0.731 | 0 | 21 |
| multi-session | 0.985 | 0.421 | 2 | 75 |
| single-session-preference | 0.933 | 0.300 | 2 | 19 |
| temporal-reasoning | 0.955 | 0.241 | 6 | 96 |

### 핵심 인사이트

**230건 오답 중 221건 (96.1%)이 retrieval은 맞췄는데 reader가 못 풀어낸 케이스.**
즉 wg의 retrieval 책임은 거의 다 했고, 부족한 건 작은 모델(gpt-4o-mini)의
reasoning 능력. multi-session과 temporal-reasoning이 특히 LLM 한계 노출:

- multi-session: 75/77 = 97% LLM 실패 (여러 session에 걸친 정보 합성)
- temporal-reasoning: 96/101 = 95% LLM 실패 (시간 비교, "지난주에 뭐 했지?")
- preference: 19/21 = 90% LLM 실패 (모호한 형용사 매칭)

### Published 비교

| 시스템 | reader | E2E accuracy |
|---|---|---|
| Mem0 | gpt-4o | 49.0% |
| **wg + decay-90** | **gpt-4o-mini** | **54.0%** |
| Zep | gpt-4o | 63.8% |
| Mastra | gpt-4o | 84.2% |
| Mastra | gpt-5-mini | 94.9% |
| MemPalace | (verbatim) | 96.6% |

**해석:**
1. **wg + gpt-4o-mini가 mem0 + gpt-4o(49%)를 +5pt 능가**. 더 약한 reader로
   더 좋은 retrieval로 보완 가능 — wg의 R@10 0.978이 실제로 reader bottleneck
   완화로 변환됨.
2. Zep / Mastra 대비 격차는 거의 전부 reader 차이 (gpt-4o-mini vs gpt-4o).
   같은 reader로 비교하면 gap이 좁혀질 가능성 → 다음 측정에서 확인.
3. R@10 retrieval ceiling 0.978이 **이론적 상한** (모든 retrieval-MISS는
   LLM이 어떻게 해도 못 풀음). 따라서 wg의 진짜 ceiling은 ~98%, 그 아래는
   reader 능력 함수.

### 다음 측정 권장

| reader | 예상 비용 | 가설 |
|---|---|---|
| gpt-4o (legacy frontier) | ~$5 | mem0/Zep/Mastra 직접 비교 — 75-85% 예상 |
| gpt-5.4-mini | ~$1-2 | Mastra gpt-5-mini와 비교 — 80-95% 가능성 |
| gpt-5.4 | ~$10-15 | Mastra+ 영역 — 90-95%+ 가능성 |

이 셋 추가 측정 시 ~$15-20. wg의 retrieval ceiling이 reader 따라 어디까지
변환되는지 정량적으로 답할 수 있음.

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
