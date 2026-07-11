# Claude Code용 AideMemo 플러그인

[English](README.md)

이 자체 포함형 플러그인은 Claude Code에 AideMemo MCP 도구, 기능별 스킬 세
개, 안전한 컨텍스트 훅을 제공합니다. 먼저 `aidememo` CLI를 설치한 뒤
[전체 설치 안내](../../aidememo-skill/setup-claude-code.ko.md)를 따르세요.

```bash
claude plugin validate ./plugins/claude
claude --plugin-dir ./plugins/claude
```

훅은 읽기 전용이며 soft-fail합니다. Fact 추출은 후보만 보여주며, 저장하려면
Claude가 AideMemo 쓰기 도구를 명시적으로 호출해야 합니다.
