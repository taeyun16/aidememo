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
aidememo --backend libsqlite mcp-install --target codex --source-id team-a
```

## 로컬 저장소 쓰기 경합 방지

기본 SQLite 백엔드는 짧은 쓰기 충돌의 busy timeout으로
`store.lock_retry_ms`를 사용합니다. 선택형 redb 백엔드도 다른 프로세스가
redb의 독점 파일 lock을 가진 경우 같은 설정으로 저장소 열기를 재시도합니다.

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

Daemon 저장소에서 `search.auto_hybrid=true`이면 `aidememo mcp-serve` 시작
시 semantic provider를 사전 워밍해 첫 사용자 쿼리가 아니라 startup이 모델
로드 비용을 지불합니다. 설정을 바꾸지 않고 사전 워밍하려면
`AIDEMEMO_PREWARM_SEMANTIC=1`로 시작합니다.

### Liquid AI LFM 모델 실험

LFM 모델은 AideMemo에 번들된 자산이 아니라 선택적 외부 모델로 취급합니다.
LFM Open License v1.0에는 상업적 사용 조건이 있으며, LFM 모델 계열마다
시스템에서 적합한 위치가 다릅니다.

LFM 계열을 하나의 전역 모델 전환이 아니라 계층형 소형 모델 스택으로
사용합니다.

| AideMemo surface | 첫 LFM 후보 | 사용할 때 | 사용하지 않을 때 |
|---|---|---|---|
| 1단계 semantic 검색 | `mlx-community/LFM2.5-Embedding-350M-4bit` | BM25가 약하거나 빈 후보를 반환할 때, 특히 강한 lexical anchor가 없는 다국어 쿼리에 사용합니다. 추적 문서 gate에서는 lexical 실패 구간만 승격해 `R@8`이 0.656에서 0.812로 향상됐고, HF agent-trace gate에서는 BM25가 이미 강할 때 순수 LFM dense가 더 낮은 성능을 보였습니다. `search.auto_hybrid=true`를 유지하고 반복 호출에는 warm daemon을 사용합니다. | 일반 lexical/code/doc 검색이 이미 포화됐거나 모든 쿼리에 LFM dense를 실행하게 될 때입니다. |
| BERT 계열 semantic baseline | `fastembed` / `bge-small-en-v1.5` | 더 무거운 BERT 스타일 encoder를 허용할 수 있는 offline 또는 high-stakes 비교에 사용합니다. HF 60-doc 구간에서 `R@8=0.994`를 기록해 BM25/BGE head 품질에 근접했으며 검증 baseline으로 유용합니다. | Agent hot path에는 사용하지 않습니다. 측정된 daemon query 평균은 쿼리당 약 1680 ms로, LFM query embedding 약 31 ms와 BM25/model2vec 경로보다 훨씬 느렸습니다. |
| Reranking | `mlx-community/LFM2.5-ColBERT-350M-4bit` 또는 `LiquidAI/LFM2.5-ColBERT-350M` sidecar | BM25/hybrid 후보 recall은 높지만 최상위 결과의 순서가 자주 잘못될 때 사용합니다. | 의도적으로 all-doc/multi-vector ColBERT index를 만들지 않는 한 올바른 팩트가 후보 집합에 없을 때는 사용하지 않습니다. |
| 팩트 추출/타입 분류 | fact-type LoRA adapter를 적용한 `LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit` | 잔여/default-note 사례나 pending review에 사용할 local high-confidence `fact_type_hint` 후보가 필요할 때 사용합니다. Corpus-only 240-iteration LoRA는 seed coding-agent corpus test와 이전 45-case holdout 모두에서 deterministic inference를 앞섰습니다. | 라벨이 없는 실제 트래픽에서 자동 고정밀 쓰기가 필요할 때는 사용하지 않습니다. 먼저 검토된 shadow corpus를 늘리고 검증합니다. |
| 쿼리 라우팅 | deterministic rules + search confidence | BM25-only, dense, ColBERT, aggregate 중 하나를 선택해야 할 때 사용합니다. | 아직 여기에 LFM text-generation 호출 비용을 쓰지 않습니다. MLX LM router micro-eval은 규칙보다 약했습니다. |
| Batch consolidation/conflict review | review hint만 사용하며 아직 자동 LFM 배치는 없음 | 사람이거나 더 강한 agent가 duplicate/supersede 제안을 검토할 때 사용합니다. | 검증된 LFM text-generation 모델이 supersede/archive 결정을 자동으로 쓰게 하지 않습니다. |
| 이미지/스크린샷 메모리 캡처 | 더 안정적인 `mlx-vlm` 호환성을 확보할 때까지 보류 | 스크린샷이나 diagram을 실험하며 review-only 출력을 허용할 수 있을 때 사용합니다. | 현재 VL smoke의 JSON/OCR 출력은 불안정하므로 screenshot-to-fact 캡처를 출시하지 않습니다. |
| 음성/회의 캡처 | 아직 Mac MLX 배치 없음 | 향후 local audio runtime에 WER와 fact-precision 측정값이 생겼을 때 사용합니다. | 기존 wiki의 text retrieval만 필요할 때는 사용하지 않습니다. |

검증해야 할 제품 차별점은 이것입니다. 모든 메모리 작업을 하나의 대형 원격
모델에 의존하는 대신, 소형 로컬 모델을 정확히 사용한 실패 지점에서 메모리
시스템이 개선된다는 점을 AideMemo가 입증할 수 있어야 합니다.

Dense 다국어 검색 실험에는 `LiquidAI/LFM2.5-Embedding-350M`을 사용합니다.
비대칭 embedding 모델이므로 vector를 다시 만들기 전에 query와 document
prefix를 설정합니다.

```bash
aidememo config set model.provider openai
aidememo config set model.endpoint http://127.0.0.1:8080/v1/embeddings
aidememo config set model.name LiquidAI/LFM2.5-Embedding-350M
aidememo config set model.dimension 1024
aidememo config set model.query_prefix "query: "
aidememo config set model.document_prefix "document: "
aidememo vector-rebuild --current-only
aidememo search "redis timeout root cause" --hybrid
```

이 모델을 `sentence-transformers`로 직접 실행할 때 Liquid model card는
bidirectional patch를 위해 `trust_remote_code=True`로 로드합니다. Hub code를
AideMemo 프로세스 안에서 실행하기보다, 감사하고 격리한 OpenAI-compatible
embedding endpoint 뒤에서 제공하는 방식을 권장합니다.

Production server를 연결하기 전에 dense가 workload에 도움이 되는지 확인하려면
scenario micro-eval을 실행합니다.

```bash
python3 scripts/lfm_dense_eval.py \
  --aidememo target/debug/aidememo \
  --model LiquidAI/LFM2.5-Embedding-350M
