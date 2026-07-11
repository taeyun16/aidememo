---
kind: doc
title: Claude Code용 AideMemo 설치
---

# Claude Code에서 AideMemo 사용하기

[English](setup-claude-code.md)

권장 설치 방법은 Claude Code 플러그인입니다. MCP 서버 정의, 용도별 스킬,
안전한 컨텍스트 훅이 한 번에 설치됩니다. 더 작은 구성이나 프로필별 구성이
필요하면 MCP와 스킬 설치기를 따로 사용하세요.

## 사전 준비

```bash
cargo install aidememo-cli
aidememo init ./wiki
claude --version
```

`aidememo`와 `claude`가 모두 `PATH`에 있어야 합니다.

## 방법 A: 플러그인 설치(권장)

이 저장소를 clone한 경로에서 실행합니다.

```bash
claude plugin marketplace add /absolute/path/to/aidememo
claude plugin install aidememo@aidememo
claude plugin list
```

설치 후 Claude Code를 다시 시작하세요. 플러그인은 다음을 제공합니다.

- AideMemo stdio MCP 서버와 현재 MCP 도구 모음
- 기능을 나눈 `aidememo`, `aidememo-context`, `aidememo-remember` 스킬
- `SessionStart`, `PostToolUse`, `UserPromptSubmit` 훅

플러그인은 선택된 기본 AideMemo store를 사용합니다. 명시적인 store나
출처가 필요하면 Claude Code를 시작하기 전에 `AIDEMEMO_STORE`,
`AIDEMEMO_SOURCE_ID`, `AIDEMEMO_ACTOR_ID`를 설정하세요. 시작 환경과 무관하게
Claude Code 설정에 등록하려면 플러그인 대신 방법 B를 선택합니다.

설치하지 않고 플러그인을 개발·검증하려면:

```bash
claude plugin validate ./plugins/claude
claude --plugin-dir ./plugins/claude
```

## 방법 B: MCP와 독립 스킬 설치(플러그인 미사용)

어느 작업 디렉터리에서 실행해도 같은 store를 사용하도록 절대 경로를
등록합니다.

```bash
aidememo --store "$(pwd)/wiki.sqlite" mcp-install \
  --target claude \
  --source-id "project:my-project" \
  --actor-id "claude:local"

aidememo skill install --target claude
claude mcp list
```

스킬 설치기는 `CLAUDE_CONFIG_DIR`를 우선 사용하고, 설정되지 않았으면
`~/.claude/skills/aidememo`에 설치합니다. 기존 파일 교체에는 `--force`를
사용하세요.

Claude Code의 MCP scope를 직접 관리할 수도 있습니다. 개인 checkout은
`local`, 저장소에 공유할 `.mcp.json`은 `project`, 여러 프로젝트에서 함께
쓸 구성은 `user` scope가 적합합니다.

## 검증

Claude Code에서 `/mcp`를 실행해 `aidememo` 연결을 확인한 뒤 요청합니다.

```text
AideMemo를 사용해 현재 프로젝트 컨텍스트를 보여줘.
```

CLI에서도 상태를 확인할 수 있습니다.

```bash
aidememo doctor
claude mcp list
```

## 문제 해결

| 증상 | 해결 방법 |
|---|---|
| `aidememo: command not found` | CLI를 설치하고 셸과 Claude Code를 다시 시작해 `PATH`를 갱신합니다. |
| MCP 연결 실패 | `aidememo mcp-install --target claude --force` 후 `claude mcp list`를 실행합니다. |
| 다른 store 사용 | 절대 경로 `--store`로 다시 설치합니다. |
| 격리 프로필에 스킬 없음 | `CLAUDE_CONFIG_DIR`를 설정한 뒤 스킬 설치기를 실행합니다. |
| 훅 컨텍스트 없음 | [훅 안내](hooks/README.ko.md)에 따라 수동 실행하고 `AIDEMEMO_STORE`를 확인합니다. |

`.claude/commands/`의 기존 파일도 호환되지만, 새 설치에서는 Claude Code의
현재 확장 방식인 스킬을 권장합니다.
