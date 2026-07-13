---
title: 운영
description: 저장소, source 범위, daemon mode, 유지 관리에 대한 실무 가이드입니다.
---

# 운영

이 페이지는 첫 빠른 시작 뒤 사용자가 일반적으로 결정해야 하는 운영 항목을
다룹니다.

## 기본 로컬 전용 경로

AideMemo의 기본 경로는 외부 LLM API를 호출하지 않습니다. 호출하는 에이전트는
LLM일 수 있지만 AideMemo 자체는 로컬 결정적 코드와 선택형 로컬 임베딩
사이드카를 통해 메모리를 저장, 검색, 제공합니다.

| 표면 | 기본 동작 | 외부 LLM 호출? | 로컬 모델 로드? | 선택형 업그레이드 |
|---|---|---:|---:|---|
| 저장소 | 로컬 BM25와 그래프 인덱스를 가진 SQLite 파일 | 아니오 | 아니오 | redb 백엔드, S3 백업/브랜치 전송 |
| `fact add` / MCP 쓰기 | 명시적 `fact_type`을 보존하고, 생략 시 결정적 strong-cue 추론 후 나머지는 `note`로 저장 | 아니오 | 아니오 | shadow `fact_type_hint` 로그와 검토된 LFM LoRA 사이드카 |
| 프라이버시 guard | 설정 전까지 비활성화하며 기본 경로에서 PII 모델을 로드하지 않음 | 아니오 | 아니오 | 저장 전 로컬 OpenAI Privacy Filter 사이드카 |
| 검색 | BM25를 먼저 확인하고 `search.auto_hybrid=true`가 준비된 semantic 사이드카에서 약한 어휘 검색만 승격 | 아니오 | 확신도 높은 BM25 또는 사이드카 없음에서는 아니오 | `--hybrid` 강제, MLX LFM 임베딩 사이드카, fastembed/BGE 평가 경로 |
| Daemon / MCP 서버 | 반복 에이전트 호출에 하나의 웜 로컬 프로세스 재사용 | 아니오 | semantic 설정이 활성화되고 준비된 경우에만 사전 워밍 | `AIDEMEMO_PREWARM_SEMANTIC=1`, 로컬 네트워크의 원격 TEI 호환 서비스 |
| 추출 | heuristic/로컬 캡처만 사용 | 아니오 | 아니오 | `extract.provider=openai`는 명시적 선택 |
| 재랭킹 | 비활성화 | 아니오 | 아니오 | TEI/ColBERT/BGE 재랭킹 사이드카 |
| SDK / 바인딩 | 같은 Rust 코어를 process 내부 또는 CLI 폴백으로 사용 | 아니오 | 선택한 검색 경로를 따름 | `fact_add` 전 에이전트별 캡처 정책 |

제품 경계는 명확합니다. 기본 메모리 루프는 토큰 비용이 없고 vendor에
독립적이며, 작은 로컬 모델은 BM25 또는 결정적 캡처가 약하다고 확인된 위치에만
측정된 사이드카로 배치합니다.

## 쓰기 시점 프라이버시 필터 사용

