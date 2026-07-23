---
title: 검증 근거
description: AideMemo의 검증된 동작, 모델 배치, 성능 주장 경계를 요약합니다.
---

# 검증 근거

AideMemo는 로컬 검색 우선 메모리 루프를 중심으로 설계됐습니다. 기본 메모리
경로는 캡처, 타입 지정 쓰기, BM25 우선 검색, MCP 또는 에이전트 SDK 읽기를
포함하며 외부 LLM 호출이 필요하지 않습니다. 원격 추출, 임베딩, 재랭킹,
reader 모델은 선택 기능입니다.

이 페이지는 현재 제품 기본값을 결정하는 결과를 요약합니다. 명령, 픽스처,
주의점, 이전 실행은 전체 [측정 원장](MEASUREMENTS.md)을 참고하세요.

## 검증된 결과

| 검증 표면과 결과 | 의미 |
|---|---|
| **LongMemEval-S 검색, 선택형 BGE와 2단계 재랭킹, 500개 질문**<br />R@10 `0.992`, MRR `0.958` | 어휘 검색이 부족한 패러프레이즈 중심 메모리에서 semantic 경로가 근거를 복구할 수 있습니다. |
| **동일 검색과 MiniMax reader를 사용한 LongMemEval-S E2E**<br />`74.0%` | reader 기반 평가를 연구할 수준이지만 기본 경로나 SOTA 주장은 아닙니다. |
| **BrainBench, 데몬을 통한 BM25**<br />P@5 `17.4%`, R@5 `64.1%`; 새 CLI 프로세스와 같은 점수로 `5.7x` 빠름 | 표면형이 겹치는 검색은 어휘 경로에 두고 저장소를 웜 상태로 유지합니다. |
| **공유 HTTP MCP, 클라이언트 2개 x 쓰기 10회**<br />20/20 저장; p50 `18.4ms`, p95 `41.8ms` | 하나의 로컬 데몬이 권장 동시 쓰기 경로입니다. |
| **토큰 없는 워크플로 데모**<br />`128ms`에 decision, lesson, error 노출 | 에이전트나 모델 호출 없이 핵심 워크플로를 시연할 수 있습니다. |
| **에이전트 간 핸드오프 Scenario P**<br />quality gate `12/12`; 핵심 근거 `4/4`, route `4/4`, 이웃 source 누출 `0`; 구조화 SDK packet과 `done_when` 보존; raw thread 대비 handoff context `-82.6%`, session canvas 대비 `-34.5%` | 오케스트레이터가 제한된 팩트 연결 context, 관찰 가능한 완료 조건, 수신자 one-command resume으로 하나의 추적 워크플로를 에이전트/프로필 경계 너머로 라우팅할 수 있습니다. 이는 결정적 artifact 계약을 증명하며 downstream 모델의 작업 성공률 결과는 아닙니다. |
| **다중 계정 핸드오프 Scenario Q**<br />`codex-one`, `codex-two`, `claude-main`에서 `10/10`; actor/source 누출 `0`; dispatch당 pointer entity 1개와 복제 fact 0개; broker/payload key `0` | 계정 설치가 vendor-local chat ID를 공유하지 않고 같은 추적 세션을 pull/확인할 수 있습니다. 인증, queue delivery, 배타 소유권, downstream 성공률의 증거는 아닙니다. |
| **Hermes Kanban 경계 Scenario R**<br />실제 임시 Hermes Kanban DB에서 `12/12`; 내부 `coding -> reviewer` 전환은 AideMemo assignment 0개; 외부 `codex-two` dispatch는 pointer 1개와 fact 0개 추가; Hermes가 card를 명시적으로 완료하기 전에 같은 session으로 evidence 반환 | Kanban은 canonical task lifecycle을 유지하고 AideMemo는 외부 설치 경계를 넘는 durable evidence를 운반합니다. 외부 CLI worker spawner, 인증, downstream 모델 성공률을 증명하지는 않습니다. |
| **외부 worker lane Scenario S**<br />fake Codex/Claude gate `14/14`; 성공은 packet과 resume 환경을 받고 같은 session에 evidence를 반환한 뒤 complete; 실패는 같은 session에 error를 기록하고 accepted 유지; 발신자 outbox/status는 두 fact를 연결 | 패키지 수신자가 shell-free argv로 handoff/return protocol을 실행하면서 Hermes Kanban을 변경하지 않음을 보입니다. live-model task success, authentication, 자동 retry, exactly-once execution, Hermes `spawn_fn` 통합을 증명하지는 않습니다. |
| **에이전트 SDK 패키지 스모크**<br />wheel 설치와 `Memory`, client, worker-lane export, 설치된 `aidememo-worker-lane --help` 검사가 `3.28s`에 통과 | 코드 우선 통합과 외부 수신자는 특정 에이전트 런타임과 독립적으로 패키징할 수 있습니다. |

