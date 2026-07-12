---
title: 설치
description: AideMemo를 설치하고 CLI 동작을 확인합니다.
---

# 설치

기본 바이너리는 `aidememo`이며 CLI와 MCP 서버를 모두 포함합니다.

## crates.io에서 설치

```bash
cargo install aidememo-cli
```

## Git에서 설치

```bash
cargo install --git https://github.com/taeyun16/aidememo aidememo-cli
```

바이너리를 확인합니다.

```bash
aidememo --help
aidememo stats
```

셸에서 명령을 찾지 못하면 Cargo 바이너리 디렉터리를 PATH에 추가합니다.

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## 체크아웃에서 설치

```bash
git clone https://github.com/taeyun16/aidememo.git
cd aidememo
mise install
cargo build -p aidememo-cli --release
export PATH="$PWD/target/release:$PATH"
```

로컬 개발 도구 버전은 `mise.toml`에 고정되어 있습니다.

```bash
mise run changelog-release-check
mise run release-preflight
mise run cargo-package-readiness
mise run public-registry-smoke
mise run public-portability-check
mise run fresh-checkout-smoke
mise run docs-build
mise run ci-lint
mise run ci-test
```

동일한 릴리스 프리플라이트는 GitHub Actions의
`.github/workflows/release-preflight.yml`에서도 실행할 수 있습니다. 깨끗한
체크아웃 온보딩 경로는 `.github/workflows/fresh-checkout-smoke.yml`에서
제공되며, 수동 실행과 설치기·CLI·코어·스모크 변경 PR에서 실행됩니다.

`changelog-release-check`는 빠른 릴리스 노트 게이트입니다. 더 넓은
프리플라이트가 문서, 레지스트리, 워크플로를 검사하기 전에 `CHANGELOG.md`가
현재 워크스페이스 버전으로 정리됐는지 확인합니다. 전체 릴리스 프리플라이트와
`cargo-package-readiness`는 Rust 배포 드라이런을 검사합니다. 먼저
`aidememo-core`를 검증하고, 같은 버전의 코어 크레이트가 배포된 뒤에 의존
크레이트를 검사합니다.

`scripts/fresh-checkout-smoke.sh`는 `target`과 `node_modules` 없이 체크아웃을
임시 디렉터리로 복사하고 CLI를 빌드한 뒤 결정적인 빠른 시작 경로를
검증합니다. `scripts/public-portability-check.py`는 공개 예제와 워크플로가
특정 개발자 환경에 의존하지 않도록 추적 파일의 macOS, Linux, Windows 홈
경로를 거부합니다. `public-registry-smoke`는 기본적으로 배포 후 계획만
출력합니다. 실제 레지스트리 배포 뒤에는
`AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE=verify`로 실행해 임시 환경에서 공개
패키지를 설치하세요.

네이티브 Python 바인딩 릴리스 경로는 고정된 `maturin` 빌드 도구를 `uvx`로
실행하므로 `mise install`만으로 로컬 wheel과 배포 드라이런을 재현할 수
있습니다.

```bash
mise run python-pack-smoke
mise run python-publish-dry-run
```

순수 Python 에이전트 패키지의 배포 드라이런은 로컬 Python 빌드 백엔드를
사용합니다.

```bash
mise run agent-sdk-publish-dry-run
mise run hermes-publish-dry-run
```

## 저장소 위치

기본적으로 AideMemo는 설정된 로컬 저장소를 사용합니다. 예제와 스크립트에는
명시적인 저장소 경로를 전달할 수 있습니다.

```bash
aidememo --store ./memory.sqlite stats
aidememo --store ./memory.sqlite fact add "A first note" --entities Project
```

`--store`는 데모, 테스트, 프로젝트별 메모리 파일에 유용합니다.

에이전트 설치는 확인된 저장소 경로를 생성한 MCP 명령에 고정합니다. 격리된 Codex
계정에는 같은 저장소를 각 프로필에 설치하면서 작성자 provenance를 분리합니다.

```bash
aidememo --store ./_meta/wiki.sqlite mcp-install --target codex \
  --codex-home "$HOME/.codex-account-a" --actor-id codex:account-a \
  --codex-home "$HOME/.codex-account-b" --actor-id codex:account-b \
  --source-id project:my-app
```

전체 인계 및 동시성 패턴은 [`여러 Codex 프로필에서 메모리 공유`](CODEX_MULTI_PROFILE.md)를
참고하십시오.

## 코딩 에이전트에 설치

CLI와 store가 동작하면 에이전트의 네이티브 방식으로 연결합니다.

| 에이전트 | 설치 경로 |
|---|---|
| Claude Code | 내장 플러그인 또는 `mcp-install --target claude` + `skill install --target claude` |
| Codex | 선택적으로 `--codex-home` 프로필을 반복하는 `mcp-install --target codex` |
| Hermes Agent | `skill install --target hermes` + `mcp-install --target hermes`, 또는 `hermes-aidememo` 플러그인 |
| pi coding agent | `skill install --target pi`. MCP 단계 없음 |
| Cursor / OpenClaw / OpenCode | 각 `mcp-install` 대상과 지원되는 스킬 |

정확한 명령, 프로필 변수, 플러그인 선택, 검증 단계는
[`코딩 에이전트 설치`](CODING_AGENTS.md)를 참고하세요. CLI에서 현재 지원
대상을 바로 출력할 수도 있습니다.

```bash
aidememo mcp-install --list-targets
aidememo skill install --list-targets
```

## 권장 첫 확인

임시 디렉터리에서 다음 명령을 실행합니다.

```bash
STORE="$(mktemp -d)/wiki.sqlite"

aidememo --store "$STORE" fact add \
  "Decision: AideMemo stores typed project memory locally." \
  --type decision \
  --entities AideMemo

aidememo --store "$STORE" search "typed project memory"
```

방금 추가한 팩트가 출력되어야 합니다.
