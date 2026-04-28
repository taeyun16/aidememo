# wg ↔ beads 벤치마크 설계 (prototype)

> 상태: **설계만**, 실제 실행은 beads(brew bottle, dolt 의존) 설치 후
> 사용자 승인을 받아 진행. 본 문서는 코드를 넣기 전 fair-comparison
> 절차를 합의하기 위함.

## 0. 사전 준비

```bash
# beads 설치 (dolt + icu4c 컴파일됨, ~수백 MB)
brew install beads          # 1.0.3
which bd                    # /opt/homebrew/bin/bd

# wg 빌드
cargo build --release -p wg-cli --features wg-core/semantic
ln -sf "$PWD/target/release/wg" /tmp/wg-bench

# 환경 정보 캡처
sw_vers                     # macOS arm64 / chip 식별
rustc --version             # 1.95.0
bd --version                # 1.0.3
```

bench HW 동일 셋: M-series, 무관한 프로세스 종료, AC 전원, 30초 idle 후
시작. 결과는 `bench-beads/` 아래 jsonl로 누적.

## 1. 데이터셋 — 동일 입력

`bench-beads/gen.py` (Python 3.12 표준라이브러리만):
- N개 record 생성. 각 record: `{id, title (~64B), description (~256B),
  type∈{decision,convention,bug,task,note}, deps:[...]}` — `deps`는
  앞서 생성된 record 중 무작위 0~3건 (평균 1.5).
- 시드 고정 (`--seed 42`).
- 출력 두 가지:
  - `corpus_wg.jsonl` — `{entities:[...], facts:[...], relations:[...]}`
    (wg의 `fact_add_many` + `relation_add` 입력)
  - `corpus_beads.jsonl` — beads `bd batch import` 형식

크기: `N ∈ {1000, 10000}`. 두 stage 모두 측정.

## 2. 시나리오 #1 — Bulk write throughput

**측정 항목**: insert wall time, 평균 throughput (records/s), final
on-disk 사이즈.

```bash
# wg
TMP=$(mktemp -d)
/usr/bin/time -l /tmp/wg-bench --store $TMP/wiki.redb \
    fact ingest-jsonl < corpus_wg.jsonl
du -sh $TMP/wiki.redb

# beads
TMP=$(mktemp -d)
cd $TMP && bd init --quiet --stealth
/usr/bin/time -l bd batch import < corpus_beads.jsonl
du -sh .beads/embeddeddolt
```

설정 정렬: 양쪽 모두 immediate-fsync. `wg config set
store.durability immediate` (default). beads는 Dolt 임베디드 single-tx.

⚠️ **wg 측 누락**: 현재 wg CLI에는 stdin JSONL 일괄 적재 커맨드가
없음. 두 가지 옵션:
  (a) `wg fact_add_many` MCP 도구를 1회 호출하는 stdio 클라이언트 작성
      (Python 7줄)
  (b) `wg-cli`에 `wg fact ingest-jsonl` 추가 (소규모 patch)
  
권장: **(a)** — 실제 에이전트 사용 시나리오와 일치, wg 코드 변경 불요.

## 3. 시나리오 #2 — Graph traversal latency

**측정 항목**: 1000회 무작위 depth-3 walk의 p50/p95/p99.

```python
# bench-beads/run_traverse.py
import random, subprocess, time, json, statistics
ids = open("ids.txt").read().split()
random.seed(42); random.shuffle(ids); ids = ids[:1000]
def time_ms(cmd):
    t = time.perf_counter_ns(); subprocess.run(cmd, capture_output=True, check=True)
    return (time.perf_counter_ns() - t) / 1e6

wg_lat = [time_ms(["/tmp/wg-bench","--store","..","traverse",i,"-d","3","--json"]) for i in ids]
bd_lat = [time_ms(["bd","show",i,"--json","--with-dep"]) for i in ids]   # bd has no native depth flag
print(f"wg p50={statistics.median(wg_lat):.2f} p95={statistics.quantiles(wg_lat,n=20)[18]:.2f}")
print(f"bd p50={statistics.median(bd_lat):.2f} p95={statistics.quantiles(bd_lat,n=20)[18]:.2f}")
```

