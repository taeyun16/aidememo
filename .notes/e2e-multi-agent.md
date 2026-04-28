# wg multi-agent 통합 e2e 결과

> 일시: 2026-04-28
> 대상: wg @ `d7aa7c2` (이슈 #1–#4 fix 적용 후), claude code, codex
> (gpt-5.4), hermes (헤르메스 에이전트 + hermes-wg plugin)
> 위치: `bench/multi-agent/`
> 결과 raw: `bench/multi-agent/results/scenario_*.json` (gitignored, 매
> 실행마다 새로 생성)

## 한 줄 결론

**A·B·C·D 네 시나리오 모두 통과** (A 3/3, B 7/7, C 3/3, D 7/7). 셋의
통합은 실 사용 수준에서 동작한다. 진행 중 발견된 wg 본체 4건은 모두
fix됨 (PR `a47d3d5`). 추가로 두 발견:
- **codex는 non-interactive mcp 호출에 sandbox/approval 우회 필요**
  (`--dangerously-bypass-approvals-and-sandbox`).
- **wg는 multi-process 동시 write 시 redb file lock 충돌로 fail-fast**
  (의도된 동작, 그러나 graceful retry/backoff는 본체 개선 후보).

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

## 시나리오 C — 자연어 prompt e2e

**목적**: 동일한 자연어 prompt를 3개 에이전트의 non-interactive CLI로
던지고, 각각 wg의 mcp tool을 실제로 호출해 답변을 만드는지 검증.

**셋업**: e2e store에 알려진 fact 5개 적재 (Redis 3, Postgres 2). 각
에이전트가 답할 수 없는 추측을 하면 즉시 들통남.

**프롬프트** (한 번, 3 에이전트 공통):
> Use the wg knowledge graph (wg_query / wg_search / wg_recent tools)
> to fetch every fact about 'Redis' that wg knows, then summarise them
> as a numbered list. Quote each fact's content verbatim and include
> its fact id. Do NOT invent or paraphrase — if wg returns nothing,
> say so explicitly. Keep the answer under 200 words.

**결과**: 3/3 통과 — 모든 에이전트가 Redis fact 3개 모두 정확히 인용.

| 에이전트 | wall | 인용된 fact id | 토큰 사용 (보고된 경우) | 비고 |
|---|---|---|---|---|
| `claude --print` | 12.8s | 3/3 | n/a (CLI 출력 없음) | 가장 빠름. Default sandbox에서 그대로 동작. |
| `codex exec --dangerously-bypass-approvals-and-sandbox` | 23.4s | 3/3 | 30 239 | **bypass 플래그가 없으면 mcp 호출이 cancel됨** (아래 발견 #5 참고). |
| `hermes chat -Q -q` | 37.6s | 3/3 | n/a | 정상. 가장 느리지만 답변에 entity fact_count까지 포함. |

세 에이전트 모두 wg의 mcp tool을 실제로 호출했고 (stderr/메타에서 확인),
답변에 추측 없이 fact id+content를 verbatim 포함. 즉 **wg는 자연어
agent의 RAG-style 컨텍스트 소스로 동작 가능**.

## 시나리오 E — mcp-serve HTTP 모드 multi-client write

**목적**: D가 보여준 stdio multi-process write의 lock 충돌을 해소하는
권장 패턴(`wg mcp-serve` 한 인스턴스 + 다중 HTTP 클라이언트)이 실제로
무상흠한 multi-agent shared write를 제공하는지 검증.

**셋업**: `wg mcp-serve --port 3939 /tmp/wg-e2e-e/wiki.redb` 백그라운드
기동, 4 client × 25 inserts (총 100) 동시 HTTP POST.

**결과**: 5/5 invariants 통과. 100/100 fact 모두 저장, ID 중복 0,
HTTP/lock 에러 0.

| 지표 | 값 |
|---|---|
| persisted | 100/100 |
| unique IDs | 100/100 |
| p50 latency | 18.6 ms |
| p95 latency | 24.6 ms |
| max latency | 27.6 ms |
| total wall | 545 ms |

**D vs E 한눈에 비교**:

| 모드 | 결과 (M=4, N=25) | wall | 비고 |
|---|---|---|---|
| D stdio default (`lock_retry_ms=0`) | CLI 7/100, MCP 25/100 | 423ms / 223ms | 사용자에게 silent loss — 위험 |
| D stdio retry (`lock_retry_ms=5000`) | CLI 100/100, MCP 100/100 | 5186ms / 868ms | 안전하지만 wait 시간 누적 |
| **E HTTP shared (`mcp-serve`)** | **100/100** | **545ms** | 가장 빠르고 가장 안전 — 권장 |

E가 D-retry보다 wall이 5배 짧은 이유: HTTP 모드는 모든 인서트가 한
process의 redb writer에서 in-memory 직렬화 → lock acquisition 라운드
없음. CLI는 매번 store open/commit/close.

**결론**: AGENTS.md "Multi-agent shared store"가 권하는 mcp-serve 단일
인스턴스 + 멀티 client 패턴은 **실증적으로 best**. lock_retry는 1회성
backup, 정기적 multi-agent write에는 mcp-serve가 맞음.

## 시나리오 D — 동시 라이터 락 동작

**목적**: 4개 프로세스가 같은 store에 N=25개씩 fact를 동시 추가했을 때
데이터 무결성·deadlock·ID 중복 여부 검증.

**결과**: 7/7 invariants 통과 — 단, 100개 중 일부만 저장됨 (CLI 7개,
MCP 25개). 이는 **redb의 single-process file lock 정책**이 의도대로
작동한 결과:

| invariant | CLI | MCP |
|---|---|---|
| 성공한 fact 간 ID 중복 없음 | ✓ | ✓ |
| 최소 1개 write 성공 | ✓ (7/100) | ✓ (25/100, 한 process만) |
| 60초 이내 종료 (deadlock 없음) | ✓ (423ms) | ✓ (223ms) |
| 모든 실패가 lock 또는 no-content 에러 | ✓ | ✓ |

**핵심 관찰**:
- MCP 모드: 한 process가 stdio session을 잡으면 그 안 N=25 inserts
  모두 성공, 다른 3개 process는 startup의 `Database already open`로
  fail. → **단일 long-lived `wg mcp` 안에서는 직렬화가 정상 동작**.
- CLI 모드: 매 호출마다 store open/close, 4개 process가 lock을 racing
  → 7개만 우연히 성공. **실제 사용 시 사용자가 race를 인지 못 할 수
  있어 위험**.

**시사점**: wg를 여러 에이전트가 공유 write할 거면 셋 중 하나 필요:
1. `wg mcp-serve --port 3000` HTTP/SSE 모드 (단일 서버, 멀티 클라이언트)
2. 에이전트마다 별도 store
3. 외부 락/큐 프로토콜로 직렬화

현재 .mcp.json/codex/hermes 모두 stdio로 wg mcp를 spawn하므로 (1)이
아니면 cross-agent shared write는 위험.

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

## 추가로 발견된 이슈

### 5. wg의 lock 충돌 시 즉시 fail (multi-agent 사용성)

`wg --store /path fact add ...` (CLI) 또는 `wg mcp /path` (MCP)가 다른
process에 의해 store가 이미 열려 있으면 `Database already open. Cannot
acquire lock.`로 즉시 종료. retry/backoff 없음.

**시나리오 D**가 보여주듯, 여러 에이전트가 stdio mcp로 wg를 spawn하면
한 명만 lock 잡고 나머지는 즉시 fail. claude-code가 `.mcp.json`으로
wg를 spawn한 상태에서 사용자가 별도 터미널에서 `wg fact add`를 치면
같은 에러. 사용자 입장에서는 "왜 안 돼?"가 됨.

**Fix 후보 (낮은 위험도)**:
- `Store::open`에 retry-with-backoff 옵션 추가 (예: 100ms × 5회).
- `wg --store ... <cmd>`의 글로벌 옵션으로 `--lock-retry <ms>` 노출.
- 또는 적어도 에러 메시지에 "다른 wg mcp 인스턴스가 열려 있을 수
  있음 — `wg mcp-serve`로 공유하거나 `--store`로 별도 store를
  지정하세요" 같은 힌트.

### 6. codex의 default sandbox/approval이 MCP tool을 silent cancel

`codex exec` 또는 `codex exec --full-auto`로 wg mcp tool을 부르면
codex가 호출을 자동 cancel하고 CLI fallback을 시도. 그 fallback이
잘못된 store(`./_meta/wiki.redb`)를 보고 빈 결과 반환 → 에이전트가
"wg에 데이터가 없습니다"라고 거짓 보고.

이건 wg 본체 이슈는 아니고 **codex 측 설정 문제**지만, wg를 codex와
연결할 때 `--dangerously-bypass-approvals-and-sandbox` 또는 특정
config(`approval_policy="never"`)이 필요함을 wg-skill의 setup-codex.md에
명시해야 사용자가 같은 함정에 빠지지 않음.

## 다음 검토 후보

1. **본체 이슈 #5 (lock retry)** — 한 PR로 추가 가능, 시나리오 D를
   회귀 테스트로 활용.
2. **wg-skill setup-codex.md 업데이트** — codex 측 sandbox/approval 우회
   필요성 안내 (1줄 추가).
3. **wg mcp-serve로 단일 인스턴스 공유 패턴 가이드** — `.mcp.json`에서
   stdio 대신 HTTP/SSE 모드를 써서 multi-agent shared write 가능.
4. **시나리오 D-2 (single-process 직렬화 회귀 테스트)** — 한 wg mcp에
   다중 stdio session을 보내면서 데이터 무결성 검증. CI에 넣을 가치.
