---
title: 여러 Codex 프로필에서 메모리 공유
description: 로그인 상태나 채팅 기록을 공유하지 않고 격리된 Codex 계정이 명시적인 프로젝트 메모리를 공유합니다.
---

# 여러 Codex 프로필에서 메모리 공유

Codex 프로필은 같은 저장소에서 작업하면서도 계정, 인증 상태, 작업 기록을 서로
분리할 수 있습니다. AideMemo는 프로필 경계를 넘어 유지되어야 하는 프로젝트 지식을
연결합니다.

> 두 Codex 계정, 하나의 프로젝트 메모리. 계정은 바꿔도 컨텍스트는 이어집니다.

이 기능은 프로젝트 메모리 연속성이지 세션 동기화가 아닙니다. AideMemo는 Codex의
쿠키, 자격 증명, 채팅 transcript 또는 계정 상태를 복사하지 않습니다.

## 경계를 올바르게 모델링하기

프로젝트에는 하나의 공유 저장소와 하나의 `source_id`를 사용합니다. 각 Codex
프로필에는 provenance를 위한 서로 다른 `actor_id`를 지정합니다.

```text
Codex 계정 A ─┐
              ├─ AideMemo MCP ─ shared.sqlite
Codex 계정 B ─┘

source_id = project:aidememo
actor_id  = codex:account-a | codex:account-b
```

`source_id`는 검색 네임스페이스입니다. 계정 A와 B가 서로 다른 `source_id`를
사용하면 기본 조회도 분리됩니다. `actor_id`는 이 사실을 어떤 프로필이 기록했는지
나타냅니다.

## 두 Codex 프로필에 설치하기

명시적인 저장소를 선택합니다. 설치기는 각 MCP 설정에 절대 경로를 기록하므로 이후
Codex가 어느 디렉터리에서 서버를 실행하더라도 같은 저장소를 사용합니다.

```bash
STORE="$(pwd)/_meta/wiki.sqlite"

aidememo config set store.lock_retry_ms 5000

aidememo --backend libsqlite --store "$STORE" mcp-install \
  --target codex \
  --codex-home "$HOME/.codex-account-a" \
  --actor-id codex:account-a \
  --codex-home "$HOME/.codex-account-b" \
  --actor-id codex:account-b \
  --source-id project:aidememo
```

`--codex-home`을 생략하면 활성 `CODEX_HOME`을 사용하고, 설정되지 않았다면
`~/.codex`를 사용합니다. 반복한 `--codex-home`과 같은 순서로 `--actor-id`를
반복합니다. 의도적으로 같은 작성자 ID를 사용할 때는 `--actor-id` 하나를 모든
프로필에 적용할 수도 있습니다.

각 프로필을 검증합니다.

```bash
CODEX_HOME="$HOME/.codex-account-a" codex mcp list
CODEX_HOME="$HOME/.codex-account-b" codex mcp list

aidememo --store "$STORE" doctor
```

활성 Codex 프로필이 다른 저장소를 가리키거나 저장소 경로가 고정되지 않았다면
`aidememo doctor`가 이를 보고합니다.

## 계정 간 작업 인계

계정 A가 지속할 결정을 기록합니다.

```json
{
  "content": "Decision: use SQLite WAL for the shared local memory store.",
  "fact_type": "decision",
  "entities": ["AideMemo", "SQLite"]
}
```

설치된 MCP 환경이 `source_id`와 `actor_id`를 제공합니다. 계정 B가 새 Codex
세션에서 `aidememo_context` 또는 `aidememo_query`를 호출하면 반환된 사실에는
`actor_id: "codex:account-a"`가 유지됩니다. 계정 B가 lesson을 추가하면 계정 A도
같은 프로젝트 네임스페이스에서 이를 조회할 수 있습니다.

코드 기반 통합에서는 같은 기본값을 `AIDEMEMO_SOURCE_ID`와
`AIDEMEMO_ACTOR_ID`로 사용할 수 있습니다.

계정 B가 추적 중인 워크플로를 명시적으로 이어받는 경우 세션을 연결합니다.

```json
{
  "title": "Resume the SQLite contention investigation",
  "parent_session_id": "session-01...",
  "source_id": "project:aidememo"
}
```

`aidememo_workflow_start`는 새 세션에서 부모 세션으로 `continued_from` 그래프
edge를 만듭니다. 전체 Codex 채팅을 저장하거나 재생하지 않고도 lineage를 유지합니다.

## 공유 쓰기 모드 선택

두 로컬 Codex 프로필은 일반적으로 기본 SQLite 저장소를 직접 공유할 수 있습니다.
AideMemo는 WAL 모드와 `BEGIN IMMEDIATE`를 사용하고, 짧은 SQLite busy timeout과
`store.lock_retry_ms` 범위의 지터 재시도를 결합합니다.

동시 쓰기가 많거나 선택형 redb 백엔드를 사용한다면 HTTP MCP 서버 하나를 실행하고
두 프로필이 같은 서버를 사용하게 합니다.

```bash
AIDEMEMO_SOURCE_ID=project:aidememo \
  aidememo --backend libsqlite --store "$STORE" mcp-serve --port 3000

CODEX_HOME="$HOME/.codex-account-a" \
  codex mcp add aidememo --url http://127.0.0.1:3000/mcp
```

HTTP 클라이언트는 서버 프로세스를 공유하므로 transport 계층에 인증된 클라이언트
identity가 추가되기 전까지 write tool 호출에 `actor_id`를 전달해야 합니다.

## Hermes 세션 저장소에서 가져온 점

[Hermes Agent session storage](https://hermes-agent.nousresearch.com/docs/developer-guide/session-storage)는
SQLite WAL을 사용하고, 세션 source와 user identity를 구분하며, 부모 세션 lineage와
쓰기 경합 시 지터 재시도를 제공합니다. AideMemo는 프로젝트 메모리 연속성을
강화하는 부분을 반영합니다.

- `source_id`와 `actor_id`를 별도 개념으로 유지합니다.
- 재개한 워크플로를 `continued_from`으로 부모 세션에 연결합니다.
- 공유 SQLite 쓰기는 이른 lock 획득과 지터 재시도를 사용합니다.

Hermes의 전체 message, tool call, reasoning, token, billing archive는 복사하지 않습니다.
AideMemo의 지속 계층은 명시적인 typed fact, relation, 감사 가능한 workflow artifact에
집중합니다. 따라서 여러 코딩 에이전트 사이에서 이식 가능하며 민감한 transcript가
의도치 않게 남을 가능성을 줄입니다.

## 범위 제한

- 서로 다른 OS 사용자는 같은 저장소에 대한 파일 권한이나 양쪽에서 접근할 수 있는
  공유 HTTP MCP 서버가 필요합니다.
- 로컬 저장소는 실시간 cross-machine 동기화를 제공하지 않습니다. 머신 간 통제된
  이동에는 backup/restore 또는 branch push/merge를 사용합니다.
- 계정 간 프로젝트 메모리 공유는 명시적으로 선택해야 합니다. 승인 없이 조직 또는
  정책 경계를 넘어 저장소를 연결하지 마십시오.