```

`embedding_health.valid`가 false이면 해당 수치를 검색 근거로 사용하지 않습니다.
안전한 `sentence-transformers` 경로는 1024차원 vector를 로드하면서도
bidirectional patch 없이 동일한 embedding을 만들 수 있습니다.

Apple Silicon의 로컬 dense 검증에는 MLX 변환본을 우선 사용합니다.

```bash
hf download mlx-community/LFM2.5-Embedding-350M-4bit \
  --local-dir /private/tmp/lfm25-embedding-mlx-4bit

python3 scripts/lfm_mlx_dense_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit
```

MLX 경로에는 Metal 접근이 필요합니다. Headless 또는 sandbox session에서
`mlx`는 `No Metal device available`로 실패할 수 있으므로 Apple GPU가 보이는
프로세스에서 실행합니다.

실제 AideMemo semantic 경로 뒤에 MLX LFM embedder를 배치하려면
TEI-compatible sidecar를 실행하고 실험용 provider alias를 설정합니다.

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_embedding_sidecar.py \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --port 8088

aidememo config set model.provider lfm-sidecar
aidememo config set model.endpoint http://127.0.0.1:8088
aidememo config set model.name mlx-community/LFM2.5-Embedding-350M-4bit
aidememo config set model.dimension 1024
aidememo config set model.query_prefix "query: "
aidememo config set model.document_prefix "document: "
aidememo vector-rebuild --current-only
aidememo daemon start
aidememo search "레디스 장애 원인" -l 8
```

이 구성은 MLX와 Liquid model code를 Rust 프로세스 밖에 두면서도 기존
`auto_hybrid` + HNSW 흐름을 사용합니다. Sidecar가 실행 중이 아니면 semantic
승격은 치명적이지 않게 실패하고 AideMemo는 BM25 probe로 폴백합니다.

`LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit`를 로컬 팩트 추출과 타입 분류
실험에 사용할 때는 pending-review helper로만 둡니다. Mac MLX micro-eval은
자동 쓰기를 지지하지 않았습니다. 관측된 최상 추출 성능은
`fact_type_accuracy=0.63`, `entity_recall=0.88`이었고 router/consolidation
정확도는 여전히 너무 낮았습니다. 230M/350M MLX 모델은 추출이 더 빠르지만
정확도가 낮았습니다. Closed-label classifier 평가도 같은 배치를 지지합니다.
`lfm_mlx_fact_type_eval.py`로 고정 AideMemo label 중 하나를 scoring해도
45-case mixed-language fixture에서 deterministic strong-cue inference보다
낮았습니다(`0.33` best LFM 대 `0.69` baseline). 다만 최상 1.2B-Instruct
실행에는 매우 작은 high-confidence review 구간이 있었습니다
(`confidence >= 0.8`: 6/6 correct). 이를 default writer가 아니라
`fact_type_hint`/pending-review 근거로 사용합니다. Fine-tuning이 이 결론을
바꾸는지 시험하려면 supervised label dataset을 만들고 MLX LoRA adapter를
학습합니다. 첫 synthetic-only smoke는 240 iteration 뒤 45-case holdout에서
`0.84`를 기록했습니다. 이후 coding-agent seed corpus에는
`fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl`을 추가했습니다
(검토된 형태의 row 108개, label당 12개, train/valid/test = 72/18/18).
Corpus-only adapter는 더 어려운 18-case test에서 deterministic baseline
`0.39` 대비 `0.61`, 기존 45-case holdout에서 baseline `0.69` 대비 `0.82`를
기록했습니다. 실제 capture traffic이 threshold와 `claim`/`convention`/`note`
경계의 안정성을 확인할 때까지 shadow `fact_type_hint` 경로로 유지합니다.

```bash
hf download LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit \
  --local-dir /private/tmp/lfm25-12b-instruct-mlx-4bit

python3 scripts/lfm_mlx_lm_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --suite extraction \
  --prompt-style compact

python3 scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --prompt-style compact \
  --summary-only

python3 scripts/lfm_fact_type_sft_data.py \
  --out /private/tmp/aidememo-lfm-fact-type-corpus-sft

/private/tmp/aidememo-lfm-venv/bin/mlx_lm.lora \
  --model /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --train \
  --data /private/tmp/aidememo-lfm-fact-type-corpus-sft \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --fine-tune-type lora \
  --mask-prompt \
  --num-layers 8 \
  --batch-size 1 \
  --grad-accumulation-steps 8 \
  --iters 240 \
  --learning-rate 5e-5

python3 scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --cases-file fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl \
  --case-split test \
  --prompt-style compact \
  --summary-only
```

지속적인 shadow loop에서는 seed fixture를 직접 수정하지 말고, agent가 라벨을
붙이고 검토된 팩트를 JSONL로 추가합니다.

```json
{"id":"agent-run-20260707-001","text":"Tried the daemon prewarm path, but the real issue was a stale HNSW sidecar.","fact_type":"lesson","scenario":"daemon_ops","language":"en","split":"train"}
```

실시간 MCP agent에서는 저장되는 팩트를 바꾸지 않으면서 성공한 쓰기를
supervised shadow corpus에 추가하게 합니다.

```bash
export AIDEMEMO_FACT_TYPE_SHADOW_LOG=~/.aidememo/fact-type-shadow.jsonl
aidememo mcp-serve --port 3000
```

Row는 append-only JSONL이며 저장된 fact id, text, fact_type,
`label_source`(`explicit`, `inferred`, `default` 중 하나), entities, source_id,
origin(`mcp_fact_add`/`mcp_fact_add_many`), timestamp를 포함합니다. 명시적으로
agent가 라벨을 붙인 row를 supervised training에 사용합니다. Inferred/default
row는 audit에 유용하지만 `--include-inferred-labels`를 전달하지 않으면
`lfm_fact_type_sft_data.py`가 건너뜁니다. `fact_type_hint`가 있는 row도 저장된
label과 strong-cue hint가 일치하지 않으므로 기본적으로 건너뜁니다. 검토한
뒤에만 `--include-disputed-labels`를 전달합니다.

