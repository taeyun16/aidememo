# wg (Wiki-Graph) — 구현 계획

> **주의 (2026-05-06):** 이 문서는 초기 설계/로드맵 기록이다. 현재 구현의
> 명령 표면, MCP 도구 목록, 검증 절차의 최신 기준은 `README.md`,
> `AGENTS.md`, `.github/workflows/ci.yml`을 우선한다. 아래의 phase 체크리스트와
> 예전 도구 설명에는 완료되었거나 대체된 내용이 남아 있을 수 있다.

> **v0.2.0** | 2026-04-12
>
> LLM Wiki의 구조 탐색과 팩트 검색을 담당하는 고속 CLI 도구.
> 마크다운 위키 옆에 놓는 구조 인덱스 엔진.
> **에이전트 무관.** 바이너리 하나로 Claude Code, Cursor, Codex, Hermes, 어떤 에이전트에서든 동일하게 동작.
> 서버 없이, 설정 없이, `wg init && wg search` 두 줄이면 시작.
>
> Org-Wiki의 entity graph + factstore 개념을 로컬로 축소하여,
> 개인~소규모 팀 위키에 구조 탐색 능력을 부여한다.
> 규모가 커지면 Org-Wiki 서버로 자연스럽게 전환.


### 변경 이력

| 버전 | 날짜 | 변경 내용 |
|------|------|-----------|
| v0.1.0 | 2026-04-12 | 초기 설계 |
| v0.1.1 | 2026-04-12 | 외부 리뷰 10개 포인트 반영 |
| v0.2.0 | 2026-04-12 | 범용 도구 포지셔닝 (현재) |

#### v0.2.0 변경 상세

| # | 변경 | 이유 | 위치 |
|---|---|---|---|
| 1 | 1차 인터페이스를 CLI + SKILL.md로 확정 | Hermes 전용이 아닌 모든 에이전트에서 동일하게 사용 | §1, §6 |
| 2 | `wg init` zero-config 시작 추가 | 첫 경험까지 2줄 (wg init → wg search) | §3.2 |
| 3 | `wg mcp-serve` MCP 서버 모드 추가 | SKILL.md 배포 없이 MCP URL만으로 사용 | §3.2, §1.1 |
| 4 | `wg watch` 자동 sync 추가 | 수동 re-ingest 제거 | §3.2 |
| 5 | fact 자동 추출 규칙 구체화 | 수동 `wg fact add` 의존 최소화 | §3.2 |
| 6 | 접근 경로를 3-tier로 재정의 | CLI(기본) → MCP(편의) → 바인딩(최적화) | §1.3 |
| 7 | 바인딩을 Phase 5로 재배치 | Phase 2까지만으로 모든 에이전트에서 사용 가능한 제품 | §8 |
| 8 | SKILL.md를 독립 섹션으로 강화 | wg의 가장 중요한 에이전트 인터페이스 | §6 |

#### v0.1.1 변경 상세 (이전)

| # | 리뷰 지적 | 대응 | 위치 |
|---|---|---|---|
| 1 | 엔티티 key가 name 문자열 | EntityId(ULID) canonical key + name/alias 보조 인덱스 | §3.3 |
| 2 | relation key에 rel_type 없어 충돌 | `source_id\0rel_type\0target_id` 복합키 | §3.3 |
| 3 | BM25 인덱스 일관성 전략 부재 | dirty flag + lazy rebuild + u32 doc_id 매핑 정의 | §4.4 |
| 4 | 성능 목표가 cold/warm 미분리 | 4-tier 벤치마크 (cold/warm/embedded/model-warm) | §9.1, §10.4 |
| 5 | 바인딩이 전체 RwLock으로 읽기 확장성 사멸 | Arc\<Database\> + 좁은 lock 동시성 모델 | §5.2 |
| 6 | S3 공유가 oneio IO 추상화만으로 부족 | WAL segment + manifest + compaction 프로토콜 명시 | §4.5 |
| 7 | MVP에 ingest가 없음 | Phase 1에 `wg ingest` + wikilink 파싱 추가 | §3.2, §8 |
| 8 | redb 안정 ≠ 앱 스키마 안정 | meta 테이블에 schema_version/feature_bitmap 추가 | §3.3 |
| 9 | fact_vectors 바이너리 포맷 미정의 | little-endian + dimension/version 헤더 명시 | §3.3 |
| 10 | Node/Bun/Deno 호환성 과대 표현 | Node.js Tier-1, Bun best-effort, Deno experimental | §5.1 |
| 추가 | search_feedback과 fact feedback CLI 불일치 | search_id 기반 click/skip 피드백으로 재설계 | §4.3 |
| 추가 | trust의 의미 미정의 | trust를 source_confidence + relevance_feedback 2축 분리 | §3.3 |


## 프로젝트 위치

```
프로젝트 설계 문서 구조:

org-wiki-final-design    v0.4.0   Layer 1: Memory & Knowledge (서버)
autonomy-design          v0.2.0   Layer 2-3: Agent Pool + Mission Control (서버)
wg-implementation-plan   v0.2.0   Layer 0: Local Index Engine ← 이 문서

관계:
  wg는 llm-wiki SKILL.md의 자연스러운 확장.
  어떤 에이전트든 셸에서 wg를 호출할 수 있으면 동작.
  개인/소규모 팀 → wg (로컬, 서버 없음)
  조직 규모 → Org-Wiki (서버, PG, MCP)
  wg의 데이터는 Org-Wiki로 import 가능 (export/import JSONL).
```


---


## 1. 아키텍처 개요

### 1.1 크레이트 구조

```
wg/
├── Cargo.toml                  # workspace
├── crates/
│   ├── wg-core/                # 핵심 라이브러리 (순수 Rust, FFI 무관)
│   │   ├── src/
│   │   │   ├── lib.rs          # WikiGraph 공개 API
│   │   │   ├── store.rs        # redb 스토리지
│   │   │   ├── graph.rs        # BFS/DFS 엔티티 그래프 탐색
│   │   │   ├── search.rs       # BM25 + Model2Vec 하이브리드 검색
│   │   │   ├── ingest.rs       # 마크다운 파싱 + 자동 추출
│   │   │   ├── index.rs        # 인메모리 인덱스 빌더
│   │   │   ├── fuzzy.rs        # strsim + trigram 퍼지 매칭
│   │   │   ├── lint.rs         # 그래프 건강도 검사
│   │   │   ├── adapt.rs        # 도메인 어댑터 학습
│   │   │   ├── migrate.rs      # JSONL ↔ redb 변환
│   │   │   ├── types.rs        # 공유 타입 (Entity, Fact, Relation)
│   │   │   ├── config.rs       # ~/.wg/config.toml
│   │   │   └── error.rs        # WgError 구조적 에러
│   │   └── Cargo.toml
│   │
│   ├── wg-cli/                 # CLI 바이너리 (1차 인터페이스)
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── cmd/            # bpaf 서브커맨드
│   │   │   │   ├── mod.rs
│   │   │   │   ├── init.rs     # wg init (zero-config 시작)
│   │   │   │   ├── ingest.rs   # wg ingest, wg sync
│   │   │   │   ├── watch.rs    # wg watch (자동 re-ingest)
│   │   │   │   ├── traverse.rs
│   │   │   │   ├── search.rs
│   │   │   │   ├── entity.rs
│   │   │   │   ├── fact.rs
│   │   │   │   ├── lint.rs
│   │   │   │   ├── mcp_serve.rs # wg mcp-serve (MCP 서버 모드)
│   │   │   │   ├── adapt.rs
│   │   │   │   └── migrate.rs
│   │   │   └── output.rs
│   │   └── Cargo.toml
│   │
│   ├── wg-skill/               # SKILL.md + 에이전트별 설정 가이드
│   │   ├── SKILL.md            # 범용 (모든 에이전트)
│   │   ├── setup-claude-code.md
│   │   ├── setup-cursor.md
│   │   ├── setup-hermes.md
│   │   └── setup-codex.md
│   │
│   ├── wg-python/              # PyO3 바인딩 (성능 최적화 경로)
│   │   └── ...
│   ├── wg-napi/                # napi-rs 바인딩
│   │   └── ...
│   ├── wg-nif/                 # Rustler NIF
│   │   └── ...
│   └── wg-ffi/                 # C-ABI
│       └── ...
│
├── models/                     # Model2Vec 모델 (gitignore)
├── tests/
└── benchmarks/
```


### 1.2 핵심 원칙

```
1. 에이전트 무관 (Agent-Agnostic)
   wg는 CLI 도구이다. 어떤 에이전트든 셸에서 호출할 수 있으면 동작한다.
   Claude Code, Cursor, Codex, Hermes, Copilot, 기타 MCP 클라이언트.
   특정 에이전트에 종속되지 않는다.

2. 설치 = 바이너리 하나
   brew install wg 또는 cargo install wg-cli.
   Python, Node.js, Docker, 데이터베이스 서버 불필요.

3. 시작 = 두 줄
   wg init ~/wiki
   wg search "query"
   
4. llm-wiki의 자연스러운 확장
   기존 llm-wiki SKILL.md에 wg 섹션을 추가하면
   에이전트가 grep 대신 wg를 호출한다.
   wg가 없으면 기존 search_files로 fallback.
   
5. 바인딩은 선택적 최적화
   CLI로 충분히 동작한다. PyO3/napi-rs는 "더 빠른 경로"이지 필수가 아니다.
```


### 1.3 접근 경로 (3-Tier)

