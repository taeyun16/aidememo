---
title: 릴리스 체크리스트
description: AideMemo 패키지의 배포 순서와 프리플라이트 검사를 설명합니다.
---

# 릴리스 체크리스트

이 페이지는 패키지 배포 순서를 기록합니다. 의도적으로 보수적입니다. 저수준
패키지를 먼저 배포하고 그 위 계층을 설치해 smoke test합니다.

## 1. 레지스트리와 저장소 설정

첫 실제 배포 전에 repository environment와 registry trusted-publisher 항목을
만듭니다. 설정된 PyPI와 npm 워크플로는 OIDC를 사용하므로 일반 릴리스 경로에
장기 `PYPI_API_TOKEN` 또는 `NPM_TOKEN` 저장소 secret을 추가하지 않습니다.

GitHub environment:

| Environment | 워크플로 | 목적 |
|---|---|---|
| `crates-publish` | `.github/workflows/crates-publish.yml` | crates.io trusted publishing 승인 게이트 |
| `pypi-publish` | `.github/workflows/aidememo-python-publish.yml`, `.github/workflows/aidememo-agent-sdk-publish.yml`, `.github/workflows/hermes-aidememo-publish.yml` | PyPI trusted publishing 승인 게이트 |
| `npm-publish` | `.github/workflows/aidememo-napi-publish.yml` | npm trusted publishing 승인 게이트 |
| `github-pages` | `.github/workflows/pages.yml` | 검증된 Docusaurus 빌드의 OIDC 기반 배포 |

레지스트리 environment에는 reviewer를 요구하고 배포 branch/tag를 프로젝트가
사용하는 릴리스 branch 또는 tag로 제한하는 것을 권장합니다.

첫 문서 배포 전에 **Settings → Pages**에서 publishing source를 **GitHub
Actions**로 설정하고, 생성된 `github-pages` environment를 `main`으로
제한합니다. Pages 워크플로는 빌드 중 repository contents를 read-only로
유지하고 배포 job에만 `pages: write`와 `id-token: write`를 허용합니다. 지원되는
유료 플랜에서 private repository로 배포하더라도 GitHub Pages 콘텐츠는 공개
인터넷에서 접근할 수 있으므로 사이트가 공개 준비를 마치기 전에는 활성화하지
않습니다.

PyPI trusted publisher:

| 프로젝트 | GitHub owner/repo | 워크플로 | Environment | 상태 |
|---|---|---|---|---|
| `aidememo-python` | `taeyun16/aidememo` | `aidememo-python-publish.yml` | `pypi-publish` | 워크플로 준비 완료 |
| `aidememo-agent-sdk` | `taeyun16/aidememo` | `aidememo-agent-sdk-publish.yml` | `pypi-publish` | 워크플로 준비 완료 |
| `hermes-aidememo` | `taeyun16/aidememo` | `hermes-aidememo-publish.yml` | `pypi-publish` | 워크플로 준비 완료 |

npm trusted publisher:

각 npm 패키지를 GitHub owner/repo `taeyun16/aidememo`, workflow
`aidememo-napi-publish.yml`, environment `npm-publish`로 등록합니다.

1. `aidememo-napi`
2. `aidememo-napi-darwin-arm64`
3. `aidememo-napi-darwin-x64`
4. `aidememo-napi-linux-arm64-gnu`
5. `aidememo-napi-linux-x64-gnu`
6. `aidememo-napi-win32-x64-msvc`

npm은 trusted publisher 설정 전에 패키지가 registry에 존재해야 하므로 최초
릴리스에서는 `npm-publish` environment secret `NPM_TOKEN`에만 저장한 단기
granular token을 사용합니다. Workflow를 `dry_run=false`, `bootstrap=true`로 한
번 실행하면 GitHub-hosted native runner에서 platform 패키지 5개를 빌드하고,
모든 platform job 성공 후 root wrapper를 배포합니다. 6개 패키지 모두 trusted
publisher를 등록한 다음 environment secret과 token을 삭제하고 bootstrap 경로를
제거합니다. 일반 릴리스는 `bootstrap=false`로 실행하고 OIDC만 사용합니다.

Rust 크레이트는 `.github/workflows/crates-publish.yml`과 `crates-publish`
environment, crates.io OIDC trusted publisher를 통해 배포합니다. 최초 릴리스만
운영자 머신에서 bootstrap했으며 일반 릴리스에서는 repository
`CARGO_REGISTRY_TOKEN` secret을 사용하지 않습니다.