그런 다음 seed corpus와 검토된 capture file을 함께 학습합니다.

```bash
python3 scripts/lfm_fact_type_sft_data.py \
  --corpus ~/.aidememo/fact-type-shadow.jsonl \
  --out /private/tmp/aidememo-lfm-fact-type-shadow-sft
```

Tuned adapter를 hint-only sidecar로 실행합니다.

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --confidence-threshold 0.8 \
  --input-jsonl ~/.aidememo/fact-type-shadow.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-fact-type-hints.jsonl

python3 scripts/lfm_fact_type_threshold_eval.py \
  --labels-jsonl ~/.aidememo/fact-type-shadow.jsonl \
  --label-split test \
  --predictions-jsonl /private/tmp/aidememo-fact-type-hints.jsonl \
  --min-precision 0.95 \
  --max-baseline-correct-harms 0
```

외부 LLM 호출 없이 local agent behavior log에서 같은 sidecar를 stress-test하려면
AgentStep/Hermes log에서 weak-labelled probe를 만들고 같은 threshold gate를
실행합니다.

```bash
python3 scripts/lfm_fact_type_log_fixture.py \
  --out-dir /private/tmp/aidememo-lfm-log-probes \
  --max-rows 72 \
  --max-per-label 12
```

더 넓은 public-trace gate를 실행하려면 Hugging Face agent trace에서
weak-labelled probe를 만듭니다. 이 과정은 Dataset Viewer API로 row를 가져오고,
email과 긴 identifier를 redact하며, `/private/tmp` 아래에 compact candidate-memory
JSONL만 기록합니다.

```bash
python3 scripts/lfm_fact_type_hf_probe.py \
  --out-dir /private/tmp/aidememo-lfm-hf-probes \
  --source-rows 100 \
  --max-rows-per-dataset 100 \
  --max-per-label 25

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --input-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-lfm-hf-probes/fact_type_hints_hf.jsonl

python3 scripts/lfm_fact_type_threshold_eval.py \
  --labels-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --label-split test \
  --predictions-jsonl /private/tmp/aidememo-lfm-hf-probes/fact_type_hints_hf.jsonl \
  --confidence-grid 0.80,0.90,0.95,0.98
```

이를 검토된 정답이 아니라 raw-trace stress test로 취급합니다. 7월 8일 public
HF 실행에서는 raw trace event에 `confidence >= 0.80`이 너무 느슨했지만,
deterministic extraction으로 stream을 durable memory-candidate category까지
먼저 걸러낸 뒤에는 high-confidence hint가 유용해졌습니다. Script는 이 두 번째
gate를 위한 `high_signal_hf_fact_type_probe.jsonl`도 기록합니다.

같은 HF trace에서 검색 배치를 시험하려면 compact probe row를 corpus/query/qrels
file로 변환한 뒤 BM25, model2vec, LFM dense, 선택적 fastembed/BGE profile을
실행합니다.

```bash
python3 scripts/lfm_hf_agent_trace_retrieval_fixture.py \
  --probe-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --out-dir /private/tmp/aidememo-lfm-hf-retrieval \
  --variants surface,paraphrase,cjk \
  --max-docs-per-source 60

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_docs_recall_eval.py \
  --aidememo target/debug/aidememo \
  --no-default-docs \
  --corpus-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_corpus.jsonl \
  --queries-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_queries.jsonl \
  --qrels-tsv /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_qrels.tsv \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --candidate-limit 8 \
  --summary-only \
  --output-json /private/tmp/aidememo-lfm-hf-retrieval/eval_summary.json
```

BGE/BERT 계열 fastembed baseline을 평가할 때는 optional feature로 CLI를
build하고, offline batch가 아니라면 더 작은 구간을 사용합니다.

```bash
cargo build -p aidememo-cli --features fastembed

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_docs_recall_eval.py \
  --aidememo target/debug/aidememo \
  --no-default-docs \
  --corpus-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_corpus.jsonl \
  --queries-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_queries.jsonl \
  --qrels-tsv /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_qrels.tsv \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --fastembed-model bge-small-en-v1.5 \
  --candidate-limit 8 \
  --summary-only
```

일회성 manual hint에는 text를 직접 전달합니다.

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --confidence-threshold 0.8 \
  --text "Tried daemon prewarm, but the real issue was a stale HNSW sidecar."
```