```
Tier 0 — CLI (기본, 모든 에이전트):
  wg 바이너리 설치 → wg init → SKILL.md를 에이전트에 배치
  에이전트가 셸에서 wg search, wg traverse 호출
  Claude Code, Cursor, Codex, Hermes, Copilot 전부 동작
  
  사용자가 하는 일: 바이너리 설치 + SKILL.md 복사
  전제 조건: 없음

Tier 1 — MCP 서버 (편의, SKILL.md 불필요):
  wg mcp-serve --port 3000
  에이전트에 MCP URL만 추가
  도구 description이 자동 호출을 유도하므로 SKILL.md 배포도 불필요
  
  사용자가 하는 일: 바이너리 설치 + MCP URL 추가
  전제 조건: MCP 지원 에이전트

Tier 2 — 언어 바인딩 (성능 최적화, 특정 런타임):
  pip install wg-python    → Hermes bridge 플러그인에서 in-process 호출
  npm install @aspect/wg   → Node.js 에이전트에서 in-process 호출
  {:wg_nif, ...}           → Elixir/Phoenix Org-Wiki에서 NIF 호출
  
  사용자가 하는 일: 패키지 매니저로 설치 + 코드에서 import
  전제 조건: 해당 런타임 환경
  이점: 프로세스 포크 없이 마이크로초 단위 호출
```

각 에이전트별 구체적 설정:

```
Claude Code:
  1. brew install wg
  2. wg init ~/wiki
  3. cp wg-skill/SKILL.md ~/.claude/skills/wiki-graph/
  → 끝. Claude Code가 SKILL.md를 읽고 wg를 자동 호출.

Cursor:
  1. brew install wg
  2. wg init ~/wiki
  3. cp wg-skill/SKILL.md .cursor/skills/wiki-graph/
  → 끝.

Codex:
  1. brew install wg
  2. wg init ~/wiki
  3. Codex instructions에 SKILL.md 내용 포함
  → 끝.

Hermes Agent:
  방법 A (CLI, 간단):
    1. brew install wg
    2. wg init ~/wiki
    3. Hermes skills/ 에 SKILL.md 배치
    → 끝. Hermes가 셸에서 wg 호출.

  방법 B (PyO3, 고성능):
    1. pip install wg-python
    2. bridge 플러그인에서 import wg
    → in-process 호출, 프로세스 포크 제거, 지연 시간 최소화.
    → 고빈도 호출 시에만 필요한 최적화.

MCP 지원 에이전트 (범용):
  1. brew install wg
  2. wg init ~/wiki
  3. wg mcp-serve --port 3000
  4. 에이전트에 MCP URL 추가: http://localhost:3000
  → SKILL.md 배포도 불필요. MCP 도구 description이 자동 호출 유도.
```

### 1.2 데이터 흐름

```
LLM Wiki (마크다운 파일들)
  │
  │  wg가 ingest 시 자동 추출
  ▼
wg-core (WikiGraph)
  ├── redb (wiki.redb — 단일 파일, 영속 스토리지)
  │   ├── entities      key: name → EntityRecord
  │   ├── relations      key: "source\0target" → RelationRecord
  │   ├── relations_rev  key: "target\0source" → RelationRecord
  │   ├── facts          key: ulid → FactRecord
  │   └── fact_vectors   key: ulid → Vec<f32>  (semantic feature)
  │
  ├── bm25::SearchEngine (인메모리, CLI 시작 시 빌드)
  │   └── fact content/entities/tags → 인버티드 인덱스 + BM25 랭킹
  │
  └── Model2Vec (인메모리, semantic feature)
      └── 쿼리 벡터 생성 → cosine similarity → RRF 합산
```


---


## 2. 기술 스택

### 2.1 의존성

| 역할 | 크레이트 | 선택 이유 |
|------|---------|----------|
| CLI 파싱 | bpaf 0.9 (derive) | clap의 절반 크기 (282KB vs 631KB), 서브커맨드 지원 |
| 직렬화 | serde + serde_json | 사실상 표준, 다른 크레이트와 호환 |
| 설정 | toml | serde 공유, Rust CLI 관례 |
| 영속 스토리지 | redb 2 | pure Rust, ACID, 안정 파일 포맷, ~200KB |
| 검색 엔진 | bm25 (language_detection) | 인메모리 인버티드 인덱스 + BM25 랭킹 자동 관리 |
| SIMD 텍스트 검색 | memchr 2 | 표준 라이브러리 대비 5-6x 빠름 (AVX2/NEON) |
| 퍼지 매칭 | strsim 0.11 | 의존성 0, Jaro-Winkler/Levenshtein |
| pg_trgm 호환 | trigram 0.4 | PostgreSQL pg_trgm similarity 재현 |
| ID 생성 | ulid 1 | 시간순 정렬 가능, serde 지원 |
| 테이블 출력 | comfy-table 7 | 터미널 테이블 포매팅 |
| 시맨틱 검색 (선택) | model2vec-rs 0.1 | 순수 Rust, 기본 multilingual-128M (101개 언어), CPU 15-25K 문장/초 |
| 병렬 처리 (선택) | rayon 1.8 | 5만+ fact 시 병렬 검색 |
| S3 공유 (선택) | oneio 0.20 | URL 하나로 로컬/S3 통합 IO |
| Python 바인딩 | pyo3 0.23 | Hermes Agent bridge 플러그인 |
| Node.js 바인딩 | napi-rs 2 | Node.js + Bun + Deno 공통 |
| Elixir 바인딩 | rustler 0.36 | Org-Wiki Phoenix 연동 |

### 2.2 품질 기준

```toml
[workspace.package]
edition = "2024"
rust-version = "1.85"

[workspace.lints.rust]
unsafe_code = "forbid"          # wg-core에서 unsafe 절대 금지

[workspace.lints.clippy]
unwrap_used = "deny"            # unwrap 금지, ? 또는 map_err 사용
expect_used = "warn"            # 초기화 시점에서만 허용, 주석 필수
panic = "deny"
todo = "warn"
dbg_macro = "deny"
missing_errors_doc = "warn"
missing_panics_doc = "warn"
```

- FFI 바인딩 크레이트(wg-python, wg-napi, wg-nif, wg-ffi)에서만 FFI 경계에 필요한 최소 unsafe 허용
- 모든 public API에 `Result<T, WgError>` 반환
- 에러 메시지에 context 포함 (파일 경로, 엔티티 이름, 시도한 연산)
- EntityNotFound 시 trigram 기반 유사 엔티티 자동 제안

### 2.3 에러 타입

```rust
#[derive(Debug)]
pub enum WgError {
    // 스토리지
    StoreOpen { path: PathBuf, source: redb::DatabaseError },
    StoreRead { table: &'static str, key: String, source: redb::StorageError },
    StoreWrite { table: &'static str, key: String, source: redb::StorageError },
    
    // 직렬화
    Serialize { context: String, source: serde_json::Error },
    Deserialize { context: String, source: serde_json::Error },
    
    // 그래프
    EntityNotFound { name: String, suggestions: Vec<String> },
    CycleDetected { path: Vec<String> },
    
    // 설정
    ConfigRead { path: PathBuf, source: std::io::Error },
    ConfigParse { path: PathBuf, source: toml::de::Error },
    
    // 모델 (semantic feature)
    #[cfg(feature = "semantic")]
    ModelNotFound { name: String, cache_dir: PathBuf },
    #[cfg(feature = "semantic")]
    ModelDownloadFailed { name: String, source: Box<dyn std::error::Error + Send + Sync> },
    #[cfg(feature = "semantic")]
    ModelLoadFailed { path: PathBuf, source: Box<dyn std::error::Error + Send + Sync> },
    
    // 마이그레이션
    MigrationFailed { phase: String, source: Box<WgError> },
    
    // S3 (feature-gated)
    #[cfg(feature = "remote")]
    RemoteIo { url: String, source: oneio::OneIoError },
}
```

### 2.4 Feature 구성

```toml
[features]
default = []
semantic = ["model2vec-rs"]         # +128MB 모델 (multilingual), 벡터 검색, RRF 합산
semantic-adapt = ["semantic"]       # + 도메인 어댑터 학습
parallel = ["rayon"]                # 5만+ fact 병렬 검색
remote = ["oneio"]                  # S3 공유 (WAL 패턴)
```

### 2.5 모델 설정

semantic feature 활성화 시, 기본 모델은 `potion-multilingual-128M` (101개 언어, 한국어 포함).
영어 전용 위키라면 `potion-base-8M`으로 전환하여 모델 크기를 16배 줄일 수 있음.

```bash
# 기본: 다국어 (한국어 포함, 128MB)
wg config set model.name "minishlab/potion-multilingual-128M"

# 영어 전용 위키라면 경량 모델로 전환 (8MB)
wg config set model.name "minishlab/potion-base-8M"

# 고성능 영어 전용 (32MB, 더 큰 어휘)
wg config set model.name "minishlab/potion-base-32M"

# 현재 설정 확인
wg config get model.name
# → minishlab/potion-multilingual-128M

# 모델 상태 확인
wg model status
# → Model: potion-multilingual-128M (128M params, 256 dim, 101 languages)
#   Path: ~/.wg/models/potion-multilingual-128M/
#   Size: 128 MB
#   Status: ready
```

**모델 선택 가이드:**