⚠️ **공정성 문제**: wg는 단일 process call로 depth-N traverse. beads의
`bd show`는 직접 deps만, transitive는 `bd dep` 등으로 추가 호출 →
beads 쪽이 N-hop이면 N+1 호출 필요. 솔루션: **MCP 경로**로 통일 측정
(아래 #3 시나리오).

## 4. 시나리오 #3 — One-shot agent context fetch

**가장 중요한 측정**. 에이전트가 실제로 받는 비용.

| 측정 | 명령 | 측정 도구 |
|---|---|---|
| wg CLI | `wg query <topic> --json` | `time` |
| wg MCP stdio | JSON-RPC `wg_query` 1회 | Python harness |
| beads CLI | `bd prime <id> --json` | `time` |
| beads MCP | `beads-mcp` Python shim → `bd ...` | Python harness |

```python
# bench-beads/run_oneshot.py — MCP 경로 측정
import json, subprocess, time

def mcp_call(cmd, payload):
    t = time.perf_counter_ns()
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    out, _ = p.communicate(json.dumps(payload).encode() + b"\n")
    return (time.perf_counter_ns() - t) / 1e6, len(out)

# wg
lat, bytes_out = mcp_call(["/tmp/wg-bench","mcp"], {
    "jsonrpc":"2.0","id":1,"method":"tools/call",
    "params":{"name":"wg_query","arguments":{"topic":"Redis","limit":10,"depth":2}}
})
# beads
lat, bytes_out = mcp_call(["uvx","beads-mcp"], {...})
```

추가 측정: **응답 토큰 수** (tiktoken). beads README는 1–2k 토큰 예산
주장 — 실제로 그러한지 확인. wg의 `query` 결과는 search/related/recent
3섹션이라 더 풍부할 가능성.

## 5. 결과 포맷

`bench-beads/results/<date>.json`:

```json
{
  "env": {
    "machine": "M2 Max", "os": "macOS 26.0",
    "wg": "714f41c", "beads": "1.0.3", "rust": "1.95.0"
  },
  "scenario_1_bulk": {
    "n": 10000,
    "wg":   {"wall_ms": ..., "throughput_per_s": ..., "size_bytes": ...},
    "beads":{"wall_ms": ..., "throughput_per_s": ..., "size_bytes": ...}
  },
  "scenario_2_traverse": { "n": 1000, "wg":{"p50":..,"p95":..}, "beads":{...} },
  "scenario_3_oneshot": {
    "n": 100,
    "wg_cli":   {"p50":..,"tokens_p50":...},
    "wg_mcp":   {"p50":..,"tokens_p50":...},
    "bd_cli":   {"p50":..,"tokens_p50":...},
    "bd_mcp":   {"p50":..,"tokens_p50":...}
  }
}
```

리포트는 `.notes/bench-beads-results.md`에 표 + 짧은 해석.

## 6. 결정 필요한 질문 (사용자 컨펌)

1. **데이터 분포 합의**: 평균 1.5 deps/record, 256B 텍스트 — 변경?
2. **beads MCP shim 사용 여부**: `uvx beads-mcp` 한번 띄우면 ~수MB 받음.
3. **wg 일괄 적재**: Python MCP 클라이언트로 가는 게 OK? 아니면 CLI에
   `fact ingest-jsonl` 추가가 더 깔끔?
4. **3가지 모두 vs 2가지 우선**: bulk + oneshot만 먼저 → traverse는 후속?
5. **결과 공개**: `.notes/`에 두기 vs `benchmarks/beads/`로 워크스페이스
   멤버화 (cargo `bench` 통합)?

위 5번까지 OK 받으면 prototype을 실 코드로 옮기고 측정 1차 돌리는 데
대략 1–2시간 추정.
