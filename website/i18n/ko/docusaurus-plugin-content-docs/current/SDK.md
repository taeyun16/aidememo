---
title: Python SDK
description: aidememo-agent-sdk로 코드에서 AideMemo 메모리를 사용합니다.
---

# Python SDK

에이전트나 스크립트가 한 번에 하나씩 도구를 호출하는 대신 프로그래밍 가능한
working set으로 메모리를 사용해야 할 때 `aidememo-agent-sdk`를 사용합니다.

## 설치

```bash
python -m pip install aidememo-agent-sdk

# 선택형 인프로세스 네이티브 바인딩
python -m pip install "aidememo-agent-sdk[binding]"
```

네이티브 바인딩이 없으면 SDK는 `PATH`의 `aidememo` CLI로 폴백합니다.
`binding` extra는 공개된 `aidememo-python` 패키지를 설치해 선택형
인프로세스 fast path를 활성화합니다.

## 네이티브 바인딩

이 페이지는 Python composition SDK를 다룹니다. 런타임별 네이티브 바인딩은
각 패키지 README에서 설명합니다.

| 런타임 | 패키지 | 릴리스 경로 | 문서 |
|---|---|---|---|
| Python 네이티브 | `aidememo-python` | PyPI에 배포됨 | [README](https://github.com/taeyun16/aidememo/tree/main/crates/aidememo-python) |
| Node.js | `aidememo-napi` | npm에 root wrapper와 platform 패키지 배포됨 | [README](https://github.com/taeyun16/aidememo/tree/main/crates/aidememo-napi) |
| Elixir | `aidememo_nif` | 로컬/경로 바인딩 문서 준비 완료; Hex 배포 워크플로는 아직 없음 | [README](https://github.com/taeyun16/aidememo/tree/main/crates/aidememo-nif) |
| C ABI | `aidememo-ffi` | Rust 크레이트와 C 헤더/링크 문서 | [README](https://github.com/taeyun16/aidememo/tree/main/crates/aidememo-ffi) |

모든 네이티브 바인딩은 CLI와 같은 백엔드 선택자를 사용합니다. 백엔드를
생략하거나 빈 문자열을 전달하면 컴파일된 기본값을 사용합니다. 기본 빌드는
SQLite를 포함하며 열 때 선택할 수 있습니다(`backend="sqlite"` 또는
`backend="libsqlite"` / `{ backend: "sqlite" }` 또는
`{ backend: "libsqlite" }` / `backend: "sqlite"` 또는
`backend: "libsqlite"` / `aidememo_open_with_backend(..., "sqlite")` 또는
`aidememo_open_with_backend(..., "libsqlite")`). redb 저장소를 열어야 할 때는
Cargo `redb` 기능으로 빌드합니다.

브랜치 로그 helper는 현재 Python composition SDK, `aidememo-python`,
`aidememo-napi`, `aidememo_nif`에서 이미 열린 handle을 통한 로컬 브랜치
아티팩트에 제공됩니다. C ABI 호출자는 저수준 ABI에 해당 표면이 필요해질
때까지 CLI의 `aidememo branch ...` 명령을 사용해야 합니다.

## 외부 Codex 또는 Claude 핸드오프 실행

SDK를 설치하면 `aidememo-worker-lane`도 설치됩니다. 이 명령은 주소가 지정된
AideMemo handoff 하나를 accept하고 현재 packet을 non-interactive coding CLI에
주입한 뒤 결과를 같은 session에 기록합니다.

```bash
aidememo-worker-lane handoff-... \
  --actor-id codex-two \
  --agent codex \
  --workspace "$PWD" \
  --store ~/.aidememo/wiki.sqlite \
  --source-id release-team \
  --kanban-task task-42
```

Claude Code에는 `--agent claude --actor-id claude-main`을 사용합니다. Codex의
기본값은 `codex exec --ephemeral --sandbox workspace-write`, Claude의 기본값은
`claude --print --permission-mode acceptEdits --no-session-persistence`입니다.
인자는 shell 없이 list로 실행되며 수신자는 accepted packet의
`AIDEMEMO_SESSION_ID`, `AIDEMEMO_SOURCE_ID`, `AIDEMEMO_ACTOR_ID`를 상속합니다.

runner는 assignment를 accept하기 전에 agent binary와 workspace를 검증하므로
로컬 설정 오류가 작업을 점유하지 않습니다. accept 이후 process 시작 실패는
아래의 일반 failure 경로로 처리합니다.

반복 사용하는 계정은 `agent add --type ... --home ...`으로 config root와
workspace를 등록하고 `handoff run ALIAS`로 실행할 수 있습니다.
Codex에는 `CODEX_HOME`, Claude에는 `CLAUDE_CONFIG_DIR`가 전달되며 profile은
자격 증명 값을 저장하지 않습니다. 기본 `core` 환경 정책을 사용하고 Codex의
`--output-schema`와 Claude의 `--json-schema`를 공통 결과 계약으로 정규화합니다.

두 adapter는 `summary`, `changed_files`, `validations`, `done_when_met`,
`blockers` 결과 계약을 사용합니다. 성공 시 session에 연결된 result fact를 먼저
기록한 뒤 AideMemo assignment를 `completed`로 바꿉니다. non-zero exit, timeout,
또는 `done_when_met=false`는 `error` fact를 기록하고
assignment를 `accepted`로 유지하므로 upstream scheduler가 retry 또는 block할
수 있습니다. `--kanban-task`는 반환 envelope와 prompt에 label만 붙이며 Kanban을
변경하지 않습니다. Hermes가 claim, retry, validation, card completion을 계속
소유합니다. 이 runner는 Hermes `spawn_fn` registration, authenticated identity,
exactly-once execution을 제공하지 않습니다.

프로그램에서는 `aidememo_agent`의 `WorkerLaneConfig`와
`run_external_assignment(...)`을 사용할 수 있습니다.

## 메모리 열기

```python
from aidememo_agent import Memory

mem = Memory.open(
    source_id="team-a",
    actor_id="codex:account-a",
    storage_backend="libsqlite",
)
```

신뢰된 공유 저장소 안에서 한 팀, 에이전트, 프로젝트를 partition하려면
`source_id`를 사용합니다. 해당 인자를 생략하면 `Memory.open(...)`은
`AIDEMEMO_SOURCE_ID`와 `AIDEMEMO_ACTOR_ID`도 상속합니다. source 기본값은
fanout search/query/aggregate, context와 recent read, entity list와 traverse,
workflow/session/project context, fact write에 전달됩니다. actor 기본값은
workflow와 fact write의 작성자 provenance를 기록하며 retrieval을 partition하지
않습니다. 정확한 content dedup은 source 안에서 적용되므로 같은 텍스트가 서로
다른 두 source에 독립적으로 존재할 수 있습니다.

이 값은 편의 기본값이지 변경 불가능한 credential이 아닙니다. 명시적인 per-call
값이 `Memory.open(...)`보다 우선하고, `remember(...)` batch item은 method 및
open-time 기본값을 덮어쓸 수 있습니다. 할당을 강제해야 한다면 호출자에게 native
store handle이나 stdio/CLI 접근을 주지 말고 HTTP bearer identity binding을 통해
AideMemo를 노출하세요.

기본 source가 설정되면 composition SDK는 `mem.client.doctor()`, `lint()`,
`stats()`를 의도적으로 거부합니다. 이 메서드는 global store metadata를
노출하기 때문입니다. 진단은 scope가 없는 관리자 process에서 실행하세요.

### 바인딩별 source scope

네이티브 바인딩은 composition SDK 기본값을 상속하지 않습니다. identity가
필요한 operation마다 다음 값을 전달하세요.

| 표면 | Source selector | Actor selector |
|---|---|---|
| `aidememo-agent-sdk` | `Memory.open(source_id=...)`, per-call 또는 per-item override 가능 | `Memory.open(actor_id=...)`, workflow/fact write에 적용 |
| `aidememo-python` | source-aware read, relation, workflow, fact write의 `source_id=` | workflow/fact write 또는 `fact_add_many` item의 `actor_id=` |
| `aidememo-napi` | option 또는 method에 문서화된 positional source argument의 `sourceId` | workflow/fact write 또는 batch item의 `actorId` |
| `aidememo_nif` | operation option의 `source_id:` | fact write 또는 batch item의 `actor_id:` |
| `aidememo-ffi` | `_scoped` 함수와 각 batch item의 `source_id` | `aidememo_fact_add_scoped` 또는 각 batch item의 `actor_id` |

이는 상호 적대적인 multi-tenant security boundary가 아닙니다. Native SDK
호출자는 다른 source를 선택할 수 있고 entity name/type은 공유 ontology를
구성합니다. 상호 신뢰하지 않는 tenant는 별도 store를 사용하고, 인증된
에이전트가 할당된 source를 재정의하면 안 될 때는 HTTP bearer identity
binding을 사용하세요.

`storage_backend`는 선택 사항이며 CLI와 네이티브 바인딩 선택자와 같은 값을
사용합니다. 컴파일된 기본값은 생략하거나 빈 문자열을 전달하고, 기본 로컬
SQLite 백엔드는 `"sqlite"` 또는 `"libsqlite"`, 설치된 바인딩이나 CLI가
Cargo `redb`로 빌드된 경우에는 `"redb"`를 사용합니다. SDK는 선택자를
`aidememo-python`과 subprocess 폴백(`aidememo --backend ...`) 모두에
전달합니다.

## 여러 주제 검색

```python
rows = mem.search_rows([
    "Redis timeout decisions",
    {"query": "billing webhook duplicates", "topic": "Billing"},
])

for row in rows:
    print(row["fact_type"], row["content"])
```

## Coverage 확인

```python
coverage = mem.coverage_by(rows, ["fact_type"])
print(coverage)
```

계획 전에 decision, lesson, error를 찾았는지 에이전트가 확인해야 할 때
유용합니다.

## 메모리 집계

```python
timeline = mem.aggregate_many([
    {"query": "release preflight", "op": "timeline"},
    {"query": "Redis timeout", "op": "count", "fact_type": "error"},
])

print(timeline)
```

다음과 같은 질문에는 집계를 사용합니다.

- "이 일이 몇 번 발생했나?"
- "타임라인은 어떻게 되나?"
- "기록한 총비용은 얼마인가?"

## 새 팩트 기억

```python
mem.remember([
    {
        "content": "Decision: Redis timeout fixes must start with DNS metrics.",
        "fact_type": "decision",
        "entities": ["Redis", "Worker"],
    },
    {
        "content": "Lesson: pool-size changes hid the real DNS failure mode.",
        "fact_type": "lesson",
        "entities": ["Redis", "Worker"],
    },
])
```

배치 쓰기는 더 빠르고 에이전트에 하나의 명확한 side effect를 제공합니다.

## 추측성 실행 브랜치

스크립트나 에이전트가 하나의 백업에서 여러 candidate 저장소를 만들고 최선의
결과만 병합하려면 브랜치 로그를 사용합니다.

```python
from aidememo_agent import Memory

candidate = Memory.open(store_path="./candidate-b.sqlite", storage_backend="libsqlite")

push = candidate.branch_push(
    "candidate-b",
    "./shared",
    base="./shared/backup-01...",
)
print(push["records_exported"])

main = Memory.open(store_path="./main.sqlite", storage_backend="libsqlite")
merge = main.branch_merge("./shared", branch="candidate-b")
print(merge["facts_inserted"])
```

로컬 브랜치 경로는 사용할 수 있을 때 `aidememo-python` fast path를
사용합니다. S3 브랜치 URI는 설치된 `aidememo --features s3` 바이너리가 AWS
credential과 압축 동작을 소유하도록 CLI로 폴백합니다.

## SDK와 MCP 선택

| SDK 사용 | MCP 사용 |
|---|---|
| 에이전트가 Python을 작성하거나 스크립트를 실행 | 모델이 도구를 직접 호출해야 함 |
| fanout 검색과 중복 제거가 필요 | 하나의 집중된 search/query가 필요 |
| 코드에서 coverage 확인이나 집계가 필요 | 모델에 보이는 도구 결과가 필요 |
| 쓰기를 배치하려 함 | 대화형 에이전트 워크플로가 필요 |
