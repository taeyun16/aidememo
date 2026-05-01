# `wg_overview` 에이전트 효과 측정 (Phase 1)

> **결론 한 줄**: overview 자체의 효과는 broad-orientation 한 부류에 집중,
> MCP wire 페이로드 슬림화가 더 큰 토큰 win (양쪽 -28%). 커뮤니티 검출
> 정식 구현은 정당화되지 않음.

`wg_overview`가 실제 LLM 에이전트한테 의미가 있는지를 A/B로 측정.
같은 fixture wiki, 같은 task 7개를 두 조건으로 돌림:

- **A**: `wg_search` + `wg_query` + `wg_traverse` + `wg_recent` +
  `wg_entity_list` + `wg_fact_list` + `wg_entity_get`
- **B**: A + `wg_overview`

스크립트: [`scripts/overview_eval.py`](../scripts/overview_eval.py).

## Fixture

27 entities × 5개 entity_type (technology / service / person / incident /
concept / rfc), 34 facts × 6개 fact_type. 5개 토픽 클러스터:
data layer / auth / frontend / observability / people.

## Task 세트

| ID | 카테고리 | 기대 |
|---|---|---|
| `broad_orientation` | "이 위키 처음" | overview의 본질 |
| `domain_map` | 도메인 라벨링 | entity_list로도 충분 |
| `data_layer_decisions` | 토픽 그룹 요약 | overview helpful |
| `postgres_facts` | specific lookup | overview 무관 |
| `people_overview` | category 그룹 | overview 살짝 도움 |
| `cross_topic` | 다중 토픽 연결 | overview 무관 |
| `recent_activity` | 시간 기반 | wg_recent 1콜 |

## 측정 매트릭스

`gpt-4o`와 `gpt-4o-mini` × `pretty payload`와 `slim payload` × repeat=3
(총 84 agent runs + 84 judge runs).

| 지표 | gpt-4o pretty | gpt-4o slim | mini pretty | mini slim |
|---|---|---|---|---|
| A 입력 토큰 합 | 72,942 | **52,399 (-28%)** | 61,076 | **43,923 (-28%)** |
| B 입력 토큰 합 | 64,391 | **47,490 (-26%)** | 51,168 | **41,864 (-18%)** |
| A tool_calls 합 | 77 | 65 | 54 | 66 |
| B tool_calls 합 | 62 | 51 | 47 | 45 |
| AVG completeness Δ (B−A) | +1.0 | -4.3 | **-7.6 ⚠** | **+6.0 ✓** |

## 핵심 발견

### 1. MCP wire 페이로드 슬림화가 가장 큰 win

| 툴 | pretty | slim | 감소 |
|---|---|---|---|
| `wg_overview` | 7,189 B | 1,789 B | **-75%** |
| `wg_entity_list` (50건) | 5,131 B | 1,340 B | **-74%** |
| `wg_fact_list` (50건) | 21,838 B | 6,383 B | **-71%** |
| `wg_recent` (20건) | ~13 KB 추정 | 3,621 B | ≈ -72% |

제거한 redundancy:
- ULID 26자 (에이전트는 name으로 lookup; ID는 사실상 unused)
- 빈 `tags: []` (대부분 비어있음)
- bucket 안 `entity_type` 중복 (bucket key가 이미 그 type)
- `access_count: 0`, `relevance_score: 0.5`, `source_confidence: 0.5`,
  `pinned: false`, `source: null`, `observed_at: null`, `superseded_*: null`
- `created_at`/`updated_at`/`last_accessed_at` epoch ms 3개 (timeline은
  `wg_recent`로 처리; agent 답변 합성에선 거의 안 봄)
- pretty-print 들여쓰기 (~30%)
- `entity_ids` → 미리 resolve된 `entities: ["Postgres", "Redis"]`로 변환
  (에이전트가 follow-up `wg_entity_get` 호출할 일 없음)

A/B 둘 다 영향이라 비교에는 영향 없지만, **운영 비용 자체가 28%
감소**. 슬림 페이로드는 모든 MCP 툴 caller가 혜택.