| 모델 | 파라미터 | 크기 | 언어 | 속도 (CPU) | 용도 |
|------|---------|------|------|-----------|------|
| potion-multilingual-128M | 128M | ~128 MB | 101개 (한국어 ✓) | ~15-20K 문장/초 | **기본값.** 한영 혼용 위키 |
| potion-base-32M | 32.3M | ~32 MB | English | ~20-25K 문장/초 | 영어 전용, 고성능 |
| potion-base-8M | 7.5M | ~8 MB | English | ~25K+ 문장/초 | 영어 전용, 최소 크기 |

모델 파일은 첫 실행 시 HuggingFace Hub에서 자동 다운로드되며,
`~/.wg/models/` 에 캐시됨. `wg model download` 로 수동 다운로드도 가능.

**config.toml 기본값:**

```toml
# ~/.wg/config.toml
[store]
path = "./_meta/wiki.redb"     # 위키 루트 기준 상대 경로

[model]
name = "minishlab/potion-multilingual-128M"   # 기본 모델
cache_dir = "~/.wg/models"                    # 모델 캐시 디렉토리
auto_download = true                          # 첫 실행 시 자동 다운로드

[search]
default_limit = 10
min_trust = 0.0
bm25_weight = 1.0              # RRF에서 BM25 가중치
semantic_weight = 1.0           # RRF에서 semantic 가중치

[lint]
orphan_threshold = 0            # 최소 inbound 링크 수
stale_days = 90                 # N일 이상 미접근 시 stale
duplicate_similarity = 0.9      # 엔티티 중복 판정 trigram 임계값
```

### 2.6 바이너리 크기 예상

```
구성                                    예상 크기 (stripped, release, opt-level=z)
최소 (bpaf + serde + redb + bm25)       ~2.0 MB
기본 (+ memchr + strsim + trigram)      ~2.2 MB
semantic (+ model2vec-rs)               ~3.2 MB (+128MB 모델 파일 별도 다운로드)
remote (+ oneio s3)                     ~3.8 MB
전부                                    ~4.5 MB

모델 파일 (별도, ~/.wg/models/ 에 캐시):
  potion-multilingual-128M              ~128 MB (기본)
  potion-base-32M                       ~32 MB
  potion-base-8M                        ~8 MB
```


---


## 3. wg-core API

### 3.1 WikiGraph 핵심 API

```rust
pub struct WikiGraph { ... }

impl WikiGraph {
    // 라이프사이클
    pub fn open(path: &Path, config: Config) -> Result<Self>;
    pub fn close(self) -> Result<()>;
    
    // === 엔티티 ===
    pub fn entity_add(&mut self, input: EntityInput) -> Result<EntityId>;
    pub fn entity_get(&self, name: &str) -> Result<EntityRecord>;
    pub fn entity_update(&mut self, name: &str, input: EntityUpdate) -> Result<()>;
    pub fn entity_list(&self, opts: ListOpts) -> Result<Vec<EntitySummary>>;
    pub fn entity_delete(&mut self, name: &str) -> Result<()>;
    
    // === 관계 ===
    pub fn relation_add(&mut self, input: RelationInput) -> Result<()>;
    pub fn relation_remove(&mut self, source: &str, target: &str, rel_type: &str) -> Result<()>;
    pub fn traverse(&self, start: &str, opts: TraverseOpts) -> Result<TraverseResult>;
    pub fn path_find(&self, from: &str, to: &str) -> Result<Option<Vec<PathStep>>>;
    
    // === 팩트 ===
    pub fn fact_add(&mut self, input: FactInput) -> Result<FactId>;
    pub fn fact_get(&self, id: &FactId) -> Result<FactRecord>;
    pub fn fact_update(&mut self, id: &FactId, input: FactUpdate) -> Result<()>;
    pub fn fact_delete(&mut self, id: &FactId) -> Result<()>;
    pub fn fact_feedback(&mut self, id: &FactId, helpful: bool) -> Result<()>;
    pub fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>>;
    
    // === 검색 ===
    pub fn search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>>;
    pub fn search_with_traverse(
        &self, query: &str, start: &str, depth: u32, opts: SearchOpts,
    ) -> Result<Vec<SearchResult>>;
    
    // === 린트 ===
    pub fn lint(&self) -> Result<LintReport>;
    
    // === 어댑터 ===
    #[cfg(feature = "semantic-adapt")]
    pub fn adapt_train(&mut self) -> Result<AdaptResult>;
    #[cfg(feature = "semantic-adapt")]
    pub fn adapt_status(&self) -> Result<AdaptStatus>;
    #[cfg(feature = "semantic-adapt")]
    pub fn adapt_eval(&self) -> Result<AdaptEvalReport>;
    
    // === import/export ===
    pub fn export_jsonl(&self, writer: &mut dyn Write, scope: ExportScope) -> Result<ExportStats>;
    pub fn import_jsonl(&mut self, reader: &mut dyn Read) -> Result<ImportStats>;
    
    // === 통계 ===
    pub fn stats(&self) -> Result<StoreStats>;
}
```

### 3.2 CLI 인터페이스

```bash
# === ingest (★ MVP 핵심) ===
wg ingest <wiki-root>              # 위키 전체 초기 인덱싱
wg ingest <wiki-root> --incremental  # 변경분만 재인덱싱
wg sync                            # ingest --incremental의 단축
wg watch <wiki-root>               # 파일 변경 감지 시 자동 sync (향후)

# ingest가 하는 일:
#   1. 마크다운 파일 스캔 (entities/, concepts/, comparisons/)
#   2. YAML frontmatter 파싱 → 엔티티 자동 생성/갱신
#   3. [[wikilink]] 파싱 → 관계 자동 추출
#   4. heading anchor 정규화 → source 경로 생성
#   5. 본문에서 fact 후보 추출 (LLM 판단 필요 시 마킹만)
#   6. 삭제/이름 변경된 파일 처리 (entity 유지, source_page 갱신)

# === 그래프 탐색 ===
wg traverse Redis --depth 3
wg traverse Redis --depth 2 --relation-type uses,depends_on
wg path Redis PostgreSQL

# === 엔티티 관리 ===
wg entity add Redis --type technology --tags infra,cache
wg entity add Redis --relate "Sentinel:uses" --relate "PostgreSQL:depends_on"
wg entity get Redis
wg entity rename Redis "Redis Server"     # EntityId 불변, name만 변경
wg entity alias Redis --add "redis-cache"  # 별칭 추가
wg entity list --sort fact-count --min-facts 2
wg entity list --type technology
wg entity delete Redis

# === 팩트 관리 ===
wg fact add "Sentinel로 HA 구성 결정" --type decision --entities Redis,Sentinel --source "entities/redis.md#ha"
wg fact get <fact-id>
wg fact list --type decision --min-confidence 0.7 --entities Redis
wg fact delete <fact-id>

# === 검색 ===
wg search "고가용성"
wg search "고가용성" --traverse-from Redis --depth 2
wg search "고가용성" --min-confidence 0.5 --limit 20
wg search "고가용성" --format json
# 검색 결과에 search_session_id가 포함됨

# === 검색 피드백 (semantic-adapt feature) ===
wg feedback click <search_session_id> <fact_id>    # 이 결과를 선택함
wg feedback skip <search_session_id> <fact_id>     # 이 결과를 건너뜀

# === 린트 ===
wg lint
wg lint --format json

# === 어댑터 (semantic-adapt feature) ===
wg adapt status
wg adapt train
wg adapt eval

# === 설정 ===
wg config get model.name
wg config set model.name "minishlab/potion-multilingual-128M"
wg config set model.name "minishlab/potion-base-8M"
wg config set search.default_limit 20
wg config list

# === 모델 관리 (semantic feature) ===
wg model status                    # 현재 모델 정보
wg model download                  # 설정된 모델 수동 다운로드
wg model download "minishlab/potion-base-8M"  # 특정 모델 다운로드
wg model rebuild-vectors           # 모델 변경 후 벡터 재계산

# === import/export ===
wg export > backup.jsonl           # schema_version 포함
wg export --scope entities > entities.jsonl
wg import < data.jsonl             # schema_version 호환성 체크

# === 통계 ===
wg stats

# === 출력 형식 (모든 커맨드 공통) ===
--format json          # 기본 (에이전트 소비용)
--format table         # 사람용 터미널 테이블
--format md            # 마크다운 (위키 페이지 삽입용)
```

#### ingest 파싱 규칙

```
wikilink 파싱:
  [[Redis]]           → 엔티티 "Redis" 참조
  [[Redis|레디스]]     → 엔티티 "Redis" 참조 (display text 무시)
  [[redis-sentinel]]  → alias → canonical entity 해소

frontmatter 규칙:
  type: entity|concept|comparison|query  → entity_type 매핑
  tags: [...]  → 엔티티/팩트 태그
  sources: [...] → evidence 경로

heading anchor 정규화:
  ## HA Strategy → "entities/redis.md#ha-strategy"
  source 경로에 사용

파일 삭제/이름 변경:
  파일 삭제 → 엔티티는 유지 (다른 참조가 있을 수 있음), source_page만 null로
  파일 이름 변경 → source_page 경로 갱신, EntityId 불변

incremental re-ingest:
  파일 mtime 비교 → 변경된 파일만 재파싱
  삭제된 파일 → 해당 파일에서 추출된 fact의 source 무효화
  meta 테이블에 last_ingest_at 기록
```

### 3.3 redb 테이블 설계

#### 핵심 원칙: 엔티티는 ID로, 이름은 인덱스로

모든 엔티티는 `EntityId(ULID)`가 canonical key. name/alias는 보조 인덱스로만 존재.
relation, fact는 전부 EntityId를 참조. CLI와 바인딩만 이름을 받아 resolve.

