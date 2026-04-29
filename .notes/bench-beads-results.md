# wg vs beads — 1차 실측 결과

> 일시: 2026-04-28
> 대상: wg @ `3de286f` (release), beads `1.0.3` (Homebrew, Dolt 1.86.6)
> 머신: macOS arm64 (Apple Silicon)
> 데이터셋: 1000 records, 64-byte titles, 256-byte descriptions, fixed seed
> 위치: `bench/beads-vs-wg/`

## 한 줄 결론

**Bulk write에서는 wg가 50× 빠르고 disk가 10× 작다. Single-query
search에서는 bd가 ~3× 빠르다.** 두 시스템의 backend 선택이 그대로
trade-off로 드러난다 — wg(redb, lean KV)는 쓰기·디스크에 강하고,
bd(Dolt, SQL+versioned)는 쿼리 latency가 짧다.

## 시나리오 #1 — Bulk write throughput

같은 1000 records를 양쪽의 first-class bulk-insert 경로로 넣는다.

| 지표 | wg (`wg_fact_add_many` MCP, 1 transaction) | beads (`bd import < jsonl`) | 비율 |
|---|---|---|---|
| wall (1000 records) | **108 ms** | 5443 ms | wg **50.2× faster** |
| throughput | **9 229 records/s** | 184 records/s | wg 50.2× higher |
| on-disk after insert | 3.7 MB | 39.2 MB | wg **10.6× smaller** |

해석:
- wg는 redb의 single-transaction append + page write 한 번. fsync 한 번 (immediate durability).
- bd는 Dolt의 SQL INSERT + commit (versioned hash chain). commit 한 번 = SHA + index + journal 갱신, 비용이 redb보다 한 자릿수 무거움.
- disk 차이는 1k records에서 가장 두드러진다. Dolt의 versioning metadata가 작은 데이터셋에선 페이로드 자체보다 큼. 이는 100k+ 스케일에서는 흡수될 가능성.

## 시나리오 #3 — Single-query search latency

같은 store에 100개 mid-frequency query (시드 고정, 모든 query는 hit
보장). 매 호출은 fresh CLI process spawn — agent 입장의 실제 latency.

| 지표 | wg search | bd search | 비율 |
|---|---|---|---|
| p50 | 1111 ms | **387 ms** | bd **2.87× faster** |
| p95 | 1221 ms | **398 ms** | bd 3.07× faster |
| max | 1263 ms | 408 ms | — |
| 평균 응답 길이 | 4683 chars | 622 chars | wg 7.5× richer |

해석:
- wg는 BM25 + (옵션) semantic 모델 로드 시도. 매 fresh spawn마다 tokenizer 초기화 비용 발생. stderr에 매번 `semantic_index=hnsw configured but no sidecar … falling back to BM25 prefilter. Run wg vector-rebuild.` 경고 — fallback path가 cold-start cost를 키움.
- bd는 SQL `LIKE` on title — 인덱스 없는 full scan이지만 문자열 매칭 자체가 가벼움. ranking 없음.
- wg의 응답이 7.5× 더 풍부 (entity context, source citation, relevance score). 단순 latency만 보면 손해지만 답변 품질 측면 정보량은 큼.
- HNSW sidecar를 미리 빌드하면 (`wg vector-rebuild`) wg search가 어떻게 변하는지는 후속 실측 필요.

## 시나리오 #4 — 4-way latency + quality (재측정)

`.notes/research-search-latency.md`에서 권장한 **A: Lazy provider**를
구현 (`--bm25` 플래그 + `semantic_weight = 0.0`이면 plain BM25 path).
같은 1k store에서 4 모드를 50 query로 측정:

| 모드 | p50 | p95 | 비고 |
|---|---|---|---|
| **wg `--bm25` (신규 lazy fast path)** | **70.9 ms** | 74.2 ms | 모델 로드 0 |
| bd search | 383.8 ms | 395.2 ms | SQL LIKE on title |
| wg (HNSW hybrid, sidecar 빌드 후) | 850.7 ms | 959.7 ms | 모델 로드 + HNSW 조회 |
| wg (BM25-fallback hybrid, sidecar 없음 = 현 default) | 1126.1 ms | 1185.5 ms | 모델 로드 + BM25 fallback |

**`wg --bm25`가 bd 대비 5.4×, wg 현 default 대비 16× 빠름.** beads의
search latency 우위가 단번에 무너짐.

### 품질 — overlap@5 (BM25-only baseline 대비)

