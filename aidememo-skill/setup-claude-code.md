---
kind: doc
title: Claude Code 설정 가이드
---

# Claude Code 에서 AideMemo 사용하기

AideMemo는 Claude Code의 MCP / Skill / Slash command 시스템 셋 다 지원합니다.
가장 큰 효과를 얻으려면 **MCP**를 먼저 등록하세요.

## 1. 빌드

```bash
cd ~/dev/aidememo
cargo build -p aidememo-cli --release
export PATH="$HOME/dev/aidememo/target/release:$PATH"   # ~/.zshrc에 추가
```

## 2. MCP 서버 등록 (가장 중요)

Claude Code가 stdio로 AideMemo를 부르도록 등록합니다. 한 번만:

```bash
claude mcp add aidememo -- aidememo mcp
```

또는 프로젝트 루트의 `.mcp.json`에 추가:

```json
{
  "mcpServers": {
    "aidememo": {
      "type": "stdio",
      "command": "aidememo",
      "args": ["mcp"]
    }
  }
}
```

이걸로 `aidememo_search`, `aidememo_entity_list`, `aidememo_fact_add`, `aidememo_lint`, `aidememo_traverse`
다섯 개 툴이 Claude Code에 자동으로 노출됩니다. 별도 서버 띄울 필요 없음.

## 3. Skill 설치 (선택)

LLM이 AideMemo를 *언제* 써야 할지 학습시키려면 SKILL.md를 복사:

```bash
mkdir -p ~/.claude/skills/aidememo
cp ~/dev/aidememo/aidememo-skill/SKILL.md ~/.claude/skills/aidememo/SKILL.md
cp ~/dev/aidememo/aidememo-skill/REFERENCE.md ~/.claude/skills/aidememo/REFERENCE.md
```

`SKILL.md`의 frontmatter `description`/`when_to_use`가 자동 트리거 단서가 됩니다.

## 4. Slash command 설치 (선택)

`/aidememo-search`, `/aidememo-add-fact`, `/aidememo-context` 단축어:

```bash
mkdir -p ~/.claude/commands
cp ~/dev/aidememo/.claude/commands/aidememo-*.md ~/.claude/commands/
```

또는 프로젝트별로 `.claude/commands/`에 두면 그 프로젝트에서만 작동.

## 5. 위키 초기화

```bash
aidememo init ./my-wiki        # 새 위키
aidememo ingest ./my-wiki      # 마크다운 → 그래프
```

`~/.aidememo/config.toml`에서 store 경로/모델 등 조정 가능.

## 검증

```bash
# stdio MCP 핸드셰이크
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | aidememo mcp

# 또는 Claude Code 안에서:
# "/mcp aidememo" → 도구 목록 확인
```

## 문제 해결

| 증상 | 해결 |
|---|---|
| `aidememo: command not found` | PATH에 `target/release` 추가 후 셸 재시작 |
| Claude Code가 도구를 못 봄 | `claude mcp list`로 등록 확인. 안 되면 `.mcp.json` 사용 |
| `store not found` | `aidememo init` 안 한 상태. 또는 `--store` 옵션으로 경로 지정 |
