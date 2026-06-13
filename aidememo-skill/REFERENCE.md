---
kind: doc
title: AideMemo 전체 API 참조
---

# AideMemo Agent Guide

## 프로젝트 구조

```
/Users/mixlink/dev/aidememo/           ← 프로젝트 루트 (AideMemo workspace)
├── Cargo.toml                   ← workspace manifest
├── AGENTS.md                   ← 에이전트용 가이드 (CLAUDE.md → import)
├── README.md
├── docs/                       ← durable measurement and positioning docs
├── crates/
│   ├── aidememo-core/                ← 핵심 라이브러리 (lib)
│   │   └── src/
│   │       ├── lib.rs          ← AideMemo public API (re-exports)
│   │       ├── store.rs        ← redb CRUD + lock_retry + meta KV
│   │       ├── graph.rs        ← 그래프 순회 / 최단 경로
│   │       ├── search.rs       ← BM25 + semantic/hybrid + RRF + adapter
│   │       ├── index.rs        ← BM25 inverted index (lazy-rebuild)
│   │       ├── vector_index.rs ← HNSW 사이드카 (instant-distance)
│   │       ├── embedding.rs    ← model2vec / TEI 프로바이더
│   │       ├── rerank.rs       ← TEI cross-encoder reranker
│   │       ├── fuzzy.rs        ← 퍼지 매칭 (strsim jaro-winkler)
│   │       ├── ingest.rs       ← 마크다운 → entity/fact/relation 추출
│   │       ├── types.rs        ← EntityInput / FactInput / SearchOpts …
│   │       ├── relations.rs    ← RelationType + RelationRecord
│   │       ├── config.rs       ← Config 로드/저장 (projects · model · search · lint)
│   │       ├── error.rs        ← AideMemoError + thiserror
│   │       ├── lint.rs         ← 그래프 건전성 검사
│   │       ├── adapt.rs        ← 도메인 어댑터 (피드백 → 랭킹 보정)
│   │       ├── wal.rs          ← search_sessions / search_feedback WAL
│   │       ├── s3.rs           ← (feature) S3 매니페스트 + 세그먼트 (현재 local-fs mirror)
│   │       └── migrate.rs      ← 스키마 마이그레이션
│   ├── aidememo-cli/                 ← CLI 바이너리 (bin "aidememo")
│   │   └── src/
│   │       ├── main.rs         ← 명령 디스패치 + tracing 초기화
│   │       ├── output.rs       ← Format::{Table, Json}
│   │       └── cmd/            ← init / watch / model / feedback / adapt /
│   │                             daemon / mcp_tools / mcp_stdio / mcp_serve / …
│   ├── aidememo-ffi/                 ← C ABI (cdylib + staticlib)
│   ├── aidememo-napi/                ← Node.js (napi-rs)
│   ├── aidememo-nif/                 ← Elixir (rustler)
│   └── aidememo-python/              ← Python (PyO3)
├── benchmarks/                 ← aidememo-benchmarks 크레이트 (golden 평가)
├── bench/                       ← 시나리오 스크립트 (beads-vs-aidememo, multi-agent)
├── aidememo-skill/                   ← 배포용 SKILL.md + REFERENCE.md (이 파일)
└── docs/                     ← 벤치 결과 + 설계 노트
```

## 핵심 기술 스택

| 구성 요소 | 기술 |
|---------|------|
| 저장소 | SQLite 기본 로컬 backend, optional redb backend |
| 풀텍스트 검색 | bm25 crate (lazy inverted index) |
| 시맨틱 검색 | model2vec-native (정적 임베딩) + instant-distance (HNSW) — `feature = "semantic"` |
| Reranker | TEI cross-encoder (BGE-reranker, etc.) — `rerank.provider = "tei"` |
| 도메인 어댑터 | tanh-스쿼시 곱셈 보정 — `feature = "semantic-adapt"` |
| 퍼지 매칭 | strsim (jaro-winkler) |
| CLI 파서 | bpaf 0.9 |
| 관측성 | tracing + EnvFilter (stderr) |
| ID 생성 | ulid |
| 파일 감시 | notify 6.x |
| MCP 트랜스포트 | stdio (newline JSON-RPC) + HTTP/SSE (axum) |

## 핵심 API

### AideMemo (lib.rs)

실제 외부 API는 `AideMemo`에 모여 있고, 내부 `Store`는 더 저수준의 CRUD를 제공합니다.