```
── 메타 ──

meta            key: "schema_version"   → u32 (현재: 1)
                key: "feature_bitmap"   → u64 (비트: semantic, adapter, remote 등)
                key: "created_by"       → String ("wg 0.1.1")
                key: "last_migrated_at" → u64 (epoch ms)
                key: "stats"            → StoreStats { entity_count, fact_count, ... }

── 엔티티 ──

entities        key: EntityId (ULID)
                value: EntityRecord {
                    id: EntityId,
                    name: String,          # display name ("Redis")
                    name_lower: String,    # 정규화 ("redis")
                    entity_type: String,   # "technology"
                    aliases: Vec<String>,  # ["redis-server", "redis-cache"]
                    tags: Vec<String>,
                    source_page: Option<String>,  # "entities/redis.md"
                    created_at: u64,
                    updated_at: u64,
                }

entity_by_name  key: "redis" (lowercase)         → EntityId
                key: "redis-server" (alias)       → EntityId
                key: "redis-cache" (alias)        → EntityId
                # rename/alias 변경 시 이 테이블만 갱신. 나머지는 불변.

── 관계 ──

relations       key: "{source_id}\0{rel_type}\0{target_id}"
                value: RelationRecord {
                    source_id: EntityId,
                    target_id: EntityId,
                    relation_type: String,   # "uses", "depends_on", "decided_by"
                    weight: f32,
                    evidence: Vec<String>,    # source page paths
                    created_at: u64,
                }
                # 같은 두 엔티티 사이에 uses + depends_on 동시 저장 가능

relations_rev   key: "{target_id}\0{rel_type}\0{source_id}"
                value: RelationRecord { ... }
                # 역방향 탐색용. relations와 동일 데이터, 키 순서만 반전.

── 팩트 ──

facts           key: FactId (ULID, 시간순 정렬)
                value: FactRecord {
                    id: FactId,
                    content: String,
                    fact_type: String,          # "decision", "pattern", "convention", "claim"
                    entity_ids: Vec<EntityId>,  # ★ 이름 아닌 ID 참조
                    tags: Vec<String>,
                    source: Option<String>,     # "entities/redis.md#sentinel-decision"

                    # trust 2축 분리 (리뷰 포인트 추가)
                    source_confidence: f32,     # 출처 신뢰도 (0-1): 수동 입력 1.0, 자동 추출 0.5
                    relevance_score: f32,       # 사용 유용도 (0-1): 피드백 기반 갱신

                    created_at: u64,
                    updated_at: u64,
                    access_count: u32,
                    last_accessed_at: u64,
                }

fact_by_entity  key: "{entity_id}\0{fact_id}"   → ()
                # 특정 엔티티의 모든 fact를 prefix scan으로 조회
                # entity_ids 변경 시 이 인덱스도 갱신

── 시맨틱 검색 (semantic feature) ──

fact_vectors    key: FactId (ULID)
                value: VectorRecord (명시적 바이너리 포맷)

                VectorRecord 포맷:
                  bytes[0]:      version (u8, 현재: 1)
                  bytes[1..3]:   dimensions (u16 LE, 현재: 256)
                  bytes[3]:      dtype (u8, 0=f32 1=f16 2=i8)
                  bytes[4..]:    little-endian 벡터 데이터

                # 차원 수 변경, quantization, adapter 추가 시 version bump으로 대응
                # 읽기 시 version 체크 → 불일치면 재계산 트리거

── 피드백 (semantic-adapt feature) ──

search_sessions key: SearchSessionId (ULID)
                value: SearchSession {
                    query: String,
                    config_snapshot: String,     # "bm25+semantic" 등
                    result_fact_ids: Vec<FactId>, # 반환된 결과 순서대로
                    created_at: u64,
                }

search_feedback key: "{search_session_id}\0{fact_id}"
                value: FeedbackRecord {
                    action: String,   # "click" | "skip"
                    rank: u32,        # 결과 내 순위
                    timestamp: u64,
                }
                # retrieval adaptation은 query-context + rank가 핵심.
                # "이 fact가 helpful" 보다 "어떤 query에서 몇 위를 클릭/스킵했는가"가 중요.

domain_adapter  key: "current"
                value: AdapterRecord {
                    weights: Vec<Vec<f32>>,  // 256×256
                    bias: Vec<f32>,
                    trained_on: u32,
                    trained_at: u64,
                }
                (semantic-adapt feature에서만 사용)
```

#### trust 2축 분리

기존 단일 `trust` 필드를 `source_confidence`와 `relevance_score`로 분리.
두 축은 독립적으로 갱신되며, 검색 랭킹에서 가중 합산.

```
source_confidence (출처 신뢰도):
  수동 입력 (human confirm):     1.0
  팀원 확인:                     +0.15 per confirmation (cap 1.0)
  자동 추출 (세션 기반):          0.5
  LLM 추론:                     0.3
  시간 감쇠:                     *= 0.95 ^ (days_since_verified / 30)

relevance_score (사용 유용도):
  초기값:                        0.5
  검색 결과 클릭:                +0.05
  검색 결과 스킵:                -0.02
  명시적 helpful:                +0.10
  명시적 not-helpful:            -0.15

검색 랭킹에서:
  combined_score = bm25_score × (0.6 × source_confidence + 0.4 × relevance_score)
  --min-trust는 source_confidence 기준 (출처 필터)
```


---


## 4. 검색 전략

### 4.1 3단계 검색 파이프라인

```
1단계: 구조 탐색 (traverse)
  → 시작 엔티티에서 BFS → 관련 엔티티 집합 발견
  → 검색 대상을 90%+ 축소
  → redb key lookup, 0.1ms

2단계: BM25 키워드 검색
  → bm25 크레이트가 인메모리 인버티드 인덱스 자동 관리
  → 토크나이저(스테밍, 불용어 제거, 유니코드 정규화) 포함
  → 1만 fact에서 ~1ms

3단계: 시맨틱 검색 (semantic feature)
  → Model2Vec (기본: potion-multilingual-128M, 101개 언어)로 쿼리/팩트 벡터 생성
  → cosine similarity → 단어가 다른 의미적 매칭 발견
  → 한영 혼용 매칭: "장애" ↔ "failure", "캐시" ↔ "cache" (다국어 모델이므로 가능)
  → RRF (Reciprocal Rank Fusion)로 BM25 + semantic 합산
  → 0.05-0.07ms (쿼리 벡터) + 0.5ms (1만 fact cosine)
  → 영어 전용 위키: wg config set model.name "minishlab/potion-base-8M" 으로 전환
```

### 4.2 BM25가 못 잡고 semantic이 잡는 케이스

```
query: "시스템 장애 대응"
  BM25 결과:    "장애 대응 매뉴얼 작성 완료" (단어 일치)
  semantic 추가: "failover 자동화 RTO 15초" (의미 매칭)
                 "p99 레이턴시 200ms 초과 시 알림" (의미 매칭)
```

### 4.3 자동 학습 파이프라인 (semantic-adapt)

```
Phase 1: 피드백 축적 (wg 런타임, 자동)
  wg search → 결과에 search_session_id 포함
  wg feedback click <session_id> <fact_id>    → "이 query에서 이 결과를 선택"
  wg feedback skip <session_id> <fact_id>     → "이 query에서 이 결과를 건너뜀"
  → redb search_sessions + search_feedback 테이블에 축적

Phase 2: 어댑터 학습 (wg adapt train, 수동/cron)
  50+ 피드백 축적 시 → 256×256 선형 변환 학습 (CPU 수초)
  → domain_adapter 테이블에 저장

Phase 3: 도메인 증류 (오프라인, Python, 선택적)
  fact 1000+개 축적 → Model2Vec 증류 (CPU 30초)
  → 도메인 특화 safetensors 생성
```


### 4.4 검색 인덱스 일관성

BM25 인메모리 인덱스와 redb 영속 스토리지 사이의 일관성 전략.

#### doc_id 매핑

bm25 크레이트는 numeric document id가 효율적이므로, FactId(ULID) ↔ u32 doc_id 매핑을 유지:

```rust
struct IndexState {
    engine: bm25::SearchEngine,
    id_map: Vec<FactId>,           // doc_id(u32 index) → FactId
    reverse_map: HashMap<FactId, u32>,  // FactId → doc_id
    dirty: bool,                   // 쓰기 후 rebuild 필요 여부
    generation: u64,               // rebuild 횟수 (avgdl 보정 추적)
}
```

#### 일관성 모드

```
1. CLI one-shot (기본):
   startup 시 full rebuild → 검색 → 종료
   일관성 문제 없음. 매 호출이 clean state.

2. Library embedded (PyO3, napi-rs, NIF):
   WikiGraph::open 시 full rebuild
   fact_add/update/delete 시 dirty = true
   다음 search 호출 전에 dirty면 incremental rebuild
   
   incremental rebuild:
     fact_add:     engine에 document 추가 + id_map push
     fact_delete:  engine에서 document 제거 + id_map 마킹
     fact_update:  delete + add
   
   단, 100회 이상 incremental 변경 누적 시 full rebuild
   (avgdl 드리프트 보정)

3. Long-running daemon (향후):
   주기적 full rebuild (매 N분 또는 변경 M건)
   rebuild 중 기존 인덱스로 검색 계속 (swap-on-complete)
```

#### avgdl 보정 규칙

