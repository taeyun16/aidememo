# wg vs beads — 비교 매트릭스

> 작성: 2026-04-28
> 대상: wg @ `714f41c`, beads @ `1.0.3` (huggingface? brew bottle)
> 목적: 실제 벤치마크에 들어가기 전에 두 시스템의 1:1 비교 가능한 축과
>       각자의 강점/약점, 그리고 fair-comparison 영역을 명확히 한다.

## TL;DR

같은 카테고리("agent용 영속 그래프 저장소")에 속하지만 풀고자 하는 문제가
다르다. **beads**는 의존성 인식 이슈 트래커 — "다음 작업이 뭐냐"를
답한다. **wg**는 마크다운 위키의 entity/fact 그래프 + hybrid retrieval —
"X에 대해 우리가 아는 게 뭐냐"를 답한다. 공정한 head-to-head는
(a) bulk write throughput, (b) graph traversal latency, (c) one-shot
agent context fetch (CLI cold-start 포함) — 이 세 축에 한정한다.
retrieval quality (P@K / nDCG)는 beads가 BM25/임베딩이 없어서 비교 불가.

## 1. 정체성

| 축 | wg | beads |
|---|---|---|
| 한 줄 정의 | "마크다운 위키의 구조화 인덱스 — 에이전트용 hybrid 검색 + 그래프 walk" | "에이전트용 분산 그래프 이슈 트래커 — markdown plan 파일을 대체" |
| 핵심 동사 | `query` (search + traverse + recent 한 번에) | `ready` (의존성 풀린 다음 작업) |
| 주 단위 | entity / fact / typed relation | issue / dependency-edge (type 다중) |
| 영속화 | redb (single-file, 1.85+ Rust) | Dolt (MySQL 호환, cell-level merge) |
| 분산성 | 단일 사용자 / 단일 라이터 | git push/pull로 여러 에이전트 병합 가능 |
| 라이선스 | MIT OR Apache-2.0 | MIT |
| 언어 | Rust | Go (CLI) + Python (MCP shim) |
| 최초 commit / 활동 | 2026 초 (current 26+ commits) | 2025 말, ~22k stars |

## 2. 데이터 모델

| 축 | wg | beads |
|---|---|---|
| 노드 타입 | `Entity` (name, type, aliases, summary, source_page, tags) + `Fact` (content, fact_type, entity_ids[], source_confidence, observed_at, superseded_at/by) | `Issue` (~50 필드, status/priority/issue_type/assignee/owner, JSON metadata, content_hash, ephemeral, wisp_type 등) |
| 엣지 타입 | `Relation { src, tgt, rel_type }` — 자유 typed | `dependencies(issue_id, depends_on_id, type, thread_id)` — 단일 테이블 멀티 의미 (`blocks/relates_to/duplicates/supersedes/replies_to/parent-child/discovered-from`) |
| 시간 모델 | validity window (`observed_at`, `superseded_at/by`) — **사실은 절대 삭제하지 않는다** | `closed_at` + `compaction_level` (오래된 closed issue를 요약으로 압축) |
| ID | ULID (26자) | 해시 기반 prefix (`bd-a1b2`) |
| 인덱스 | redb 키 + alias 인덱스 + BM25 inverted index + HNSW vector index | SQL B-tree 인덱스: status/priority/type/assignee/created_at/spec_id/external_ref |
| free-text 검색 | BM25 + cross-encoder rerank + 임베딩 (model2vec/e5-small/bge-reranker, TEI native) | SQL `LIKE` / ID prefix scan |
| 의미 유사도 | HNSW ANN over fact embeddings, optional 가중 통합 | 없음 |
| 그래프 walk | `traverse <entity> -d N`, `path <a> <b>`, `backlinks` | `dep`, `show <id>` (transitive blocker resolution), 사이클 탐지 |

## 3. 인터페이스

| 축 | wg | beads |
|---|---|---|
| CLI 서브커맨드 수 | ~30 (`search/query/recent/traverse/path/graph/entity *`/`fact *`/`relation *`/`bench/lint/doctor/ingest/watch/...`) | ~60 (`init/create/q/update/close/show/list/ready/blocked/stale/search/dep/label/comment/children/count/stats/prime/context/doctor/migrate/compact/backup/export/import/batch/branch/diff/audit/defer/cleanup/daemon/sync/...`) |
| JSON 출력 | 모든 read 커맨드 `--json` 플래그 | 거의 모든 read에 `--json` |
| MCP transport | **stdio + HTTP/SSE 둘 다** in-process Rust (`wg mcp`, `wg mcp-serve`) | Python FastMCP shim — 각 tool 호출이 `bd <subcommand>` subprocess fork |
| MCP 도구 수 | 13 | ~15 |
| 언어 바인딩 | Python / Node / Elixir / C — 동일 ~22 메서드, in-process | 없음 (CLI/MCP만) |
| 에이전트 onboarding | `wg skill check / install` (claude / hermes / openclaw / codex / cursor) | `claude-plugin/skills/beads/` 자체 plugin |

## 4. 성능 특성 (공시)