```rust
// 열기 / 생성
pub fn open(path: &Path, config: Config) -> Result<AideMemo>

// Entity CRUD
pub fn entity_add(&self, input: EntityInput) -> Result<EntityId>
pub fn entity_get(&self, name: &str) -> Result<EntityRecord>
pub fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord>
pub fn entity_update(&self, name: &str, input: EntityUpdate) -> Result<()>
pub fn entity_list(&self, opts: ListOpts) -> Result<Vec<EntitySummary>>
pub fn entity_delete(&self, name: &str) -> Result<()>
pub fn entity_rename(&self, old_name: &str, new_name: &str) -> Result<()>
pub fn entity_alias_add(&self, name: &str, alias: &str) -> Result<()>
pub fn resolve_entity(&self, name: &str) -> Result<EntityId>
pub fn suggest_similar_entities(&self, name: &str) -> Result<Vec<String>>

// Fact CRUD
pub fn add_fact(&self, input: FactInput) -> Result<FactId>
pub fn fact_add(&self, input: FactInput) -> Result<FactId> // alias
pub fn fact_add_many(&self, inputs: Vec<FactInput>) -> Result<Vec<FactId>>
// One redb write txn for the whole batch — amortizes the per-commit fsync (~3-5 ms on macOS APFS,
// ~70× faster per fact than sequential fact_add at typical batch sizes). All-or-nothing.
pub fn fact_get(&self, id: &FactId) -> Result<FactRecord>
pub fn fact_update(&self, id: &FactId, input: FactUpdate) -> Result<()>
pub fn fact_delete(&self, id: &FactId) -> Result<()>
pub fn fact_feedback(&self, id: &FactId, helpful: bool) -> Result<()>
pub fn fact_supersede(&self, old_id: &FactId, new_id: &FactId) -> Result<()>
// Validity-window invalidate. Sets old.superseded_at=now / superseded_by=new.
// `current_only` filters and `--as-of <date>` queries respect this.
pub fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>>
// FactListOpts now carries `as_of: Option<u64>` (epoch ms) for historical queries.

// Relations / graph
pub fn relation_add(&self, input: RelationInput) -> Result<()>
pub fn relation_remove(&self, source: &str, target: &str, rel_type: &str) -> Result<()>
pub fn relations_get(&self, entity: &str, direction: TraverseDirection) -> Result<Vec<RelationRecord>>
pub fn traverse(&self, start: &str, opts: TraverseOpts) -> Result<TraverseResult>
pub fn path_find(&self, from: &str, to: &str) -> Result<Option<Vec<PathStep>>>

// Ingest
pub fn ingest(&mut self, wiki_root: &Path, incremental: bool) -> Result<ingest::IngestStats>
// incremental is currently parsed but behaves the same as a full ingest

// Search
#[cfg(feature = "semantic")]
pub fn search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>>
#[cfg(feature = "semantic")]
pub fn search_with_traverse(&self, query: &str, start: &str, depth: u32, opts: SearchOpts) -> Result<Vec<SearchResult>>
#[cfg(feature = "semantic")]
pub fn hybrid_search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>>
// Note: hybrid_search/search_with_traverse live in search.rs and are feature-gated;
// there is no Store::hybrid_search method.
// If the `semantic` feature is disabled, `AideMemo::search` returns SearchFailed.

// Lint / stats
pub fn lint(&self) -> Result<Vec<LintIssue>> // wrapper around LintEngine::lint(); counts are dropped
pub fn stats(&self) -> Result<StoreStats>
pub fn config(&self) -> &Config
```

### Store (store.rs, internal low-level API)

