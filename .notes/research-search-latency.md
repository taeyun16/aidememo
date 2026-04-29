# wg search latency — 연구 노트

> 일시: 2026-04-29
> 컨텍스트: 1차 실측 (`.notes/bench-beads-results.md`)에서 wg search
> p50=1111 ms vs bd 387 ms. 이 격차의 원인 분석과 개선 후보를 정리.
> **본 문서는 코드 변경 없는 연구물**.

## 1. Hot path 시간 분포 (현재)

직접 측정 (release 빌드, 1k records, fresh CLI spawn):

```bash
$ time wg --store … search redis -l 5 --json > /dev/null
0.83s user 0.09s system 108% cpu  0.846 total
```

거의 모든 시간이 user CPU. 즉 process spawn / IO가 아닌 **CPU bound**.

`semantic_index = bm25`로 변경하고 같은 호출:

```bash
0.83s user 0.09s system 108% cpu  0.846 total
```

**똑같다**. 이유: `WikiGraph::hybrid_search`(`crates/wg-core/src/lib.rs:589`)
는 `semantic_index` 값과 무관하게 진입 직후 항상 `embed_provider()?`
를 호출 — query 자체를 임베딩해서 `hybrid_search_with_ctx`에 넘기기
때문. fallback path도 동일.

```rust
pub fn hybrid_search(&self, query: &str, opts: SearchOpts) -> Result<...> {
    let provider = self.embed_provider()?;        // ← 매 spawn마다 모델 로드
    // ... HNSW 시도, fallback도 provider 사용
}
```

`embed_provider()`는 model2vec(potion-multilingual-128M, ~30 MB) 또는
설정된 모델을 로드. 모델 파일 read + tokenizer init이 cold path의
~700-900 ms를 차지하는 것으로 추정.

대조: plain `WikiGraph::search` (line 580)는 `SearchEngine` BM25만
호출하고 `embed_provider`를 부르지 않음. CLI는 항상 hybrid path를
사용하므로 우회 불가.

**진단**: cold-start tax = 모델 로드. fallback 경고 자체가 시간을
쓰는 게 아님 (eprintln 한 줄).

## 2. 개선 아이디어 매트릭스

| # | 옵션 | 추정 효과 (cold p50) | 복잡도 | 호환성 | 우선순위 |
|---|---|---|---|---|---|
| **A** | **Lazy provider — BM25-only fast path** | 1100 → ~150–250 ms | low | semantic 명시적 opt-in으로 동작 변경 | ★★★ |
| B | Daemon + thin CLI client (UDS 또는 `wg mcp-serve` 재사용) | warm ~10–50 ms | high | CLI 호출 의미 변화, 기존 lock 모델과 호환 검증 필요 | ★★★ |
| C | Query 임베딩 LRU + 모델 mmap | 재검색 -50–80 ms | low | 메모리 ~수 MB 추가 | ★★ |
| D | mcp-serve에서 동시 임베딩 batch | 동시 요청 -30–50% | med | 단일 client에선 효과 0 | ★★ |
| E | 더 작은 default 모델 (potion-8M) | 모델 로드 -200–400 ms | low | P@5 -3~-8% 추정 (재측정 필요) | ★ |
| F | Tokenizer 정적 link + mmap | 모델 로드 -100–200 ms | med | 빌드 사이즈 +~30 MB | ★ |

**A의 정확한 작동**: `WikiGraph::hybrid_search` 진입에서 다음 조건이면
provider 로드 skip → `WikiGraph::search` (plain BM25) 결과 반환:

- `opts.bm25_only == true` (CLI에 `--bm25` 플래그 추가), **또는**
- `config.search.semantic_weight == 0.0`, **또는**
- `query.split_whitespace().count() < N` (예: N=3, 짧은 토큰은 어차피
  BM25 우세).

이 경로에서는 `embed_provider` 호출이 없어 모델 로드 0. CLI
default를 BM25-only로 두고 hybrid는 `--semantic` 또는 mcp tool에서
명시 호출 시에만. 기존 사용자 경험 약간 변화 — 다만 wg search의
default는 본래 hybrid가 의미 있는 wiki 사용 시점 (저자가 의도적으로
embeddings 활용)이고, 1k 미만 빈 wiki (e2e 시나리오 같은) 에서는
사실상 BM25만 의미.

**B의 작동**: `wg mcp-serve` 가 이미 HTTP/SSE로 single-process daemon.
`wg search` CLI에 `--via http://localhost:3000/mcp` 옵션을 추가해
wg mcp-serve로 JSON-RPC 호출. daemon 띄운 상태라면 모델은 daemon이
warm 상태로 들고 있어 매 search는 ~10–50 ms. 시나리오 E가 이미
입증한 패턴.

## 3. Backend 검토 — redb vs 대안