```
bm25의 avgdl(평균 문서 길이)은 인덱스 빌드 시 계산됨.
incremental add/delete가 누적되면 실제 avgdl과 인덱스의 avgdl이 괴리.

규칙:
  |actual_avgdl - indexed_avgdl| / indexed_avgdl > 0.1  → full rebuild 트리거
  또는 incremental 변경 100건 초과 → full rebuild 트리거
  rebuild 시 generation += 1
```


---


## 5. 바인딩 전략

### 5.1 각 런타임별 배포

| 바인딩 | 기술 | 패키지 | 대상 런타임 | 지원 수준 |
|-------|------|--------|-----------|----------|
| wg-cli | Rust 바이너리 | cargo install, homebrew | 터미널, 에이전트 셸 | Tier-1 |
| wg-python | PyO3 | pip install wg-python | Hermes Agent, Python 스크립트 | Tier-1 |
| wg-napi | napi-rs | npm install @aspect/wg | Node.js ≥18 | Tier-1 |
| wg-napi | napi-rs | (동일 패키지) | Bun ≥1.0 | Best-effort |
| wg-napi | napi-rs | (동일 패키지) | Deno (Node 호환 모드) | Experimental |
| wg-nif | Rustler | hex (native 빌드) | Elixir/Phoenix (Org-Wiki) | Tier-1 |
| wg-ffi | C-ABI | libwg.so/dylib | Clojure/JVM, Swift, Go 등 | Tier-2 |

Bun의 Node-API 호환성은 향상되고 있으나 완전하지 않음.
Deno는 Node 호환 모드에서 동작 가능하나 검증 범위 제한적.
동작하지 않는 경우 wg-ffi(C-ABI)로 대체 가능.

### 5.2 wg-core와 바인딩의 동시성 모델

wg-core 내부는 redb의 concurrent readers + single writer를 활용.
바인딩에서 전체 WikiGraph를 하나의 RwLock으로 감싸면 읽기 확장성이 사멸하므로,
구조를 분리한다:

```rust
// wg-core 내부 구조
pub struct WikiGraph {
    db: Arc<redb::Database>,           // redb: 다중 read_txn 동시 가능
    index: Arc<RwLock<IndexState>>,    // BM25 인덱스: rebuild 시에만 write lock
    config: Arc<Config>,               // 불변
    
    #[cfg(feature = "semantic")]
    model: Arc<StaticModel>,           // Model2Vec: 불변 (로드 후 변경 없음)
    
    #[cfg(feature = "semantic")]
    vectors: Arc<RwLock<VectorCache>>, // 벡터 캐시: 추가 시에만 write lock
}

// 읽기 연산 (search, traverse, entity_get):
//   db.begin_read() — redb read txn, lock-free, 동시 N개 가능
//   index.read() — RwLock read, 동시 N개 가능
//   model — Arc, 불변, 동시 접근 무제한

// 쓰기 연산 (fact_add, entity_add):
//   db.begin_write() — redb write txn, 단일 writer (redb가 보장)
//   index.write() — dirty flag 설정만 (즉시 rebuild 아님)
//   다음 search 전에 dirty면 index.write()로 rebuild

// 결과: 읽기는 거의 lock-free, 쓰기만 좁게 직렬화
```

**바인딩별 래핑:**

```
wg-python (PyO3 0.23):
  #[pyclass] struct PyWikiGraph(Arc<WikiGraph>)
  WikiGraph가 Send + Sync이면 PyO3 free-threaded Python 3.13 호환
  별도 RwLock 없이 Arc만으로 충분 (내부 lock이 세밀하게 나뉨)

wg-napi (napi-rs):
  #[napi] struct WgStore(Arc<WikiGraph>)
  동일 패턴. napi-rs는 Send + Sync만 요구.

wg-nif (Rustler):
  ResourceArc<WikiGraph>
  Rustler ResourceArc가 thread-safe 참조 제공.
  mutable state는 WikiGraph 내부의 RwLock이 처리.
```

### 5.3 Hermes Agent 연동 (wg-python)

```python
# bridge 플러그인에서 MCP 왕복 없이 in-process 호출
import wg

wiki = wg.PyWikiGraph("/srv/hermes/members/alice/wiki/_meta/wiki.redb")

def on_session_start(session_data, agent_context):
    entities = extract_recent_entities(session_data)
    results = wiki.search(" ".join(entities), traverse_from=entities[0], depth=2)
    if results:
        agent_context["team_knowledge"] = format_results(results)
```

### 5.4 Node.js/Bun 연동 (wg-napi)

```typescript
import { WgStore } from '@aspect/wg'  // TypeScript 타입 자동 생성

const wiki = new WgStore('./wiki/_meta/wiki.redb')
const results = wiki.search('고가용성', { traverseFrom: 'Redis', depth: 2 })
```


---


## 6. SKILL.md — 에이전트 인터페이스

wg의 가장 중요한 에이전트 인터페이스.
이 파일 하나로 Claude Code, Cursor, Codex, Hermes 어디서든 wg를 사용.
기존 llm-wiki SKILL.md를 **대체하지 않고 확장**한다.
SKILL.md 전문과 에이전트별 설정 가이드는 wg-skill/ 디렉토리에 포함.

wg가 없으면 기존 search_files + index.md 방식으로 fallback.
wg가 있으면 에이전트가 grep 대신 wg search를 호출하여 구조 탐색과 BM25 검색을 활용.


---


## 7. Org-Wiki와의 관계

### 7.1 마이그레이션 경로

```
개인 wg 위키  →  Org-Wiki 서버

wg export > my_facts.jsonl
# JSONL에 entities, relations, facts가 모두 포함

# Org-Wiki MCP 도구로 import
curl -X POST https://wiki.acme.internal/api/mcp/.../tools/call \
  -d '{"name": "import_personal_facts", "arguments": {"jsonl": "..."}}'
```

### 7.2 개념 매핑

| wg (로컬) | Org-Wiki (서버) |
|----------|----------------|
| redb entities | public.personal_facts + team_facts |
| redb relations | schema.entity_relations |
| redb facts | public.personal_facts |
| BM25 인메모리 인덱스 | PostgreSQL tsvector + GIN |
| Model2Vec cosine | (Phase 5 pgvector 선택적) |
| wg lint | Quality Gate Stage 1 |
| JSONL export | MCP promote_fact, promote_wiki_page |
| strsim/trigram | pg_trgm |
| redb BFS | PostgreSQL recursive CTE |

### 7.3 공존

wg가 있어도 Org-Wiki가 필요한 시점:
- 팀 10명+ → ACL, Quality Gate, 크로스팀 검색이 필요
- 실시간 멀티유저 동시 편집 → PostgreSQL MVCC
- Gateway (Slack/Discord) 연동 → Phoenix 서버
- 세션 트랜스크립트 아카이브 → S3 Parquet

wg는 개인/소규모 팀에서 "서버 없이 구조 탐색"을 제공하고,
Org-Wiki는 조직 규모에서 "서버 기반 지식 관리"를 제공합니다.


---


## 8. 구현 로드맵

Phase 순서 원칙:
- Phase 2 완료 시 **모든 에이전트에서 사용 가능한 제품** (CLI + SKILL.md)
- MCP 서버를 Phase 3에 배치하여 SKILL.md 없이도 사용 가능
- 바인딩(PyO3, napi-rs)은 Phase 5 — CLI로 충분히 동작하므로 성능 최적화 단계

### Phase 1 (3주): store + ingest + wg init + traverse

**목표: `wg init ~/wiki && wg traverse Redis --depth 2` 동작.**

- [ ] workspace 셋업 (Cargo.toml, lints, edition 2024, profile)
- [ ] wg-core/types.rs — EntityId(ULID), FactId(ULID), EntityRecord, FactRecord, RelationRecord
- [ ] wg-core/error.rs — WgError (구조적 에러, 퍼지 제안, ModelNotFound 등)
- [ ] wg-core/config.rs — ~/.wg/config.toml 파서 (기본값으로 즉시 동작)
- [ ] wg-core/store.rs — redb 열기/닫기, meta 테이블 (schema_version, feature_bitmap)
- [ ] wg-core/store.rs — entities + entity_by_name (ULID key + name/alias 보조 인덱스)
- [ ] wg-core/store.rs — relations + relations_rev (source_id\0rel_type\0target_id 복합키)
- [ ] wg-core/store.rs — facts + fact_by_entity (EntityId 참조)
- [ ] wg-core/store.rs — entity/relation/fact CRUD (EntityId canonical, CLI는 name resolve)
- [ ] wg-core/store.rs — trust 2축: source_confidence + relevance_score
- [ ] wg-core/ingest.rs — 마크다운 파서 (frontmatter YAML, [[wikilink]], heading anchor)
- [ ] wg-core/ingest.rs — fact 자동 추출 (decision/convention 헤딩, 마커 패턴)
- [ ] wg-core/ingest.rs — wg ingest: 전체 스캔 → 엔티티/관계/팩트 자동 추출
- [ ] wg-core/ingest.rs — incremental re-ingest (mtime 비교, 변경분만)
- [ ] wg-core/ingest.rs — 파일 삭제/이름변경 처리 (EntityId 불변, source_page 갱신)
- [ ] wg-core/graph.rs — BFS 탐색 (traverse), 경로 찾기 (path_find), EntityId 기반
- [ ] wg-core/fuzzy.rs — 엔티티 퍼지 매칭 (strsim + trigram, entity_by_name 활용)
- [ ] wg-core/lib.rs — WikiGraph 공개 API
- [ ] wg-cli/cmd/init.rs — `wg init`: config 생성 + ingest 자동 실행 + 첫 검색 예시 출력
- [ ] wg-cli/cmd/ — bpaf 서브커맨드 (init, ingest, sync, entity, fact, traverse, path)
- [ ] wg-cli/output.rs — JSON/table/md 출력
- [ ] tests/ — 통합 테스트 (rename/alias 변경 시 연쇄 영향 없음 검증)
- [ ] benchmarks/criterion/ — 규모별 startup/traverse/fact_add 벤치마크
- [ ] benchmarks/retrieval/golden/wiki/ — 골든 데이터셋 위키 구축 시작
- [ ] 기본 README