OpenAI Privacy Filter의
[모델](https://huggingface.co/openai/privacy-filter),
[소스](https://github.com/openai/privacy-filter),
[모델 카드](https://cdn.openai.com/pdf/c66281ed-b638-456a-8ce1-97e9f5264a90/OpenAI-Privacy-Filter-Model-Card.pdf)는
저장 전 선택형 guard로 실행할 수 있습니다. 생성형 LLM이 아니라 양방향 token
classification 모델이며 on-prem PII 탐지와 masking 워크플로를 위한
모델입니다. 공식 모델 카드는 여덟 span category를 정의합니다.
`account_number`, `private_address`, `private_email`, `private_person`,
`private_phone`, `private_url`, `private_date`, `secret`입니다.

이를 모든 anonymization 또는 compliance를 보장하는 기능으로 취급하지
마세요. AideMemo는 기본적으로 guard를 비활성화하고, 명시적으로 활성화한 뒤
사이드카에 연결할 수 없으면 fail closed합니다.

AideMemo는 `AideMemo::add_fact` / `fact_add_many` 안에서 guard를 적용하므로
CLI, MCP, extract-apply, pending approve, 코어 쓰기 API를 호출하는 네이티브
바인딩이 같은 저장 전 동작을 공유합니다.

사이드카 정책 전에는 일반 API key prefix(`sk-proj-`, `sk-`, `github_pat_`,
`ghp_`, `gho_`, `xoxb-`, `AKIA`, `AIza`)를 찾는 결정적 로컬 secret
prefilter도 실행합니다. Bare key 형태의 토큰이 `secret` 대신
`private_person`으로 분류될 수 있는 관찰된 OPF 실패를 보완합니다.

| 쓰기 표면 | 배치 위치 | 먼저 검증할 기본 정책 |
|---|---|---|
| CLI `aidememo fact add` | `FactInput` 저장 전; daemon 경로는 웜 로컬 필터 프로세스 재사용 | 먼저 `report`, 이후 확신도 높은 이름 외 span에 `redact` |
| MCP `aidememo_fact_add` / `aidememo_fact_add_many` | 단일/배치 쓰기가 일치하도록 batch insert 전 공유 helper | 배치 `redact`; 탐지 label은 로그에 남기고 원본 span은 저장하지 않음 |
| `aidememo extract --apply`와 pending approve | 추출 후 승인 쓰기 전 candidate 콘텐츠 필터링 | `secret` 차단; email/phone/address/account/url/date redact; 사람 이름 검토 |
| Markdown ingest | 가져온 note와 log를 위한 선택형 프로젝트 guard | 프로젝트가 파괴적 redaction을 명시적으로 선택하지 않으면 `report` mode |

로컬 사이드카 실행:

```bash
python3 -m pip install git+https://github.com/openai/privacy-filter.git
python3 scripts/privacy_filter_sidecar.py --device cpu --port 8090
```

Apple Silicon에서는 측정상 지연이 더 낮은 MLX mxfp4 변환을 사용합니다.

```bash
python3 -m pip install git+https://github.com/Blaizzy/mlx-embeddings.git
hf download mlx-community/openai-privacy-filter-mxfp4 \
  --local-dir /private/tmp/openai-privacy-filter-mlx-mxfp4
python3 scripts/privacy_filter_mlx_sidecar.py \
  --model-dir /private/tmp/openai-privacy-filter-mlx-mxfp4 \
  --port 8091
```

쓰기 guard 활성화:

```bash
aidememo config set privacy.provider openai-privacy-filter
aidememo config set privacy.endpoint http://127.0.0.1:8090  # or the MLX sidecar port
aidememo config set privacy.mode redact
```

동일한 설정 형태:

```toml
[privacy]
provider = "openai-privacy-filter"   # empty disables the guard
mode = "report"                      # report | redact | block
endpoint = "http://127.0.0.1:8090"   # local sidecar; no remote inference by default
api_key_env = ""
block_labels = ["secret"]
redact_labels = ["private_email", "private_phone", "private_address", "account_number", "private_url", "private_date"]
review_labels = ["private_person"]
store_summary = true                 # reserved for label/count summaries; raw spans are never stored
```

첫 자동 redaction 단계에서 `private_person`을 분리하세요. 프로젝트 메모리에서
이름은 올바른 엔티티 key일 수 있지만 개인/팀 로그에서는 민감 정보일 수
있습니다. 모델 정확도가 아니라 정책 결정으로 다룹니다.

Checkpoint가 웜 상태인 macOS arm64 로컬 CPU 비용은 sidecar `/filter` p50 약
244ms, guard를 사용한 `aidememo fact add` p50 약 261ms, 필터 없는 경우 p50
약 17ms였습니다. 웜 sidecar RSS는 약 3.8GB였습니다. 모든 scratch-memory
캡처가 아니라 안전에 민감한 쓰기 또는 팀/프로젝트 저장소에서 선택적으로
사용합니다.

Apple Silicon에서는 런타임이 준비되면 MLX mxfp4 사이드카를 권장합니다.
`mlx-community/openai-privacy-filter-mxfp4`는 디스크 약 739MB, 웜 RSS
1.28GB, sidecar `/filter` p50 약 18ms, `aidememo fact add` p50 약 51ms로
측정됐고 baseline은 22.5ms였습니다. 측정된 MLX 경로는 PyPI 릴리스가
따라오기 전까지 GitHub main의 `mlx-embeddings` 0.1.1이 필요합니다. MLX stream
state가 thread-local이므로 사이드카는 single-thread로 실행해야 합니다.
공유/프로젝트 저장소의 강한 사전 워밍 선택지지만 보편적 기본값은 아닙니다.

쓰기에 사용하기 전 평가 게이트:

1. Synthetic 예제, 로컬 에이전트 trace, redacted 공개 trace로 예상 span과
   label을 가진 fixture를 만듭니다.
2. `secret`, email, phone, address, URL, account/date, person-name의 span
   recall을 따로 측정합니다.
3. Redaction 전후 entity recall, fact-type accuracy, retrieval R@K,
   answerability로 utility loss를 추적합니다.
4. Strict mode에서 raw secret 저장이 0인지 요구하고 승격 전 모든 false
   negative를 검사합니다.
5. 소스를 공식 OpenAI repository/model handle로 고정하고 모델 카드를 모방한
   typosquat repository를 피합니다.

## 저장소 레이아웃 선택

| 레이아웃 | 사용 시점 |
|---|---|
| 로컬 기본 저장소 하나 | 한 머신의 개인 메모리 |
| 프로젝트별 저장소 | 저장소 간 메모리를 공유하지 않아야 함 |
| 공유 팀 저장소 하나 | 여러 로컬 에이전트가 컨텍스트를 공유해야 함 |
| 한 저장소 안의 `source_id` | 팀, 에이전트, tenant가 인프라를 공유하지만 범위가 지정된 검색이 필요 |

스크립트와 CI에서는 명시적 저장소 경로를 권장합니다.

```bash
aidememo --store ./project.sqlite query "release checklist"
```

파일 suffix를 백엔드와 일치시키세요(SQLite / `libsqlite`는 `.sqlite`, redb는
`.redb`). Storage engine이 요구하지는 않지만 `store.backend`와 확장자가 다른
지속성 계층을 가리키면 이후 오해하기 쉬우므로 `aidememo doctor`가 경고합니다.

## Source 범위 사용

`source_id`는 이웃 프로젝트 또는 팀의 팩트가 쿼리에 섞이는 것을 막습니다.

```bash
aidememo fact add \
  "Decision: Team A deploys through release train alpha." \
  --type decision \
  --entities Release \
  --source-id team-a

aidememo query "release train" --source-id team-a
```

MCP 설치:

```bash
aidememo --backend libsqlite --store ~/.aidememo/team.sqlite \
  mcp-install --target codex --source-id team-a
```

같은 경계가 fact get/list/mutation, pinned context, entity, graph read에도
적용됩니다. 정확히 같은 content의 중복 제거 key는
`(source_id, content_hash)`이므로, 두 source에 있는 같은 문장은 독립된 두
fact로 유지됩니다. Relation은 별도의 소유 source namespace를 가지며, source
범위가 있는 traversal/path는 namespace가 정확히 같은 relation만 반환합니다.
기존의 범위 없는 relation을 모든 source가 상속하지 않습니다. Network client가
자기 scope를 선택하면 안 되는 경우 `mcp-serve --auth-bindings-file`을 사용하세요.
자세한 내용은 [MCP 설정](MCP.md)을 참고합니다.

이 경계는 상호 적대적인 tenant boundary가 아니라 신뢰된 팀 내부 partition으로
사용하세요. Entity name/type은 의도적으로 공유 ontology이며 native/CLI 호출자는
자기 `source_id`를 선택할 수 있습니다. 상호 신뢰하지 않는 tenant는 별도 store를
사용해야 합니다. Reference 배포 형태와 production checklist는
[`공용 메모리 레이어`](SHARED_MEMORY.md)를 참고하세요.

## 로컬 저장소 쓰기 경합 방지

기본 SQLite 백엔드는 WAL mode를 사용하고 `BEGIN IMMEDIATE`로 쓰기를 시작합니다.
각 SQLite busy wait은 최대 1초로 제한하며, 전체 `store.lock_retry_ms` budget이
소진될 때까지 20–150 ms jitter를 두고 충돌을 재시도합니다. 선택형 redb
백엔드도 다른 프로세스가 redb의 독점 파일 lock을 가진 경우 같은 전체
budget으로 저장소 열기를 재시도합니다.

공유 쓰기에는 하나의 daemon을 실행합니다.

```bash
aidememo --backend libsqlite daemon start --store ~/.aidememo/team.sqlite --port 3000
```

Daemon 자동 발견은 백엔드를 인식합니다. 같은 경로에 `redb`로 시작한 daemon은
`sqlite` / `libsqlite`로 설정된 CLI 호출에 재사용되지 않으며 반대도 같습니다.

짧은 로컬 경합에는 대기 budget을 설정합니다.

```bash
aidememo config set store.lock_retry_ms 5000
```

## 유용한 메모리 유지

상태 검사를 실행합니다.

```bash
aidememo doctor
aidememo lint
```

오래된 메모리를 archive 또는 consolidate합니다.

```bash
aidememo fact archive --older-than 90d --type note
aidememo consolidate --semantic-threshold 0.85 --dry-run
```

큰 consolidation 뒤에는 현재 vector를 재구축합니다.

```bash
aidememo vector-rebuild --current-only
```

## 임베딩 mode 선택

기본 경로는 대부분의 코드와 문서 워크플로에 적합합니다. 결정적 hook, demo,
CI 검사에는 `--bm25-only`를 사용합니다.

```bash
aidememo workflow start "Release smoke ticket" --bm25-only
```

질문과 저장된 팩트의 표현이 다를 수 있고 모든 쿼리에서 semantic 비용을
지불하려면 semantic/hybrid 검색을 강제합니다.

```bash
aidememo search "favorite camera setup" --hybrid
```

Auto-hybrid가 기본 검색 정책입니다. AideMemo는 먼저 BM25를 실행하고 결과가
없거나 top score가 약하거나 쿼리가 CJK이며 BM25 근거가 강하지 않을 때
semantic 검색으로 승격합니다. 저장소별 평가가 더 나은 cutoff를 보이지 않으면
기본 threshold를 유지합니다.

```bash
aidememo config set search.auto_hybrid true
aidememo config set search.auto_hybrid_min_bm25_hits 1
aidememo config set search.auto_hybrid_min_top_score 1.0
```

결정적 demo, hook, CI 검사 또는 surface-form BM25가 이미 포화된 저장소에는
`--bm25-only` 또는 `search.auto_hybrid=false`를 사용합니다.

```bash
aidememo search "Redis timeout" --bm25-only
aidememo config set search.auto_hybrid false
```

기본 HNSW semantic index에서 HNSW 사이드카가 없으면 auto-hybrid는 embedding
provider를 cold-load하지 않고 `aidememo vector-rebuild`가 사이드카를 만들
때까지 BM25를 유지합니다. 사이드카가 있는 새 CLI 프로세스에서 승격된 약한
쿼리/CJK 쿼리는 embedding model cold load 비용을 지불합니다. Semantic
승격에 실패하면 BM25로 폴백합니다. 반복 에이전트 호출은 daemon을 통해
실행해 모델을 웜 상태로 유지합니다.

Daemon 저장소에서 `search.auto_hybrid=true`이면 `aidememo mcp-serve`가 listener를
bind한 뒤 background task에서 semantic provider와 설정된 HNSW index를 사전
워밍합니다. 따라서 health check와 lexical request는 model cold load를 기다리지
않고 사용할 수 있습니다. Provider 생성은 동시에 들어온 첫 semantic request와
직렬화되므로 background task와 request path가 provider를 중복 생성하지 않습니다.
`/admin/status`의 `semantic_prewarm`은 `warming`, `ready`, `failed`, `disabled` 중
하나입니다. 사전 워밍이 실패해도 server는 중단되지 않으며, 이후 semantic
request가 일반 on-demand path에서 다시 시도합니다. 설정을 바꾸지 않고 사전
워밍하려면 `AIDEMEMO_PREWARM_SEMANTIC=1`로 시작합니다.

### 선택적 Liquid AI LFM 실험

LFM은 번들 자산이나 전역 임베딩 기본값이 아닌 선택적 외부 모델 경로입니다.
1단계 검색은 BM25 우선 auto-hybrid gate 뒤에 두고, fact-type 출력은 검토
전용 shadow hint로 취급하며, reranking은 후보 recall이 이미 높을 때만
사용합니다.

모델 배치, sidecar 설정, 학습, 평가 절차는
[Liquid AI LFM 실험](LFM_EXPERIMENTS.md)을 참고합니다. 간결한 주장 요약은
[근거](EVIDENCE.md#모델-배치)에 유지합니다.

## 저장소 백업

AideMemo의 기본 저장소는 SQLite입니다. 실행 중인 `.sqlite` file을 직접
복사하지 말고 backup command를 사용합니다. 이 명령은 일관된 SQLite
snapshot과 byte count 및 SHA-256 checksum이 담긴 manifest를 만들고,
restore 시 target store를 교체하기 전에 manifest와 `PRAGMA integrity_check`를
검증합니다. `<store>.cold.sqlite`가 있으면 함께 snapshot하고 같은 manifest에
별도의 size와 checksum을 기록합니다. Local backup은 `wiki.cold.sqlite`, S3
backup은 `wiki.cold.sqlite.zst`로 저장합니다. archive 이동이 hot-then-cold
snapshot 구간과 겹치면 cold copy를 우선하고, hot snapshot 및 FTS/entity
index에서 중복 FactId를 제거합니다. 또한 더 늦은 hot metadata snapshot에서
cold fact가 필요로 하는 entity/name mapping만 복사하며, 해결되지 않은 cross-tier
entity reference가 있으면 orphan을 복원하지 않고 backup을 실패시킵니다.
Backup cursor는 snapshot sync export를 빈 page까지 모두 소진한 뒤에만 기록하므로
`branch push --base`가 baseline record를 다시 내보내지 않고 완전한 baseline
이후부터 시작합니다.

```bash
aidememo --store ~/.aidememo/wiki.sqlite backup create ~/backups/aidememo
aidememo --store ~/.aidememo/wiki.sqlite backup restore ~/backups/aidememo/backup-01... --force
```

S3 backup/restore는 optional build feature입니다. `--features s3`로 CLI를
build한 뒤 S3 prefix를 destination 또는 source로 사용합니다.

```bash
aidememo --store ~/.aidememo/wiki.sqlite backup create s3://my-bucket/aidememo
aidememo --store ~/.aidememo/wiki.sqlite backup restore s3://my-bucket/aidememo/backup-01... --force
```

S3 경로는 live database가 아니라 backup 저장용입니다. Restore는 offline
maintenance 작업이므로 먼저 `aidememo daemon`과 target store를 사용하는 모든
process를 중지해야 합니다. Restore는 기존 hot/cold SQLite store를 checkpoint한
뒤 완전한 `<store>.restore-prev...` safety snapshot을 만들고 나서 WAL/SHM 및
HNSW sidecar를 제거하며, checkpoint가 busy이면 restore를 거부합니다. Tier
target을 건드리기 전에 manifest에 기록된 모든 payload를 checksum 검증, decode,
SQLite integrity-check합니다. Tier 설치가 실패하면 이전 snapshot 두 개를 모두
rollback하고 integrity-check하며, rollback 실패도 원래 restore 오류와 함께
보고합니다. 기존 hot-only backup은
target의 cold tier를 물려받지 않습니다. `--force` restore는
기존 cold file을 `<store>.restore-prev.cold.sqlite` safety snapshot으로 보존하며,
`--force`가 없으면 hot store나 cold tier 중 하나만 존재해도 restore를
중단합니다. Manifest payload 이름은 선택한 backup prefix 바로 아래의 단순한
파일명이어야 하고 hot/cold object가 달라야 하며, manifest backend는
SQLite-compatible이어야 합니다.

## 복제 저장소를 안전하게 pull

`aidememo sync pull`은 entity, fact, relation에 독립된 high-water mark를
유지합니다. Entity와 fact update stream은 `(updated_at, id)`로 pagination하여
같은 millisecond에 발생한 mutation이 page 사이에서 누락되지 않습니다. 기존
timestamp-only cursor는 boundary를 한 번 재실행합니다. Relation sync는 정렬된
전체 relation snapshot을 `relation_generation`으로 fingerprint하고, snapshot이
변하지 않았을 때 `relation_scan_key`로 이어서 스캔합니다. Fingerprint는 각
relation record 전체의 직렬화 결과를 포함하므로 과거 timestamp의 늦은 insert나
weight, evidence, scope 또는 다른 payload 변경은 generation을 바꾸어 idempotent
full scan을 재시작합니다. 업그레이드 전 client를 위해 기존
`(created_at, relation_key)` field도 fallback으로 유지합니다.

전체 JSONL envelope(supported leading header, 유효한 record shape, 하나의 유효한
trailing cursor)를 첫 write 전에 검증하므로 잘못되거나 지원하지 않는
batch는 record를 하나도 적용하지 않습니다. Store-level 오류는 이미 적용된
record를 남길 수 있지만 upsert가 idempotent하고 cursor를 노출하지 않아 전체
batch를 안전하게 재시도할 수 있습니다. CLI는 pull/import/persist 전체에 걸쳐
같은 directory의 cursor lock을 유지하고, 고유한 temporary cursor file을 atomic
rename하기 전에 `fsync`합니다. 따라서 동시 pull process가 서로의 cursor를
덮어쓰거나 부분적으로 기록된 cursor를 저장하지 못합니다.

이 구조는 여전히 canonical-writer, pull-only replication model입니다. Relation
record는 append/upsert만 복제하며 relation delete는 전파하지 않습니다. Delete도
snapshot generation을 바꾸어 남은 record를 다시 실행하지만, wire에 downstream
copy를 제거할 tombstone이 없습니다. 모든 write/delete를 canonical shared
`mcp-serve`로 보낸 뒤, local cache를 peer로 다루지 말고 canonical state를 read
cache로 pull하세요.

## 클라우드 에이전트 브랜치 공유

별도 machine에서 실행되는 agent는 모든 worker가 같은 실행 중인 SQLite file에
쓰게 하지 말고 backup snapshot에서 시작해 agent별 branch log를 push합니다.
Backup manifest는 sync cursor를 기록하며 `branch push --base`는 이 cursor를
사용해 baseline 이후에 기록된 record만 export합니다. 이는 what-if memory
실험에도 적합합니다. 하나의 backup에서 여러 candidate store를 fork하고, 각
시도가 local lesson을 기록하게 한 뒤 가장 나은 branch만 merge하고 나머지는
merge하지 않습니다.
선택한 모든 segment를 원래 LWW 순서대로 merge한 뒤, 변경된 과거 entity/fact
ID에는 coordinator relay timestamp 하나를 부여합니다. 따라서 원래 ID를 이미
지난 downstream cursor도 merge 결과를 다시 pull합니다.
Relation에는 이 timestamp가 필요하지 않습니다. Relation이 insert 또는 replace되면
`relation_generation`이 바뀌어 downstream이 relation snapshot을 다시 실행합니다.

```bash
# Coordinator creates a baseline snapshot.
aidememo --store ./coordinator.sqlite backup create ./shared

# Agent restores the baseline, writes local memory, then pushes a branch segment.
aidememo --store ./agent-a.sqlite backup restore ./shared/backup-01... --force
aidememo --store ./agent-a.sqlite fact add "Agent A learned X" --entities AgentA --type lesson
aidememo --store ./agent-a.sqlite branch push \
  --branch agent-a \
  --base ./shared/backup-01... \
  ./shared

# Coordinator merges one branch, or omit --branch to merge every branch under SOURCE.
aidememo --store ./coordinator.sqlite branch merge --branch agent-a ./shared
```

`--features s3`를 사용하면 같은 command의 backup과 branch-log 위치에
`s3://bucket/prefix`를 전달할 수 있습니다. S3 branch payload는 byte count와
SHA-256 checksum manifest를 가진 zstd-compressed JSONL segment입니다.

이는 완전한 multi-master conflict resolution이 아닙니다. 현재 merge는 기존의
idempotent `sync_import` 경로에 의존합니다. 동일하거나 오래된 entity/fact
record는 건너뛰고, 같은 ID의 더 새로운 record는 LWW 순서로 적용하며, relation
identity는 insert 또는 replace하고 독립적인 새 fact는 append합니다. Cloud
agent나 worker마다 서로 다른 branch id를 사용하고, 경쟁 decision 사이의
semantic conflict 처리는 현재 application policy로 다룹니다. 추측 실행 workflow와 storage layout은
[`브랜치 로그`](BRANCHES.md)를 참고합니다.