```rust
pub fn open(path: &Path, config: Config) -> Result<Store>
pub fn schema_version(&self) -> Result<u32>
pub fn stats(&self) -> Result<StoreStats>
pub fn set_last_ingest_at(&self) -> Result<()>

pub fn entity_add(&mut self, input: EntityInput) -> Result<EntityId>
pub fn entity_get(&self, name: &str) -> Result<EntityRecord>      // by name; no entity_get_by_name exists
pub fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord>
pub fn entity_update(&mut self, name: &str, input: EntityUpdate) -> Result<()>
pub fn entity_list(&self, opts: ListOpts) -> Result<Vec<EntitySummary>>
pub fn entity_delete(&mut self, name: &str) -> Result<()>
pub fn suggest_similar_entities(&self, name: &str) -> Result<Vec<String>>
pub fn resolve_entity(&self, name: &str) -> Result<EntityId>

pub fn fact_add(&mut self, input: FactInput) -> Result<FactId>
pub fn fact_get(&self, id: &FactId) -> Result<FactRecord>
pub fn fact_update(&mut self, id: &FactId, input: FactUpdate) -> Result<()>
pub fn fact_delete(&mut self, id: &FactId) -> Result<()>
pub fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>>
pub fn fact_feedback(&mut self, id: &FactId, helpful: bool) -> Result<()>

pub fn relation_add(&mut self, input: RelationInput) -> Result<()>
pub fn relation_remove(&mut self, source: &str, target: &str, rel_type: &str) -> Result<()>
pub fn relations_get(&self, entity_name: &str, direction: TraverseDirection) -> Result<Vec<RelationRecord>>
pub fn relations_get_by_id(...) -> Result<Vec<RelationRecord>> // used by graph traversal

// No store-level search/traverse/ingest/lint methods; those live in search.rs/graph.rs/ingest.rs/lint.rs or via AideMemo wrappers.
// Behavioral notes:
// - entity_get(name) resolves name/alias through the secondary index and returns suggestions on miss.
// - entity_delete(name) resolves first, then removes the canonical record plus all alias index entries.
// - fact_add defaults missing fact_type/entity_ids/source_confidence to Unknown/empty/0.5 respectively.
// - entity_list/fact_list provide filtering + pagination; traverse/search are separate higher-level modules.
```

### 타입 (types.rs)

```rust
pub struct EntityInput { pub name: String, pub entity_type: Option<EntityType>, pub aliases: Option<Vec<String>>, pub tags: Option<Vec<String>>, pub source_page: Option<String> }
pub struct EntityUpdate { pub name: Option<String>, pub entity_type: Option<EntityType>, pub aliases: Option<Vec<String>>, pub tags: Option<Vec<String>>, pub source_page: Option<String> }
pub enum EntityType { Technology, Concept, Comparison, Query, Person, Team, Unknown }
pub struct FactInput { pub content: String, pub fact_type: Option<FactType>, pub entity_ids: Option<Vec<EntityId>>, pub tags: Option<Vec<String>>, pub source: Option<String>, pub source_confidence: Option<f32> }
pub struct FactUpdate { pub content: Option<String>, pub fact_type: Option<FactType>, pub tags: Option<Vec<String>>, pub source: Option<String> }
pub enum FactType { Decision, Pattern, Convention, Claim, Note, Question, Unknown }
pub struct ListOpts { pub entity_type: Option<EntityType>, pub min_facts: Option<u32>, pub sort_by: EntitySort, pub limit: Option<usize>, pub offset: usize }
pub enum EntitySort { Name, UpdatedAt, FactCount }
pub struct FactListOpts { pub fact_type: Option<FactType>, pub entity_id: Option<EntityId>, pub min_confidence: Option<f32>, pub limit: Option<usize>, pub offset: usize }
pub struct TraverseOpts { pub depth: u32, pub relation_types: Option<Vec<RelationType>>, pub direction: TraverseDirection }
pub enum TraverseDirection { Forward, Reverse, Both }
pub struct SearchOpts {
    pub limit: Option<usize>,
    pub min_confidence: Option<f32>,
    pub entity_filter: Option<Vec<EntityId>>,
    pub bm25_weight: f32,
    pub semantic_weight: f32,
    pub current_only: bool,         // superseded_at != None인 fact 제외
    pub since: Option<u64>,         // observed_at >= since (epoch ms)
    pub until: Option<u64>,         // observed_at < until
    pub as_of: Option<u64>,         // 시점 재현: as_of 시점에 valid한 fact만
    pub bm25_only: bool,            // hybrid_search → BM25 fast-path
    pub session_id: Option<String>, // 피드백 트래킹용
}
pub struct IngestStats { pub entities_added, pub entities_updated, pub relations_added, pub facts_added, pub files_scanned, pub errors: Vec<String> }
pub struct StoreStats { pub entity_count, pub fact_count, pub relation_count, pub total_size_bytes, pub last_ingest_at: Option<u64> }
pub struct LintReport { pub issues, pub entity_count, pub fact_count, pub relation_count }
pub struct LintIssue { pub severity, pub code, pub message, pub entity_id, pub fact_id }
```

### Config (config.rs)

