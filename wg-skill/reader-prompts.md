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

`scripts/longmemeval_e2e.py`의 `READER_SYSTEM` 상수가 reader 시스템
prompt. 위 합쳐진 prompt로 교체하면 같은 retrievals로 +Npt 측정
가능. 비용 X (reader call 한 번당 system prompt token 추가).

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
- [`.notes/compare-omega.md`](../.notes/compare-omega.md) — wg vs OMEGA 격차 분해
- [`.notes/bench-longmemeval.md`](../.notes/bench-longmemeval.md) — wg 측정 결과 + LongMemEval-S 데이터셋 noise 분석
