---
kind: doc
title: Hermes Agent 설정 가이드
---

# Hermes Agent에서 AideMemo 사용하기

Hermes에서는 가벼운 **skill + MCP** 경로와 자동 컨텍스트·슬래시 명령까지
제공하는 **네이티브 플러그인** 경로 중 하나를 선택할 수 있습니다.

English: [`setup-hermes.md`](setup-hermes.md)

## 공통 준비

```bash
cd ~/dev/aidememo
cargo build -p aidememo-cli --release
export PATH="$PWD/target/release:$PATH"
```

## 경로 A: skill + MCP

```bash
aidememo skill install --target hermes
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target hermes \
  --source-id project:my-project \
  --actor-id hermes:local
```

`mcp-install`은 Hermes가 발견한 AideMemo 도구를 모두 활성화한 뒤
`hermes mcp list`로 등록 여부를 검증합니다. 격리 프로필에서
`HERMES_HOME=/path/to/profile`을 사용하면 skill과 MCP 설정 모두 해당
프로필에 설치됩니다.

확인:

```bash
hermes mcp list
hermes mcp test aidememo
hermes skills list
```

## 경로 B: 네이티브 플러그인

플러그인은 MCP 도구에 더해 세션 시작 자동 컨텍스트, 8개 슬래시 명령,
선택형 pending-first 캡처를 제공합니다. 반드시 Hermes 자체 Python 환경에
설치하세요.

```bash
HERMES_PY="${HERMES_PY:-$HOME/.hermes/hermes-agent/venv/bin/python3}"
"$HERMES_PY" -m pip install -e packages/aidememo-agent-sdk -e plugins/hermes
hermes plugins enable aidememo
```

`~/.hermes/config.yaml` 또는 `$HERMES_HOME/config.yaml`:

```yaml
plugins:
  enabled:
    - aidememo
  aidememo:
    store_path: ~/.aidememo/wiki.sqlite
    source_id: project:my-project
    actor_id: hermes:local
    auto_capture:
      enabled: false
      mode: pending
```

개발 체크아웃을 실제 사용자 홈과 분리해 시험하려면 다음을 사용합니다.

```bash
./scripts/setup-hermes-test-env.sh setup
eval "$(./scripts/setup-hermes-test-env.sh env)"
./scripts/setup-hermes-test-env.sh seed
./scripts/test-hermes-e2e.sh
```

## 첫 사용

```text
/aidememo-context Redis
/aidememo-start "Fix Redis timeout" --source github:org/repo#123
/aidememo-add "Redis timeout is 30 seconds" --type decision --entities Redis
```

플러그인을 사용하지 않는 경우에도 Hermes 모델은 설치된 skill을 통해
`aidememo_query`, `aidememo_context` 등의 MCP 도구를 선택할 수 있습니다.