Elixir NIF는 현재 로컬/경로 바인딩으로 문서화되어 있습니다. 아직 Hex 배포
워크플로와 repository `HEX_API_KEY` 요구사항은 없으며, 프로젝트가
`aidememo_nif`를 Hex로 배포하기로 결정할 때만 추가합니다.

`OPENAI_API_KEY` 같은 선택형 런타임 key는 로컬 기능
(`aidememo extract --llm`)에 사용하며 릴리스 secret이 아닙니다.

## 2. 로컬 프리플라이트

깨끗한 체크아웃에서 로컬 릴리스 게이트를 실행합니다.

```bash
scripts/release-preflight.sh
```

같은 게이트는 수동 GitHub workflow
`.github/workflows/release-preflight.yml`에서도 사용할 수 있습니다. 일반
release candidate 검사에는 `profile=local`을 사용합니다. Rust 패키지
dry-run 또는 Python/npm 배포 dry-run도 필요할 때만 `profile=full`을
사용하고 workflow input으로 명시적으로 활성화합니다.

이 게이트에는 버전 pin, changelog 릴리스 게이트, registry readiness, public
portability, workflow syntax lint, docs feature coverage, docs-site build,
binding smoke, agent SDK/Hermes wheel smoke, workflow smoke, SDK promotion
check가 포함됩니다.

Changelog 릴리스 게이트는 오프라인이며 현재 릴리스 노트를 `Unreleased`에서
분리한 뒤 통과해야 합니다.

```bash
mise run changelog-release-check
python3 scripts/changelog-release-check.py 0.1.0
```

`CHANGELOG.md`에 빈 `[Unreleased]` section이 하나 있고, 바로 아래에 날짜가
있는 현재 버전 section과 비어 있지 않은 release note가 있는지 확인합니다.
집중된 비릴리스 디버깅에서만 `AIDEMEMO_RELEASE_PREFLIGHT_CHANGELOG=0`을
설정합니다.

Registry readiness 게이트도 오프라인이며 registry 항목을 만들거나 수정하기
전에 통과해야 합니다.

```bash
python3 scripts/registry-readiness-check.py
```

PyPI trusted-publisher project 이름, workflow 이름, GitHub environment, npm
root/platform 패키지 이름, 이 릴리스 문서가 일치하는지 확인합니다. 또한
first-party publish workflow가 장기 publish token 가정으로 회귀하는 것을
거부합니다.

릴리스 tag를 push하기 전에 public onboarding 게이트도 통과해야 합니다.

```bash
python3 scripts/public-portability-check.py
scripts/fresh-checkout-smoke.sh
```

Portability 게이트는 first-party 추적 파일의 개발자별 home path를 거부합니다.
Fresh-checkout smoke는 `.github/workflows/fresh-checkout-smoke.yml`에서도
사용할 수 있으며 `.git`, `target`, Node build output이 없는 복사본에서 다시
빌드하고 결정적인 빠른 시작을 실행합니다.

`maturin`은 `PATH`에서 우연히 발견된 버전이 아니라 `mise.toml`의 고정된
spec을 사용해 `uvx`로 실행합니다.

Python 바인딩은 PyO3 0.29를 사용합니다. Release smoke script는 CI와 같은
Python 3.13 interpreter를 우선하지만 PyO3가 지원하는 로컬 interpreter도
받습니다. 특정 interpreter를 강제하려면 명시적으로 설정합니다.

```bash
AIDEMEMO_PYO3_PYTHON=python3.13 scripts/release-preflight.sh
```

전체 registry dry-run:

```bash
AIDEMEMO_RELEASE_PREFLIGHT_PROFILE=full scripts/release-preflight.sh 0.1.0
```

Full profile은 Rust publish dry-run readiness 게이트도 실행합니다. 독립 실행:

```bash
scripts/cargo-package-readiness.sh
```

CI의 `cargo-package-readiness` job도 같은 게이트를 실행합니다. 이 PR guard는
`aidememo-core`의 `cargo publish --dry-run`을 강제하고, `aidememo-core`가
crates.io에 존재할 때까지 의존 Rust 크레이트를 문서화된 배포 순서 skip으로
유지합니다.

## 3. 릴리스 tag 계약

정식 소스 릴리스 tag는 `v<version>`이며 첫 공개 릴리스는 `v0.1.0`을
사용합니다. 원격 CI와 full release preflight가 배포할 정확한 commit에서
통과한 뒤에만 생성합니다.

