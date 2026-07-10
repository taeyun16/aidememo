---
title: 기능 목록
description: 문서 기능 게이트가 추적하는 AideMemo의 전체 기능 표면입니다.
---

# 기능 목록

이 페이지는 사용자가 볼 수 있는 AideMemo 기능 표면의 공개 체크리스트입니다.
빠른 시작 페이지보다 의도적으로 더 넓습니다. 모든 최상위 CLI 명령과 MCP
도구가 여기에 있어야 하므로, 릴리스 변경에서 문서를 수정하지 않은 채 기능을
추가·제거·변경할 수 없습니다.

다음 명령으로 게이트를 실행합니다.

```bash
python3 scripts/docs-feature-gate.py
python3 scripts/docs-site-e2e.py
```

## CLI 명령

| 명령 | 기능 범위 |
|---|---|
| `aidememo entity` | 엔티티 추가, 조회, 목록, 이름 변경, alias, 삭제, 설명, 표시 |
| `aidememo fact` | 팩트 추가, 조회, 목록, 삭제, pin, unpin, supersede, archive, 검사 |
| `aidememo traverse` | 한 엔티티에서 바깥 방향으로 그래프 탐색 |
| `aidememo path` | 두 엔티티 사이의 최단 그래프 경로 검색 |
| `aidememo search` | BM25, 선택형 semantic 검색, 필터, 프로젝트 fanout으로 팩트 검색 |
| `aidememo query` | 검색, 그래프, 최근 팩트, 결과 shaping을 포함한 주제 컨텍스트 팩 조회 |
| `aidememo lint` | 원시 그래프 상태 검사 실행 |
| `aidememo doctor` | 사용자 중심 상태 검사와 공유 저장소 안내 실행 |
| `aidememo recent` | 최근 추가되거나 변경된 팩트 표시 |
| `aidememo edit` | append, prepend, replace, 전체 콘텐츠 편집으로 팩트 수정 |
| `aidememo graph` | 엔티티 그래프를 Mermaid 또는 DOT으로 렌더링 |
| `aidememo project` | 이름 있는 프로젝트와 저장소 경로 관리 |
| `aidememo bench` | golden JSONL 세트에 대해 검색 품질 벤치마크 |
| `aidememo skill` | 에이전트 skill 파일 검증 또는 설치 |
| `aidememo backup` | manifest 검증을 포함한 SQLite snapshot 백업 생성 또는 복원 |
| `aidememo branch` | 클라우드 에이전트와 추측성 실험을 위한 append-only 메모리 브랜치 로그 push 또는 merge |
| `aidememo export` | 엔티티, 관계, 팩트를 JSONL로 내보내기 |
| `aidememo import` | JSONL 데이터 가져오기 |
| `aidememo stats` | 저장소 통계 표시 |
| `aidememo ingest` | Markdown 파일을 저장소로 ingest |
| `aidememo sync` | 로컬 Markdown을 증분 ingest하거나 MCP 서버에서 원격 delta 가져오기 |
| `aidememo config` | 로컬 설정 조회와 변경 |
| `aidememo model` | 로컬 임베딩 모델 캐시 상태 확인과 관리 |
| `aidememo feedback` | 순위 조정을 위한 검색 결과 feedback 기록 |
| `aidememo adapt` | 순위 adapter 학습, 상태 확인, 평가 |
| `aidememo init` | AideMemo 저장소 생성과 선택형 wiki ingest 또는 에이전트 등록 |
| `aidememo watch` | Markdown 파일 변경 감시와 재-ingest |
| `aidememo mcp-serve` | 공유 웜 접근을 위한 HTTP와 SSE MCP 제공 |
| `aidememo mcp` | 로컬 에이전트를 위한 stdio MCP 제공 |
| `aidememo mcp-install` | 지원하는 에이전트에 AideMemo MCP 등록 |
| `aidememo completions` | 셸 completion 스크립트 출력 |
| `aidememo pending` | dry-run으로 추출된 팩트 검토, 승인, 거절 |
| `aidememo vector-rebuild` | 모델 또는 인덱스 변경 뒤 HNSW vector sidecar 재구축 |
| `aidememo daemon` | 장기 실행 background `mcp-serve` 프로세스 관리 |
| `aidememo extract` | 텍스트에서 candidate 팩트 추출, 선택적으로 설정된 LLM provider 사용 |
| `aidememo session` | 추적 에이전트 세션 생성, 확인, 워밍 |
| `aidememo workflow` | 추적 컨텍스트로 이슈, PR, 자동화 워크플로 시작 |
| `aidememo profile` | 현재 타입 지정 팩트에서 읽기 전용 프로젝트 프로필 아티팩트 생성 |
| `aidememo auto-relate` | semantic 유사도에서 관련 엔티티 간선 탐색 |
| `aidememo overview` | 익숙하지 않은 저장소의 첫 인상 snapshot 생성 |
| `aidememo consolidate` | 생명주기 관리를 위해 팩트 중복 제거, 만료, GAC clustering |
| `aidememo auth` | HTTP MCP bearer-token credential 생성, 저장, 목록, 삭제 |

