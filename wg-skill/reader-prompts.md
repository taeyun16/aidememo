---
kind: doc
title: Agent reader-prompt patterns
---

# Reader prompt 패턴 (LongMemEval 관찰 기반)

OMEGA가 LongMemEval 점수를 93.2 → 95.4%로 끌어올린 **4 prompting
tricks** ([dev.to 저자 글](https://dev.to/singularityjason/how-i-built-a-memory-system-that-scores-954-on-longmemeval-1-on-the-leaderboard-2md3))는
모두 reader 단계의 prompt 엔지니어링 — wg가 retrieval로 evidence를
가져와도 reader가 어떻게 처리하느냐에 따라 답이 갈림. 이 문서는
같은 패턴을 wg + 임의의 reader (gpt-4o / gpt-4.1 / MiniMax / Claude /
Gemini) 사용 시 적용할 수 있도록 정리.

> 모두 wg가 직접 적용하는 게 아닌, **agent가 reader를 호출할 때 추가할
> system / user prompt 조각**입니다.

## 1. Temporal-aware prompting (+5q on LongMemEval)

기본 reader prompt에 시간 비교 가이드 추가:

```
When the question references a time period (last week / last month /
since X / between X and Y), check the `created_at` and `observed_at`
fields of each retrieved fact. If the fact's date doesn't fall in
the asked window, downweight it even if the content matches.

If retrievals contain conflicting facts dated differently, prefer
the most recent one UNLESS the question explicitly asks about a
historical state ('what was X back in 2024?').
```

wg는 search response에 `created_at` / `observed_at` / `age_days` /
`freshness_warning` 필드를 슬림 페이로드로 전달 (`crates/wg-cli/src/cmd/mcp_tools.rs::slim_fact_record`).
reader가 이 메타를 활용하도록 prompt에 명시.

## 2. Knowledge-update current-state prompting (+4q)

기본 reader prompt에:

```
When the user's information has changed over time (location / job /
preferences / tooling), the latest non-superseded fact is the
correct answer. Wg's `current_only` filter and `superseded_by`
chain already exclude old facts from the default search; if you
DO see a fact with `superseded_by != null`, treat it as historical
and look for its replacement.
```

wg는 default `current_only=true`로 superseded fact 제외. reader가
이 디폴트를 신뢰하도록 명시 + supersede chain follow 가이드.

## 3. Query augmentation (+2q)

retrieval 전에 query를 한 번 LLM으로 rewrite:

```
Rewrite this user question into a search query that captures both:
1. The semantic intent (what they want to know)
2. Specific entity names / nouns / proper nouns that might appear
   in stored facts

Keep it under 80 characters. Don't add stop words.
```

Original: "How did we end up handling auth in the new system?"
Rewritten: "auth system migration decision Keycloak JWT-Service"

wg + 이 변환된 query → hybrid_search 정확도 증가.

대안: agent가 직접 multiple queries (대화 의도 + entity name 분리)
호출하고 결과 병합.

## 4. Preference personalization (+2q)

reader prompt에 **wg_context의 personalisation 섹션 활용** 명시:

```
Before answering, scan the `personalisation` array (preference /
lesson / error type facts). If any preference contradicts a
generic answer you'd give, defer to the user's preference.
Examples: 'I prefer dark mode' overrides 'most users prefer light',
'tried bare-metal Postgres, hit IO limits' steers away from
recommending bare-metal.
```

wg `wg_context` 응답에 `personalisation` 섹션 자동 surface (Tier A3).
reader가 이걸 사용하도록 명시만 하면 됨.

## 합쳐진 reader system prompt 예시

OMEGA-style 4 tricks 통합:

```
You are answering using snippets retrieved from the user's
knowledge wiki. Apply these rules in order:

1. PERSONALISATION FIRST — scan `personalisation` array; user's
   own preferences and prior lessons override generic best-practice.
2. CURRENT-STATE — when the user asks 'what is X' (not 'what was X'),
   only use facts with no `superseded_by`. The wiki's `current_only`
   filter is already on by default.
3. TEMPORAL — when the question references a time window, downweight
   facts whose `observed_at` / `created_at` falls outside it. Recent
   facts win for current-state queries; historical facts win when
   the user asks about the past.
4. CONFIDENCE — quote / paraphrase directly from the snippets;
   don't extrapolate. If no snippet covers the question, say so.
```

이 system prompt를 reader 호출에 prepend → 추정 +5-10pt overall on
LongMemEval-style queries (categorical 측정 필요).

## 측정 가이드

`scripts/longmemeval_e2e.py`의 `--reader-prompt {basic,omega-tricks}`
flag로 같은 retrievals + 같은 reader에 prompt만 바꿔 A/B 가능.

```bash
python3 scripts/longmemeval_e2e.py \
  --retrievals /tmp/wg_retrievals_500_bge_rerank_wide.jsonl \
  --gold /tmp/longmemeval/longmemeval_s_cleaned.json \
  --reader gpt-4.1 --judge gpt-4o --reader-max-tokens 800 \
  --reader-prompt omega-tricks    # vs basic
```

비용 차이 ≈ 0 (system prompt token 200개 추가 × 500q ≈ 100k tokens
≈ \$0.02 on gpt-4o-mini).

## 실측 결과 — prompt는 reader 모델 의존적 ⚠

같은 retrievals + 같은 judge + 같은 reader, prompt만 변경:

### gpt-4o-mini (작은 모델)

| Category | basic | omega-tricks | Δ |
|---|---|---|---|
| **preference** | 60.0 | **70.0** | **+10.0** ⭐ |
| single-session-assistant | 94.6 | 96.4 | +1.8 |
| temporal | 37.6 | 39.1 | +1.5 |
| multi-session | 59.4 | 57.9 | -1.5 |
| single-session-user | 100 | 97.1 | -2.9 |
| **knowledge-update** | 74.4 | **66.7** | **-7.7** ⚠ |
| **Overall** | **65.6** | **64.8** | **-0.8** |

→ **mini reader는 4-rule prompt를 못 소화**. preference 가이드는 효과
있지만 current-state / temporal / grounding 룰 복잡도가 KU
처리에 방해. 작은 모델은 prompt 단순화 필요.

### gpt-4.1 (큰 모델, OMEGA-equivalent reader)

| Category | basic | omega-tricks | Δ |
|---|---|---|---|
| KU | 82.1 | 80.8 | -1.3 |
| **multi-session** | 72.9 | 68.4 | **-4.5** ⚠ |
| preference | 83.3 | 83.3 | 0 |
| temporal | 42.1 | 41.4 | -0.7 |
| user/assistant | ≈ saturated | ≈ saturated | ≈ |
| **Overall** | **72.6** | **71.0** | **-1.6** ⚠ |

→ **gpt-4.1조차 우리 omega-tricks 구현은 효과 없음**. multi-session
에서 -4.5pt 큼. 가능한 원인:
1. 우리 prompt가 OMEGA 원본과 다름 (OMEGA prompt 비공개)
2. OMEGA의 +5-10pt는 데이터셋-specific tuning (per-category prompt 등)
3. OMEGA의 reader prompt 효과는 다른 baseline 대비 (이미 강한 wg
   baseline 위에선 marginal 또는 negative)

## 권고 (2026-05-01 실측 갱신)

| Reader | 추천 prompt | 이유 |
|---|---|---|
| **gpt-4.1 / gpt-4o-class** | **basic** | 우리 omega-tricks 구현은 효과 없거나 negative. 진짜 효과 있는 prompt는 별도 R&D 필요. |
| **MiniMax-M2.7 등 reasoning** | basic | 측정 안 됨, 단 reasoning 모델은 default로 잘 함 |
| **gpt-4o-mini / Haiku** | basic (preference만 surface 시 +10pt) | mini는 4-rule 못 소화 |

**중요 결론**: OMEGA 격차의 대부분은 reader prompt가 아닌
**ingest-time LLM-aided + lifecycle**에 있음. 격차 메우려면 reader
영역 외 작업 필요. `wg ingest --llm` / hook auto-apply / async LLM
extract 등이 진짜 ROI.

## 후속 R&D 가능성

- **per-category prompt** — temporal 질문엔 temporal-only prompt,
  preference 질문엔 personalisation-only prompt. Pre-classifier 필요.
- **Few-shot examples** — basic prompt + 2-3 in-context exemplars
  카테고리별. 추가 토큰 비용 ~$0.01/질문.
- **Reader-router** — temporal → MiniMax (reasoning), preference
  → gpt-4.1 (instruction-following), 나머지 → basic. Routing
  overhead 1 LLM call/질문. 추정 +3-5pt overall.

## 한계

- **wg 슬림 페이로드는 `age_days` / `freshness_warning` 노출**하지만,
  `created_at` / `observed_at` 절대 시각은 노출 안 함 (토큰 절약).
  Temporal prompting 적용 시 fact_get으로 추가 fetch 필요할 수도.
- **personalisation 섹션은 wg_context 호출에서만 surface** —
  wg_search / wg_query는 자동 포함 X. agent가 wg_context를
  top-of-turn entry point로 일관 사용해야 효과 발휘.
- **Query augmentation은 사용자 자체 LLM 호출** — wg 영역 밖. agent
  가 wg_search 호출 전에 자체 rewrite 단계 추가 필요.

## 참고

- [OMEGA dev.to "How I Built"](https://dev.to/singularityjason/how-i-built-a-memory-system-that-scores-954-on-longmemeval-1-on-the-leaderboard-2md3) — 4 prompting tricks 원본
- [`docs/MEASUREMENTS.md`](../docs/MEASUREMENTS.md) — wg vs OMEGA 격차 분해
- [`docs/MEASUREMENTS.md`](../docs/MEASUREMENTS.md) — wg 측정 결과 + LongMemEval-S 데이터셋 noise 분석
