---
name: wg-agent
description: Wiki-Graph (wg) CLI + Rust crate project — structured wiki indexing for LLM workflows
category: project-specific
tags: [rust, wiki-graph, redb, bm25, bpaf, local-ai]
version: 0.1.0
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
│   │   │   ├── search.rs       ← BM25 + hybrid search
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

```rust
// 열기 / 생성
pub fn open(path: &Path, config: Config) -> Result<WikiGraph>

// Entity CRUD
pub fn add_entity(&mut self, input: EntityInput) -> Result<Ulid>
pub fn get_entity(&self, name: &str) -> Result<Option<Entity>>
pub fn list_entities(&self, opts: ListOpts) -> Result<Vec<Entity>>
pub fn update_entity(&mut self, id: Ulid, update: EntityUpdate) -> Result<()>
pub fn delete_entity(&mut self, name: &str) -> Result<()>

// Fact CRUD
pub fn add_fact(&mut self, input: FactInput) -> Result<Ulid>
pub fn get_fact(&self, id: Ulid) -> Result<Option<Fact>>
pub fn list_facts(&self, opts: FactListOpts) -> Result<Vec<Fact>>

// Ingest (markdown → entity/fact 자동 추출)
pub fn ingest(&mut self, wiki_root: &Path, incremental: bool) -> Result<IngestStats>

// Search
pub fn search(&self, opts: SearchOpts) -> Result<SearchResults>

// Graph
pub fn traverse(&self, opts: TraverseOpts) -> Result<TraverseResult>
pub fn path_find(&self, from: &str, to: &str) -> Result<Option<Vec<Step>>>
```

### 타입 (types.rs)

```rust
pub struct EntityInput { pub name, pub entity_type, pub tags, pub aliases, pub source_page }
pub struct FactInput { pub content, pub fact_type, pub entities, pub tags, pub source, pub confidence }
pub struct RelationInput { pub from_entity, pub relation_type, pub to_entity }
pub enum EntityType { Concept, Technology, Person, Team, Product, Project, Standard, ... }
pub enum FactType { Decision, Pattern, Convention, Claim, Note, Question, ... }
pub enum RelationType { DependsOn, Implements, Provides, Uses, ... }
pub struct IngestStats { entities_added, entities_updated, relations_added, facts_added, files_scanned, errors }
```

### Config (config.rs)

```rust
pub struct Config {
    pub store: StoreConfig,         // .path: String
    pub search: SearchConfig,       // .default_limit: usize, .bm25_weight: f32, .semantic_weight: f32
    pub semantic: SemanticConfig,   // .enabled: bool, .model: String
}
pub fn Config::load() -> Result<Config>   // ~/.wg/config.toml에서 로드
pub fn Config::load_or_default() -> Config
```

## 주요 규칙

### bpaf CLI 규칙
- **positional/command item은 struct/tuple의 가장 오른쪽**에 위치
- `construct!` 매크로 인자 순서는 struct 필드 순서와 일치
- `construct!` 배열 안에 `::` 경로 함수 호출 (`init::init_command()`) 불가 → 반드시 지역 변수 바인딩 사용
- Parser 반환 타입: `impl Parser<Command>` (구체 타입 아님)

### redb 규칙
- `Store`는 `Arc<RwLock<Store>>`로 공유 (mutable borrow 문제 해결)
- `Store::new(path)`, `Store::open(path)`, `Store::get_or_create(path)`
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