## CLI 하위 명령

| 영역 | 하위 명령 |
|---|---|
| 엔티티 관리 | `aidememo entity add`, `aidememo entity get`, `aidememo entity list`, `aidememo entity rename`, `aidememo entity alias`, `aidememo entity delete`, `aidememo entity describe`, `aidememo entity show` |
| 팩트 관리 | `aidememo fact add`, `aidememo fact get`, `aidememo fact list`, `aidememo fact delete`, `aidememo fact feedback`, `aidememo fact supersede`, `aidememo fact pin`, `aidememo fact unpin`, `aidememo fact pinned`, `aidememo fact archive` |
| 팩트 편집 | `aidememo edit fact` |
| 프로젝트 관리 | `aidememo project list`, `aidememo project show`, `aidememo project create`, `aidememo project use`, `aidememo project remove` |
| 에이전트 skill | `aidememo skill check`, `aidememo skill install` |
| 백업과 복원 | `aidememo backup create`, `aidememo backup restore` |
| 브랜치 로그 | `aidememo branch push`, `aidememo branch merge` |
| 동기화 | `aidememo sync ingest`, `aidememo sync pull`, `aidememo sync status` |
| 설정 | `aidememo config list`, `aidememo config get`, `aidememo config set` |
| 모델 캐시 | `aidememo model list`, `aidememo model status`, `aidememo model download` |
| 순위 adapter | `aidememo adapt train`, `aidememo adapt status`, `aidememo adapt eval` |
| 대기 팩트 검토 | `aidememo pending review`, `aidememo pending list`, `aidememo pending approve`, `aidememo pending reject`, `aidememo pending stats` |
| 데몬 | `aidememo daemon start`, `aidememo daemon stop`, `aidememo daemon status` |
| 세션 | `aidememo session start`, `aidememo session new`, `aidememo session current`, `aidememo session list`, `aidememo session canvas` |
| 워크플로 | `aidememo workflow start` |
| 프로필 아티팩트 | `aidememo profile export` |
| 인증 | `aidememo auth generate`, `aidememo auth login`, `aidememo auth logout`, `aidememo auth list` |

## MCP 도구

| 도구 | 기능 범위 |
|---|---|
| `aidememo_search` | 필터, formatting control, feedback session ID, 선택형 archive 검색으로 팩트 검색 |
| `aidememo_feedback` | 이전 검색 결과에 helpful 또는 not-helpful feedback 기록 |
| `aidememo_session_start` | pinned 팩트, 최근 팩트, 상위 엔티티, lint hint를 포함한 세션 warmup envelope 반환 |
| `aidememo_pinned_context` | 항상 로드되는 pinned 팩트 tier 반환 |
| `aidememo_fact_pin` | 팩트 pin 또는 unpin |
| `aidememo_extract` | 원시 텍스트에서 candidate 팩트를 추출하고 선택적으로 저장 |
| `aidememo_path` | 두 엔티티 사이의 최단 경로 검색 |
| `aidememo_fact_list` | pagination과 필터로 팩트 목록 반환 |
| `aidememo_entity_get` | 이름 또는 alias로 엔티티 하나 조회 |
| `aidememo_fact_get` | ID로 팩트 하나 조회 |
| `aidememo_entity_list` | 타입과 pagination 필터로 엔티티 목록 반환 |
| `aidememo_traverse` | 정방향 또는 역방향으로 그래프 이웃 탐색 |
| `aidememo_aggregate` | 팩트를 결정적으로 count, enumerate, group, sum, timeline 처리 |
| `aidememo_doctor` | 상태, lint, 공유 저장소 진단 반환 |
| `aidememo_overview` | 익숙하지 않은 wiki의 orientation snapshot 반환 |
| `aidememo_recent` | 최근 팩트 반환 |
| `aidememo_context` | 넓은 턴 시작 컨텍스트 envelope 반환 |
| `aidememo_workflow_start` | 추적되는 이슈, PR, 티켓, 자동화 워크플로 시작 |
| `aidememo_session_canvas` | 긴 워크플로 재개를 위한 범위 제한 Markdown + Mermaid canvas 반환 |
| `aidememo_profile_export` | 현재 타입 지정 팩트에서 읽기 전용 프로젝트 프로필 텍스트 아티팩트 반환 |
| `aidememo_query` | 집중된 주제 컨텍스트 팩 반환 |
| `aidememo_entity_describe` | 엔티티 요약 설정 또는 삭제 |
| `aidememo_fact_add` | 자체 분류 타입과 선택형 session/source 범위로 팩트 하나 추가 |
| `aidememo_fact_add_many` | 한 transaction에 여러 팩트 추가 |
| `aidememo_fact_supersede` | 이전 팩트를 replacement 팩트로 교체하고 retire |
| `aidememo_fact_archive` | 팩트를 cold-tier archive로 이동 |
| `aidememo_fact_edit` | 팩트 콘텐츠를 제자리에서 편집 |