**체크포인트:**
```
wg init ~/wiki
# → "Indexed 42 entities, 156 facts, 89 relations. Try: wg search 'Redis'"
wg traverse Redis --depth 2
# → 관계 그래프 출력
wg entity rename Redis "Redis Server"
# → 관계/팩트에 영향 없음
```

### Phase 2 (2주): BM25 검색 + 린트 + SKILL.md + watch

**목표: Phase 2 완료 = 모든 에이전트에서 사용 가능한 제품.**

이 Phase가 끝나면 Claude Code, Cursor, Codex, Hermes 어디서든
SKILL.md를 배치하고 wg를 사용할 수 있다.

- [ ] wg-core/index.rs — BM25 인메모리 인덱스 빌더 (u32 doc_id ↔ FactId 매핑)
- [ ] wg-core/index.rs — dirty flag + lazy rebuild + avgdl 보정 규칙
- [ ] wg-core/search.rs — search(), search_with_traverse()
- [ ] wg-core/search.rs — combined_score 계산
- [ ] wg-core/search.rs — memchr 기반 fallback
- [ ] wg-core/lint.rs — 고립 엔티티, 단방향 관계, 중복 후보
- [ ] wg-core/migrate.rs — export_jsonl (schema_version), import_jsonl (호환성 체크)
- [ ] wg-cli/cmd/ — search, lint, export, import 커맨드
- [ ] wg-cli/cmd/watch.rs — `wg watch`: 파일 변경 감지 → 자동 re-ingest
- [ ] wg-skill/SKILL.md — 범용 SKILL.md 작성 (모든 에이전트용)
- [ ] wg-skill/setup-*.md — 에이전트별 설정 가이드 (claude-code, cursor, hermes, codex)
- [ ] benchmarks/criterion/ — cold start + warm start 분리 벤치마크
- [ ] benchmarks/retrieval/ — 골든 데이터셋 완성 + ablation 실행
- [ ] benchmarks/retrieval/metrics.rs — Recall, Precision, MRR, nDCG
- [ ] benchmarks/agent_quality/ — LLM-as-Judge 파이프라인 (CLI로 에이전트 품질 비교)

**체크포인트:**
```
# Claude Code에서:
cp wg-skill/SKILL.md ~/.claude/skills/wiki-graph/SKILL.md
# → Claude Code가 자동으로 wg search 호출

# Cursor에서:
cp wg-skill/SKILL.md .cursor/skills/wiki-graph/SKILL.md
# → 동일하게 동작

# 에이전트 품질: with_wg vs without_wg에서 +1.0점 이상
```

### Phase 3 (2주): MCP 서버 + 시맨틱 검색

**목표: MCP URL 하나로 SKILL.md 배포 없이 사용. 시맨틱 검색 추가.**

- [ ] wg-cli/cmd/mcp_serve.rs — `wg mcp-serve`: MCP 서버 모드
- [ ] MCP 도구 정의 (wg_search, wg_traverse, wg_entity_list, wg_fact_add, wg_lint)
- [ ] MCP 도구 description에 자동 호출 유도 문구 (SKILL.md 불필요하게)
- [ ] wg-core/search.rs — Model2Vec 로드/다운로드/캐시 (기본 multilingual-128M)
- [ ] wg-core/search.rs — VectorRecord, RRF 합산, 벡터 재계산
- [ ] wg-cli/cmd/ — config, model 커맨드
- [ ] 모델 비교 벤치마크 (multilingual vs base-8M)

**체크포인트:**
```
wg mcp-serve --port 3000
# → 에이전트에 http://localhost:3000 추가만으로 동작
# → SKILL.md 배포 불필요

wg search "장애 대응"
# → "failover RTO" 팩트가 시맨틱으로 발견
```

### Phase 4 (2주): 피드백 + 어댑터 + 고도화

**목표: 사용할수록 좋아지는 검색. 피드백 기반 어댑터 학습.**

- [ ] search_sessions + search_feedback (query-context 기반)
- [ ] wg feedback click/skip 커맨드
- [ ] DomainAdapter 학습/저장/평가
- [ ] wg adapt status/train/eval 커맨드
- [ ] 어댑터 학습 전/후 recall 비교 벤치마크

**체크포인트:**
피드백 50+ 축적 → 어댑터 학습 → recall +10% 이상.

### Phase 5 (2주): 언어 바인딩 (성능 최적화)

**목표: CLI 포크 오버헤드가 병목인 고빈도 사용에서 in-process 호출.**

Phase 2-3까지 CLI로 충분히 동작하지만,
Hermes bridge 플러그인이나 Node.js 서버처럼 초당 수십 번 호출하는 환경에서는
프로세스 포크 오버헤드가 누적됨. 이 Phase에서 in-process 바인딩 제공.

- [ ] wg-python — PyO3 (Arc\<WikiGraph\>, free-threaded 3.13 호환)
- [ ] wg-napi — napi-rs (Node.js Tier-1, Bun best-effort)
- [ ] wg-nif — Rustler NIF (Elixir/Phoenix)
- [ ] wg-ffi — C-ABI (범용)
- [ ] 각 바인딩 테스트 + 사용 가이드

### Phase 6 (2주): S3 multi-writer + 전체 문서화

**목표: WAL segment + manifest + compaction 프로토콜.**

- [ ] S3 multi-writer 프로토콜 구현
- [ ] 전체 문서화 (README, CONTRIBUTING, CHANGELOG)
- [ ] 축적 시뮬레이션, 실험 결과 정리
- [ ] GitHub Actions CI


---


## 9. 검증 기준

### 9.1 성능 목표

4-tier 벤치마크. CLI one-shot과 embedded session의 특성이 다르므로 분리 측정.

**Tier 1: Cold start (CLI one-shot, semantic off)**
프로세스 시작 → redb open → BM25 rebuild → 쿼리 → 종료.

| 연산 | 1K fact | 10K fact | 100K fact |
|------|---------|----------|-----------|
| startup (redb + BM25 rebuild) | <15ms | <60ms | <400ms |
| traverse (depth=3) | <0.5ms | <1ms | <5ms |
| search (BM25 only) | <1ms | <3ms | <15ms |
| fact_add | <1ms | <1ms | <1ms |
| lint | <10ms | <50ms | <500ms |

**Tier 2: Warm start (Library embedded, semantic off)**
WikiGraph가 이미 열린 상태. 인덱스 빌드 완료.

| 연산 | 1K fact | 10K fact | 100K fact |
|------|---------|----------|-----------|
| search (BM25) | <0.5ms | <2ms | <10ms |
| search_with_traverse | <1ms | <3ms | <12ms |
| traverse (depth=3) | <0.2ms | <0.5ms | <3ms |
| fact_add (+ incremental index) | <0.5ms | <0.5ms | <1ms |

**Tier 3: Cold start with semantic model**
프로세스 시작 → redb open → BM25 rebuild → Model2Vec load → 쿼리 → 종료.
모델 로드가 지배적.

| 연산 | 1K fact | 10K fact | 100K fact |
|------|---------|----------|-----------|
| startup (전체) | <200ms | <300ms | <800ms |
| search (hybrid) | <2ms | <5ms | <20ms |

**Tier 4: Model-warm (Library embedded, semantic on)**
모델과 인덱스 모두 로드 완료 상태.

| 연산 | 1K fact | 10K fact | 100K fact |
|------|---------|----------|-----------|
| search (hybrid) | <1ms | <3ms | <15ms |
| fact_add (+ vector) | <0.5ms | <0.5ms | <1ms |

**메모리 사용량 목표:**

```
1만 fact 기준:
  redb 파일:         ~1 MB   (~100B/fact)
  인메모리 인덱스:    ~5 MB   (~500B/fact, BM25 인버티드 인덱스)
  벡터 스토리지:      ~10 MB  (~1KB/fact, 256차원, semantic feature)
  모델 로드:          ~130 MB (multilingual-128M, 1회, semantic feature)
  총 메모리:          ~150 MB (semantic) / ~6 MB (without semantic)
```

### 9.2 바이너리 크기 목표

- wg-cli (기본 feature): <3 MB (stripped)
- wg-cli (전 feature): <5 MB (stripped)
- 모델 파일 (potion-multilingual-128M): ~128 MB (기본, 별도 다운로드)
- 모델 파일 (potion-base-8M): ~8 MB (영어 전용 대안)

### 9.3 정확도 목표

- BM25 recall@10: >70% (키워드 일치 기반)
- BM25 + semantic recall@10: >85% (의미적 매칭 포함)
- 어댑터 학습 후 recall 개선: >10% (50+ 피드백 기준)
- 퍼지 엔티티 매칭: pg_trgm similarity >0.9 동등

### 9.4 호환성 목표

- Rust edition 2024, MSRV 1.85
- 크로스 컴파일: linux-x64, linux-arm64, darwin-arm64, darwin-x64, windows-x64
- Node.js ≥18 (Tier-1)
- Bun ≥1.0 (Best-effort, Node-API 호환 범위 내)
- Deno (Experimental, Node 호환 모드 또는 C-ABI 대체)
- Python ≥3.9 (free-threaded 3.13 호환)
- Elixir ≥1.15 / OTP ≥26