전체 필드는 `crates/aidememo-core/src/config.rs`에서 확인하세요. 자주 조정하는
키만 발췌:

```rust
pub struct Config {
    pub store: StoreConfig,
    pub model: ModelConfig,
    pub search: SearchConfig,
    pub lint: LintConfig,
    pub rerank: RerankConfig,
    pub projects: HashMap<String, String>,
    pub default_project: Option<String>,
}
pub struct StoreConfig {
    pub path: String,
    pub durability: String,        // "immediate" (default) | "eventual"
    pub lock_retry_ms: u64,        // multi-agent contention 처리
}
pub struct ModelConfig {
    pub provider: String,          // "model2vec" (default) | "tei"
    pub name: String,
    pub endpoint: Option<String>,  // TEI 사용 시
    pub auto_download: bool,
    pub dimension: usize,
    pub quantize: bool,
}
pub struct SearchConfig {
    pub default_limit: usize,
    pub min_trust: f32,
    pub bm25_weight: f32,
    pub semantic_weight: f32,
    pub semantic_prefilter: usize, // 시맨틱 후보 컷오프
    pub graph_prefilter: bool,     // BM25 hit → graph N-hop 확장
    pub graph_depth: u32,
    pub graph_fact_cap: usize,
    pub semantic_index: String,    // "bm25" | "hnsw"
    pub weight_by_confidence: bool,
    pub time_decay_tau_ms: u64,    // 0 = 비활성
    pub use_adapter: bool,         // 학습된 어댑터 적용 여부
}

// Load / save
pub fn Config::load() -> Result<Config>;
pub fn Config::save(&self) -> Result<()>;
pub fn Config::get(&self, key: &str) -> Option<String>;
pub fn Config::set(&mut self, key: &str, value: &str) -> Result<()>;
```

## 주요 규칙

### bpaf CLI 규칙
- **positional/command item은 struct/tuple의 가장 오른쪽**에 위치
- `construct!` 매크로 인자 순서는 struct 필드 순서와 일치
- `construct!` 배열 안에 `::` 경로 함수 호출 (`init::init_command()`) 불가 → 반드시 지역 변수 바인딩 사용
- Parser 반환 타입: `impl Parser<Command>` (구체 타입 아님)

### redb 규칙
- `AideMemo`는 `Arc<RwLock<Store>>`로 공유하고, `Store` 내부는 `Arc<Database>`를 사용
- `Store::open(path, config)`만 존재함 (`new` / `get_or_create`는 없음)
- 트랜잭션: `db.begin_write()`, `db.begin_read()`
- 테이블 open 후 반드시 `drop(meta)` 등 필요 (write_txn.commit() 전)

### 빌드 명령

```bash
# 전체 빌드
cargo build

# 개별 패키지
cargo build -p aidememo-core
cargo build -p aidememo-cli

# check only (빠른 검증)
cargo check -p aidememo-core -p aidememo-cli

# CLI 실행
./target/debug/aidememo --help
./target/debug/aidememo init ./my-wiki
./target/debug/aidememo ingest ./my-wiki
./target/debug/aidememo entity list
```

## Phase 진척도

| 영역 | 상태 |
|---|---|
| Phase 1 — store / CRUD / ingest / BM25 / config | ✅ |
| Phase 2 — `aidememo lint` / 마이그레이션 / `aidememo import` · `aidememo export` | ✅ |
| Phase 3 — 시맨틱 검색 (model2vec) + HNSW + TEI / `aidememo model` | ✅ |
| Phase 4 — 피드백 → 어댑터 → 랭커 결합 (`search.use_adapter`) | ✅ |
| Phase 5 — MCP (stdio + HTTP/SSE, 17 툴) + 4개 언어 바인딩 + 데몬 | ✅ |
| Phase 6 — S3 매니페스트 / 세그먼트 (현재 local-fs 미러) | ⏳ |

남은 phase6 정확한 위치는 `grep -rn "TODO(phase" crates/`로 확인.

## MCP 도구 (총 22개)

