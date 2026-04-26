# Codex CLI 에서 wg 사용하기

OpenAI Codex CLI는 `~/.codex/config.toml`에서 stdio MCP 서버를 그대로 받습니다.
설정 한 줄로 끝납니다.

## 1. 빌드

```bash
cd ~/dev/wg
cargo build -p wg-cli --release
export PATH="$HOME/dev/wg/target/release:$PATH"
```

## 2. MCP 서버 등록

`~/.codex/config.toml`에 추가:

```toml
[mcp_servers.wg]
command = "wg"
args = ["mcp"]
# 선택:
# tool_timeout_sec = 30
# enabled_tools = ["wg_search", "wg_entity_list", "wg_traverse"]
# default_tools_approval_mode = "approve"   # 매번 승인 안 받고 싶을 때
```

도구 5종이 Codex 세션에 자동으로 노출됩니다.

## 3. AGENTS.md 작성 (프로젝트별 지침)

Codex는 프로젝트 루트의 `AGENTS.md`를 자동으로 읽습니다. wg 사용 패턴을
넣어두면 모델이 적절히 호출합니다:

```markdown
## Knowledge base

This project uses `wg` (Wiki-Graph) for persistent context. Before answering
non-trivial questions, call `wg_search` for prior context. Record key
decisions via `wg_fact_add`.
```

`wg` 자체 repo의 `AGENTS.md`가 좋은 예시입니다.

## 4. 위키 초기화

```bash
wg init ./docs
wg ingest ./docs
```

## 검증

```bash
# stdio JSON-RPC 직접 호출
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | wg mcp

# Codex 세션 안에서: 도구 목록에 wg_* 가 보이는지 확인
```

## 문제 해결

| 증상 | 해결 |
|---|---|
| Codex가 wg를 못 봄 | `~/.codex/config.toml` 경로 + TOML 키 이름 (`mcp_servers`) 확인 |
| 매번 승인 프롬프트가 뜸 | `default_tools_approval_mode = "approve"` 설정 |
| `command not found: wg` | 절대경로 사용: `command = "/Users/me/dev/wg/target/release/wg"` |