각 측정은 데이터셋과 실행 조건이 다릅니다. 하나의 종합 점수로 합치지 말고
각 벤치마크 안에서 비교하세요.

## 모델 배치

| 실패 지점과 현재 배치 | 근거의 경계 |
|---|---|
| **일반 코드·문서 검색**<br />BM25 우선 `search.auto_hybrid=true`, 다국어 model2vec semantic 폴백, 데몬 사전 워밍 | BrainBench의 어휘 데몬 경로는 품질을 유지하면서 새 프로세스 비용을 피했습니다. |
| **영어 패러프레이즈 중심 메모리**<br />선택형 `bge-small-en-v1.5` | LongMemEval-S R@5가 `96.2%`에서 `98.0%`로 올랐지만 약 `10x`의 웜 쿼리 비용은 모든 워크로드에 정당화되지 않습니다. |
| **약한 1단계 어휘 재현율**<br />게이트된 MLX LFM 임베딩 실험 | 180개 에이전트 트레이스 문서와 540개 쿼리에서 BM25 R@8은 `0.991`, 순수 LFM dense는 `0.887`, guarded auto는 약한 2개 사례만 승격해 `0.993`을 기록했습니다. LFM은 전역 기본 임베딩 대체가 아닙니다. |
| **좋은 후보와 잘못된 순서**<br />웜 LFM ColBERT 실험 | 작은 픽스처에서 hit@1이 `0.57`에서 `0.86`으로 올랐지만 제품 배치 전 후보 재현율, 문서 토큰 비용, 더 큰 코퍼스 검증이 필요합니다. |
| **누락되거나 모호한 팩트 타입**<br />LFM 1.2B LoRA shadow 힌트 | confidence `>= 0.98`에서 확장된 high-signal 트레이스 게이트는 39/155개 힌트를 precision `0.923`, baseline-correct harm 0으로 수락했습니다. 자동 쓰기 결정이 아니라 리뷰 데이터로 유지합니다. |
| **프라이버시 민감 쓰기**<br />선택형 로컬 MLX 프라이버시 사이드카와 결정적 secret prefix | 측정된 MLX 사이드카는 CPU 모델보다 웜 쓰기 오버헤드를 줄였지만 메모리와 지연 비용 때문에 명시적 활성화가 정직한 기본값입니다. |

## 성능 주장 경계

- AideMemo는 메모리·검색 시스템이며 호스팅 에이전트 런타임이 아닙니다.
- 기본 메모리 루프는 외부 LLM을 호출하지 않습니다. 선택형 extractor, TEI
  endpoint, reranker, 벤치마크 reader는 호출할 수 있습니다.
- 작은 로컬 모델은 시나리오 게이트가 품질과 지연의 이점을 보인 위치에만
  배치합니다. 중립적인 결과라면 더 저렴한 경로를 유지합니다.
- LongMemEval 결과는 검색과 reader 동작을 보정하기 위한 것이며 AideMemo는
  SOTA 주장을 앞세우지 않습니다.
- 레지스트리 배포 상태는 별도의 [릴리스 체크리스트](RELEASE.md)에서
  관리합니다.

이 경로들의 시스템 경계는 [아키텍처](ARCHITECTURE.md)를 참고하세요.
