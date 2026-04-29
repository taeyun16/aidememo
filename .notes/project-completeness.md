# wg 프로젝트 완성도 평가

> 일시: 2026-04-29
> 시점: `caa1426` (CLI default flip + flaky 테스트 fix 직후)
> 방법: Explore agent + 정량 메트릭 (LOC/test/commit) + 세션 누적 측정

## 정량 스냅샷

| 지표 | 값 |
|---|---|
| Crate 수 | 7 (wg-core, wg-cli, wg-ffi, wg-napi, wg-nif, wg-python, wg-benchmarks) |
| 총 LOC (.rs, 테스트 포함) | 23 643 |
| 가장 큰 파일 3개 | `store.rs` 2475 · `search.rs` 1182 · `main.rs` 1141 |
| 워크스페이스 test | 170 passed / 5 ignored / 0 failed |
| Commit 수 (전체) | 113 |
| 최근 30일 commit | 113 (집중 active 개발 phase) |
| Bindings | Python · Node · Elixir · C (모두 ~22 메서드 풀 커버) |
| MCP tools | 13 (CLI + stdio + HTTP 동일 dispatch) |
| CI 잡 | 7 (lint·test×2·dhat·ffi·hermes·scenario-d) + opt-in e2e workflow |

## 영역별 점수

| 영역 | 점수 | 핵심 |
|---|---|---|
| 코드 구조 / 응집도 | ★★★★☆ | 19개 모듈 단방향 DAG, 순환 의존 없음 |
| Error handling | ★★★★★ | `WgError` 30+ variants, suggestions/path/context, `?` 체계적 |
| 운영성 (doctor/lint) | ★★★☆☆ | `wg doctor`/`wg lint` 강함, `--fix --shell` actionable. `tracing` 부재 — `eprintln!`만 |
| 테스트 커버리지 | ★★★★☆ | 170 + 5 ignored (HF model 의존 격리). store/search/graph/lint/ingest 단위 |
| PLAN.md / 진행률 | ★★★★☆ | 67KB 상세, Phase 1-2 완료 검증, TODO/FIXME 0 |
| 보안 / lint policy | ★★★★☆ | `unsafe_code = forbid`, `unwrap_used = deny`, `panic = deny`. `cargo audit` CI 부재 |
| 성능 (이번 세션 측정) | ★★★★★ | 10k bulk 339× faster than bd, daemon hot path 132× faster, disk 32× smaller |
| 문서 | ★★★★★ | AGENTS.md, README, wg-skill 4종, .notes/ 6+ |
| 통합 / CI | ★★★★★ | 7잡 ~6분 그린, concurrency cancel, scenario-d 회귀 |
| 다중 agent / binding | ★★★★★ | 4 binding 풀 커버 + claude/codex/hermes e2e A-E 5/5 통과 |

**종합 4.4 / 5** — Phase 2 완료, 프로덕션 진입 직전 단계.

## 강점

1. **성능 우위 검증** — 같은 카테고리 OSS(beads) 대비 10k에서 bulk 339×, search hot path 132× 빠름. backend 교체 없이 `redb + lazy + daemon` 패턴으로 달성.
2. **API 일관성** — CLI / MCP / 4 binding 같은 `SearchOpts.bm25_only`/`QueryOpts.bm25_only` 표면. 한 곳 변경이 모든 caller에 자동 적용.
3. **e2e 입증** — 시나리오 A/B/C/D/E + beads vs wg 5개 시나리오 모두 통과. multi-agent shared-store 패턴까지 수치 검증.

## 빈 곳 (gap, ROI 순)

| # | gap | 영향 | 추천 작업 |
|---|---|---|---|
| 1 | **`tracing` 없음** | 프로덕션 운영 시 debug 어려움 | `tracing` + `tracing-subscriber`, `WG_LOG=debug` env. ~1-2일 |
| 2 | **`cargo audit` CI 부재** | upstream CVE 모름 | ci.yml `cargo-audit` job. ~30분 |
| 3 | **Self-hosted runner 미등록** | 시나리오 C 자동화 X | 사용자 측 5단계 등록 |
| 4 | **opportunistic daemon discovery 없음** | manual CLI fast path 추가 매끄러움 가능 | `~/.wg/daemon.sock` 자동 탐지 (~200줄) |
| 5 | **PLAN vs 코드 sync 약함** | 미완 항목 inline 추적 안 됨 | `// TODO(phase3):` 마커 도입 |

## 페이즈별 위치

```
Phase 1 (entity/fact/relation 그래프)        ✅ 완료
Phase 2 (BM25 + lint + watch + ingest)       ✅ 완료
Phase 3 (MCP + 바인딩 + 멀티 에이전트)        ✅ 완료 (이번 세션)
Phase 4 (모델 증류 + adapt)                   🟡 부분 (semantic-adapt feature 존재, 미사용)
Phase 5 (S3 동기 + WAL compaction)            🟡 설계 (s3.rs, wal.rs 모듈 존재, e2e X)
```

## 결론

**프로덕션 진입 직전의 잘 정돈된 0.x 프로젝트** (semver 0.1.0, ~Phase 3 완료). 핵심 기능 + 성능 + 통합이 일관된 수준. 남은 작업은 운영 도구 (tracing, cargo audit) + Phase 4-5의 advanced features가 대부분.

가장 큰 ROI 작업: **`tracing` 도입** (운영성 ★★★ → ★★★★★). 1-2일.
