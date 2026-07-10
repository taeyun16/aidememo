---
title: 빠른 시작
description: 몇 개의 명령으로 메모리를 추가하고 검색한 뒤 워크플로를 시작합니다.
---

# 빠른 시작

이 가이드는 작은 로컬 저장소를 만들고 메모리를 기록·검색한 뒤 간단한
티켓에서 워크플로를 시작합니다.

먼저 [설치 가이드](INSTALLATION.md)에 따라 AideMemo를 설치하고 CLI를
확인합니다.

```bash
aidememo --help
```

## 1. 데모 저장소 만들기

```bash
export AIDEMEMO_DEMO_STORE="$(mktemp -d)/wiki.sqlite"
```

아래 명령은 모두 이 저장소를 사용합니다.

```bash
am() {
  aidememo --store "$AIDEMEMO_DEMO_STORE" "$@"
}
```

## 2. 팩트 추가하기

결정을 추가합니다.

```bash
am fact add \
  "Decision: Redis timeout fixes must go through the Worker job wrapper." \
  --type decision \
  --entities Redis,Worker
```

교훈을 추가합니다.

```bash
am fact add \
  "Lesson: The last Worker Redis timeout was DNS resolution, not pool size." \
  --type lesson \
  --entities Redis,Worker
```

피해야 할 오류를 추가합니다.

```bash
am fact add \
  "Error: Avoid increasing Redis pool size before checking DNS metrics." \
  --type error \
  --entities Redis,Worker
```

## 3. 메모리 검색하기

```bash
am search "Redis timeout"
```

검색 결과와 주변 그래프 컨텍스트가 함께 필요하면 `query`를 사용합니다.

```bash
am query "Fix Redis timeout in worker" --bm25-only --limit 5 --depth 2
```

## 4. 티켓에서 워크플로 시작하기

이슈, PR, 티켓 자동화의 권장 진입점은 `workflow start`입니다. 추적 세션을
만들고 티켓을 저장한 뒤 이전 결정, 교훈, 오류, 검색 결과를 반환합니다.

```bash
am workflow start "Fix Redis timeout in worker" \
  --body "Worker jobs intermittently time out against Redis." \
  --source "github:org/app#123" \
  --bm25-only
```

출력에는 다음 필드가 포함됩니다.

- `session_id`: 이후 팩트를 이 작업에 연결합니다.
- `ticket_fact_id`: 저장된 입력 티켓 팩트입니다.
- `relevant_decisions`: 작업을 이끌어야 하는 결정입니다.
- `prior_lessons`: 유사 작업에서 얻은 교훈입니다.
- `prior_errors`: 피해야 할 알려진 실패 패턴입니다.

## 5. 세션 이어가기

CLI가 출력한 export 명령을 사용하면 이후 `fact add` 호출이 활성 워크플로
세션에 연결됩니다.

```bash
export AIDEMEMO_SESSION_ID=session-...

am fact add \
  "Lesson: This timeout was caused by a missing DNS retry around the worker wrapper." \
  --type lesson \
  --entities Redis,Worker
```

## 6. 최근 메모리 확인하기

```bash
am recent --last 1d
am stats
```

이제 CLI, MCP, SDK에서 사용할 수 있는 로컬 메모리 저장소가 준비됐습니다.