---


## 10. 벤치마크 및 실험 설계

wg의 실제 효과를 측정하는 5가지 실험.
핵심 질문: **"wg가 있으면 에이전트의 지식 작업 품질이 실제로 올라가는가?"**

### 10.1 실험 1: 검색 품질 (Retrieval Quality)

#### 골든 데이터셋

사람이 만든 정답 셋으로 검색 품질을 정량 측정한다.

```
wiki_benchmark/
├── wiki/                          # 실제 LLM Wiki (마크다운 파일들)
│   ├── SCHEMA.md
│   ├── index.md
│   ├── entities/                  # 30-50개 엔티티 페이지
│   ├── concepts/                  # 20-30개 개념 페이지
│   └── raw/                       # 원본 소스 10-20개
│
├── golden/
│   ├── queries.jsonl              # 질의 50-100개 (유형별 분류)
│   └── relevance.jsonl            # 질의별 관련 fact 매핑 (정답)
│
└── wg_store/
    └── wiki.redb                  # wg로 인덱싱된 상태
```

질의 유형을 의도적으로 분류하여 wg의 각 구성 요소가 어디서 효과를 내는지 분리 측정:

```jsonl
{"id":"q01","query":"Redis 고가용성 전략","type":"exact","expected_entities":["Redis","Sentinel"],"expected_facts":["f001","f002","f003"]}
{"id":"q02","query":"캐시 무효화 방법","type":"semantic","expected_entities":["Redis","Cache"],"expected_facts":["f010","f011"]}
{"id":"q03","query":"시스템 장애 대응","type":"cross_lingual","expected_entities":["Redis","Monitoring"],"expected_facts":["f020","f021","f022"]}
{"id":"q04","query":"배포 파이프라인 설정","type":"multi_hop","expected_entities":["Kubernetes","ArgoCD","GitHub Actions"],"expected_facts":["f030","f031","f032","f033"]}
{"id":"q05","query":"성능 병목 분석","type":"implicit","expected_entities":["PostgreSQL","Redis"],"expected_facts":["f040","f041"]}
```

```
질의 유형          설명                              BM25 예상    wg 전체 예상
──────────       ──────────────────────            ──────────  ──────────
exact            단어가 정확히 일치                   높음         높음 (차이 적음)
semantic         의미는 같지만 단어가 다름             낮음         높음 (시맨틱 효과)
cross_lingual    한영 혼용 매칭 필요                  매우 낮음     높음 (multilingual 효과)
multi_hop        여러 엔티티를 거쳐야 발견             낮음         높음 (구조 탐색 효과)
implicit         직접 언급 안 됨, 관계 추론 필요       매우 낮음     중간 (그래프 효과)
```

#### Ablation 비교 대상

wg의 각 구성 요소가 얼마나 기여하는지 분리 측정:

```
A. grep_baseline:  search_files (grep)              ← 현재 llm-wiki 스킬의 기본 방식
B. bm25_only:      wg search (BM25만, semantic off)
C. struct_bm25:    wg search --traverse-from (구조 탐색 + BM25)
D. full_hybrid:    wg search --traverse-from (구조 + BM25 + semantic)
E. full_adapted:   D + 도메인 어댑터 학습 후
```

#### 측정 지표

```
Recall@K:       상위 K개 결과에 정답 fact가 몇 개 포함 (K = 5, 10, 20)
Precision@K:    상위 K개 중 정답인 비율
MRR:            첫 번째 정답의 역순위 (Mean Reciprocal Rank)
nDCG@10:        정답의 위치를 고려한 랭킹 품질
Entity Recall:  질의의 expected_entities 중 결과에 포함된 비율
Latency:        질의 응답 시간 (p50, p95, p99)
```

#### 예상 결과 형식

```
검색 품질 Ablation 결과:

                     Recall@10   Precision@10   MRR      nDCG@10   Latency(p50)
────────────────     ─────────   ────────────   ────     ───────   ────────────
A. grep baseline     0.35        0.12           0.28     0.30      2ms
B. bm25_only         0.55        0.22           0.45     0.48      3ms
C. struct + bm25     0.72        0.35           0.62     0.65      4ms
D. full hybrid       0.82        0.40           0.71     0.74      5ms
E. full + adapter    0.87        0.43           0.76     0.79      5ms

질의 유형별 Recall@10 breakdown:

                     exact   semantic   cross_lingual   multi_hop   implicit
────────────────     ─────   ────────   ─────────────   ─────────   ────────
A. grep baseline     0.60    0.15       0.05            0.20        0.10
B. bm25_only         0.75    0.35       0.20            0.30        0.25
C. struct + bm25     0.78    0.40       0.25            0.70        0.50
D. full hybrid       0.80    0.72       0.68            0.75        0.55
E. full + adapter    0.82    0.80       0.75            0.78        0.60
```

핵심 가설: exact 질의에서는 차이가 적지만,
semantic/cross_lingual에서 wg의 가치가 극명하게 드러남.
multi_hop에서는 구조 탐색(C)이 결정적.


### 10.2 실험 2: 에이전트 작업 품질 (End-to-End Agent Quality)

검색이 좋아지면 에이전트 산출물도 좋아지는가? LLM-as-Judge로 평가.

#### 태스크

```jsonl
{"id":"t01","task":"Redis Sentinel 관련 코드를 작성하는데, 우리 팀의 HA 전략을 반영해라","type":"code_with_context","domain":"infrastructure"}
{"id":"t02","task":"새로운 팀원에게 우리 캐시 전략을 설명하는 문서를 작성해라","type":"document_generation","domain":"architecture"}
{"id":"t03","task":"auth-service의 JWT 설정을 변경하려는데, 관련된 보안 결정사항을 알려줘","type":"knowledge_retrieval","domain":"security"}
{"id":"t04","task":"최근 배포에서 발생한 p99 레이턴시 이슈를 분석해라","type":"debugging","domain":"operations"}
{"id":"t05","task":"PostgreSQL과 Redis 간의 데이터 일관성 전략을 제안해라","type":"architectural_decision","domain":"data"}
```

#### 실험 조건

```
조건 A (without_wg):
  에이전트에게 태스크 + 위키 디렉토리 접근 제공
  검색 도구: search_files (grep), read_file
  → 현재 llm-wiki 스킬의 기본 동작

조건 B (with_wg):
  에이전트에게 태스크 + wg CLI 접근 제공
  검색 도구: wg search, wg traverse, wg entity list, read_file
  → wg 통합 llm-wiki 스킬

각 조건에서 동일한 LLM으로 태스크 수행. 동일 위키, 동일 시드.
```

#### LLM-as-Judge 평가 차원

```
factual_accuracy (1-5):    위키의 팀 지식을 정확히 반영했는가
completeness (1-5):        관련된 지식을 빠뜨리지 않았는가
consistency (1-5):         위키의 기존 결정과 모순되지 않는가
source_attribution (1-5):  어떤 지식에 기반했는지 추적 가능한가
actionability (1-5):       산출물이 즉시 사용 가능한가
```

Judge 프롬프트에는 태스크, 관련 위키 지식(ground truth), 에이전트 응답을 함께 제공하여
평가자가 위키 내용과 대조하여 채점할 수 있도록 한다.

#### 예상 결과 형식

```
에이전트 작업 품질 (LLM-as-Judge, 1-5 점):

                     factual   complete   consistent   sourced   actionable   평균
────────────────     ───────   ────────   ──────────   ───────   ──────────   ────
without_wg (A)       2.8       2.3        3.1          1.5       3.0          2.54
with_wg (B)          4.2       3.8        4.0          3.5       3.6          3.82

→ wg 사용 시 평균 +1.28점 (50% 향상)

태스크 유형별 격차:

                     code_ctx   doc_gen   knowledge   debugging   arch_decision
────────────────     ────────   ───────   ─────────   ─────────   ─────────────
without_wg           2.8        2.5       2.0         2.5         2.9
with_wg              3.9        4.0       4.5         3.5         3.6
격차                  +1.1       +1.5      +2.5        +1.0        +0.7
```

가설: knowledge_retrieval 태스크에서 격차가 가장 큼 (wg의 직접적 효과).
코드 작성/디버깅처럼 지식 외 스킬이 중요한 태스크에서는 격차가 상대적으로 작음.


### 10.3 실험 3: 지식 축적 효율 (Knowledge Accumulation)

시간에 따른 위키 성장과 재활용을 측정. wg가 "사용할수록 좋아지는가?"를 본다.

#### 시뮬레이션 설계

```
10일간의 에이전트 세션 시뮬레이션:

Day 1-2:   소스 5개 ingest → 초기 위키 구축
Day 3-4:   코딩 세션 10회 → fact 자동 추출
Day 5-6:   소스 5개 추가 ingest → 위키 확장
Day 7-8:   코딩 세션 10회 → 기존 지식 활용 + 새 fact
Day 9-10:  크로스 도메인 질의 → 지식 재활용 측정

각 세션에서 에이전트가 실제로 위키를 읽고/쓰는 과정을 기록.
```

#### 측정 지표