| Backend | Read p50 | Write TPS | Disk | Multi-proc | FTS | Rust 1.85 | Eco |
|---|---|---|---|---|---|---|---|
| **redb 2.x** (현재) | 빠름 | 중 | 중 (B-tree) | 단일 writer lock | ✗ | ✓ | 활발 |
| sled | 빠름 | 중 | 낮음 (WAL bloat) | ✗ | ✗ | ✓ | **frozen** |
| fjall 2.x | 중 | 높음 | 좋음 (LSM) | 단일 proc | ✗ | ✓ | 신생 |
| rocksdb (binding) | 중 | **매우 높음** | 좋음 | 단일 proc | ✗ | ✓ | 성숙 |
| heed (LMDB) | **최고 (mmap)** | 낮음 | 중 | **다중 reader OK** | ✗ | ✓ | 성숙 |
| **rusqlite + FTS5** | 중–빠름 | 중 | 좋음 | **WAL 다중 reader** | **O (BM25)** | ✓ | **최고** |
| persy | 중 | 중 | 중 | 단일 proc | ✗ | ✓ | 작음 |

해석:
- **redb는 "쓰기·디스크 효율" 측면에서 우리 워크로드에 잘 맞음** —
  bd vs wg 1k bulk write 결과(50× faster, 10× smaller)가 redb의 강점.
- **search latency가 약점**인 진짜 이유는 redb가 아니라 모델 로드.
  backend 교체로는 search p50을 줄일 수 없다 (CPU bound + I/O 미미).
- **SQLite FTS5**가 매력적인 점: 우리가 직접 짠 BM25 inverted index
  (`crates/wg-core/src/search.rs`)를 SQLite 내장 BM25로 대체 가능 —
  유지비 감소. 다만 마이그레이션 + 데이터 모델 재설계 비용이 큼.
  read-only 동시성도 자연스럽게 해결 (시나리오 D의 lock 충돌).
- **heed/LMDB**는 read 최속 + 다중 reader지만 FTS 자체 구현 필요. wg가
  이미 BM25 자체 구현이라 redb → heed 이전은 marginal.
- **sled는 frozen** — 옮기지 말 것.
- **fjall**은 신생, write-heavy 전용이라 read-heavy인 wg와 mismatch.

## 4. 권장 우선순위

| 순위 | 작업 | 예상 시간 | 효과 | 위험도 |
|---|---|---|---|---|
| 1 | **A: Lazy provider** | 1일 | cold p50 1100 → 150 ms (bd보다 빠름) | low — `--semantic` 옵트인으로 hybrid 보존 |
| 2 | **B: mcp-serve를 권장 모드로 + thin CLI client** | 2-3일 | warm p50 ~10–50 ms | med — UDS/HTTP IPC 추가, 기존 stdio 호환 유지 |
| 3 | **(중장기) SQLite + FTS5 PoC** | 1-2주 | BM25 직접 구현 제거 + 다중 reader 자연스러움 | high — 마이그레이션 비용, 데이터 모델 재설계 |

## 5. 결론

- **redb는 적절하다** — wg의 bulk write/disk 우위는 redb 덕분이고,
  search latency 격차는 backend가 아닌 model 로드 hot path 문제.
- **가장 큰 ROI는 #A (Lazy provider)**: 1일 작업으로 wg search p50을
  bd보다 빠르게 만들 수 있음. semantic은 사용자 의도가 명시될 때만
  로드.
- **#B (Daemon)** 는 모든 모드에서 ~10 ms latency 가능 — `wg
  mcp-serve` 인프라가 이미 있음. CLI 측에 thin client만 붙이면
  됨. 시나리오 E의 측정값(p50 18.6 ms)이 이미 daemon 모드 효과 입증.
- **SQLite FTS5**는 흥미로운 PoC지만 우선순위 낮음. wg의 `bm25_index`
  + redb 조합이 이미 `cache BM25 inverted index on WikiGraph` 같은
  최적화를 갖고 있고 (`commit bcc66cc`), 진짜 병목은 임베딩 path.

## 6. 후속 검증 후보

- A 적용 후 동일 1k store에서 p50/p95 재측정 → bd와 정량 비교
- mcp-serve daemon에 4 client 동시 search → throughput
- HNSW sidecar `wg vector-rebuild` 후 hybrid path 측정 → semantic이
  진짜 필요한 워크로드의 baseline

## 참고

- redb: https://github.com/cberner/redb
- SQLite FTS5 BM25: https://www.sqlite.org/fts5.html
- sled feature-frozen: https://github.com/spacejam/sled
- 1차 실측: `.notes/bench-beads-results.md`
- 시나리오 E 결과: `.notes/e2e-multi-agent.md` (mcp-serve p50 18.6 ms)
