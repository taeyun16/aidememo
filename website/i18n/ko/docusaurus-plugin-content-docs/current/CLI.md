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

`--source-id`는 공유 프로젝트 또는 tenant namespace로 유지합니다. 팩트를 작성한
profile이나 agent는 `--actor-id`로 기록합니다.

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
aidememo session canvas "$AIDEMEMO_SESSION_ID" --limit 20 --output session_canvas.md
```

Canvas는 파생된 Markdown 아티팩트입니다. 먼저 Mermaid 지도를 제공하고,
이어서 `aidememo fact get <id>`를 가리키는 팩트 ID 상세 조회 줄을 제공합니다.
MCP 에이전트는 `aidememo_session_canvas`로 같은 텍스트를 요청할 수 있고,
Python 에이전트는 `Memory.session_canvas(...)`를 호출할 수 있습니다.

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
aidememo entity list
aidememo entity show Redis
aidememo fact list --type decision --limit 20
aidememo fact get 01H...
```

## 그래프 탐색

```bash
aidememo traverse Redis --depth 2
aidememo path Worker Redis
aidememo graph --from Redis --depth 2 --format mermaid
```

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
