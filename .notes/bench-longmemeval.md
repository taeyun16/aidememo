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
  현재 BM25는 timestamp를 점수에 반영하지 않음. wg의 `time_decay_tau_ms`
  + `as_of` 필터가 이 카테고리를 더 끌어올릴 수 있을지 후속 측정 가치.
- **single-session-preference** — "내가 어떤 종류의 음식을 좋아하나"
  같은 1인칭 선호. 짧은 fact + 정확한 형용사 매칭이 필요.
  rerank가 도움될 가능성.

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
