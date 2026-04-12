# Setup Guide: Hermes Agent에서 wg 사용하기

## 개요

Hermes Agent는 `~/.hermes/skills/` 또는 프로젝트 내 `.hermes/skills/` 디렉터리에서 SKILL.md를 로드합니다.

## 설치 단계

### 1. wg 빌드

```bash
cd ~/dev/wg
cargo build -p wg-cli --release
```

### 2. Hermes skills 디렉터리에 복사

```bash
mkdir -p ~/.hermes/skills/wiki-graph/
cp ~/dev/wg/wg-skill/SKILL.md ~/.hermes/skills/wiki-graph/SKILL.md
```

### 3. PATH 추가

```bash
# ~/.zshrc에:
export PATH="$HOME/dev/wg/target/release:$PATH"
```

## 사용

Hermes Agent가 자동으로 wg를 호출합니다. `.claude.md`, `.codex.md`도 함께 배치하면 더 많은 에이전트에서 동작합니다:

```bash
cp ~/dev/wg/.claude.md ~/projects/your-project/.claude.md
cp ~/dev/wg/.codex.md ~/projects/your-project/.codex.md
```

## 설정 파일

`~/.wg/config.toml` (자동 생성 또는手動作成):
```toml
[store]
path = "~/.wg/wiki.redb"

[search]
default_limit = 20
bm25_weight = 0.7
semantic_weight = 0.3
```

## 업데이트

```bash
cd ~/dev/wg && cargo build -p wg-cli --release
```
