---
title: 코딩 에이전트 설치
description: Claude Code, Codex, Hermes Agent, pi, Cursor, OpenClaw, OpenCode에 AideMemo를 설치합니다.
---

# 코딩 에이전트 설치

AideMemo는 MCP, Agent Skill, 네이티브 플러그인, 직접 CLI 사용을 지원합니다.
사용하는 코딩 에이전트가 지원하는 가장 작은 통합 방식을 선택하세요.

## 통합 방식 선택

| 에이전트 | 권장 경로 | 대안 | 프로필 설정 |
|---|---|---|---|
| Claude Code | Claude 플러그인: MCP + 기능별 스킬 + 읽기 전용 훅 | 독립 MCP + 스킬 | `CLAUDE_CONFIG_DIR` |
| Codex | store를 고정한 stdio MCP | 프로젝트 `AGENTS.md` 사용 지침 | `CODEX_HOME` / `--codex-home` |
| Hermes Agent | 스킬 + MCP | 훅과 slash command를 포함한 네이티브 Python 플러그인 | `HERMES_HOME` |
| pi coding agent | 네이티브 Agent Skill + 로컬 CLI | 없음. pi는 MCP를 받지 않음 | `PI_CODING_AGENT_DIR` |
| Cursor | stdio MCP | 수동 `mcp.json` | Cursor 설정 디렉터리 |
| OpenClaw | 스킬 + stdio MCP | 공유 `~/.agents/skills` 스킬 | OpenClaw 설정 디렉터리 |
| OpenCode | `AGENTS.md` 지침 + stdio MCP | 수동 JSON 설정 | OpenCode 설정 디렉터리 |

## AideMemo 준비

에이전트를 설정하기 전에 CLI를 설치하고 store를 만들거나 선택합니다.

```bash
cargo install --git https://github.com/taeyun16/aidememo aidememo-cli
mkdir -p ./_meta
aidememo --store "$(pwd)/_meta/wiki.sqlite" stats
```

에이전트 등록에는 절대 store 경로를 사용하세요. 하나의 신뢰된 store에 여러
프로젝트나 에이전트 namespace가 있으면 `source_id`를, 어느 에이전트 프로필이
썼는지 남겨야 하면 `actor_id`를 추가합니다. MCP install은 호출자가 덮어쓸 수
있는 환경 기본값을 설정하므로 source 할당을 강제해야 하면 HTTP token binding을
사용합니다.

## Claude Code

저장소에는 AideMemo MCP, 기능별 스킬 세 개, 읽기 전용 컨텍스트 훅 세 개를
포함한 자체 완결형 Claude Code 플러그인이 있습니다.

```bash
claude plugin marketplace add /absolute/path/to/aidememo
claude plugin install aidememo@aidememo
claude plugin list
```

플러그인은 Claude Code가 상속한 기본 store와 환경을 사용합니다. 명시적인
store와 provenance가 필요하면 Claude를 시작하기 전에 `AIDEMEMO_STORE`,
`AIDEMEMO_SOURCE_ID`, `AIDEMEMO_ACTOR_ID`를 설정합니다.

Claude Code 자체 설정에 등록을 유지하려면 플러그인 대신 독립 경로를
선택합니다.

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target claude \
  --source-id project:my-app \
  --actor-id claude:local
aidememo skill install --target claude
claude mcp list
```

스킬 설치기는 변수가 있으면 `$CLAUDE_CONFIG_DIR/skills/aidememo`에, 없으면
`~/.claude/skills/aidememo`에 씁니다. 새 설치는 스킬을 사용하며
`.claude/commands`는 기존 호환성 용도로만 남아 있습니다.

플러그인 개발 검증:

```bash
claude plugin validate ./plugins/claude
claude --plugin-dir ./plugins/claude
```

## Codex

활성 Codex 프로필에 store가 고정된 stdio MCP 서버를 등록합니다.

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target codex \
  --source-id project:my-app \
  --actor-id codex:local
```

여러 격리 프로필이 한 store를 공유하면 `--codex-home`과 `--actor-id`를 같은
순서로 반복합니다.

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target codex \
  --codex-home "$HOME/.codex-account-a" --actor-id codex:account-a \
  --codex-home "$HOME/.codex-account-b" --actor-id codex:account-b \
  --source-id project:my-app