## SDK와 바인딩

| 표면 | 기능 범위 |
|---|---|
| `aidememo-agent-sdk` | 코드 실행 에이전트를 위한 Python composition 계층, `session_canvas()`와 `project_profile()` 아티팩트 helper 포함 |
| `aidememo-python` | Python용 PyO3 네이티브 바인딩 |
| `aidememo-napi` | Node.js 네이티브 바인딩 |
| `aidememo-nif` | Elixir/Erlang NIF 바인딩 |
| `aidememo-ffi` | C ABI 바인딩 |
| `hermes-aidememo` | Hermes Agent plugin, slash command, lifecycle hook, SDK re-export, 선택형 pending-first 캡처 adapter |

네이티브 Python, Node, Elixir, C 바인딩은 CLI와 같은 백엔드 선택자를
사용합니다. 기본 빌드는 로컬 SQLite 백엔드를 포함하며 redb 저장소를 열어야
할 때는 Cargo `redb`로 빌드합니다. Python composition SDK는
`Memory.open(storage_backend=...)`으로 같은 값을 제공하고
`aidememo-python` fast path와 CLI 폴백 모두에 전달합니다.

## 게이트 계약

`scripts/docs-feature-gate.py`는 다음 소스 수준 드리프트 검사를 강제합니다.

1. `aidememo --help`의 모든 명령이 이 페이지에 `` `aidememo <command>` ``
   형식으로 포함되어야 합니다.
2. `cmd/mcp_tools.rs::list_tools()`의 모든 MCP 도구가 backtick으로 감싼
   이름으로 이 페이지에 포함되어야 합니다.
3. MCP 도구 수, CLI 명령 수, 아키텍처 다이어그램 수, AGENTS 핵심 도구 수와
   같은 공개 수치가 구현에서 계산한 값과 일치해야 합니다.
4. Docusaurus 사이드바가 이 페이지를 포함하고 공개 문서와 소스 문자열에
   알려진 오래된 소문자 제품 표기가 없어야 합니다.
5. 공개 저장소 설명은 SQLite를 기본 백엔드, redb를 선택형 Cargo 기능
   백엔드로 유지해야 합니다.
6. 온보딩 문서는 설치기, 체크아웃 경로, 결정적 `query --bm25-only` 빠른
   시작을 CLI와 일치시켜야 합니다.
7. 기여자, 보안, 이슈, PR 템플릿이 구현된 CLI 표면과 일치해야 합니다.
8. 한국어 locale의 홈페이지 메시지, 번역 문서 범위, 원문 fingerprint,
   명시적 영어 폴백이 공개 사이드바와 동기화되어야 합니다.
9. 영문과 한국어 루트 README는 서로를 가리키는 언어 링크와 설치 명령을
   유지해야 하며, `COMPARE.md`도 수치, 문구, 저장소 배치 드리프트 검사에
   포함되어야 합니다.

게이트는 기본적으로 내부 count-claim self-test도 실행합니다. 드리프트 감지기가
오래된 수치 주장을 거부하는지만 확인하려면
`python3 scripts/docs-feature-gate.py --self-test`를 사용합니다.

`scripts/docs-site-e2e.py`는 렌더된 Docusaurus 사이트를 빌드하고 영어와
한국어 route graph가 sidebar/homepage 계약과 일치하는지, baseUrl 범위의
link/asset/anchor가 해석되는지, locale별 페이지 H1과 `html lang` /
`hreflang` metadata가 올바른지, 아키텍처 문서의 구현 경로가 저장소에
존재하는지 확인합니다.

게이트가 문장의 의미적 완벽성을 증명할 수는 없습니다. 대신 기능과 구조
드리프트를 눈에 띄게 만듭니다. 문서를 갱신하지 않은 CLI/MCP 기능 변경,
배포된 `/aidememo/` route graph 손상, 저장소 기본값 설명의 회귀는 CI를
실패시킵니다.
