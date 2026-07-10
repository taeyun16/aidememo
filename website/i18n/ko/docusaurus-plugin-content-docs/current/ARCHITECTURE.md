---
title: 아키텍처
description: AideMemo의 CLI, MCP, 에이전트 SDK, 바인딩, 코어, 저장소, 검색 흐름을 시각적으로 설명합니다.
---

# 아키텍처

AideMemo는 여러 접근 표면을 가진 하나의 Rust 코어입니다. 동일한 타입 지정
팩트, 엔티티, 관계, 유효 기간, BM25 인덱스, semantic HNSW 사이드카, 아카이브
동작을 CLI, MCP 도구, Python 에이전트 SDK, 네이티브 바인딩에서 제공합니다.

## 시스템 지도

```mermaid
flowchart LR
  codex["코딩 에이전트<br/>Codex / Claude Code / Cursor / Hermes"]
  human["CLI 사용자"]
  scripts["스크립트와 테스트"]
  plugins["도구 개발자<br/>Python / Node / Elixir / C"]

  cli["aidememo-cli<br/>명령과 데몬"]
  mcp["MCP 전송<br/>stdio: aidememo mcp<br/>HTTP/SSE: aidememo mcp-serve"]
  sdk["aidememo-agent-sdk<br/>Memory.open / search_rows / remember"]
  bindings["네이티브 바인딩<br/>aidememo-python / aidememo-napi<br/>aidememo-nif / aidememo-ffi"]

  core["aidememo-core<br/>AideMemo API"]
  backend["StoreKind 디스패치<br/>SQLite 기본 / redb 선택"]
  sqlite[("SQLite hot 저장소<br/>entities / facts / relations")]
  redb[("redb hot 저장소<br/>선택형 Cargo 기능")]
  cold[("Cold tier<br/>*.cold.sqlite / *.cold.redb")]
  indexes["검색 사이드카<br/>BM25 + semantic HNSW"]

  human --> cli
  codex --> mcp
  codex --> sdk
  scripts --> sdk
  scripts --> cli
  plugins --> bindings
  cli --> core
  mcp --> core
  sdk --> bindings
  sdk --> cli
  bindings --> core
  core --> backend
  backend --> sqlite
  backend --> redb
  core --> cold
  core --> indexes
```

공개 디스패치 지점은 `aidememo-core`의 `AideMemo`입니다. 저장소 선택은
`StoreKind` 뒤에 집중되어 있습니다. SQLite / `libsqlite`가 기본 런타임
백엔드이며, `redb`는 선택형 Cargo 기능으로 빌드하고 설정 또는 CLI에서
요청할 때만 선택됩니다.

## 검색 흐름

```mermaid
flowchart TD
  request["사용자 또는 에이전트가 질문"]
  entry{"진입점"}
  search["aidememo_search<br/>aidememo search"]
  query["aidememo_query<br/>aidememo query"]
  context["aidememo_context"]
  filters["필터 적용<br/>source_id / current_only / include_archive / as_of"]
  bm25["BM25 어휘 검색"]
  hnsw["선택형 semantic HNSW"]
  rerank["선택형 TEI 재랭킹<br/>실패 시 비치명적 폴백"]
  graphCtx["선택형 그래프와 최근 컨텍스트"]
  result["순위 팩트와 컨텍스트 팩"]

  request --> entry
  entry --> search
  entry --> query
  entry --> context
  search --> filters
  query --> filters
  context --> filters
  filters --> bm25
  filters --> hnsw
  bm25 --> rerank
  hnsw --> rerank
  rerank --> graphCtx
  graphCtx --> result
```

직접 순위 결과에는 `search`, 집중된 컨텍스트 팩에는 `query`, 넓은 턴 시작
범위에는 `context`를 사용합니다. CLI 기본값은 auto-hybrid 정책입니다.
BM25 증거가 충분하면 어휘 경로를 유지하고, semantic 경로가 준비된 상태에서
약한 쿼리 또는 CJK 쿼리를 semantic 검색으로 승격합니다. `--hybrid`는 모든
쿼리에 semantic 순위를 강제합니다. MCP 호출자는 결정적인 저지연 동작이
필요하면 `bm25_only:true`를 전달할 수 있습니다.

## 쓰기와 생명주기 흐름

```mermaid
flowchart TD
  fact["fact add / fact_add_many<br/>타입 지정 콘텐츠 + 엔티티"]
  classify["에이전트가 fact_type 분류<br/>decision / lesson / error / preference / note"]
  entities["엔티티 자동 생성 또는 확인"]
  hot["Hot 저장소 쓰기<br/>facts + entity links + relations"]
  session["선택형 세션 연결<br/>AIDEMEMO_SESSION_ID 또는 session_id"]
  pin["고정 컨텍스트"]
  supersede["유효 기간<br/>이전 팩트 supersede"]
  archive["Cold-tier 아카이브<br/>FactId 보존"]
  consolidate["Consolidate / GAC / TTL"]
  rebuild["vector-rebuild --current-only"]
  read["이후 search / query / context"]

  fact --> classify --> entities --> hot
  hot --> session
  hot --> pin
  hot --> supersede
  hot --> archive
  hot --> consolidate
  consolidate --> rebuild
  pin --> read
  supersede --> read
  archive --> read
  rebuild --> read
```

