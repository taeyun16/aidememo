# wg multi-agent 통합 e2e 결과

> 일시: 2026-04-28
> 대상: wg @ `22fb34a`, claude code, codex (gpt-5.4), hermes (헤르메스 에이전트 + hermes-wg plugin)
> 위치: `bench/multi-agent/`
> 결과 raw: `bench/multi-agent/results/scenario_a.json`, `…/scenario_b.json`

## 한 줄 결론

**A + B 모두 통과 (3/3, 7/7)**. 셋의 통합은 동작한다. 다만 진행 중에 wg
본체의 작은 버그/UX 이슈 4건이 드러났고, 그중 하나는 사용자가 실제로
부딪힐 가능성이 높다 (`wg mcp`가 글로벌 `--store`를 무시).

## 환경 셋업

| 항목 | 값 | 비고 |
|---|---|---|
| wg release 빌드 | `target/release/wg` | `cargo build -p wg-cli --release` |
| PATH symlink | `~/.local/bin/wg` → `target/release/wg` | hermes plugin이 PATH lookup 사용 |
| e2e 전용 store | `~/.wg-e2e/wiki.redb` | 사용자 개인 store와 격리 |
| Claude Code | `.mcp.json` (이미 있음) | `./target/debug/wg mcp` |
| Codex | `~/.codex/config.toml` | 새 `[mcp_servers.wg]` 블록 추가 (백업: `.bak`) |
| Hermes | `plugins/hermes/src/hermes_wg/client.py` | `WgClient(store_path)` CLI fallback |

## 시나리오 A — MCP 프로토콜 smoke

**목적**: 각 에이전트의 통합 형태로 wg mcp를 spawn했을 때 핸드셰이크 →
tools/list → wg_query 1회가 모두 성공하는지.

**결과**: 3/3 통과 (전수 동일 응답)

| 클라이언트 형태 | 핸드셰이크 | tool 수 | wg_query 키 | latency |
|---|---|---|---|---|
| claude-code-shape (`./target/debug/wg mcp`) | ✓ (proto 2025-06-18) | 13 | `entity / recent_facts / related / search / topic` | 1702 ms |
| codex-shape (`~/.local/bin/wg mcp <STORE>`) | ✓ | 13 | 동일 | 747 ms |
| hermes-shape (release wg + STORE positional) | ✓ | 13 | 동일 | 712 ms |

claude-code-shape의 latency가 ~2배인 것은 `target/debug/`(unoptimized)와
`target/release/`의 차이. 셋 다 같은 JSON-RPC 응답을 반환하므로 클라이언트
간 결정적 일관성은 OK.

## 시나리오 B — cross-client store 일관성

**목적**: 한 클라이언트로 fact 추가 → 다른 둘이 동일 store에서 read하면
같은 데이터를 보는가.

**결과**: 7/7 invariant 통과

| Step | Actor | Action | 결과 |
|---|---|---|---|
| 1 | claude-shape | `wg_fact_add` "Redis Sentinel..." | id=`01KQ91W0WCC...` |
| 2 | codex-shape | `wg_recent` | facts=1 (fact_a 보임) ✓ |
| 3 | hermes-shape | `wg recent` (CLI) | facts=1 (fact_a 보임) ✓ |
| 4 | codex-shape | `wg_fact_add` "Postgres..." | id=`01KQ91W12QM...` |
| 5 | claude-shape | `wg_recent` | facts=2 (둘 다 보임) ✓ |
| 6 | hermes-shape | `wg recent` | facts=2 (둘 다 보임) ✓ |
| — | codex-shape | `wg_query Redis` | fact_a hit ✓ |
| — | hermes-shape | `wg query Redis` (CLI) | fact_a hit ✓ |

세 가지 진입 형태 (in-process MCP / 별도 프로세스 MCP / CLI shell-out)
모두 같은 redb store를 보고 같은 데이터를 round-trip한다. **에이전트 A가
저장한 사실을 에이전트 B가 곧바로 읽을 수 있다는 e2e 보장이 성립**한다.

## 진행 중에 드러난 wg 본체 이슈 → 모두 해결

> 4건 모두 같은 PR에서 fix. 기존 시나리오 A/B 모두 통과 유지 + 새
> 단위 테스트 4건 추가. 자세한 변경은 git diff 참고.

### 1. `wg mcp`가 글로벌 `--store` 옵션을 무시 (사용성) — **fixed**

```bash
wg --store /custom/path mcp     # 동작 안 함 → ./_meta/wiki.redb 사용
wg mcp /custom/path             # 동작 (positional)
```

`crates/wg-cli/src/main.rs:97`에서 `cmd::Command::Mcp(sub)` 분기가
`store_path`(글로벌)을 무시하고 `sub.wiki_root`(positional)만 전달.
다른 모든 서브커맨드는 `store_path`를 받는다. 이 비일관성 때문에
`~/.codex/config.toml`에 wg를 등록할 때 `args = ["mcp", STORE]`로
positional로 줘야 함을 e2e 셋업 중 우연히 발견.

