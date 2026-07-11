---
kind: doc
title: Claude Code용 AideMemo 훅
---

# Claude Code 훅으로 AideMemo 자동화하기

[English](README.md)

이 선택형 훅은 Claude Code를 막거나 fact를 자동 저장하지 않으면서 프로젝트
메모리를 제공합니다.

| 이벤트 | matcher | 동작 |
|---|---|---|
| `SessionStart` | `startup\|resume\|compact` | pinned fact, 개요, 최근 활동을 주입합니다. |
| `PostToolUse` | `Edit\|Write\|NotebookEdit` | 편집 경로와 관련된 fact를 BM25 전용 빠른 경로로 조회합니다. |
| `UserPromptSubmit` | 없음 | 200자 이상 prompt에서 fact 후보만 미리 보여줍니다. |

권장 Claude 플러그인에는 이 훅들이 이미 포함됩니다. 직접 설치하려면:

```bash
mkdir -p ~/.claude/hooks
cp aidememo-skill/hooks/aidememo-*.py ~/.claude/hooks/
chmod +x ~/.claude/hooks/aidememo-*.py
cp .claude/settings.example.json .claude/settings.json
```

기존 settings가 있으면 덮어쓰지 말고 예제 내용을 병합하세요. 기본 store가
현재 프로젝트용이 아니면 `AIDEMEMO_STORE` 또는 `AIDEMEMO_PROJECT`를
설정합니다. LLM 추출이 의도된 경우에만 `AIDEMEMO_EXTRACT_LLM=1`을
설정하세요. 기본 추출은 로컬 heuristic입니다.

## 출력 계약 검증

```bash
echo '{}' | python3 ~/.claude/hooks/aidememo-session-start.py | jq .

echo '{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs"}}' \
  | python3 ~/.claude/hooks/aidememo-post-tool.py | jq .
```

컨텍스트가 있으면 Claude Code의 이벤트별 envelope를 출력합니다.

```json
{
  "continue": true,
  "hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "additionalContext": "..."
  }
}
```

바이너리, store, 조회에 문제가 있으면 모든 훅은 `{"continue": true}`로
soft-fail합니다. fact를 추가·편집·교체·삭제하지 않습니다. 비활성화하려면
settings에서 이벤트를 지우거나 플러그인을 끄세요.
