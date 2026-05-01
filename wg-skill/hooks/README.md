---
kind: doc
title: wg + Claude Code hooks
---

# Claude Code 훅으로 wg 자동화

Claude Code의 hook 시스템을 사용해 매 세션 / 편집 / user prompt마다
wg 위키 컨텍스트를 자동 surface 하고 fact 후보를 미리 추출합니다.
OMEGA의 `omega_welcome` + 자동 capture 패턴을 참고했습니다 (Apache 2.0).

## 제공 훅

| 훅 | 매처 | 역할 | 비용 |
|---|---|---|---|
| `wg-session-start.py` | `SessionStart`, `*` | pinned + recent + overview를 새 세션 시작 시 inject | 로컬 only (~50 ms) |
| `wg-post-tool.py` | `PostToolUse`, `Edit\|Write\|MultiEdit` | 편집한 파일 관련 wg 사실 surface | 로컬 only (~30 ms) |
| `wg-extract-facts.py` | `UserPromptSubmit`, `*` (≥ 200자) | user prompt에서 fact 후보 추출 (preview only) | 기본 heuristic (~5 ms); `WG_EXTRACT_LLM=1`이면 OpenAI 호출 (~$0.0001/prompt) |

세 훅 모두:
- soft-fail (wg 바이너리 부재 / store 없음 등에 세션 안 막음)
- `additionalContext` 필드로 inject (block 안 함)
- 환경변수: `WG_BIN`, `WG_STORE`, `WG_PROJECT`로 store 위치 지정

## 설치 (3 단계)

### 1. 훅 스크립트 복사

```bash
mkdir -p ~/.claude/hooks
cp ~/dev/wg/wg-skill/hooks/wg-*.py ~/.claude/hooks/
chmod +x ~/.claude/hooks/wg-*.py
```

### 2. `~/.claude/settings.json` 또는 프로젝트 `.claude/settings.json`에 등록

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "$HOME/.claude/hooks/wg-session-start.py",
            "statusMessage": "wg: loading wiki context…"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [
          {
            "type": "command",
            "command": "$HOME/.claude/hooks/wg-post-tool.py"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "$HOME/.claude/hooks/wg-extract-facts.py"
          }
        ]
      }
    ]
  }
}
```

### 3. 환경변수 (선택)

기본은 `~/.wg/wiki.redb` (또는 config의 default project). 프로젝트별
다른 store를 쓰려면 셸에:

```bash
export WG_STORE=/path/to/project-wiki.redb
# 또는
export WG_PROJECT=work          # `wg project list`로 확인
```

LLM 추출 활성:
```bash
export WG_EXTRACT_LLM=1         # extract.provider 설정 필요
```

## 동작 검증

```bash
# 단독 테스트 (각 훅이 stdin JSON 받음)
echo '{}' | python3 ~/.claude/hooks/wg-session-start.py | jq .

echo '{"tool_name":"Edit","tool_input":{"file_path":"crates/wg-core/src/lib.rs"}}' \
  | python3 ~/.claude/hooks/wg-post-tool.py | jq .

echo '{"prompt":"We decided to migrate Postgres replicas to read-only mode after the 2026-04-12 outage. Auth flow now uses Keycloak with JWT-Service issuing 15min tokens."}' \
  | python3 ~/.claude/hooks/wg-extract-facts.py | jq .
```

성공 출력은 `{"additionalContext": "...", "continue": true}` 형태.

## 비활성화

훅별로 `~/.claude/settings.json`에서 항목 제거하거나,
스크립트 첫 줄에 `sys.exit(0)` 추가하여 즉시 종료.

## 안전성

- **모든 훅 read-only**. fact 자동 생성 / supersede / 삭제 안 함.
- **LLM 호출 opt-in only**. `WG_EXTRACT_LLM=1` 명시할 때만.
- **타임아웃**: 4-8초 hard cap. 느린 wg 호출이 세션을 막지 않음.
- **silent on failure**. wg 부재 / store 없음 / 에러 → 세션 정상 진행.

## 한계

- `Stop` 훅 (session summary 자동 생성) 은 **포함하지 않음**.
  세션 요약을 자동으로 fact로 저장하면 노이즈 위험. 사용자가 명시적
  `wg fact add` 또는 `wg consolidate`로 정제하는 게 안전.
- `wg-extract-facts.py`는 `additionalContext`로 후보를 surface 하지만
  Claude가 자동 적용하지 않음. agent가 `wg_fact_add` 명시 호출 필요.

## 참고

- Claude Code 훅 공식 문서: `claude --help` 또는 settings.json schema.
- OMEGA의 비슷한 메커니즘: `omega_welcome` (SessionStart 등가),
  hook-based auto-capture (PostToolUse / UserPromptSubmit / Stop) —
  see [`.notes/omega-pipeline-analysis.md`](../../.notes/omega-pipeline-analysis.md).
