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
args = ["--backend", "libsqlite", "--store", "/absolute/project/_meta/wiki.sqlite", "mcp"]

[mcp_servers.aidememo.env]
AIDEMEMO_SOURCE_ID = "project:my-app"
AIDEMEMO_ACTOR_ID = "codex:account-a"
```

Claude Code 독립 등록:

```bash
aidememo --store /absolute/project/_meta/wiki.sqlite mcp-install \
  --target claude \
  --source-id project:my-app \
  --actor-id claude:local
```

이 명령은 Claude Code의 현재 CLI 인자 순서를 사용하고 확인된 store를
고정합니다. 기능별 스킬과 읽기 전용 훅을 함께 제공하는 Claude 플러그인을
대신 사용할 수도 있습니다. Hermes, Cursor, OpenClaw, OpenCode에도 설치 대상이
있습니다. pi는 MCP를 받지 않으므로 의도적으로 스킬 전용입니다. 전체 표는
[`코딩 에이전트 설치`](CODING_AGENTS.md)를 참고하세요.

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

네트워크에 노출한 공유 저장소에서는 모든 클라이언트에 같은 범위 없는 토큰을
주기보다 bearer token마다 하나의 source와 writer identity를 고정하세요.

```json title="/etc/aidememo/token-bindings.json"
{
  "tokens": [
    {
      "token": "replace-with-a-random-secret",
      "source_id": "project:my-app",
      "actor_id": "codex:account-a"
    }
  ]
}
```

```bash
chmod 600 /etc/aidememo/token-bindings.json
aidememo --store ~/.aidememo/team.sqlite mcp-serve \
  --bind 0.0.0.0 \
  --auth-bindings-file /etc/aidememo/token-bindings.json
```

환경 변수로는 `AIDEMEMO_MCP_AUTH_BINDINGS_FILE`을 사용합니다. Bound token으로
호출하면 서버가 설정된 `source_id`와 `actor_id`를 모든 MCP tool call에 주입하고,
`aidememo_fact_add_many` item 내부를 포함해 호출자가 다른 값을 전달하면
거부합니다. Bound token은 범위 없는 `/sync/since`와 `/admin/status` endpoint를
사용할 수 없으며 `/health`에서는 health와 semantic prewarm 상태만 받습니다.
신뢰할 수 있는 범위 없는 관리자는 기존 `--auth-token-file` mode를 사용하세요.

`mcp-serve` 자체는 평문 HTTP를 사용합니다. Bearer binding은 identity와 scope를
강제하지만 전송 구간을 암호화하지 않습니다. Loopback이 아닌 배포에서는 반드시
TLS를 종료하는 reverse proxy 또는 암호화된 private tunnel 뒤에 서버를 두고,
backend port에 대한 직접 접근을 제한하세요.

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
aidememo --backend libsqlite mcp-install --target <agent> --source-id team-a
```

클라이언트가 명시적인 `source_id`를 전달하지 않으면 MCP 도구는 이 소스
네임스페이스를 기본값으로 사용합니다. 설치 명령은 선택한 저장소 백엔드와 확인된
저장소 경로도 고정하므로 에이전트 프로세스가 다른 설정 기본값이나 작업 디렉터리로
이동하지 않습니다. 여러 에이전트 프로필이 같은 네임스페이스를 공유하면서 작성자
provenance가 필요하면 별도로 `--actor-id`를 사용합니다.

Source 범위는 fact search/list/get, pinned context, entity read, graph
traversal/path/export, ID 기반 fact mutation에 일관되게 적용됩니다. Source 범위가
있는 entity 결과는 해당 source의 fact가 연결된 경우에만 반환하며, source
provenance가 없는 전역 entity metadata는 제외합니다. 같은 fact content는 한
source 안에서만 중복 제거되고, 서로 다른 두 source의 같은 텍스트는 독립된 두
fact ID로 유지됩니다. Graph relation은 별도의 source provenance를 가지므로,
source 범위가 있는 graph read는 relation namespace가 정확히 같은 edge만
허용합니다. 기존의 범위 없는 edge나 다른 source의 evidence, weight,
relation type은 노출되지 않습니다. 클라이언트가 자기 범위를 선택하거나
덮어쓰면 안 되는 경우 위 token binding을 사용하세요.

이 경계는 하나의 신뢰된 팀 저장소에서 협력하는 에이전트에 강한 partition을
제공하지만, 상호 적대적인 tenant를 위한 완전한 database boundary는 아닙니다.
Entity name과 entity type은 source 간 공유 ontology를 의도합니다. Tenant끼리
ontology조차 공유하면 안 되거나 서로의 resource 사용으로부터 격리해야 한다면
별도 store 또는 별도 AideMemo process를 사용하세요.

격리된 Codex 계정에는 같은 명시적 저장소를 가리키면서 `--codex-home`과
`--actor-id`를 반복합니다. [`여러 Codex 프로필에서 메모리 공유`](CODEX_MULTI_PROFILE.md)를
참고하십시오.

## 문제 해결

| 증상 | 해결 방법 |
|---|---|
| 에이전트에서 도구가 보이지 않음 | MCP 설정 경로를 확인하고 에이전트를 다시 시작합니다. |
| Claude 격리 프로필에 스킬이 없음 | `skill install --target claude` 전에 `CLAUDE_CONFIG_DIR`를 설정합니다. |
| 한 Codex 프로필에서만 AideMemo가 보이지 않음 | 활성 `CODEX_HOME`에 설치하거나 `--codex-home`을 명시적으로 전달합니다. |
| Hermes 격리 프로필에서 보이지 않음 | 스킬과 MCP를 설치하기 전에 `HERMES_HOME`을 설정합니다. |
| pi가 MCP 단계를 제안함 | AideMemo를 업데이트하고 `skill install --target pi`만 사용합니다. |
| `command not found: aidememo` | MCP 설정에 절대 경로를 사용합니다. |
| 에이전트가 잘못된 저장소를 엶 | 전역 `--store`로 다시 설치합니다. `aidememo doctor`가 Codex 저장소 불일치를 보고합니다. |
| 저장소 잠금 오류 | 공유 쓰기는 하나의 `aidememo mcp-serve` 프로세스를 사용합니다. |
| 다른 프로젝트 컨텍스트가 나타남 | `source_id` 범위를 추가하거나 확인합니다. |
