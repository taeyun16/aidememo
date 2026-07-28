---
title: 추적 작업 핸드오프
description: 채팅 기록을 복사하지 않고 하나의 AideMemo 워크플로를 다른 코딩 에이전트 계정으로 넘깁니다.
---

# 추적 작업 핸드오프

공유 메모리는 연결된 모든 에이전트가 프로젝트 맥락을 사용할 수 있게 합니다.
핸드오프는 작업 주체가 바뀔 때 사용하는 명시적인 단계입니다. 같은 추적 세션을
명확한 목표와 완료 조건과 함께 지정한 에이전트 계정으로 보내고, 반환된 근거를
그 세션에 연결합니다.

> Handoff는 현재 아직 릴리스되지 않은 `main` 기능입니다. 공개 v0.1.0
> 산출물에는 이 명령이 없습니다. 자동 경로를 진행하기 전에 현재 `main`의
> 두 진입점을 모두 설치하세요.
>
> ```bash
> cargo install --path crates/aidememo-cli
> python -m pip install -e ./packages/aidememo-agent-sdk
> aidememo-worker-lane --help
> ```
>
> `aidememo handoff run`은 SDK가 설치한 `aidememo-worker-lane`에 실행을
> 위임합니다. 마지막 명령은 설치 사전 점검입니다.

## 가장 짧은 경로

### 1. 추적 작업 시작

```bash
eval "$(aidememo session new --source-id release-team 'Redis timeout 패치 검토')"
export AIDEMEMO_ACTOR_ID=codex-one
```

`AIDEMEMO_SESSION_ID`가 설정된 동안 추가한 fact는 이 워크플로에 연결됩니다.
다음 작업자에게 필요한 결정, 실패한 시도, 교훈, 열린 질문을 기록합니다.

### 2. 목적지를 한 번만 연결

```bash
aidememo agent add codex-two --type codex \
  --home /path/to/codex-two-home \
  --workspace "$PWD" \
  --source-id release-team
```

프로필에는 경로와 라우팅 메타데이터만 들어가며 자격 증명은 저장하지 않습니다.
AideMemo는 실행 시 설정된 home을 코딩 에이전트에 전달합니다.

### 3. 활성 세션 전송

```bash
aidememo handoff send codex-two \
  --focus "Redis timeout 패치 검토" \
  --done-when "집중 테스트가 통과하고 리뷰 결과가 기록됨"
```

`send`는 환경에서 현재 세션과 발신자를 추론합니다. 채팅이나 세션 fact를
복사하지 않고 작은 assignment pointer만 저장합니다.

### 4. 수신 계정에서 이어서 작업

```bash
aidememo handoff run codex-two
```

runner는 `codex-two`의 가장 오래된 pending assignment를 accept하고, 추적
세션에서 현재 packet을 다시 만든 뒤 설정된 코딩 에이전트를 실행합니다.
결과는 같은 세션으로 반환됩니다.

### 5. 반환 결과 확인

`send`가 출력한 ID를 사용합니다.

```bash
aidememo handoff show handoff-...
aidememo handoff outbox --actor-id codex-one
```

발신자는 수신자의 vendor-local 채팅을 열지 않고도 반환 결과와 연결된 result
fact를 확인합니다.

## 유지되는 것

| 워크플로에 유지 | 작업자와 함께 변경 |
|---|---|
| `session_id`, 지속 가능한 fact, 결정, 실패, 결과 근거 | `actor_id`, 코딩 에이전트 설치, 런타임, 역할 |
| `source_id` 아래의 프로젝트 또는 tenant 범위 | 이 assignment의 명시적 `focus`와 `done_when` |
| 검증 가능한 fact 이력 | vendor-local 채팅 또는 프로세스 상태 |

공유 메모리와 핸드오프는 서로 보완합니다.

- **공유 메모리는 항상 켜져 있습니다.** 연결된 에이전트는 같은 source 범위
  저장소에서 지속 가능한 프로젝트 지식을 검색할 수 있습니다.