```
축적 지표:
  facts_total:         총 fact 수
  facts_per_session:   세션당 추출된 fact 수
  entities_total:      총 엔티티 수
  relations_total:     총 관계 수
  graph_density:       relation_count / (entity_count × (entity_count - 1))

재활용 지표:
  reuse_rate:          검색에서 기존 fact가 반환된 비율
  cross_session_hits:  이전 세션의 fact가 현재 세션에서 활용된 횟수
  knowledge_decay:     시간 경과에 따른 fact 접근 빈도 변화

건강도 지표 (wg lint 기반):
  orphan_rate:         고립 엔티티 비율
  stale_rate:          N일 이상 미접근 fact 비율
  contradiction_count: 모순 탐지 횟수
```

#### 예상 결과

```
10일 시뮬레이션 결과:

                     without_wg      with_wg       차이
────────────────     ──────────      ───────       ────
facts_total          ~50 (수동)      ~200 (자동)    4x
unique entities      ~20             ~60            3x
relations            ~5 (wikilink)   ~80 (typed)    16x
cross_session_hits   ~3              ~25            8x
orphan_rate          40%             8%             5x 감소
contradiction_found  0               3              ∞

핵심: wg는 "수동으로는 하지 않았을 지식 구조화"를 자동으로 수행.
typed relation(uses, depends_on, decided_by)이 핵심 차별점.
contradiction 탐지는 wg 없이는 아예 불가능.
```


### 10.4 실험 4: 성능 벤치마크 (Performance)

규모별 레이턴시와 리소스 사용량을 criterion으로 측정.

#### 4-tier 벤치마크 매트릭스

```
Tier 1: Cold start (CLI one-shot, semantic off)
  startup + BM25 rebuild → 쿼리 → 종료

Tier 2: Warm start (Library embedded, semantic off)
  WikiGraph 이미 열림 + 인덱스 빌드 완료

Tier 3: Cold start with semantic model
  startup + BM25 rebuild + Model2Vec load → 쿼리 → 종료

Tier 4: Model-warm (Library embedded, semantic on)
  모델과 인덱스 모두 로드 완료

각 Tier에서:
  규모: [100, 1_000, 10_000, 50_000, 100_000] facts
  연산: startup, traverse_d3, search_bm25, search_hybrid, fact_add, lint, export
```

#### 레이턴시 목표 (p95)

```
연산                100 fact   1K fact   10K fact   50K fact   100K fact
──────────────     ────────   ───────   ────────   ────────   ─────────
startup            <5ms       <10ms     <30ms      <100ms     <200ms
traverse (d=3)     <0.2ms     <0.5ms    <1ms       <3ms       <5ms
search (bm25)      <0.5ms     <1ms      <3ms       <10ms      <15ms
search (hybrid)    <1ms       <2ms      <5ms       <15ms      <20ms
fact_add           <0.5ms     <0.5ms    <1ms       <1ms       <1ms
lint               <5ms       <10ms     <50ms      <200ms     <500ms
export (jsonl)     <2ms       <5ms      <30ms      <100ms     <200ms
```

#### 메모리 사용량 목표

```
1만 fact 기준:
  redb 파일:         ~1 MB   (~100B/fact)
  인메모리 인덱스:    ~5 MB   (~500B/fact, BM25 인버티드 인덱스)
  벡터 스토리지:      ~10 MB  (~1KB/fact, 256차원, semantic feature)
  모델 로드:          ~130 MB (multilingual-128M, 1회, semantic feature)
  총 메모리:          ~150 MB (semantic) / ~6 MB (without semantic)
```

#### 외부 비교 (참고, 정합성 확인용)

```
동일 데이터로 비교하되, 목적은 "wg가 합리적 범위인가" 확인:

  SQLite FTS5 (rusqlite):  동일 fact를 SQLite에 넣고 FTS5 검색
  tantivy:                 동일 fact를 tantivy에 넣고 검색

  주의: wg의 차별화는 속도가 아니라 구조 탐색 + 시맨틱 결합.
  외부 비교는 "더 빠른가"가 아니라 "비정상적으로 느리지 않은가"를 확인.
```


### 10.5 실험 5: 모델 비교 (Model Comparison)

potion-multilingual-128M(기본) vs potion-base-8M(영어 전용)을 한영 혼용 위키에서 비교.

#### 테스트셋

```
한영 혼용 질의 20개:

유형 A (한→한):  "장애 대응"     → "장애 복구 절차"
유형 B (한→영):  "장애 대응"     → "failover RTO 15s"
유형 C (영→한):  "caching"      → "캐시 무효화 정책"
유형 D (영→영):  "Redis HA"     → "Sentinel configuration"
```

#### 예상 결과

```
                     multi-128M   base-8M   차이
────────────────     ──────────   ───────   ────
A. 한→한 Recall@10    0.75         0.40      +0.35
B. 한→영 Recall@10    0.65         0.10      +0.55
C. 영→한 Recall@10    0.60         0.08      +0.52
D. 영→영 Recall@10    0.70         0.72      -0.02
평균 Recall@10        0.68         0.33      +0.35
속도 (ms/query)       0.07         0.04      +0.03

→ 한영 혼용에서 multilingual이 2배 높은 recall.
  영어 전용에서는 거의 동일 (base-8M이 근소하게 빠름).
  모델 전환 가이드: 영어 전용 위키 → base-8M, 그 외 → multilingual (기본).
```


### 10.6 벤치마크 실행 계획

각 실험을 Phase별로 배치:

```
Phase 1 (wg-core 기본):
  ✓ benchmarks/performance.rs 셋업 (criterion)
  ✓ 규모별 startup/traverse/fact_add 벤치마크
  ✓ 골든 데이터셋 위키 구축 시작 (마크다운 30-50 페이지)

Phase 2 (검색 + 린트):
  ✓ 골든 데이터셋 완성 (queries.jsonl 50-100개 + relevance.jsonl)
  ✓ 실험 1: grep vs bm25 vs struct+bm25 ablation
  ✓ 규모별 search/lint 벤치마크

Phase 3 (시맨틱):
  ✓ 실험 5: multilingual-128M vs base-8M 모델 비교
  ✓ 실험 1 확장: full hybrid + adapter ablation
  ✓ 어댑터 학습 전/후 recall 비교

Phase 4-5 (바인딩):
  ✓ 실험 2: end-to-end 에이전트 품질 테스트 (Python에서 실행)
  ✓ LLM-as-Judge 평가 파이프라인 구축
  ✓ 조건 A(without_wg) vs B(with_wg) 비교

Phase 7 (문서화):
  ✓ 실험 3: 10일 지식 축적 시뮬레이션
  ✓ 전체 결과 정리, README에 벤치마크 결과 포함
  ✓ 실험 재현 스크립트 공개
```

### 10.7 벤치마크 인프라

```
benchmarks/
├── criterion/                    # Rust 성능 벤치마크
│   ├── performance.rs            # 규모별 레이턴시
│   └── model_comparison.rs       # 모델별 속도 비교
│
├── retrieval/                    # 검색 품질 실험
│   ├── golden/                   # 골든 데이터셋
│   │   ├── wiki/                 # 테스트 위키 (마크다운)
│   │   ├── queries.jsonl         # 질의 셋
│   │   └── relevance.jsonl       # 정답 매핑
│   ├── ablation.rs               # ablation 실행기
│   └── metrics.rs                # Recall, Precision, MRR, nDCG 계산
│
├── agent_quality/                # 에이전트 품질 실험
│   ├── tasks.jsonl               # 태스크 정의
│   ├── run_agent.py              # 에이전트 실행 (with/without wg)
│   ├── judge.py                  # LLM-as-Judge 평가
│   └── report.py                 # 결과 집계 + 시각화
│
├── accumulation/                 # 지식 축적 시뮬레이션
│   ├── simulate.py               # 10일 시뮬레이션 실행
│   ├── sources/                  # 시뮬레이션용 소스 자료
│   └── analyze.py                # 축적/재활용 지표 분석
│
└── results/                      # 결과 아카이브 (git tracked)
    ├── retrieval_ablation.json
    ├── agent_quality.json
    ├── accumulation.json
    ├── performance.json
    └── model_comparison.json
```


---


## 11. 리스크와 대응

| 리스크 | 영향 | 대응 |
|--------|------|------|
| redb 파일 포맷 변경 | 데이터 손실 | redb 2.x는 안정 선언 + meta.schema_version으로 앱 스키마 마이그레이션 |
| redb 앱 스키마 변경 시 TypeDefinitionChanged | 기존 DB 열기 실패 | schema_version 체크 + 마이그레이션 코드 + JSONL fallback |
| bm25 크레이트 한국어 토크나이저 품질 | 검색 정확도 하락 | 커스텀 토크나이저 fallback 구현 |
| BM25 avgdl 드리프트 | incremental 변경 누적 시 점수 부정확 | dirty flag + 100건 또는 10% 드리프트 시 full rebuild |
| Model2Vec 다국어 모델 크기 (128MB) | 첫 다운로드 시간 | 자동 다운로드 + 캐시, config set으로 경량 모델 전환 |
| cold start + model load 레이턴시 | CLI 응답 느림 | 4-tier 벤치마크로 분리 측정, embedded 모드 권장 |
| napi-rs Bun 호환성 | 일부 API 미동작 | Node.js Tier-1, Bun best-effort, C-ABI fallback |
| S3 동시 쓰기 | 데이터 충돌 | WAL segment + manifest + compaction lease 프로토콜 |
| 바이너리 크기 초과 | 배포 부담 | opt-level=z, feature gate 분리 |
| ingest 파싱 엣지 케이스 | 위키 구조 미인식 | SCHEMA.md 규칙 준수 전제, 파싱 실패 시 경고 + skip |