| 모드 | overlap@5 | 해석 |
|---|---|---|
| BM25-only (self) | 1.00 | sanity (deterministic) |
| HNSW hybrid | 0.04 | **거의 안 겹침** — 임베딩이 완전히 다른 hit 반환 |
| BM25-fallback hybrid | 0.20 | 일부 겹침 |

해석:
- 합성 corpus(lorem random)라 임베딩이 의미 잡을 게 없어 사실상 random
  hit를 반환. 실제 wiki에서는 HNSW가 BM25 token 미스를 보완하는 것이
  `.notes/bench-miracl-ko.md`에서 입증됨 (P@5 +2.5pp).
- 즉 **품질 vs latency trade-off는 실제**: 정확도가 정말 필요한 query만
  hybrid/HNSW로, 나머지는 lazy BM25.
- 사용자 선택 패턴 권고:
  - **CLI 즉답** (agent의 hot path) → `--bm25` default
  - **wiki 검색·요약** (sentinel keyword가 약한 한글/일본어 등) → hybrid + HNSW

## 시나리오 #5 — daemon (mcp-serve) warm path

연구 노트의 권장 #B(daemon thin client)를 `wg search --via URL`로
구현. 5 회 warmup 후 50 query 측정 (1k store).

| 모드 | p50 | p95 | bd 대비 |
|---|---|---|---|
| **`wg --via --bm25` (daemon warm)** | **5.2 ms** | 5.7 ms | **74× 빠름** |
| **`wg --via` hybrid HNSW (daemon warm)** | **42.7 ms** | 64.0 ms | **9.1× 빠름, 의미 검색 보존** |
| `wg --bm25` (local fresh CLI) | 70.9 ms | 74.2 ms | 5.4× 빠름 |
| `bd search` | 383.8 ms | 395.2 ms | baseline |
| `wg` HNSW hybrid (local fresh) | 850.7 ms | 959.7 ms | 2.2× 느림 |

해석:
- 모든 wg 모드가 bd보다 빠름. `--via` daemon의 BM25는 사실상 HTTP
  RTT(~5 ms)만 든다.
- **HNSW가 daemon에서는 다시 빠름** (43 ms p50). 모델은 daemon이
  warm 상태로 들고 있고, 첫 호출에서만 ~700 ms 모델 로드 한 번.
  이후 query 임베딩만 돌리면 됨.
- 즉 연구 노트의 별도 작업 #2(HNSW 모델 amortization)는 #1 daemon
  으로 자연스럽게 해결 — 별도 mmap/static-link 작업 가치 없음.

**권장 사용 패턴 정리**:

| 시나리오 | 명령 |
|---|---|
| Agent 빈번한 hot path (latency 최우선) | `wg search --via http://localhost:3000 --bm25` |
| Agent recall 중요 (한국어/일본어 등 BM25 약함) | `wg search --via http://localhost:3000` (HNSW) |
| 임시 manual CLI, daemon 없을 때 | `wg search --bm25` |
| 풀 의미 정확도, 1회성 | `wg search` (HNSW + 매 spawn 모델 로드) |

여러 에이전트가 같은 store를 공유한다면 한 번 `wg mcp-serve --port
3000 /path/to/store &` 띄우고 모두 `--via`로 붙는 것이 best — 시나리오
E의 lock 정책 검증과 #5의 latency 측정 모두 그 패턴을 가리킴.

## 10k 재측정 — 스케일링 확인

같은 워크로드를 N=10000으로 다시 돌려 1k 결과의 ROI가 dataset이 커져도
유지되는지 확인. (gen.py `--n 10000 --seed 42`, 100 queries / 50
queries.)

### Bulk write — 격차가 더 벌어짐

| 지표 | 1k | 10k | 1k→10k 증감 |
|---|---|---|---|
| wg wall | 108 ms | 220 ms | 2.0× |
| bd wall | 5443 ms | **74 600 ms** | 13.7× |
| **wg vs bd 속도 차** | **50× faster** | **339× faster** | **격차 6.8× 더 큼** |
| wg disk | 3.7 MB | 17.4 MB | 4.7× |
| bd disk | 39.2 MB | **555 MB** | 14.2× |
| **wg vs bd 디스크 차** | 10.6× smaller | **31.9× smaller** | 격차 3× 더 큼 |

bd의 Dolt journal/version metadata가 record 수에 super-linear로 증가.
wg의 redb는 page-based로 sublinear에 가깝게 증가.

### Search latency — daemon warm은 거의 무영향

