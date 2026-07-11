---
kind: doc
title: pi coding agent 설정 가이드
---

# pi coding agent에서 AideMemo 사용하기

pi는 MCP 대신 Agent Skills와 내장 `bash` 도구를 사용합니다. AideMemo는
pi의 네이티브 skill 디렉터리에 설치되며 전체 지침을 매 턴 시스템
프롬프트에 넣지 않습니다.

English: [`setup-pi.md`](setup-pi.md)

## 설치

```bash
cd ~/dev/aidememo
cargo build -p aidememo-cli --release
export PATH="$PWD/target/release:$PATH"
aidememo skill install --target pi
```

기본 설치 위치는 `~/.pi/agent/skills/aidememo/`입니다. 별도 pi 프로필은
환경변수로 지정할 수 있습니다.

```bash
export PI_CODING_AGENT_DIR="$HOME/.pi/work-profile"
aidememo skill install --target pi
```

## 확인

새 pi 세션에서 다음 중 하나를 사용합니다.

```text
/skill:aidememo
```

또는 자연어로 요청합니다.

```text
프로젝트 메모리에서 Redis 타임아웃 관련 결정을 찾아줘.
이 결정을 AideMemo에 decision으로 기록해줘.
```

pi는 skill의 지침에 따라 다음과 같은 로컬 CLI 명령을 실행합니다.

```bash
aidememo --json query "Redis timeout" --bm25-only
aidememo fact add "Redis timeout is 30 seconds" \
  --type decision --entities Redis
```

pi에는 MCP 등록 단계가 없습니다. 설치 후 안내에 `mcp-install --target pi`가
표시된다면 오래된 AideMemo 바이너리를 실행 중인 것이므로 바이너리를 다시
빌드하거나 업데이트하세요.
