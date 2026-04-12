# Setup Guide: Claude Code에서 wg 사용하기

## 개요

Claude Code는 `~/.claude/` 디렉터리의 SKILL.md를 자동으로 로드합니다.
`wg-skill/SKILL.md`를 복사하면 Claude Code가 자동으로 wg를 호출합니다.

## 설치 단계

### 1. wg 바이너리 빌드/설치

```bash
cd ~/dev/wg
cargo build -p wg-cli
# 또는 release 빌드:
cargo build -p wg-cli --release
# release 바이너리: ./target/release/wg
```

### 2. SKILL.md 복사

```bash
mkdir -p ~/.claude/skills/wiki-graph/
cp ~/dev/wg/wg-skill/SKILL.md ~/.claude/skills/wiki-graph/SKILL.md
```

### 3. PATH에 wg 추가 (필요시)

```bash
# ~/.zshrc 또는 ~/.bashrc에 추가:
export PATH="$HOME/dev/wg/target/debug:$PATH"
# 또는 release:
export PATH="$HOME/dev/wg/target/release:$PATH"
```

### 4. 위키 초기화 (처음使用时)

```bash
wg init ./my-wiki
wg ingest ./my-wiki
```

## 사용법

Claude Code가 자동으로 SKILL.md의 지침에 따라 wg를 호출합니다.

### 직접 호출 예시

```
 Claude: wg entity list
 Claude: wg search "분산 시스템"
 Claude: wg traverse Redis --depth 2
```

## 자주 보는 문제

### wg: command not found
→ PATH에 wg 디렉터리가 포함되지 않음. `export PATH=...` 추가 후 재시작.

### wg init: store already exists
→ 이미 초기화된 경우. `wg entity list` 등으로 확인.

## 설정 파일

`~/.wg/config.toml`:
```toml
[store]
path = "~/.wg/wiki.redb"

[search]
default_limit = 20
bm25_weight = 0.7
semantic_weight = 0.3
```

## 업데이트

wg 코드를 갱신할 때마다:
```bash
cd ~/dev/wg && git pull && cargo build -p wg-cli --release
```
