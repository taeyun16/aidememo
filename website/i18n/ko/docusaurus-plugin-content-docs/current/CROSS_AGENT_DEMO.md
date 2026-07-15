---
title: 에이전트 간 연속성 데모
description: Hermes에서 Codex를 거쳐 Claude Code로 이어지는 실제 프로젝트 메모리 인계를 녹화합니다.
---

# 에이전트 간 연속성 데모

이 녹화는 채팅 동기화가 아니라 프로젝트 연속성을 보여줍니다. Hermes가 인증
실패를 진단하고 지속 가능한 결과를 기록합니다. 새 프로세스의 Codex가 그
지식에서 작업을 이어가고, Claude Code는 두 에이전트의 대화 내용 없이 같은
프로젝트 메모리로 결과를 검토합니다.

## 프로젝트 준비

```bash
scripts/prepare-continuity-demo.sh
cd /tmp/aidememo-continuity-demo
```

스크립트는 refresh token 테스트 하나가 실패하는 작은 Node 프로젝트를 만들고
세 개의 `mcp-install` 명령을 출력합니다. 녹화 전에 이 명령들을 한 번
실행하세요. 모든 에이전트가 같은 SQLite store를 사용하되 작성자마다 서로 다른
`actor_id`를 유지합니다.

에이전트를 열기 전에 설정을 확인합니다.

```bash
aidememo --backend libsqlite \
  --store /tmp/aidememo-continuity-demo/.aidememo/project-memory.sqlite \
  doctor
npm test
```

다시 녹화하려면 `scripts/prepare-continuity-demo.sh`를 재실행합니다. 데모 store를
삭제하고 원래의 실패하는 소스 파일을 복원합니다.

## 녹화 계획

터미널 너비는 110~120열, 고정폭 글꼴은 20~22 px로 설정하고 계정 이름과 API
키를 숨긴 채 1440p로 녹화합니다. 최종 영상은 45~55초가 적당합니다.

### 장면 1 - Hermes가 실패 원인을 찾습니다

데모 디렉터리에서 Hermes를 열고 다음 프롬프트를 사용합니다.

```text
Run the authentication test and diagnose the failure, but do not edit the code.
Before finishing, save three durable AideMemo facts for this project: the error,
the lesson, and the implementation decision the next coding agent should follow.
```

유용한 팩트에는 다음 내용이 들어가야 합니다.

- 오류: 사용이 끝난 refresh token을 저장해서 나중에 replay 감지가 발생합니다.
- 교훈: 공급자는 refresh가 성공할 때마다 refresh token을 교체합니다.
- 결정: 새 access token과 새 refresh token을 함께 저장합니다.

AideMemo 쓰기가 성공하는 장면을 잠시 보여주되 원시 tool-call JSON에는 시간을
쓰지 않습니다.

화면 챕터 문구: `YESTERDAY / HERMES FOUND WHAT FAILED`

### 장면 2 - Codex가 작업을 이어갑니다

Hermes를 완전히 종료하고 터미널을 지운 뒤 같은 폴더에서 Codex를 시작합니다.
Hermes 대화는 붙여 넣지 말고 다음 프롬프트만 사용합니다.

```text
Continue the authentication fix. Check the project memory before editing, then
implement the agreed fix and run the test.
```

복구된 오류, 교훈, 결정을 2초 동안 보여준 다음 Codex가
`session.refreshToken`을 `refreshed.refreshToken`으로 바꾸고 테스트가 통과하는
장면을 보여줍니다.

화면 챕터 문구: `NEW SESSION / DIFFERENT AGENT / NO RE-EXPLAINING`

### 장면 3 - Claude Code가 인계를 검증합니다

Codex를 종료하고 같은 폴더에서 Claude Code를 시작합니다. 다음 프롬프트를
사용합니다.

```text
Review the authentication change. Use the project memory to explain which prior
failure this avoids, then run the test. Do not modify the code unless the review
finds a real issue.
```

Claude Code가 통과한 구현을 Hermes가 기록한 실패와 연결하는 장면을
보여줍니다. 이는 메모리가 한 에이전트 세션이 아니라 프로젝트에 속한다는 것을
입증합니다.

화면 챕터 문구: `THE CONVERSATION ENDED / THE WORK CONTINUED`

## 마지막 프레임

홈페이지 문구를 사용합니다.

```text
Switch coding agents. Keep the work moving.

Local project memory for Hermes, Codex, Claude Code, and more.
aidememo.taeyun.me
```

## 정확한 주장 범위

결정, 실패한 시도, 교훈, 구조화된 프로젝트 컨텍스트가 에이전트 사이에서
이어진다고 표현하세요. 실시간 세션 전송이라고 부르면 안 됩니다. AideMemo는
채팅 기록, 실행 중인 프로세스, 자격 증명 또는 숨겨진 모델 상태를 도구 사이에서
옮기지 않습니다.
