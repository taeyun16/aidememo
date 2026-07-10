---
title: MCP 설정
description: AideMemo를 MCP 서버로 등록하고 핵심 도구를 사용합니다.
---

# MCP 설정

AideMemo는 에이전트가 도구로 메모리를 검색하고 쓸 수 있도록 같은 메모리
저장소를 MCP로 제공합니다. 전체 도구 목록은 [`기능 목록`](FEATURES.md),
턴마다 알맞은 도구를 선택하는 방법은
[`에이전트 워크플로`](AGENT_WORKFLOWS.md)를 참고하세요.

## Stdio MCP

로컬 에이전트에는 stdio MCP를 사용합니다.

```bash
aidememo mcp
```

Codex 설정 예시:

```toml
[mcp_servers.aidememo]
command = "aidememo"
args = ["--backend", "libsqlite", "mcp"]
```

Claude Code 명령 예시:

```bash
claude mcp add aidememo -- aidememo mcp
```

## HTTP MCP 서버

여러 에이전트가 하나의 웜 프로세스를 공유해야 할 때 HTTP를 사용합니다.

```bash
aidememo mcp-serve --port 3000 --store ~/.aidememo/team.sqlite
```

MCP 클라이언트가 다음 주소를 사용하도록 설정합니다.

```text
http://127.0.0.1:3000/mcp
```

HTTP 모드는 웜 모델 재사용과 공유 쓰기에 유용합니다. 한 번에 하나의 writer
프로세스만 데이터베이스 잠금을 가질 수 있는 redb 저장소에는 특히 권장합니다.

## 핵심 도구

대부분의 에이전트 워크플로에는 다음 도구만 필요합니다.

| 도구 | 사용 시점 |
|---|---|
| `aidememo_workflow_start` | 이슈, PR, 티켓, 간단한 프롬프트에서 작업을 시작할 때 |
| `aidememo_context` | 에이전트가 턴 시작 시 프로젝트 컨텍스트를 필요로 할 때 |
| `aidememo_query` | 특정 주제를 더 깊이 살펴볼 때 |
| `aidememo_search` | 정확한 대상을 빠르게 검색할 때 |
| `aidememo_aggregate` | 정확한 개수, 합계, 날짜 집합, 타임라인이 필요할 때 |
| `aidememo_session_canvas` | 긴 추적 워크플로를 다시 시작할 때 |
| `aidememo_profile_export` | 간결한 읽기 전용 프로젝트 프로필이 필요할 때 |
| `aidememo_fact_add` | 새 팩트 하나를 배웠을 때 |
| `aidememo_fact_add_many` | 여러 팩트를 배워 배치로 기록해야 할 때 |

## 권장 에이전트 패턴

티켓을 시작할 때:

```json
{
  "title": "Fix Redis timeout in worker",
  "body": "Worker jobs intermittently time out against Redis.",
  "source": "github:org/app#123",
  "source_id": "team-a",
  "bm25_only": true
}
```

다음 도구를 호출합니다.

```text
aidememo_workflow_start
```

이후 팩트를 추가할 때 반환된 `session_id`를 사용합니다.

```json
{
  "content": "Lesson: the timeout was DNS resolution, not pool size.",
  "fact_type": "lesson",
  "entities": ["Redis", "Worker"],
  "session_id": "session-..."
}
```

다음 도구를 호출합니다.

```text
aidememo_fact_add
```

## 소스 범위 지정

공유 저장소에 여러 팀, 프로젝트, 사용자, 에이전트가 포함되면 `source_id`를
사용합니다.

```bash
aidememo --backend libsqlite mcp-install --target codex --source-id team-a
```

클라이언트가 명시적인 `source_id`를 전달하지 않으면 MCP 도구는 이 소스
네임스페이스를 기본값으로 사용합니다. 설치 명령은 선택한 저장소 백엔드도
고정하므로 에이전트 프로세스가 다른 설정 기본값으로 돌아가지 않습니다.

## 문제 해결

| 증상 | 해결 방법 |
|---|---|
| 에이전트에서 도구가 보이지 않음 | MCP 설정 경로를 확인하고 에이전트를 다시 시작합니다. |
| `command not found: aidememo` | MCP 설정에 절대 경로를 사용합니다. |
| 저장소 잠금 오류 | 공유 쓰기는 하나의 `aidememo mcp-serve` 프로세스를 사용합니다. |
| 다른 프로젝트 컨텍스트가 나타남 | `source_id` 범위를 추가하거나 확인합니다. |