| 항목 | wg (`bench-*.md`) | beads (`BENCHMARKS.md`, M2 Pro) |
|---|---|---|
| Bulk insert 1k | (별도 측정 필요 — `wg fact_add_many` 단일 트랜잭션) | `BulkCreate1000Issues` ≈ 5s 추정 (per-create 2.5ms × 1k 미만, 트랜잭션 묶음) |
| Single create | `fact_add` ms 단위 (`store.durability=immediate` fsync 포함) | 2.5 ms / 8.9 KB heap |
| Search 10K | hybrid: 수십 ms (BM25 cache 후), 임베딩 포함 시 P@5 측정값은 `.notes/bench-miracl-ko.md` | SQL search (no filter): 12.5 ms / 6.3 MB |
| Graph traversal | depth-3 traverse 수 ms (redb 인덱스) | `GetReadyWork (10K)` 30 ms / 16.8 MB (transitive blocker resolution) |
| Cold start | 새 redb open: ms 단위 | embedded Dolt: 수십 ms (`BenchmarkColdStart`) |
| 검색 품질 | MIRACL/ko P@5, R@5, BM25 vs 임베딩 vs HNSW 차트 보유 | 없음 (latency/메모리만) |

## 5. 사용 시나리오

| 시나리오 | wg가 더 잘 맞음 | beads가 더 잘 맞음 |
|---|---|---|
| "Redis 운영 결정사항 모아 보여줘" | ✓ — fact 검색 + traverse | ✗ |
| "다음에 어떤 작업을 먼저 해야 하지?" | ✗ — 우선순위/의존성 모델 없음 | ✓ — `bd ready` |
| "팀 위키의 마크다운 변경을 자동 수집" | ✓ — `wg watch` + frontmatter | ✗ |
| "여러 에이전트가 같은 작업 보드를 분기 후 머지" | ✗ — 단일 라이터 | ✓ — Dolt cell-merge |
| "한국어 자연어 질의로 의미적 hit 받기" | ✓ — TEI/임베딩/리랭커 | ✗ |
| "오래된 작업의 메모리 압박 줄이기" | △ — supersede + 인덱스 prune | ✓ — `compact` 2-tier 요약 |
| "에이전트가 코드 리뷰 중 사실 1개 빠르게 추가" | ✓ — `wg fact add` (in-process MCP) | △ — `bd create` (Dolt 트랜잭션) |
| "결과를 dependency edge로 연결" | △ — typed relation으로 가능하지만 first-class scheduler 없음 | ✓ — built-in |

## 6. 비교 가능한 영역 (벤치 후보)

✅ **Bulk write throughput** — 10K 노드 + ~25K 엣지 삽입 wall time + 디스크 사용량.
- wg: `wg_fact_add_many` 또는 `wg ingest`로 동일 N건 fact + relation
- beads: `bd batch import < jsonl` 또는 `bd create` 루프
- 측정: 1) durability=immediate / 2) eventual

✅ **Graph traversal latency (depth-3, 10K corpus)** — 1000회 무작위 walk의 p50/p95.
- wg: `wg traverse <entity> -d 3 --json`
- beads: `bd show <id> --json` + `bd dep <id>` (transitive)

✅ **One-shot agent context fetch** — 새 프로세스 spawn + JSON 응답까지의 latency, 그리고 응답의 토큰 수.
- wg CLI: `wg query <topic> --json`
- wg MCP: stdio JSON-RPC 1회 (in-process)
- beads CLI: `bd prime <id> --json` 또는 `bd ready --json && bd show <top> --json`
- beads MCP: `beads-mcp` shim (Python → bd subprocess)

✅ **Cold start to first useful answer** — fresh DB → init → 첫 의미있는 응답까지.
- wg: `wg init` → `wg ingest` → `wg query`
- beads: `bd init` → `bd create` 루프 또는 `bd import` → `bd ready`

❌ **검색 품질 (P@K / nDCG)** — beads가 BM25/임베딩 없어 fair 비교 불가.
❌ **분산 머지 / 충돌 해소** — wg가 다중 라이터 구조 없음.
❌ **Markdown 위키 ingest 정확도** — beads가 마크다운 파서 없음.

## 7. 위험 / 주의사항

- **CLI cold-start 차이가 큼**: beads는 매 호출마다 dolt embedded engine을
  부팅; wg CLI는 redb open만 → wg가 자연스럽게 유리. **공정한 비교는
  MCP 경로** (stdio JSON-RPC 1회 = 1 process boot).
- **데이터 분포 동일성**: 노드 fan-out(평균 2–3), 텍스트 길이(64B 제목 +
  256B 설명), edge 비율(1.5/노드)을 양쪽에 동일하게 강제.
- **fsync 정책**: wg `store.durability` ↔ beads의 Dolt commit 모드 (auto
  vs manual). 둘 다 `immediate`로 통일하지 않으면 throughput이 한 자릿수
  배로 차이 남.
- **HW**: M2 Pro (beads 공시 환경) ↔ M-series 로컬 (wg 측정 환경) 일치
  시키지 않으면 micro-bench 비교 의미 약화.
- **버전 동결**: beads 1.0.3 brew bottle, wg @ 714f41c 시점.
