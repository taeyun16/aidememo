---
title: CLI 사용법
description: 자주 사용하는 AideMemo CLI 명령을 예제와 함께 설명합니다.
---

# CLI 사용법

CLI는 메모리를 추가하고 확인하고 유지하는 가장 빠른 방법입니다. 전체 최상위
명령 목록은 [`기능 목록`](FEATURES.md), 작업 형태에 따라 CLI, MCP, SDK
진입점을 고르는 방법은 [`에이전트 워크플로`](AGENT_WORKFLOWS.md)를
참고하세요.

## 검색과 질의

직접 검색에는 `search`를 사용합니다. 기본적으로 AideMemo는 BM25를 먼저
확인하고 어휘 신호가 약하거나 쿼리가 CJK이며 semantic 경로가 준비된
경우에만 semantic 검색으로 승격합니다.

```bash
aidememo search "Redis timeout" --limit 5
aidememo search "레디스 장애 원인" --limit 5
```

임베딩 모델을 로드하지 않아야 하는 결정적 데모, hook, CI 검사에는
`--bm25-only`를 사용합니다. 모든 쿼리에 semantic 검색을 적용하려면
`--hybrid`를 사용합니다.

```bash
aidememo search "Redis timeout" --bm25-only --limit 5
aidememo search "favorite camera setup" --hybrid --limit 5
```

더 풍부한 컨텍스트 팩에는 `query`를 사용합니다.

```bash
aidememo query "Redis worker timeout" --limit 8 --depth 2 --recent-limit 5
aidememo query "Redis worker timeout" --bm25-only --limit 8
```

여러 에이전트, 팀, 프로젝트가 저장소를 공유하면 `--source-id`를 사용합니다.

```bash
aidememo query "billing webhook duplicates" --source-id team-a
```

## 팩트 추가

```bash
aidememo fact add \
  "Decision: Billing webhook retries must use idempotency keys." \
  --type decision \
  --entities Billing,Webhook \
  --source-id team-a \
  --actor-id codex:account-a
```

`--source-id`는 신뢰된 공유 프로젝트 또는 에이전트 namespace로 유지합니다.
팩트를 작성한 profile이나 agent는 `--actor-id`로 기록합니다.
정확히 같은 content의 dedup은 정규화된 source ID 범위에서 수행됩니다.
같은 source에서 반복해 쓰면 기존 fact를 반환하지만, 다른 source의 동일한
content는 별도 fact로 유지됩니다.

팩트 타입을 의도적으로 선택합니다.

| 타입 | 예시 |
|---|---|
| `decision` | "Use idempotency keys for billing retries." |
| `lesson` | "Duplicate Stripe events came from retry races." |
| `error` | "Do not disable signature checks while debugging." |
| `preference` | "Prefer local-first tools for agent memory." |
| `note` | "The worker uses Redis for queue state." |

## 워크플로 시작

짧은 이슈, PR, 티켓에서 작업을 시작할 때 사용합니다.

```bash
aidememo workflow start "Stop duplicate billing webhook processing" \
  --body "Stripe webhooks sometimes process the same invoice twice." \
  --source "linear:ENG-456" \
  --source-id team-a
```

이전 tracked workflow를 lineage와 함께 이어가려면 `--parent-session <session-id>`를
전달합니다. AideMemo는 전체 chat transcript를 복사하는 대신 `continued_from`
relation을 기록합니다.

결정적 데모, hook, CI 검사에서는 semantic 모델 로드를 건너뜁니다.

```bash
aidememo workflow start "Fix Redis timeout" --bm25-only
```

결과 스레드를 범위가 제한되고 감사 가능한 canvas로 내보냅니다.

```bash
aidememo session canvas "$AIDEMEMO_SESSION_ID" --limit 20 \
  --source-id team-a --output session_canvas.md
```