| 도구 | 용도 |
|---|---|
| `aidememo_query` | 한 번에 컨텍스트 (search + traverse + recent) — 우선순위 |
| `aidememo_search` | 순수 하이브리드 검색 |
| `aidememo_recent` | 최근 N일 fact |
| `aidememo_entity_list` / `aidememo_entity_get` | 엔티티 브라우즈 / 단건 조회 |
| `aidememo_fact_list` / `aidememo_fact_get` | fact 필터 목록 / ULID 단건 조회 |
| `aidememo_traverse` / `aidememo_backlinks` | 정방향 / 역방향 그래프 워크 |
| `aidememo_path` | 두 엔티티 간 최단 경로 |
| `aidememo_doctor` / `aidememo_lint` | 건전성 스냅샷 / 원시 이슈 |
| `aidememo_entity_describe` | 엔티티의 prose 요약 설정 / 삭제 |
| `aidememo_fact_add` | 단일 fact 추가 |
| `aidememo_fact_add_many` | 배치 (단일 fsync, 3개 이상이면 권장) |
| `aidememo_fact_supersede` | 유효 기간 무효화 (`old.superseded_at = now`) |
| `aidememo_fact_edit` | append / prepend / find+replace / content |
| `aidememo_feedback` | aidememo_search 결과에 helpful/not 표기 — 어댑터 학습 시그널 |
| `aidememo_extract` | 대화/문단 → 후보 fact 추출 (휴리스틱). `apply:true`로 일괄 추가 |
| `aidememo_session_start` | 한 호출 warmup — pinned/recent/top_entities/open_issues 묶음 |
| `aidememo_pinned_context` / `aidememo_fact_pin` | "always loaded" 메모리 계층 (Letta-style) |

스키마: `aidememo-cli/src/cmd/mcp_tools.rs::list_tools()`.

검색·열람 계열(`aidememo_search` / `aidememo_query` / `aidememo_fact_list`)은 `current_only=true`가
기본 — superseded fact를 결과에서 제외합니다. 타임라인 / 시점 재현이 필요하면
명시적으로 `current_only=false`로 호출하거나 `aidememo_search`의 `as_of` (ISO 날짜)·
`since`/`until` (ISO 또는 `30d`/`12h` duration), `entity`(이름/alias 1개로 한정),
`min_confidence` 필터를 사용합니다.

## 데몬 — 백그라운드 mcp-serve + 자동 발견

```bash
aidememo daemon start    # mcp-serve를 백그라운드 + ~/.aidememo/daemon.json 등록
aidememo daemon status   # registry + /health 프로브
aidememo daemon stop     # SIGTERM
```

`aidememo daemon start` 이후 일반 CLI 호출 (`aidememo search`, `aidememo query`, `aidememo fact add` …)
은 `~/.aidememo/daemon.json`을 보고 자동으로 데몬으로 디스패치합니다 — 모델 워밍
없이 ~9 ms (BM25) / ~45 ms (HNSW). `AIDEMEMO_NO_DAEMON=1`로 일회성 우회.

다중 에이전트 공유: SQLite가 기본 스토어이며 여러 로컬 프로세스의 짧은
쓰기 경합을 기본 경로로 처리합니다. 선택적 redb backend는 프로세스당 단일
락이라, 여러 에이전트가 같은 redb 스토어를 쓰려면 **하나의
`aidememo mcp-serve`**를 띄우고 모두 그 HTTP 엔드포인트를 가리키는 편이
안전합니다. 자세한 패턴은 `AGENTS.md`의 "Multi-agent shared store" 절.

## 언어 바인딩

4개 모두 `current_only` 필터 + `fact_supersede`까지 풀 커버:

```python
# Python
import aidememo_python as aidememo
g = aidememo.AideMemo("./_meta/wiki.sqlite")
ctx = g.query("Redis", current_only=True)
```

```javascript
// Node
const { AideMemoStore } = require('aidememo-napi');
const g = new AideMemoStore('./_meta/wiki.sqlite');
g.factSupersede(oldId, newId);
```

```elixir
# Elixir
g = AideMemoNif.open!("./_meta/wiki.sqlite")
ctx = AideMemoNif.query(g, "Redis", current_only: true)
```

```c
/* C */
char* json = aidememo_query(g, "Redis", 5, 2, 5, /* current_only */ true);
aidememo_free_string(json);
```

## 자주 보는 에러

### bpaf 패닉
```
bpaf usage BUG: all positional and command items must be placed in the right most position
```
→ positional 필드를 struct 오른쪽으로 이동 + construct! 인자 순서 동기화

### redb 트랜잭션
```
error: Cannot start a read transaction inside a read transaction
```
→ 중첩 트랜잭션 불가. txn을 drop 후 재시작

### bm25 / strsim
```
method not found: normalized_similarity
```
→ trigram crate API가 다름. `strsim::jaro_winkler` 사용
