---
title: 브랜치 로그
description: 추측성 메모리 변경과 클라우드 에이전트 실험에 AideMemo 브랜치 로그를 사용합니다.
---

# 브랜치 로그

브랜치 로그를 사용하면 여러 에이전트가 같은 백업 snapshot에서 시작해 로컬
메모리를 독립적으로 기록한 뒤 유지할 브랜치만 병합할 수 있습니다.

브랜치 로그는 저장소나 머신 사이에서 원본 레코드를 이동합니다. 오케스트레이터가
수신자별로 간결한 프롬프트 아티팩트도 필요하면 `aidememo session handoff`와
함께 사용하세요. 선택한 브랜치를 병합한 뒤 같은 `session_id`로 handoff packet을
생성하거나 소비합니다.

메모리 자체가 실험의 일부일 때 유용합니다.

| 상황 | 브랜치 로그가 도움이 되는 이유 |
|---|---|
| 여러 클라우드 에이전트가 하나의 baseline에서 같은 작업을 시도 | 각 worker가 hot SQLite 파일을 공유하지 않고 자체 팩트를 기록합니다. |
| prompting, extraction, retrieval 전략 비교 | candidate lesson을 분리하고 외부 평가 후 승자를 병합합니다. |
| 위험한 자동화가 noisy 메모리를 기록할 수 있음 | 브랜치 로그를 push하고 검사한 뒤 유용할 때만 병합합니다. |
| 에이전트 실행의 재생 가능한 아티팩트가 필요 | segment JSONL과 manifest가 해당 브랜치가 추가하려 한 내용을 기록합니다. |

브랜치 로그를 완전한 multi-master 충돌 해결로 사용하지 마세요. Merge는
멱등적이고 append 중심입니다. 동일하거나 오래된 entity/fact record는 건너뛰고,
같은 ID의 더 새로운 record는 LWW 순서로 적용하며, relation identity는 insert
또는 replace합니다. 독립적인 새 팩트는 추가하고, 경쟁하는 두 결정 같은 semantic
충돌은 호출자 정책에 맡깁니다.

## 워크플로

Baseline 백업을 만듭니다.

```bash
aidememo --store ./main.sqlite backup create ./shared
```

백업을 별도의 candidate 저장소로 복원합니다.

```bash
aidememo --store ./candidate-a.sqlite backup restore ./shared/backup-01... --force
aidememo --store ./candidate-b.sqlite backup restore ./shared/backup-01... --force
```

서로 다른 시도를 실행하고 메모리를 로컬에 기록합니다.

```bash
aidememo --store ./candidate-a.sqlite fact add \
  "Candidate A used broad context and produced noisy results." \
  --type lesson \
  --entities Experiment

aidememo --store ./candidate-b.sqlite fact add \
  "Candidate B used focused context and produced the best result." \
  --type lesson \
  --entities Experiment
```

백업 cursor 이후의 delta로 각 브랜치를 push합니다.

```bash
aidememo --store ./candidate-a.sqlite branch push \
  --branch candidate-a \
  --base ./shared/backup-01... \
  ./shared

aidememo --store ./candidate-b.sqlite branch push \
  --branch candidate-b \
  --base ./shared/backup-01... \
  ./shared
```

승리한 브랜치만 병합합니다.

```bash
aidememo --store ./main.sqlite branch merge --branch candidate-b ./shared
```

브랜치를 버린다는 것은 병합하지 않는다는 뜻입니다. 저장된 아티팩트도
제거하려면 해당 브랜치 디렉터리 또는 S3 prefix를 삭제합니다.

```text
./shared/branches/candidate-a/
s3://bucket/prefix/branches/candidate-a/
```

소스 아래의 모든 브랜치를 병합하려면 `--branch`를 생략합니다.

```bash
aidememo --store ./main.sqlite branch merge ./shared
```

## SDK와 바인딩 호출

Python composition SDK는 코드 우선 에이전트에 같은 흐름을 제공합니다.

```python
from aidememo_agent import Memory

candidate = Memory.open(store_path="./candidate-b.sqlite", storage_backend="libsqlite")
candidate.branch_push(
    "candidate-b",
    "./shared",
    base="./shared/backup-01...",
)

main = Memory.open(store_path="./main.sqlite", storage_backend="libsqlite")
main.branch_merge("./shared", branch="candidate-b")
```

`aidememo-python`은 `branch_push(branch, destination, base=None)`와
`branch_merge(source, branch=None)`을 제공합니다. `aidememo-napi`는 JSON
문자열 보고서를 반환하는 `branchPush`, `branchMerge`를 제공합니다.
`aidememo_nif`는 decode된 map 보고서를 반환하는
`AideMemoNif.branch_push/4`, `AideMemoNif.branch_merge/3`을 제공합니다.
로컬 경로는 이미 열린 네이티브 저장소 handle을 사용하므로 SDK 또는 plugin
코드가 같은 파일을 다시 열지 않습니다. S3 지원은 CLI의 `--features s3`
빌드가 제어하므로 S3 브랜치 URI는 CLI를 사용해야 합니다.

## 저장 구조

로컬 브랜치 로그는 다음 경로에 저장됩니다.

```text
<DEST>/branches/<branch-id>/segments/<segment-id>.jsonl
<DEST>/branches/<branch-id>/segments/<segment-id>.manifest.json
```

S3 브랜치 로그는 같은 prefix 구조를 사용하고 payload를 압축합니다.

```text
s3://bucket/prefix/branches/<branch-id>/segments/<segment-id>.jsonl.zst
s3://bucket/prefix/branches/<branch-id>/segments/<segment-id>.manifest.json
```

각 segment manifest는 저장된 object와 decode된 JSONL payload 모두의 byte
수와 SHA-256 checksum을 기록합니다. Merge는 import 전에 이를 검증합니다.

## 보장과 한계

현재 보장하는 항목:

- `branch push --base <BACKUP>`은 백업 manifest의 sync cursor 이후에 기록된
  레코드를 내보냅니다.
- `branch merge --branch <ID>`는 해당 브랜치만 가져옵니다.
- 같은 segment를 다시 병합해도 `sync_import`를 거치므로 팩트가 중복되지
  않습니다.
- 선택한 모든 segment는 원래 LWW 순서를 유지합니다. Import 뒤 merge로 실제
  변경된 record에는 coordinator relay timestamp 하나를 부여하므로 이미 진행된
  downstream sync cursor도 과거 ID의 변경을 관찰합니다.
- Relay timestamp는 entity/fact record에 적용합니다. Relation insert 또는
  replacement는 전체 relation snapshot generation을 바꾸므로 이미 진행된
  downstream도 relation snapshot을 다시 실행합니다.
- S3는 브랜치 아티팩트 전송 수단이며 live database 백엔드가 아닙니다.

아직 보장하지 않는 항목:

- Candidate의 자동 품질 평가
- 경쟁하는 결정 사이의 semantic 충돌 해결
- 일급 `branch delete` 명령
- 실행 중인 저장소 사이의 양방향 live replication

현재 검증 근거는 [`측정 원장`](MEASUREMENTS.md)의 "Branch Log Push / Merge"를
참고하세요.