팩트는 의도적으로 명시적입니다. 일반 에이전트 루프에서 호출하는 에이전트가
이미 더 강한 모델을 가지고 있고 쓰기 전에 오래 유지할 팩트를 분류해야 하므로
AideMemo에 내장 호스팅 extractor가 필요하지 않습니다. `extract`와 `pending`
명령은 선택형 캡처 및 리뷰 워크플로를 위해 제공됩니다.

## 클라우드와 브랜치 로그 흐름

```mermaid
sequenceDiagram
  participant C as Coordinator 저장소
  participant B as Baseline 백업
  participant A as Agent candidate 저장소
  participant L as Branch 로그

  C->>B: backup create
  B->>A: backup restore --force
  A->>A: 실험 중 로컬 fact 쓰기
  A->>L: branch push --base backup
  C->>L: 선택한 branch 검사
  L->>C: branch merge --branch winner
```

브랜치 로그는 클라우드 에이전트와 추측성 메모리 실험을 위한 append-only
아티팩트입니다. 완전한 multi-master 충돌 해결은 아닙니다. `sync_import`로
중복 레코드를 건너뛰고 독립 팩트를 추가하며, 경쟁하는 결정 사이의 semantic
충돌은 애플리케이션 정책으로 남깁니다.

## 소스 지도

| 시스템 영역 | 주요 구현 | 공개 문서 |
|---|---|---|
| CLI 명령과 파서 | `crates/aidememo-cli/src/cmd/mod.rs`, `crates/aidememo-cli/src/main.rs` | [`CLI 사용법`](CLI.md), [`기능 목록`](FEATURES.md) |
| MCP 도구와 스키마 | `crates/aidememo-cli/src/cmd/mcp_tools.rs` | [`MCP 설정`](MCP.md), [`에이전트 워크플로`](AGENT_WORKFLOWS.md) |
| 코어 API와 검색 | `crates/aidememo-core/src/lib.rs`, `search.rs`, `graph.rs` | [`아키텍처`](ARCHITECTURE.md), [`운영`](OPERATIONS.md) |
| 저장소 디스패치 | `crates/aidememo-core/src/backend.rs`, `sqlite_store.rs`, `store.rs` | [`운영`](OPERATIONS.md), [`기능 목록`](FEATURES.md) |
| Python 에이전트 SDK | `packages/aidememo-agent-sdk/src/aidememo_agent/sdk.py` | [`Python SDK`](SDK.md), [`에이전트 워크플로`](AGENT_WORKFLOWS.md) |
| 네이티브 바인딩 | `crates/aidememo-python`, `crates/aidememo-napi`, `crates/aidememo-nif`, `crates/aidememo-ffi` | [`Python SDK`](SDK.md), 패키지 README |
| 검증과 릴리스 게이트 | `scripts/changelog-release-check.py`, `scripts/registry-readiness-check.py`, `scripts/cargo-package-readiness.sh`, `scripts/docs-feature-gate.py`, `scripts/docs-i18n-status.py`, `scripts/docs-site-e2e.py`, `scripts/*smoke*.sh`, `scripts/ci-local.sh` | [`측정 원장`](MEASUREMENTS.md), [`릴리스 체크리스트`](RELEASE.md) |

## 문서 계약

문서 검증은 두 계층으로 구성됩니다.

`scripts/docs-feature-gate.py`는 소스 수준 공개 문서 드리프트 게이트이며 다음을
확인합니다.

- `aidememo --help`에 표시되는 모든 최상위 CLI 명령과 하위 명령이
  [`기능 목록`](FEATURES.md)에 포함되어 있는지 확인합니다.
- `cmd/mcp_tools.rs::list_tools()`의 모든 MCP 도구가
  [`기능 목록`](FEATURES.md)에 포함되어 있는지 확인합니다.
- MCP 도구 수, CLI 명령 수, 아키텍처 다이어그램 수, AGENTS 핵심 도구 수와
  같은 공개 수치가 구현에서 계산한 값과 일치하는지 확인합니다.
- 이 페이지, [`에이전트 워크플로`](AGENT_WORKFLOWS.md),
  [`측정 원장`](MEASUREMENTS.md) 같은 핵심 설명 문서가 Docusaurus에
  노출되는지 확인합니다.
- Mermaid가 활성화되어 시스템 다이어그램이 코드가 아닌 다이어그램으로
  렌더되는지 확인합니다.
- 한국어 번역 범위와 원문 fingerprint가 공개 영어 문서와 일치하고, 의도한
  영어 폴백이 명시적으로 기록되어 있는지 확인합니다.
- 공개 문구가 SQLite를 기본 백엔드, redb를 선택형 Cargo 기능 백엔드로
  유지하는지 확인합니다.

`scripts/docs-site-e2e.py`는 렌더된 사이트 게이트입니다. Docusaurus를
빌드하고 영어와 한국어 sitemap, sidebar, homepage card, 페이지 H1,
baseUrl 범위 링크, 정적 자산, anchor, 아키텍처 문서의 구현 경로가 현재
저장소와 일치하는지 확인합니다.

문서를 배포하기 전에 다음 명령을 실행합니다.

```bash
python3 scripts/docs-feature-gate.py
python3 scripts/docs-site-e2e.py
```