- **핸드오프는 의도적으로 사용합니다.** 지정한 작업자가 추적 작업을 이어받고
  근거를 반환해야 할 때 사용합니다.

## 수동 수신 흐름

검증된 자동 adapter가 없거나 오케스트레이터가 lifecycle을 직접 제어해야 할 때
수동 흐름을 사용합니다.

```bash
aidememo agent add cursor-review --type manual --workspace "$PWD" \
  --source-id release-team
aidememo handoff send cursor-review --focus "패치 검토"

AIDEMEMO_ACTOR_ID=cursor-review aidememo handoff inbox
AIDEMEMO_ACTOR_ID=cursor-review aidememo handoff accept handoff-...

# accept가 출력한 세션 ID를 재개한 뒤 수신자 소유 근거를 기록합니다.
eval "$(aidememo session resume --source-id release-team session-...)"
export AIDEMEMO_ACTOR_ID=cursor-review
aidememo fact add "리뷰 통과" --type note --entities Release \
  --source-id release-team --actor-id cursor-review
aidememo handoff return \
  --outcome succeeded \
  --result-fact-id 01... \
  handoff-...
```

수동 프로필에는 process adapter가 없으므로 `handoff run cursor-review`는
의도적으로 거절됩니다. 결과 fact가 전달된 세션에 연결되지 않았거나 정확한
`source_id`를 사용하지 않거나 수신 actor가 작성하지 않았다면 `return`도
fail-closed로 거절됩니다.

## 라우팅 모델

| 필드 | 역할 |
|---|---|
| `session_id` | 연속성: 수신자가 이어받는 추적 워크플로 |
| `source_id` | 범위: 워크플로가 검색할 수 있는 프로젝트, 팀 또는 tenant fact |
| `actor_id` | 주소: 사용자가 지정한 계정 또는 설치 별칭이며 인증이 아님 |
| agent/profile | 작업을 실행할 위치를 설명하는 런타임 메타데이터 |
| `focus` | 다음의 구체적인 목표 |
| `done_when` | 관찰 가능한 완료 조건 |

dispatch하지 않으면 `aidememo session handoff`는 읽기 전용 packet 미리보기로
남습니다. dispatch하면 수신자가 assignment pointer 하나를 pull하고 `accept`가
현재 세션 근거에서 packet을 다시 만듭니다.

## 운영 경계

- `handoff board`는 `ready`, `in_progress`, `attention`, `returned`
  assignment를 보여주는 파생 뷰이며 별도 Kanban 시스템이 아닙니다.
- 자동 실행의 기본 timeout은 1800초입니다. 더 긴 작업에는
  `handoff run codex-two --timeout 14400`을 사용합니다.
- 장시간 실행하는 worker는 기본 3600초마다 AideMemo heartbeat를 기록합니다.
- 연결된 Hermes card의 claim, dependency, retry, completion은 계속 Hermes가
  소유합니다. AideMemo는 외부 session pointer와 결과 근거를 전달합니다.
- assignment ledger는 메시지 broker가 아닙니다. topic, offset, consumer group,
  delivery retry, exactly-once 실행 보장이 없습니다.
- 결과 반환은 fail-closed입니다. fact는 전달된 세션과 정확한 source 범위에
  속하고 수신 actor의 작성 provenance를 가져야 합니다.
- 반환 결과는 연결된 근거이지 downstream 모델이 작업을 올바르게 완료했다는
  자동 증명이 아닙니다. `done_when`은 별도로 검증해야 합니다.

도구 수준 스키마는 [`MCP 설정`](MCP.md), SDK와 저수준 오케스트레이터 패턴은
[`에이전트 워크플로`](AGENT_WORKFLOWS.md)를 참고하세요. 토큰 없는 protocol
smoke는 `scripts/demo-agent-handoff.sh`를 실행합니다.