| 모드 | 1k p50 | 10k p50 | 증감 | 비고 |
|---|---|---|---|---|
| **`wg --via --bm25` (daemon warm)** | **5.2 ms** | **5.3 ms** | 1.0× | HTTP RTT만 (서버 logic 무관) |
| **`wg --via` HNSW hybrid (daemon warm)** | **42.7 ms** | **42.8 ms** | 1.0× | HNSW logarithmic, 모델 warm |
| `wg --bm25` (local cold spawn) | 71 ms | 289 ms | 4.1× | BM25 inverted index 빌드 비용 |
| `bd search` | 384 ms | **703 ms** | 1.8× | SQL LIKE full-table scan |
| `wg` HNSW hybrid (local cold) | 851 ms | 1053 ms | 1.2× | 모델 로드 dominant (이미 ceiling) |
| `wg` fallback hybrid (현 default, local cold) | 1126 ms | 4349 ms | 3.9× | 모델 로드 + BM25 빌드 모두 |

### 10k 권장 패턴 vs bd 격차

| | wg | bd | 비율 |
|---|---|---|---|
| Agent hot path (daemon `--bm25`) | **5.3 ms** | 703 ms | **bd보다 132× 빠름** |
| Recall-critical (daemon HNSW) | **42.8 ms** | 703 ms | bd보다 16.4× 빠름 + recall 보존 |
| Manual CLI (`--bm25`) | 289 ms | 703 ms | bd보다 2.4× 빠름 |

### 10k 시사점

- **bulk write 격차는 dataset 크기에 따라 더 벌어진다** (1k의 50× → 10k의
  339×). bd의 Dolt 백엔드가 large dataset에 부적합한 게 분명해짐.
- **daemon warm 모드는 dataset 크기에 거의 영향 없다**. 1k와 10k에서
  사실상 같은 latency. agent의 hot path 비용은 dataset 성장과 무관.
- **HNSW는 모델 로드가 ceiling이라 dataset 영향 크지 않다** — 1k에서
  851ms나 10k에서 1053ms나 비슷. 데이터 더 커져도 daemon HNSW가 ~43 ms.
- **fallback hybrid는 가장 나쁜 scaling** — 모델 로드 + inverted
  index 빌드 둘 다 매번 발생하는 (현재 default 동작) 가장 큰 이슈. 이게
  `--bm25` lazy fast path의 가치를 강하게 뒷받침.

## 발견 / 후속 작업

1. **wg search의 cold-start tax**: 매 fresh CLI 호출마다 모델 로드를
   시도하면서 semantic 미설정 경고를 내보내는 것은 사용자 인지 latency
   를 키운다. 두 가지 개선 후보:
   - sidecar 미존재 시 경고는 한 번만 (또는 `--quiet`로 silence)
   - 또는 default config가 HNSW를 요구하지 않게 (bm25 모드 기본)
2. **장기 사용 시 disk 비교 재측정 필요**: 1k records는 작아서
   metadata가 페이로드보다 커진 케이스. 10k / 100k에서 wg 우위가
   유지되는지 확인.
3. **wg의 entity 자동 생성 미지원**: `wg_fact_add_many`에서 `entities`
   인자에 미존재 entity 이름을 넘기면 silent drop (entity_count=0
   결과). 자동 생성 옵션이 있으면 ingest 패턴이 더 직관적.
4. **시나리오 #2 (graph traversal)와 시나리오 #4 (cold-start)**는
   설계만 한 상태. wg와 bd의 그래프 모델이 의미적으로 다르고 (entity-
   relation vs issue-dependency) traversal 의미도 달라서, 측정 fair
   하게 만들려면 별도 정의 필요.

## 데이터/스크립트

- `bench/beads-vs-wg/gen.py` — 데이터셋 generator
- `bench/beads-vs-wg/scenario_1_bulk_write.py`
- `bench/beads-vs-wg/scenario_3_search_latency.py`
- `bench/beads-vs-wg/data/` — generator 출력 (gitignore)
- `bench/beads-vs-wg/results/` — 측정 결과 raw JSON (gitignore)

재현:

```bash
# 1) 데이터셋 생성
python3 bench/beads-vs-wg/gen.py --n 1000 --seed 42

# 2) bulk write
python3 bench/beads-vs-wg/scenario_1_bulk_write.py

# 3) search latency
python3 bench/beads-vs-wg/scenario_3_search_latency.py
```

환경변수: `WG_BIN`, `BD_BIN`, `BENCH_DATA`, `BENCH_RESULTS`로 override.
