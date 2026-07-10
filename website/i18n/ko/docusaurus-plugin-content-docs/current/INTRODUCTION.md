---
title: AideMemo란?
description: AideMemo의 역할과 사용 시점을 설명하는 사용자 중심 개요입니다.
---

# AideMemo란?

AideMemo는 코딩 에이전트와 개발자 도구를 위한 로컬 메모리 계층입니다.
팩트, 결정, 교훈, 오류를 하나의 로컬 데이터베이스에 저장하고 CLI, MCP 도구,
에이전트 SDK, 네이티브 바인딩을 통해 제공합니다.

기본 메모리 경로는 캡처, 타입이 지정된 쓰기, BM25 우선 검색, MCP 또는
에이전트 SDK 읽기를 포함합니다. 이 경로는 외부 LLM 호출 없이 로컬에서
동작합니다. 원격 추출, 임베딩, 재랭킹은 선택 기능입니다.

세션, 편집기, 모델 제공자가 달라져도 유지되는 프로젝트 메모리가 필요할 때
AideMemo를 사용합니다.

시스템 구성은 [`아키텍처`](ARCHITECTURE.md), 검증 결과는
[`검증 근거`](EVIDENCE.md), 턴마다 알맞은 도구를 고르는 방법은
[`에이전트 워크플로`](AGENT_WORKFLOWS.md)를 참고하세요.

## 제공하는 기능

| 필요 | AideMemo 기능 |
|---|---|
| 프로젝트 결정 기억 | `aidememo fact add --type decision` |
| 이전 컨텍스트 검색 | `aidememo search`, `aidememo query` |
| 간단한 티켓에서 작업 시작 | `aidememo workflow start` |
| 여러 에이전트가 메모리 공유 | `source_id` 범위 지정, `aidememo mcp-serve` |
| 에이전트에 도구 제공 | `aidememo mcp` 또는 HTTP MCP |
| 코드에서 메모리 사용 | `aidememo-agent-sdk` |

## 기본 모델

AideMemo는 세 가지 주요 정보를 저장합니다.

- **엔티티**: `Redis`, `Billing`, `Codex`, 세션 ID와 같은 주제입니다.
- **팩트**: 엔티티에 연결된 타입 지정 메모리입니다.
- **관계**: 엔티티 사이의 그래프 간선입니다.

팩트에 타입을 지정하면 중요한 메모리가 더 높은 순위에 배치됩니다.

| 팩트 타입 | 용도 |
|---|---|
| `decision` | 이후 작업을 이끌어야 하는 결정 |
| `lesson` | 이전 시도에서 배운 내용 |
| `error` | 피해야 할 실패 패턴 |
| `preference` | 사용자 또는 팀의 선호 |
| `pattern` | 반복되는 아키텍처 또는 워크플로 패턴 |
| `note` | 일반 컨텍스트 |
| `question` | 새 이슈, 티켓, 열린 조사 항목 |

## 일반적인 워크플로

1. `aidememo` CLI를 설치합니다.
2. 작업하면서 팩트를 추가합니다.
3. 계획을 세우기 전에 메모리를 검색하거나 질의합니다.
4. 에이전트에 AideMemo MCP 서버를 등록합니다.
5. 이슈, PR, 티켓 자동화는 `workflow start`로 시작합니다.

```bash
aidememo fact add \
  "Decision: Redis timeout fixes must go through the Worker job wrapper." \
  --type decision \
  --entities Redis,Worker

aidememo query "Fix Redis timeout in worker"
```

## AideMemo가 아닌 것

AideMemo는 호스팅 메모리 서비스나 전체 에이전트 런타임이 아니며, 이슈
트래커를 대체하지 않습니다. 기존 도구가 호출할 수 있는 로컬 메모리
시스템입니다.

클라우드 관리형 에이전트 플랫폼이 필요하다면 호스팅 메모리 또는 런타임
제품을 사용하세요. CLI, MCP 도구, SDK 접근을 갖춘 명시적인 로컬 메모리가
필요하다면 AideMemo를 사용하세요.