**Fix**: `Mcp(sub)` 분기에서 `sub.wiki_root.unwrap_or(store_path)`로
바꿔 다른 명령들과 동일 의미. 검증: `wg --store /path mcp` 호출 시
`stdio transport ready (store=/path)` 출력 확인.

### 2. `wg_recent` MCP tool과 CLI `wg recent`의 인자 표면이 다름 — **fixed**

| 표면 | 시간 인자 | 한도 인자 |
|---|---|---|
| MCP tool `wg_recent` | `last_days` (정수, 기본 7) | `limit` (정수, 기본 20) |
| CLI `wg recent` | `--last <30d/12h/4w/1y>` (DSL) | `-n N` |

호출자가 둘을 헷갈려서 시나리오 B 1차 실행에서 `last="1h"`을 MCP tool에
넘겼는데 그건 무시되고 default(7일)만 남아 통과해버림. 명시 인자는
**무시되고도 에러를 내지 않는다** — silently dropped. 추가로 MCP는 시간
범위를 일 단위로만 표현 가능해서 `last_hours`/`since` 등이 필요한
시나리오를 표현 못 함.

**Fix**: MCP `wg_recent`에 `last` (DSL string) 인자를 추가했음.
`last_days`는 backward-compat로 유지. 둘 다 주어지면 `last`가 우선.
잘못된 DSL 문자열은 `tool error`로 거부 (e.g. `last="30"` → "duration
needs a unit suffix").

### 3. `wg_fact_add` 응답이 plain text "Fact added: <ULID>" — **fixed**

다른 모든 read tool은 JSON, write tool은 `wg_fact_add_many`도 JSON
(`{"ids":[...]}`)인데 `wg_fact_add`만 텍스트 한 줄. 클라이언트가 ULID를
얻으려면 split해야 함.

**Fix**: `{"id": "<ULID>"}` JSON으로 통일. wg-ffi 헬퍼와 동일 shape.

### 4. `wg_recent` 응답이 raw JSON array (다른 tool은 객체로 wrap) — **fixed**

```json
// wg_query → {"topic": "...", "search": [...], "related": [...], ...}
// wg_recent → [ {fact_a}, {fact_b} ]   ← top-level array
```

JSON-RPC envelope 자체에는 영향 없지만 클라이언트가 `data["facts"]`
같은 일관된 접근 패턴을 못 씀. 작은 표면 일관성 이슈.

**Fix**: `{"facts": [...]}`로 wrap. 단위 테스트 `recent_wraps_facts_in_object`
추가.

## 비교 — Claude Code, Codex, Hermes 의 wg 통합 특성

| 축 | Claude Code | Codex | Hermes |
|---|---|---|---|
| 등록 위치 | 프로젝트 `.mcp.json` | `~/.codex/config.toml` | Python plugin (`hermes-wg`) |
| 호출 형태 | stdio MCP (in-process spawn) | stdio MCP (config 기반 spawn) | wg CLI shell-out (default) 또는 wg-python in-process (옵션) |
| store path 명시 방식 | 프로젝트 .mcp.json args | config args | `WgClient(store_path=...)` |
| cold start | `wg` binary spawn (debug ~1.7s, release ~0.7s) | 동일 | CLI 호출당 spawn — 멀티콜 시 누적 |
| 멀티 에이전트 충돌 | 단일 redb 라이터 락 | 동일 | 동일 (CLI도 wg를 spawn하므로) |
| 강점 | 프로젝트별 store 분리 자연스러움 | 글로벌 단일 wg 인스턴스, 어느 워킹 디렉토리에서도 동일 store | Python 코드에서 wg를 import해 쓸 수 있음 |
| 약점 | 프로젝트마다 .mcp.json 필요 | mcp install 시 글로벌 변경 (다른 프로젝트에도 영향) | CLI shell-out 비용, MCP tool 표면을 우회 |

## 다음 검토 후보

1. **위 본체 이슈 4건을 정식 PR/이슈로 등록** — 특히 #1은 사용자 발 걸리는 패턴.
2. **시나리오 C (자연어 prompt 통합)** 진행 여부 — `claude --print`,
   `codex exec`, hermes non-interactive로 같은 프롬프트를 던지고 wg를
   실제로 호출해서 답변하는지 + tool-call 횟수 + 답변 일치/품질.
   모델 비용이 들고 비결정적이라 1회 데모 정도가 적절.
3. **동시성 시나리오** — 2개 에이전트가 동시에 wg_fact_add → 누가 이기나,
   redb 라이터 락이 깔끔하게 양보하는지. wg는 single-writer라 직렬화
   되어야 함.