```bash
git tag -a v0.1.0 -m "AideMemo 0.1.0"
git push origin v0.1.0
```

`aidememo-python-v0.1.0`, `aidememo-napi-v0.1.0` 같은 패키지별 tag는 선택형
artifact 또는 dry-run trigger입니다. 정식 `v0.1.0` 소스 tag를 대체하지
않습니다. 실제 PyPI와 npm 배포는 정확한 버전 input과 approval environment를
사용하는 수동 workflow dispatch로 유지합니다.

## 4. Rust 크레이트

의존 순서대로 배포합니다.

1. `aidememo-core`
2. `aidememo-cli`
3. `aidememo-ffi`, `aidememo-napi`, `aidememo-nif`, `aidememo-python`

`aidememo-cli`와 모든 네이티브 바인딩은 `aidememo-core`에 의존하므로
같은 버전의 `aidememo-core`가 crates.io에 배포되기 전에는
`cargo publish --dry-run` 검사가 실패합니다.

Readiness script는 기본적으로 `aidememo-core`에 `cargo publish --dry-run`을
실행하고, 첫 배포 순서 blocker가 사라질 때까지 의존 Rust 크레이트를 의도적인
skip으로 기록합니다. 같은 버전의 `aidememo-core`가 crates.io에 보이면 전체
의존 검사를 실행합니다.

```bash
AIDEMEMO_CARGO_PACKAGE_CHECK_DEPENDENTS=1 scripts/cargo-package-readiness.sh
```

## 5. Python 패키지

Composition 패키지보다 네이티브 바인딩을 먼저 배포합니다.

1. `aidememo-python`
2. `aidememo-agent-sdk`
3. `hermes-aidememo`

로컬 Python payload 검사:

```bash
mise run python-pack-smoke
mise run python-publish-dry-run
mise run agent-sdk-publish-dry-run
mise run hermes-publish-dry-run
```

첫 PyPI 릴리스 전 사용자 문서는 checkout 설치를 보여야 합니다.

```bash
python -m pip install -e packages/aidememo-agent-sdk
python -m pip install -e plugins/hermes
```

PyPI 릴리스 후에는 다음 설치를 안내할 수 있습니다.

```bash
python -m pip install aidememo-agent-sdk
python -m pip install "aidememo-agent-sdk[binding]"
python -m pip install hermes-aidememo
```

## 6. Node 패키지

Root wrapper보다 platform 패키지를 먼저 배포합니다.

1. `aidememo-napi-*` platform 패키지
2. `aidememo-napi`

최초 1회 배포에서는 수동 workflow에 정확한 버전과 `dry_run=false`,
`bootstrap=true`를 입력합니다. 일부 배포 후 재실행해도 npm에 이미 보이는
package version은 건너뜁니다. Trusted publisher 설정 후에는
`bootstrap=false`를 사용합니다.

정확한 버전 input으로 각 trusted-publisher workflow를 사용합니다. 기본
workflow mode는 dry-run입니다.

1. `.github/workflows/aidememo-python-publish.yml`
2. `.github/workflows/aidememo-agent-sdk-publish.yml`
3. `.github/workflows/hermes-aidememo-publish.yml`

## 7. 릴리스 후 검사

레지스트리가 공개되기 전에는 post-release smoke를 plan mode로 실행해 수행할
설치 검사를 확인할 수 있습니다.

```bash
scripts/public-registry-smoke.sh
```

각 registry 배포 후 verify mode를 실행합니다.

```bash
AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE=verify scripts/public-registry-smoke.sh
```

같은 검사는 수동 GitHub workflow
`.github/workflows/public-registry-smoke.yml`에서도 사용할 수 있습니다.
레지스트리 공개 전에는 `mode=plan`, 배포 후에는 정확한 릴리스 버전과
`mode=verify`를 사용합니다. Workflow는 registry별 toggle을 제공하므로 일부만
배포된 경우 변경된 registry만 검증할 수 있습니다.

이 검사는 임시 환경에 공개 레지스트리의 `aidememo-cli`,
`aidememo-agent-sdk`, `aidememo-agent-sdk[binding]`, `hermes-aidememo`,
`aidememo-napi`를 설치하고 import 또는 실행합니다. 한 registry만 배포했다면
`AIDEMEMO_PUBLIC_REGISTRY_SMOKE_*` toggle로 범위를 좁힙니다.

그런 다음 실제 공개된 패키지의 README와 문서에서 "릴리스 전에는 checkout"
주의 문구를 제거합니다.