Canvas는 파생된 Markdown 아티팩트입니다. 먼저 Mermaid 지도를 제공하고,
이어서 `aidememo fact get <id>`를 가리키는 팩트 ID 상세 조회 줄을 제공합니다.
MCP 에이전트는 `aidememo_session_canvas`로 같은 텍스트를 요청할 수 있고,
Python 에이전트는 `Memory.session_canvas(...)`를 호출할 수 있습니다.

## 다른 에이전트, 프로필, 계정으로 핸드오프

현재 작업에서 오래 유지할 내용을 기록한 뒤 간결한 packet을 만듭니다.

```bash
aidememo session handoff \
  --from-actor codex-one \
  --to-actor codex-two \
  --from codex/coding \
  --to codex/reviewer \
  --source-id team-a \
  --focus "패치를 검증하고 릴리스 프리플라이트 실행" \
  --done-when "집중 테스트와 릴리스 프리플라이트 통과" \
  --dispatch \
  "$AIDEMEMO_SESSION_ID"
```

`--dispatch`가 없으면 packet은 읽기 전용 미리보기입니다. 세션 ID를 유지하고 결정, 열린 질문, 교훈,
오류를 그룹화하며 모든 항목을 원본 팩트 ID와 연결합니다. Agent/profile 값은
라우팅 label이고 `source_id`는 포함할 공유 저장소 namespace를 제어합니다.
MCP에서는 `aidememo_handoff`, Python에서는 `Memory.handoff(...)`로 같은
아티팩트를 얻습니다.

dispatch한 세션 포인터는 수신자가 pull하고 확인합니다.

```bash
aidememo handoff inbox --actor-id codex-two --source-id team-a
aidememo handoff accept --actor-id codex-two handoff-...
aidememo handoff return --actor-id codex-two --outcome succeeded \
  --result-fact-id 01... handoff-...
aidememo handoff outbox --actor-id codex-one
aidememo handoff show handoff-...
```

여러 Codex/Claude 계정은 `agent add --type ... --home ...`으로 연결하고
`handoff send ALIAS`, `handoff run ALIAS`로 실행할 수 있습니다. profile은 자격 증명 값을
저장하지 않습니다. `config_home`은 Codex의 `CODEX_HOME` 또는 Claude의
`CLAUDE_CONFIG_DIR`가 되며 기본 `core` 정책을 사용합니다. 추가 환경 변수는
`--pass-env NAME`으로 허용합니다. `outcome=succeeded`는 완료 acknowledgement,
`failed`는 accepted 유지이며 `return`은 결과 fact를 연결합니다.
완료 결과는 outbox에 기본 포함되며 `--pending-only`로 숨길 수 있습니다.
기존 `installation` 및 `handoff run --installation ALIAS --next` 표기도 계속
지원합니다.

`accept`는 최신 packet과 resume 환경을 반환합니다. `complete`는 장부 상태만
바꾸며 테스트 통과를 증명하지 않습니다. `mcp-install --actor-id codex-two`로
설치 기본값을 지정하거나 `AIDEMEMO_ACTOR_ID`를 설정할 수 있습니다. actor
별칭은 인증 정보가 아닙니다.

이 인터페이스에는 topic, offset, consumer group, lease, retry, payload 복제,
exactly-once delivery가 없습니다. 모든 할당은 기존 추적 세션을 가리킵니다.

packet에는 수신자 bootstrap 명령이 하나 포함됩니다. 이 명령은 세션 존재를
검증하고 연속성과 검색 scope를 함께 활성화합니다.

```bash
eval "$(aidememo session resume --source-id team-a session-...)"
```

기존 통합이 분리된 필드를 생성한다면 `--from-agent`, `--from-profile`,
`--to-agent`, `--to-profile` 옵션도 계속 사용할 수 있습니다.
`--output`으로 packet 파일을 쓰면 stdout에도 검증된 수신자 resume 명령이
표시되므로 파일을 다시 열지 않고 활성화할 수 있습니다.

## 프로젝트 프로필 내보내기

현재 타입 지정 팩트에서 읽기 전용 프로필을 생성합니다.