```

전체 동시성 및 workflow lineage 패턴은
[여러 Codex 프로필에서 메모리 공유](CODEX_MULTI_PROFILE.md)를 참고하세요.

## Hermes Agent

가벼운 경로는 네이티브 스킬을 설치하고 모든 AideMemo MCP 도구를 등록합니다.
두 설치기 모두 `HERMES_HOME`을 따릅니다.

```bash
aidememo skill install --target hermes
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target hermes \
  --source-id project:my-app \
  --actor-id hermes:local
hermes mcp test aidememo
hermes skills list
```

네이티브 플러그인은 session context, slash command, SDK 조합, 선택형
pending-first capture를 추가합니다. Hermes 자체 Python 환경에 설치하세요.

```bash
HERMES_PY="${HERMES_PY:-$HOME/.hermes/hermes-agent/venv/bin/python3}"
"$HERMES_PY" -m pip install hermes-aidememo
hermes plugins enable aidememo
```

`$HERMES_HOME/config.yaml` 또는 `~/.hermes/config.yaml`의
`plugins.aidememo.store_path`, `source_id`, `actor_id`로 store와 쓰기 출처를
선택합니다.

## pi coding agent

pi는 Agent Skill과 로컬 `bash` 도구를 사용합니다. 의도적으로 MCP 등록 단계가
없습니다.

```bash
aidememo skill install --target pi
```

기본 설치 위치는 `~/.pi/agent/skills/aidememo`입니다. 격리 프로필은 다른
네이티브 스킬 디렉터리를 선택할 수 있습니다.

```bash
export PI_CODING_AGENT_DIR="$HOME/.pi/work-profile"
aidememo skill install --target pi
```

새 pi 세션에서 `/skill:aidememo`를 실행하거나 자연어로 프로젝트 메모리 조회와
기록을 요청하세요. 이전 설치기가 `mcp-install --target pi`를 제안한다면
AideMemo를 업데이트해야 합니다. pi는 upstream에서 MCP를 받지 않습니다.

## Cursor, OpenClaw, OpenCode

```bash
# Cursor: ~/.cursor/mcp.json의 mcpServers.aidememo에 기록
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target cursor \
  --source-id project:my-app --actor-id cursor:local

# OpenClaw: 네이티브 스킬과 MCP 등록
aidememo skill install --target openclaw
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target openclaw \
  --source-id project:my-app --actor-id openclaw:local

# OpenCode: 관리되는 지침을 추가하고 mcp.aidememo에 기록
aidememo skill install --target opencode
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target opencode \
  --source-id project:my-app --actor-id opencode:local
```

각 설치기에 `--list-targets`를 사용하면 지원 대상과 설치 위치를 확인할 수
있습니다. `mcp-install --print`로 변경을 미리 보고, 기존 AideMemo 항목을
의도적으로 바꿀 때만 `--force`를 사용하세요.

## 검증과 사용

```bash
aidememo doctor
aidememo mcp-install --list-targets
aidememo skill install --list-targets
```

MCP 에이전트에서는 `aidememo` 연결을 확인한 다음 일반 턴은
`aidememo_context`, 티켓은 `aidememo_workflow_start`로 시작합니다. 더 좁은
후속 조회에는 `aidememo_query`를 사용하고, 오래 유지할 decision, convention,
preference, lesson, 반복 error만 기록합니다.

위 `aidememo doctor`는 관리자 측 CLI 점검입니다. Source-scoped MCP identity는
global store metadata를 반환하는 `aidememo_doctor`와 `aidememo_overview`를 호출할
수 없습니다. 해당 진단은 scoped agent 밖에서 실행하고 agent 안에서는
`aidememo_context`, `aidememo_query`, scoped entity/fact tool을 사용하세요.

| 증상 | 해결 방법 |
|---|---|
| `aidememo: command not found` | Cargo bin 디렉터리를 에이전트 프로세스 `PATH`에 추가하고 다시 시작합니다. |
| 다른 store가 열림 | 전역 `--store`와 절대 경로로 다시 설치합니다. |
| 격리 프로필에 스킬이 없음 | 설치 전에 에이전트별 프로필 환경 변수를 설정합니다. |
| MCP가 등록됐지만 연결되지 않음 | 에이전트의 MCP list/test 명령과 `aidememo doctor`를 실행합니다. |
| 공유 store에서 다른 프로젝트 결과가 보임 | 안정적인 `--source-id`로 설치합니다. |

설치 후 도구 선택 방법은 [에이전트 워크플로](AGENT_WORKFLOWS.md)와
[MCP 설정](MCP.md)을 이어서 참고하세요.