Sidecar는 `suggested_fact_type`, confidence, margin, runner-up, `accepted`를
반환하며 AideMemo에 쓰지 않습니다. 먼저 pending review나 UI surface에
연결합니다. 더 큰 reviewed capture corpus가 낮은 baseline-correct harm을
입증하기 전에는 명시적인 agent `fact_type`을 덮어쓰게 하지 않습니다.

Synthetic augmentation은 실험으로만 사용합니다.

```bash
python3 scripts/lfm_fact_type_sft_data.py \
  --corpus /path/to/reviewed-agent-facts.jsonl \
  --examples-per-label 40 \
  --out /private/tmp/aidememo-lfm-fact-type-shadow-synth40-sft
```

7월 7일 비교에서 synthetic augmentation은 taxonomy를 편향시킬 수 있었습니다.
Mixed adapter는 corpus-test accuracy를 `0.61`로 유지했지만 이전 holdout은
`0.82`에서 `0.71`로 낮아졌고 원래의 모든 `note` 예제를 `lesson`으로
분류했습니다. Synthetic row는 training set을 늘리는 기본 방식이 아니라
stress test로 취급합니다.

Native multi-vector index를 검토하기 전에
`LiquidAI/LFM2.5-ColBERT-350M`을 sidecar reranker로 사용합니다.
`LiquidAI/LFM2-ColBERT-350M`도 `--model LiquidAI/LFM2-ColBERT-350M`으로
같은 script에서 사용할 수 있습니다. 원래 LFM2 결과를 재현할 때가 아니라면
2.5를 기본으로 유지합니다. 현재 AideMemo는 fact당 vector 하나를 저장하지만,
ColBERT는 token당 vector 하나를 저장하고 MaxSim으로 scoring합니다. Apple
Silicon의 local MaxSim 검증에는 MLX 변환본을 우선 사용합니다.

```bash
hf download mlx-community/LFM2.5-ColBERT-350M-4bit \
  --local-dir /private/tmp/lfm25-colbert-mlx-4bit

python3 scripts/lfm_mlx_colbert_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-colbert-mlx-4bit \
  --candidate-limit 8
```

```bash
aidememo search "redis timeout root cause" --json -l 50 \
  | python3 scripts/lfm_colbert_rerank.py \
      --query "redis timeout root cause" \
      --top-k 10
```

Sidecar는 기본적으로 `trust_remote_code`를 끕니다. Model repository code를
감사하고 승인한 뒤에만 `--trust-remote-code`를 전달합니다.

## 저장소 백업

AideMemo의 기본 저장소는 SQLite입니다. 실행 중인 `.sqlite` file을 직접
복사하지 말고 backup command를 사용합니다. 이 명령은 일관된 SQLite
snapshot과 byte count 및 SHA-256 checksum이 담긴 manifest를 만들고,
restore 시 target store를 교체하기 전에 manifest와 `PRAGMA integrity_check`를
검증합니다.

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

S3 경로는 live database가 아니라 backup 저장용입니다. Restore는 local SQLite
store를 교체하고 target 옆의 오래된 SQLite WAL/SHM 및 HNSW sidecar file을
제거합니다.

## 클라우드 에이전트 브랜치 공유

별도 machine에서 실행되는 agent는 모든 worker가 같은 실행 중인 SQLite file에
쓰게 하지 말고 backup snapshot에서 시작해 agent별 branch log를 push합니다.
Backup manifest는 sync cursor를 기록하며 `branch push --base`는 이 cursor를
사용해 baseline 이후에 기록된 record만 export합니다. 이는 what-if memory
실험에도 적합합니다. 하나의 backup에서 여러 candidate store를 fork하고, 각
시도가 local lesson을 기록하게 한 뒤 가장 나은 branch만 merge하고 나머지는
merge하지 않습니다.

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
idempotent `sync_import` 경로에 의존합니다. 중복 entity, fact, relation은
건너뛰고 독립적인 새 fact는 append합니다. Cloud agent나 worker마다 서로 다른
branch id를 사용하고, 경쟁 decision 사이의 semantic conflict 처리는 현재
application policy로 다룹니다. 추측 실행 workflow와 storage layout은
[`브랜치 로그`](BRANCHES.md)를 참고합니다.