### 2. mini 모델은 description 보강 없으면 함정

이전 (description: "Designed for an agent arriving at an unfamiliar
wiki — one call instead of stats + entity_list + fact_list"):

| Task | mini A comp | mini B comp |
|---|---|---|
| `data_layer_decisions` | 93 | 67 (**-27pt**) |
| `people_overview` | 100 | 80 (**-20pt**) |

→ mini는 overview JSON 받고 거기서 답을 합성하려다 detail miss.

description 보강 후 (위와 동일 + "**IMPORTANT: orientation map only.
Does NOT contain underlying facts. You MUST follow up with `wg_query`/
`wg_search`/`wg_fact_list` for any specific fact**"):

| Task | mini A comp | mini B comp |
|---|---|---|
| `data_layer_decisions` | 80 | 92 (**+12**) |
| `domain_map` | 20 | 50 (**+30**) |

13.6pt 회복. mini는 가이드 강하게 줘야 detail follow-up.

### 3. `broad_orientation`이 overview의 유일한 명확한 win

| 모델/조건 | A calls | B calls | A comp | B comp |
|---|---|---|---|---|
| gpt-4o slim | 6.0 | **1.0** | 53.3 | 75.0 (**+21.7**) |
| mini slim | 5.0 | **1.0** | 70.0 | 83.3 (**+13.3**) |

5~6배 tool call 절감, 한 자리수 pt 정확도 향상. 두 모델 모두 일관된 win.

다른 task는 ±5~15pt noise 안. `data_layer_decisions`, `cross_topic`은
gpt-4o slim에서 -25, -28pt 변동 — LLM judge의 noise 가능성도 있음
(같은 답을 다르게 점수 매김; n=3은 통계적으로 약함).

### 4. 커뮤니티 검출 정당성 약화

overview조차 "broad orientation 한 부류에서만 명확한 효과"가 결론.
정식 커뮤니티 알고리즘 (Louvain 등)은:
- 추가 ~500 LOC + 새 사이드카 + opt-in 갱신 비용
- 에이전트 활용 측면에서 overview보다 **명확하게 더 큰 효과** 입증
  필요 — 본 측정 결과로는 그 baseline이 약함

`wg_overview` + 강화된 description으로 broad-orientation 케이스를
이미 5~6배 절감 중. 커뮤니티가 추가로 줄일 여지는 marginal.

→ **정식 커뮤니티 검출은 보류 정당화.**

## 적용된 변경

- `mcp_tools.rs::tool_overview` — 컴팩트 JSON serialization
  (`to_string` not `_pretty`, ULID 제거, 중복 type 제거)
- `mcp_tools.rs::tool_entity_list` / `tool_fact_list` /
  `tool_entity_get` / `tool_fact_get` / `tool_recent` — 동일 패턴
  적용. `slim_fact_record` / `slim_entity_summary` 헬퍼 도입
- `mcp_tools.rs::list_tools()`의 `wg_overview` description에
  "orientation only / MUST follow up" 가이드 추가

## 한계

- n=3 repeat은 LLM judge noise 다 제거 못 함. 각 cell의 ±5pt는 무시.
- fixture는 27 entities / 34 facts — 작은 규모. 큰 위키 (1000+)에선
  overview의 효과가 더 클 가능성 (entity_list가 페이지네이션 필요).
- task 7개는 broad. 토픽 디스커버리 task가 더 많이 들어가면 overview
  효과 평균이 올라갈 수 있음.

## 미구현 / 후속

- Phase 2 (가상 커뮤니티 시뮬레이션) — Phase 1에서 정당성 약하다고
  판단. 큰 위키에서 overview 효과 재측정한 뒤 결정.
- 큰 위키 (LongMemEval-S session 등)에서 동일 측정 — 페이지네이션
  비용이 들어가면 다른 그림 나올 수 있음.
- `entity_list` / `fact_list`도 슬림화 됐지만, 더 큰 결과셋에서
  여전히 잘 동작하는지 (limit=200 등) 추가 검증.
