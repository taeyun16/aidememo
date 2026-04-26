---
kind: doc
title: Wiki-Graph 전체 API 참조
---

# Wiki-Graph Agent Guide

## 프로젝트 구조

```
/Users/mixlink/dev/wg/           ← 프로젝트 루트 (wg workspace)
├── Cargo.toml                   ← workspace manifest
├── Cargo.lock
├── PLAN.md                     ← Phase 1-6 구현 계획
├── README.md
├── .gitignore
├── crates/
│   ├── wg-core/                ← 핵심 라이브러리 (lib)
│   │   ├── src/
│   │   │   ├── lib.rs          ← WikiGraph, re-export
│   │   │   ├── store.rs        ← redb 기반 저장소
│   │   │   ├── graph.rs        ← 그래프 순회/경로 탐색
│   │   │   ├── search.rs       ← BM25 + semantic/hybrid search (feature `semantic`)
│   │   │   ├── fuzzy.rs        ← 퍼지 매칭 (strsim)
│   │   │   ├── ingest.rs       ← 마크다운 → entity/fact 추출
│   │   │   ├── types.rs        ← EntityInput, FactInput, etc.
│   │   │   ├── config.rs       ← Config 로드/저장
│   │   │   ├── error.rs        ← WgError enum
│   │   │   ├── lint.rs         ← 그래프 건전성 검사
│   │   │   └── migrate.rs      ← 스키마 마이그레이션
│   │   └── Cargo.toml
│   └── wg-cli/                 ← CLI 바이너리 (bin "wg")
│       ├── src/
│       │   ├── main.rs         ← entry point + command dispatch
│       │   └── cmd/
│       │       ├── mod.rs      ← bpaf command definitions
│       │   ├── init.rs         ← `wg init` subcommand
│       │   └── watch.rs        ← `wg watch` subcommand
│       └── Cargo.toml
├── benchmarks/
├── models/
└── tests/
```

## 핵심 기술 스택

| 구성 요소 | 기술 |
|---------|------|
| 저장소 | redb 2.x (embedded key-value DB) |
| 풀텍스트 검색 | bm25 crate |
| 퍼지 매칭 | strsim crate |
| CLI 파서 | bpaf 0.9 |
| ID 생성 | ulid |
| 파일 감시 | notify 6.x |
| 시맨틱 검색 | (feature flag `semantic` 사용) |

## 핵심 API

### WikiGraph (lib.rs)

실제 외부 API는 `WikiGraph`에 모여 있고, 내부 `Store`는 더 저수준의 CRUD를 제공합니다.

```rust
// 열기 / 생성
pub fn open(path: &Path, config: Config) -> Result<WikiGraph>

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
pub fn fact_get(&self, id: &FactId) -> Result<FactRecord>
pub fn fact_update(&self, id: &FactId, input: FactUpdate) -> Result<()>
pub fn fact_delete(&self, id: &FactId) -> Result<()>
pub fn fact_feedback(&self, id: &FactId, helpful: bool) -> Result<()>
pub fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>>

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
// If the `semantic` feature is disabled, `WikiGraph::search` returns SearchFailed.

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

// No store-level search/traverse/ingest/lint methods; those live in search.rs/graph.rs/ingest.rs/lint.rs or via WikiGraph wrappers.
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
pub struct SearchOpts { pub limit: Option<usize>, pub min_confidence: Option<f32>, pub entity_filter: Option<Vec<EntityId>>, pub bm25_weight: f32, pub semantic_weight: f32 }
pub struct IngestStats { pub entities_added, pub entities_updated, pub relations_added, pub facts_added, pub files_scanned, pub errors: Vec<String> }
pub struct StoreStats { pub entity_count, pub fact_count, pub relation_count, pub total_size_bytes, pub last_ingest_at: Option<u64> }
pub struct LintReport { pub issues, pub entity_count, pub fact_count, pub relation_count }
pub struct LintIssue { pub severity, pub code, pub message, pub entity_id, pub fact_id }
```

### Config (config.rs)

```rust
pub struct Config {
    pub store: StoreConfig,
    pub model: ModelConfig,
    pub search: SearchConfig,
    pub lint: LintConfig,
}
pub struct StoreConfig { pub path: String }
pub struct ModelConfig { pub name: String, pub download_dir: String, pub cache_dir: String, pub auto_download: bool }
pub struct SearchConfig { pub default_limit: usize, pub min_trust: f32, pub bm25_weight: f32, pub semantic_weight: f32 }
pub struct LintConfig { pub orphan_threshold: u32, pub stale_days: u32, pub duplicate_similarity: f32 }
pub fn Config::load() -> Result<Config>
pub fn Config::load_from(path: &Path) -> Result<Config>
pub fn Config::save(&self) -> Result<()>
pub fn Config::save_to(&self, path: &Path) -> Result<()>
pub fn Config::get(&self, key: &str) -> Option<String>
pub fn Config::set(&mut self, key: &str, value: &str) -> Result<()>
```

## 주요 규칙

### bpaf CLI 규칙
- **positional/command item은 struct/tuple의 가장 오른쪽**에 위치
- `construct!` 매크로 인자 순서는 struct 필드 순서와 일치
- `construct!` 배열 안에 `::` 경로 함수 호출 (`init::init_command()`) 불가 → 반드시 지역 변수 바인딩 사용
- Parser 반환 타입: `impl Parser<Command>` (구체 타입 아님)

### redb 규칙
- `WikiGraph`는 `Arc<RwLock<Store>>`로 공유하고, `Store` 내부는 `Arc<Database>`를 사용
- `Store::open(path, config)`만 존재함 (`new` / `get_or_create`는 없음)
- 트랜잭션: `db.begin_write()`, `db.begin_read()`
- 테이블 open 후 반드시 `drop(meta)` 등 필요 (write_txn.commit() 전)

### 빌드 명령

```bash
# 전체 빌드
cargo build

# 개별 패키지
cargo build -p wg-core
cargo build -p wg-cli

# check only (빠른 검증)
cargo check -p wg-core -p wg-cli

# CLI 실행
./target/debug/wg --help
./target/debug/wg init ./my-wiki
./target/debug/wg ingest ./my-wiki
./target/debug/wg entity list
```

## Phase 1 완료 상태

| 기능 | 상태 |
|-----|------|
| store (redb CRUD) | ✅ |
| entity/fact/relation CRUD | ✅ |
| graph traverse | ✅ |
| search (BM25) | ✅ |
| fuzzy matching | ✅ |
| config 관리 | ✅ |
| ingest (markdown → entity/fact) | ✅ |
| `wg init` | ✅ |
| `wg watch` | ✅ |
| `wg --help` 런타임 정상 | ✅ |

## Phase 2-6 예정

- `wg import` / `wg export`
- `wg model` (시맨틱 벡터 자동 다운로드)
- `wg lint` — 그래프 건전성 검사
- 스키마 마이그레이션
- MCP server mode (`wg mcp serve`)
- NAPI / Python bindings

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
