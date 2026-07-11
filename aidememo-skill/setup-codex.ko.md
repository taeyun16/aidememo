---
kind: doc
title: Codex용 AideMemo 설치
---

# Codex에서 AideMemo 사용하기

[English](setup-codex.md)

Codex는 활성 `CODEX_HOME/config.toml`에서 stdio MCP 서버를 읽습니다.
`CODEX_HOME`이 없으면 기본 프로필은 `~/.codex`입니다.

## CLI 설치

```bash
cargo install --git https://github.com/taeyun16/aidememo aidememo-cli
aidememo --help
```

## MCP 등록

Codex의 작업 디렉터리가 바뀌어도 storage backend와 절대 store 경로가
달라지지 않도록 설치기를 사용합니다.

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install \
  --target codex \
  --source-id project:my-app \
  --actor-id codex:local
```

여러 격리 Codex 프로필이 한 store를 공유하면 `--codex-home`과 `--actor-id`를
같은 순서로 반복합니다.

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target codex \
  --codex-home "$HOME/.codex-account-a" --actor-id codex:account-a \
  --codex-home "$HOME/.codex-account-b" --actor-id codex:account-b \
  --source-id project:my-app
```

생성되는 설정은 다음과 같습니다.

```toml
[mcp_servers.aidememo]
command = "aidememo"
args = ["--backend", "libsqlite", "--store", "/absolute/project/_meta/wiki.sqlite", "mcp"]

[mcp_servers.aidememo.env]
AIDEMEMO_SOURCE_ID = "project:my-app"
AIDEMEMO_ACTOR_ID = "codex:local"
```

## Codex에 프로젝트 지침 제공

Codex는 프로젝트의 `AGENTS.md`를 자동으로 읽습니다. 지침은 짧고 작업 중심으로
유지하세요.

```markdown
## Project memory

Use `aidememo_context` once when a task depends on prior project knowledge.
Start issue or PR work with `aidememo_workflow_start`. Record only durable
decisions, conventions, preferences, lessons, and recurring errors.
```

## 검증

```bash
aidememo doctor
codex mcp list
```

새 Codex 작업에서 AideMemo 도구가 보이는지 확인합니다. 여러 계정의 인계와
동시성 패턴은 저장소의 `docs/CODEX_MULTI_PROFILE.md`를 참고하세요.

| 증상 | 해결 방법 |
|---|---|
| 한 프로필에서 AideMemo가 없음 | 활성 `CODEX_HOME`에 설치하거나 `--codex-home`을 전달합니다. |
| 다른 store가 열림 | 전역 `--store`와 절대 경로로 다시 설치합니다. |
| `aidememo`를 찾지 못함 | Cargo bin 디렉터리를 Codex 프로세스 `PATH`에 추가하고 다시 시작합니다. |
| 공유 프로필의 writer provenance가 없음 | 각 `--codex-home`에 고유한 `--actor-id`를 같은 순서로 지정합니다. |
