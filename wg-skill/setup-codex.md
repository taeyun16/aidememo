# Setup Guide: Codex (OpenAI CLI)에서 wg 사용하기

## 개요

Codex CLI는 `.codex.md` 파일을 프로젝트 루트에서 로드합니다.
프로젝트에 `.codex.md`를 복사하면 Codex가 wg를 인식합니다.

## 설치 단계

### 1. wg 바이너리 준비

```bash
cd ~/dev/wg
cargo build -p wg-cli --release
# ./target/release/wg
```

### 2. 프로젝트에 .codex.md 복사

```bash
cp ~/dev/wg/.codex.md ~/my-project/.codex.md
```

### 3. PATH 설정

```bash
# ~/.zshrc에 추가:
export PATH="$HOME/dev/wg/target/release:$PATH"
```

## 사용법

Codex가 `.codex.md`의 지침에 따라 자동으로 wg를 호출합니다.

## 직접 호출

```
User: wg entity list
User: wg search "검색어"
User: wg traverse <엔티티> --depth 2
```

## 프로젝트별 초기화

각 프로젝트마다 위키를 초기화:
```bash
cd ~/my-project
wg init ./docs          # docs/ 디렉터리를 위키 루트로
wg ingest ./docs
```
