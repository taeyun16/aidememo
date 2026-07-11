---
kind: doc
title: Codex CLI 설정 가이드
---

# Codex CLI 에서 AideMemo 사용하기

OpenAI Codex CLI는 활성 `CODEX_HOME/config.toml`에서 stdio MCP 서버를 받습니다.
`CODEX_HOME`이 없으면 기본 경로는 `~/.codex/config.toml`입니다.
설정 한 줄로 끝납니다.

## 1. 빌드

```bash
cd ~/dev/aidememo
cargo build -p aidememo-cli --release
export PATH="$HOME/dev/aidememo/target/release:$PATH"
```

## 2. MCP 서버 등록

`~/.codex/config.toml`에 추가:

```toml
[mcp_servers.aidememo]
command = "aidememo"
args = ["mcp"]
# 선택:
# tool_timeout_sec = 30
# enabled_tools = ["aidememo_search", "aidememo_entity_list", "aidememo_traverse"]
# default_tools_approval_mode = "approve"   # 매번 승인 안 받고 싶을 때
```

프로젝트 저장소가 실행 디렉터리에 따라 바뀌지 않게 하려면 installer가 생성하는
명시적 `--store` 구성을 사용하십시오.

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target codex
```

여러 Codex 계정/프로필이 같은 프로젝트 메모리를 공유할 때는 `--codex-home`과
`--actor-id`를 같은 순서로 반복하고 공통 `--source-id`를 사용합니다.

```bash
aidememo --store "$(pwd)/_meta/wiki.sqlite" mcp-install --target codex \
  --codex-home "$HOME/.codex-account-a" --actor-id codex:account-a \
  --codex-home "$HOME/.codex-account-b" --actor-id codex:account-b \
  --source-id project:my-app
```

도구 5종이 Codex 세션에 자동으로 노출됩니다.

> ⚠️ **`codex exec` non-interactive 사용 시 주의**: Codex의 default 및
> `--full-auto` sandbox/approval 정책은 MCP tool 호출을 자동 cancel하고
> 로컬 CLI fallback으로 빠집니다 (잘못된 store를 보게 될 수 있음).
> 자동화 스크립트나 CI에서는 `codex exec
> --dangerously-bypass-approvals-and-sandbox …`로 호출하거나
> `~/.codex/config.toml`에 `approval_policy = "never"` +
> `sandbox_mode = "danger-full-access"`를 명시해야 합니다. 인터랙티브
> 사용에서는 영향 없습니다.

## 3. AGENTS.md 작성 (프로젝트별 지침)

Codex는 프로젝트 루트의 `AGENTS.md`를 자동으로 읽습니다. aidememo 사용 패턴을
넣어두면 모델이 적절히 호출합니다:

```markdown
## Knowledge base

This project uses `aidememo` (AideMemo) for persistent context. Before answering
non-trivial questions, call `aidememo_search` for prior context. Record key
decisions via `aidememo_fact_add`.
```

`aidememo` 자체 repo의 `AGENTS.md`가 좋은 예시입니다.

## 4. 위키 초기화

```bash
aidememo init ./docs
aidememo ingest ./docs
```

## 검증

```bash
# stdio JSON-RPC 직접 호출
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | aidememo mcp

# Codex 세션 안에서: 도구 목록에 aidememo_* 가 보이는지 확인
```

## 문제 해결

| 증상 | 해결 |
|---|---|
| Codex가 AideMemo를 못 봄 | 활성 `CODEX_HOME/config.toml` 경로 + TOML 키 이름 (`mcp_servers`) 확인 |
| 한 계정에서만 보임 | 해당 프로필을 `--codex-home`으로 추가 설치 |
| 다른 저장소가 열림 | 전역 `--store`로 다시 설치하고 `aidememo doctor` 실행 |
| 매번 승인 프롬프트가 뜸 | `default_tools_approval_mode = "approve"` 설정 |
| `command not found: aidememo` | 절대경로 사용: `command = "/path/to/aidememo/target/release/aidememo"` |