```bash
aidememo profile export --output project_profile.md
aidememo profile export --source-id team-a --limit 80
```

이 명령은 팩트를 생성하거나 수정하지 않습니다. AideMemo의 타입 지정 팩트를
근거 기록으로 유지하면서 에이전트에 간결한 프로젝트 또는 persona 보기를
제공합니다. MCP 에이전트는 `aidememo_profile_export`로 같은 텍스트를 요청할
수 있고, Python 에이전트는 `Memory.project_profile(...)`을 호출할 수 있습니다.

## 엔티티와 팩트 탐색

```bash
aidememo entity list --source-id team-a
aidememo entity get Redis --source-id team-a
aidememo entity show Redis --source-id team-a
aidememo fact list --type decision --limit 20 --source-id team-a
aidememo fact get 01H... --source-id team-a
aidememo fact pinned --source-id team-a
aidememo fact pin 01H... --source-id team-a
aidememo fact unpin 01H... --source-id team-a
aidememo fact delete 01H... --source-id team-a
aidememo fact feedback 01H... --helpful --source-id team-a
aidememo fact supersede 01HOLD... 01HNEW... --source-id team-a
aidememo fact archive --ids 01H... --source-id team-a
```

Source 범위 entity output은 fact 기반이며 전역 prose metadata를 제외합니다. 다른
source가 소유한 fact ID를 source 범위로 조회하거나 ID 기반 mutation을 시도하면
not-found를 반환합니다. `--source-id`를 생략하면 신뢰된 unscoped administrator
동작을 유지합니다.

## 그래프 탐색

```bash
aidememo traverse Redis --depth 2 --source-id team-a
aidememo path Worker Redis --source-id team-a
aidememo graph --from Redis --depth 2 --format mermaid --source-id team-a
```

Source 범위 graph read는 같은 source가 명시적으로 소유한 relation만 포함하며,
기존 범위 없는 edge를 상속하지 않습니다.

## Tracked session의 source 범위 유지

Session marker와 파생 context를 수집 대상 fact와 같은 namespace에
유지합니다.

```bash
eval "$(aidememo session new 'billing retry audit' --source-id team-a)"
aidememo session current --source-id team-a
aidememo session list --source-id team-a
aidememo session start --source-id team-a
```

## 공유 HTTP 서버에 identity 고정

`AIDEMEMO_SOURCE_ID`는 신뢰된 process의 기본값일 뿐입니다. 독립적으로
인증된 agent들이 HTTP 서버 하나를 공유한다면 bearer token마다 고정된
source와 writer identity를 바인딩합니다.

```json
{"tokens":[{"token":"replace-me","source_id":"team-a","actor_id":"codex:a"}]}
```

```bash
chmod 600 ./token-bindings.json
aidememo mcp-serve --port 3000 --auth-bindings-file ./token-bindings.json
```

바인딩은 batch item을 포함한 모든 MCP 호출에 주입되며, 호출자는
`source_id`나 `actor_id`를 재정의할 수 없습니다. Bound token은
`/admin/status`나 `/sync/since`를 읽을 수 없습니다. `mcp-serve`는 평문
HTTP를 사용하므로 loopback 외 배포 앞에 TLS termination 또는 암호화된
private tunnel을 두세요.

## 메모리 유지 관리

문제가 있다고 느껴지면 `doctor`를 실행합니다.

```bash
aidememo doctor
aidememo doctor --json
```

원시 그래프 상태 검사에는 `lint`를 실행합니다.

```bash
aidememo lint
```

오래되거나 중복된 메모리를 통합합니다.

```bash
aidememo consolidate --semantic-threshold 0.85 --dry-run
aidememo consolidate --ttl note=30 --ttl question=14
```

## 명시적 저장소 사용

스크립트에서는 명령이 실수로 기본 저장소를 읽지 않도록 `--store`를
전달합니다.

```bash
aidememo --store ./team.sqlite search "release checklist"
```
