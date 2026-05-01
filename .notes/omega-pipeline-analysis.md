# OMEGA 파이프라인 분석 + wg 적용 가능성

> **결론 한 줄**: OMEGA의 95.4%는 retrieval 알고리즘이 아니라
> **memory lifecycle management** (write-time 정제, 시간 흐름 따라
> consolidation, dedup, hook-based auto-capture)의 결과. wg가 이미
> 갖춘 부분이 절반, 적용 ROI 높은 부분이 4-5개.

조사 출처:
- [omega-memory GitHub README](https://github.com/omega-memory/omega-memory)
- [How I Built — dev.to (저자 directly)](https://dev.to/singularityjason/how-i-built-a-memory-system-that-scores-954-on-longmemeval-1-on-the-leaderboard-2md3)
- [omegamax.co/benchmarks](https://omegamax.co/benchmarks)

## OMEGA 시스템 구조 (확정 사항)

### 같은 retrieval base
- 임베딩: bge-small-en-v1.5 (384-dim, ONNX) — **wg와 동일**
- 스토리지: SQLite + sqlite-vec — wg는 redb
- FTS: sqlite FTS5 — wg는 BM25

### 6-stage retrieval pipeline
1. Vector similarity (cosine via sqlite-vec)
2. FTS5 keyword
3. **Type-weighted scoring (decision/lesson 2×)**
4. Contextual re-rank (tag / project / content overlap)
5. **Time-decay (0.35 floor, preference/error 면제)**
6. Dedup at retrieval

### Memory lifecycle / forgetting (5 메커니즘)
1. **Exact dedup** — SHA-256 hash
2. **Semantic dedup** — embedding similarity ≥ 0.85 → 새 fact 안 만들고 기존 "evolve"
3. **TTL** — session summary 1일, lesson/preference 영구
4. **Compaction** — Jaccard 클러스터링, 원본 superseded
5. **Conflict detection** — newer decision 자동 resolve, lesson은 review 플래그

### Hook-based auto-capture (Claude Code)
- `SessionStart` → 관련 메모리 surface
- `PostToolUse` → 편집 파일 관련 메모리
- `UserPromptSubmit` → 대화 분류 + decisions/lessons/errors 추출
- `Stop` → session summary 생성

### 25 tool catalog (요약)
- Storage / retrieval (3): store / query / delete
- Session (4): welcome / clear_session / **checkpoint / resume_task**
- Analysis (8): lessons / similar / timeline / traverse / type_stats / session_stats / **weekly_digest** / profile
- Maintenance (4): **consolidate / compact** / backup / health
- Metadata (6): edit / preferences / feedback / **remind**(×3)

### 95.4% 카테고리별
| Category | OMEGA | wg (gpt-4o) | Δ |
|---|---|---|---|
| Information Extraction | **100%** | 80.8% (KU) | +19 |
| Multi-Session | 83.5% | 61.7% | +22 |
| Temporal | **94.0%** | 39.8% | **+54** |
| Knowledge Update | 96.2% | 80.8% | +15 |
| Preference | **98.6%** | 63.3% | **+35** |
| **Overall** | 95.4% | 67.6% | +27.8 |

→ 가장 큰 격차는 **temporal (+54)** + **preference (+35)**. 둘 다 OMEGA가
prompting trick 적용 카테고리.

## wg 현재 상태 vs OMEGA — 항목별 매트릭스

| OMEGA 메커니즘 | wg 상태 | 갭 |
|---|---|---|
| Vector + FTS hybrid | ✅ BM25 + HNSW + RRF | 동등 |
| Cross-encoder rerank | ✅ bge-reranker (wide K=20→10) | wg 우위 (OMEGA는 contextual rerank만) |
| **Type-weighted scoring** | ✅ `search.fact_type_weights` (commit fd5dcbe) | 동등 |
| **Time-decay + exemption** | ✅ `decay_exempt_types` (commit 24bd7bd) | wg에 0.35 floor 없음 (작은 차이) |
| **Exact dedup (SHA-256)** | ❌ `existing_similar` 힌트만 | **추가 가능** (low cost) |
| **Semantic dedup (0.85 evolve)** | ❌ 자동화 안 됨 | **추가 가능** (medium cost) |
| **Conflict detection (auto-supersede)** | ❌ `wg_fact_supersede` 수동 | **추가 가능** (low cost) |
| **TTL by fact_type** | ❌ 영구 저장만 | **추가 가능** (low cost) |
| **Compaction (cluster + summary)** | ❌ | 추가 가능 (high cost — LLM 필요) |
| **Hook-based auto-capture** | ❌ | **추가 가능** (wg-skill에) |
| **Access count tracking** | 🟡 `last_accessed_at` 있음, count 없음 | 작은 추가 |
| **Lessons (cross-session, by access)** | 🟡 `wg_pinned_context`로 부분 커버 | 추가 가능 |
| **Context virtualization (checkpoint)** | ❌ | 추가 가능 (medium cost) |
| **Weekly digest** | 🟡 `wg_overview` 비슷 | 시간 윈도우 추가 |
| **Reminders** | ❌ | 추가 가능 (low) |
| **UDS daemon (warm MCP)** | 🟡 HTTP daemon (`wg daemon`) | 우선순위 낮음 |

## 우선순위 — ROI / 비용 매트릭스

### Tier 1: 즉시 적용 (high ROI, low cost, < 100 LOC)

**1. Conflict detection — same entity + decision/convention 자동 supersede**
   - `wg_fact_add` 시: 새 fact가 decision/convention 타입이면, 같은 entity에 붙은 동일 type 기존 fact를 자동으로 superseded 마킹.
   - 기존 `existing_similar` 인프라 재사용.
   - 추정 효과: **knowledge-update +5pt** (newer decision이 retrieval에 노출됨), preference +2pt.
   - LOC: ~50

**2. Exact dedup at write (SHA-256)**
   - `wg_fact_add`에서 content hash가 이미 존재하면 reject + 기존 ID 반환.
   - 중복 fact 노이즈 제거.
   - 추정 효과: retrieval noise 감소, indirectly +1-2pt.
   - LOC: ~30

**3. Semantic dedup with evolution (0.85 threshold)**
   - 새 fact의 임베딩이 기존 fact와 cosine ≥ 0.85면, 새 fact 만드는 대신 기존을 supersede + 새 content로 update.
   - "Evolution" 패턴 — 메모리 누적 방지.
   - 추정 효과: **multi-session +8-10pt** (동일 정보 반복 안 쌓임), preference +5pt.
   - LOC: ~80 (embed 비교 + supersede 통합)

**Tier 1 합계 추정: +14-19pt → wg 67.6 → 81-86%**, Mastra(84%) / OMEGA 영역 진입.

### Tier 2: 중간 ROI / 중간 비용 (~200-400 LOC)

**4. Hook-based auto-capture (wg-skill에 Claude Code hooks)**
   - `SessionStart` → `wg_pinned_context` 자동 호출
   - `PostToolUse` → 편집한 파일 관련 fact 자동 surface
   - `UserPromptSubmit` → `wg_extract --llm` 자동 실행
   - `Stop` → session summary 생성 (LLM)
   - 가장 큰 가치는 **사용자 마찰 제거** — 메모리가 자동으로 쌓임.
   - LOC: 거의 wg-skill만 (Rust 변경 거의 없음). 훅 스크립트 작성.

**5. TTL by fact_type**
   - `note` / `question` 같은 ephemeral type에 30일 TTL. `decision` / `convention` / `pattern` 영구.
   - `wg lint` 또는 `wg consolidate` 명령에서 만료 처리.
   - 추정 효과: noise 감소, +2-3pt.
   - LOC: ~100

**6. Compaction (clustering + LLM summary)**
   - 시간 지난 비슷한 fact들을 N→1로 합치고 원본 supersede.
   - LLM extract 기반 (이미 wg에 인프라 있음 — `extract.provider`).
   - 추정 효과: long-running wiki에서 retrieval 정확도 +3-5pt (시간 갈수록 효과 큼).
   - LOC: ~250

### Tier 3: 보류 또는 후속 (~500+ LOC)

**7. Context virtualization (checkpoint/resume)**
   - 작업 상태 + agent context 저장
   - LongMemEval ROI 적음 (단일 turn 평가). agent UX는 큼.

**8. Reminders / weekly digest**
   - LongMemEval과 무관. dogfood UX 가치만.

**9. UDS daemon**
   - 이미 HTTP daemon 있음. UDS는 marginal latency 개선.

## OMEGA의 prompting tricks (LongMemEval-specific)

OMEGA가 93.2% → 95.4%로 끌어올린 4 가지 (저자 글):
- 더 나은 **temporal prompting** (+5 questions)
- **Knowledge-update current-state prompting** (+4)
- **Query augmentation** (+2)
- **Preference personalization** (+2)

이건 reader-side prompt engineering. wg는 reader 영역 밖이라 직접 적용
어렵지만, **reader prompt 가이드**를 wg-skill에 추가하면 wg 사용자가
같은 효과 볼 수 있음.

## 추천 액션 순서

**Phase 1 (Tier 1 — 1-2일, high ROI)**:
1. Exact dedup (SHA-256) — 30 LOC
2. Conflict detection (decision/convention auto-supersede) — 50 LOC
3. Semantic dedup with evolution — 80 LOC
4. → 측정: LongMemEval E2E 재실행, 67.6 → 81-86 예상

**Phase 2 (Tier 2 — Phase 1 결과 보고 결정, 1주일)**:
5. Hook-based auto-capture (wg-skill, 코드 거의 없음)
6. TTL by fact_type
7. Compaction (선택적)

**Phase 3 (Tier 3 — 별도 가치 평가)**:
8. Context virtualization
9. Reminders / digest

## 측정 계획

각 Tier 적용 후:
1. `cargo test` — 회귀 없음
2. fixture 위키에서 동작 검증
3. LongMemEval E2E 재측정 (gpt-4o reader, ~$5)
4. 카테고리별 차이 분석 — knowledge-update / preference / multi-session에 집중
