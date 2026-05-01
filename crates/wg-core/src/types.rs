//! Shared types for WikiGraph.
//!
//! Core types:
//! - [`EntityId`] — ULID-based canonical entity identifier
//! - [`FactId`] — ULID-based fact identifier
//! - [`EntityRecord`] — persisted entity data
//! - [`FactRecord`] — persisted fact data
//! - [`RelationRecord`] — persisted relation data

use serde::{Deserialize, Serialize};
use std::str::FromStr;
use ulid::Ulid;

/// Canonical entity identifier (ULID-based).
///
/// All entities are identified by EntityId, not by name.
/// Name/aliases are secondary indexes only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityId(pub Ulid);

impl EntityId {
    /// Create a new random EntityId.
    pub fn new() -> Self {
        Self(Ulid::new())
    }

    /// Parse from string representation.
    pub fn parse(s: &str) -> Option<Self> {
        Ulid::from_str(s).ok().map(Self)
    }

    /// Return raw bytes as a fixed-size array.
    pub fn as_bytes(&self) -> [u8; 16] {
        self.0.to_bytes()
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Canonical fact identifier (ULID-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FactId(pub Ulid);

impl FactId {
    /// Create a new random FactId.
    pub fn new() -> Self {
        Self(Ulid::new())
    }

    /// Parse from string representation.
    pub fn parse(s: &str) -> Option<Self> {
        Ulid::from_str(s).ok().map(Self)
    }

    /// Return raw bytes as a fixed-size array.
    pub fn as_bytes(&self) -> [u8; 16] {
        self.0.to_bytes()
    }
}

impl Default for FactId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Entity type classification.
///
/// `Custom(String)` lets each wiki define domain-specific types — e.g.
/// `service`, `rfc`, `paper`, `incident` — without recompiling. Strings are
/// normalized to lowercase. All variants serialize as flat lowercase strings
/// (built-in or custom), so JSON round-trips are uniform.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EntityType {
    /// Technology (tools, infrastructure, software)
    Technology,
    /// Concept (patterns, principles, architectural styles)
    Concept,
    /// Comparison (feature matrix, pros/cons)
    Comparison,
    /// Query (question, investigation)
    Query,
    /// Person
    Person,
    /// Team
    Team,
    /// Unknown/unclassified
    #[default]
    Unknown,
    /// Custom user-defined type — e.g. `service`, `rfc`. Always lowercase.
    Custom(String),
}

impl EntityType {
    /// Parse a string into an EntityType, recognizing built-in variants and
    /// falling back to `Custom(s)` for anything else.
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "technology" | "tech" => EntityType::Technology,
            "concept" => EntityType::Concept,
            "comparison" | "compare" => EntityType::Comparison,
            "query" | "question" => EntityType::Query,
            "person" => EntityType::Person,
            "team" => EntityType::Team,
            "" | "unknown" => EntityType::Unknown,
            other => EntityType::Custom(other.to_string()),
        }
    }
}

impl serde::Serialize for EntityType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for EntityType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // Accept two forms for backward compatibility:
        //   1. "service"                 — flat lowercase string (current).
        //   2. {"custom": "service"}     — legacy tagged form from earlier
        //                                  versions of this enum.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Flat(String),
            Tagged { custom: String },
        }
        match Either::deserialize(d)? {
            Either::Flat(s) => Ok(EntityType::parse(&s)),
            Either::Tagged { custom } => Ok(EntityType::parse(&custom)),
        }
    }
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntityType::Technology => write!(f, "technology"),
            EntityType::Concept => write!(f, "concept"),
            EntityType::Comparison => write!(f, "comparison"),
            EntityType::Query => write!(f, "query"),
            EntityType::Person => write!(f, "person"),
            EntityType::Team => write!(f, "team"),
            EntityType::Unknown => write!(f, "unknown"),
            EntityType::Custom(s) => write!(f, "{}", s),
        }
    }
}

/// Entity record stored in redb.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRecord {
    /// Canonical ID (ULID).
    pub id: EntityId,
    /// Display name (e.g., "Redis").
    pub name: String,
    /// Lowercase normalized name for search.
    pub name_lower: String,
    /// Entity type classification.
    pub entity_type: EntityType,
    /// Alternative names/aliases.
    pub aliases: Vec<String>,
    /// Tags for categorization.
    pub tags: Vec<String>,
    /// Source markdown page path (e.g., "entities/redis.md").
    pub source_page: Option<String>,
    /// Creation timestamp (epoch ms).
    pub created_at: u64,
    /// Last update timestamp (epoch ms).
    pub updated_at: u64,
    /// "Compiled truth" — a synthesized prose summary of what we currently
    /// believe about this entity. Distinct from the fact list (the evidence
    /// trail). Set explicitly via `wg entity describe` or via bindings;
    /// not auto-generated. `None` until a user provides one.
    #[serde(default)]
    pub summary: Option<String>,
    /// When the summary was last set (epoch ms). `None` while `summary` is `None`.
    #[serde(default)]
    pub summary_updated_at: Option<u64>,
}

impl EntityRecord {
    /// Create a new entity record.
    pub fn new(name: String, entity_type: EntityType) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let name_lower = name.to_lowercase();
        Self {
            id: EntityId::new(),
            name,
            name_lower,
            entity_type,
            aliases: Vec::new(),
            tags: Vec::new(),
            source_page: None,
            created_at: now,
            updated_at: now,
            summary: None,
            summary_updated_at: None,
        }
    }

    /// Update the record with new data.
    pub fn update(&mut self, input: EntityUpdate) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        if let Some(name) = input.name {
            self.name = name.clone();
            self.name_lower = name.to_lowercase();
        }
        if let Some(entity_type) = input.entity_type {
            self.entity_type = entity_type;
        }
        if let Some(aliases) = input.aliases {
            self.aliases = aliases;
        }
        if let Some(tags) = input.tags {
            self.tags = tags;
        }
        if let Some(source_page) = input.source_page {
            self.source_page = Some(source_page);
        }
        if let Some(summary) = input.summary {
            // Empty string clears the summary; non-empty sets it.
            if summary.is_empty() {
                self.summary = None;
                self.summary_updated_at = None;
            } else {
                self.summary = Some(summary);
                self.summary_updated_at = Some(now);
            }
        }
        self.updated_at = now;
    }
}

/// Input for creating a new entity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityInput {
    pub name: String,
    #[serde(default)]
    pub entity_type: Option<EntityType>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub source_page: Option<String>,
}

/// Input for updating an entity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityUpdate {
    pub name: Option<String>,
    pub entity_type: Option<EntityType>,
    pub aliases: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub source_page: Option<String>,
    /// "Compiled truth" prose summary. `Some("")` clears it, `Some(text)` sets
    /// it (and bumps `summary_updated_at`), `None` leaves it unchanged.
    #[serde(default)]
    pub summary: Option<String>,
}

/// Summary of an entity (for list operations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySummary {
    pub id: EntityId,
    pub name: String,
    pub entity_type: EntityType,
    pub fact_count: u32,
    pub tags: Vec<String>,
}

/// Relation type (user-defined).
///
/// Common types: "uses", "depends_on", "decided_by", "implements", "manages".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationType(pub String);

impl RelationType {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl std::fmt::Display for RelationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for RelationType {
    fn default() -> Self {
        Self("related".to_string())
    }
}

/// Relation record stored in redb.
///
/// Key: "{source_id}\0{rel_type}\0{target_id}"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationRecord {
    /// Source entity ID.
    pub source_id: EntityId,
    /// Target entity ID.
    pub target_id: EntityId,
    /// Relation type (e.g., "uses", "depends_on").
    pub relation_type: RelationType,
    /// Weight/confidence (0.0-1.0).
    pub weight: f32,
    /// Evidence/source paths.
    pub evidence: Vec<String>,
    /// Creation timestamp (epoch ms).
    pub created_at: u64,
}

/// Input for creating a relation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationInput {
    pub source: String, // entity name (resolved to ID)
    pub target: String, // entity name (resolved to ID)
    pub relation_type: RelationType,
    #[serde(default)]
    pub weight: Option<f32>,
    #[serde(default)]
    pub evidence: Option<Vec<String>>,
}

/// Fact type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum FactType {
    Decision,
    Pattern,
    Convention,
    Claim,
    Note,
    Question,
    #[default]
    Unknown,
}

impl std::fmt::Display for FactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FactType::Decision => write!(f, "decision"),
            FactType::Pattern => write!(f, "pattern"),
            FactType::Convention => write!(f, "convention"),
            FactType::Claim => write!(f, "claim"),
            FactType::Note => write!(f, "note"),
            FactType::Question => write!(f, "question"),
            FactType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Fact record stored in redb.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactRecord {
    /// Canonical ID (ULID).
    pub id: FactId,
    /// The fact content.
    pub content: String,
    /// Fact type.
    pub fact_type: FactType,
    /// Referenced entity IDs (not names).
    pub entity_ids: Vec<EntityId>,
    /// Tags.
    pub tags: Vec<String>,
    /// Source page path (e.g., "entities/redis.md#ha").
    pub source: Option<String>,
    /// Source confidence (0-1): manual=1.0, auto-extract=0.5, LLM=0.3.
    pub source_confidence: f32,
    /// Relevance score (0-1): updated via feedback.
    pub relevance_score: f32,
    /// Creation timestamp (epoch ms): when the fact was inserted into the store.
    pub created_at: u64,
    /// Last update timestamp (epoch ms).
    pub updated_at: u64,
    /// Observed timestamp (epoch ms): when the fact was actually observed/decided
    /// in the real world. Distinct from `created_at` (DB insertion time).
    /// Sourced from frontmatter `date`/`decided_at`/`observed_at` during ingest,
    /// or set explicitly via `wg fact add --observed-at`.
    #[serde(default)]
    pub observed_at: Option<u64>,
    /// When this fact was superseded (epoch ms). `None` means still current.
    /// Set via `wg fact supersede <old> <new>`. Filter with `--current` on
    /// list / search / query to hide superseded facts.
    #[serde(default)]
    pub superseded_at: Option<u64>,
    /// The fact that replaced this one (if any). Forms a chain so callers can
    /// follow `latest_of(fact_id)` through history.
    #[serde(default)]
    pub superseded_by: Option<FactId>,
    /// Number of times accessed.
    pub access_count: u32,
    /// Last access timestamp (epoch ms).
    pub last_accessed_at: u64,
    /// Memory-hierarchy flag: when `true`, the fact is part of the
    /// "always loaded" tier — agents pull these into context at
    /// session start via `wg_pinned_context` regardless of recency or
    /// search relevance. Use sparingly: pinned facts compete with the
    /// agent's working-memory budget. Defaulting to `false` means
    /// existing wikis behave unchanged after migration.
    #[serde(default)]
    pub pinned: bool,
}

impl FactRecord {
    /// Create a new fact record.
    pub fn new(content: String, fact_type: FactType, entity_ids: Vec<EntityId>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        Self {
            id: FactId::new(),
            content,
            fact_type,
            entity_ids,
            tags: Vec::new(),
            source: None,
            source_confidence: 0.5,
            relevance_score: 0.5,
            created_at: now,
            updated_at: now,
            observed_at: None,
            superseded_at: None,
            superseded_by: None,
            access_count: 0,
            last_accessed_at: now,
            pinned: false,
        }
    }

    /// Update the fact with new data.
    pub fn update(&mut self, input: FactUpdate) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        if let Some(content) = input.content {
            self.content = content;
        }
        if let Some(fact_type) = input.fact_type {
            self.fact_type = fact_type;
        }
        if let Some(tags) = input.tags {
            self.tags = tags;
        }
        if let Some(source) = input.source {
            self.source = Some(source);
        }
        if let Some(observed_at) = input.observed_at {
            self.observed_at = Some(observed_at);
        }
        if let Some(ts) = input.superseded_at {
            self.superseded_at = if ts == 0 { None } else { Some(ts) };
        }
        if let Some(by) = input.superseded_by {
            self.superseded_by = Some(by);
        }
        if let Some(pinned) = input.pinned {
            self.pinned = pinned;
        }
        self.updated_at = now;
    }

    /// Record an access.
    pub fn record_access(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.access_count += 1;
        self.last_accessed_at = now;
    }
}

/// Input for creating a fact.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FactInput {
    pub content: String,
    #[serde(default)]
    pub fact_type: Option<FactType>,
    #[serde(default)]
    pub entity_ids: Option<Vec<EntityId>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub source_confidence: Option<f32>,
    /// When the fact was actually observed/decided (epoch ms).
    /// Distinct from creation time. Optional.
    #[serde(default)]
    pub observed_at: Option<u64>,
}

/// Input for updating a fact.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FactUpdate {
    pub content: Option<String>,
    pub fact_type: Option<FactType>,
    pub tags: Option<Vec<String>>,
    pub source: Option<String>,
    #[serde(default)]
    pub observed_at: Option<u64>,
    /// If set, marks the fact as superseded at this epoch ms. Use `Some(0)` to
    /// reset (un-supersede); any non-zero value sets the timestamp.
    #[serde(default)]
    pub superseded_at: Option<u64>,
    #[serde(default)]
    pub superseded_by: Option<FactId>,
    /// Toggle the pinned-tier flag. `Some(true)` adds the fact to
    /// `wg_pinned_context`; `Some(false)` removes it. `None` leaves
    /// the existing value untouched (default for routine edits).
    #[serde(default)]
    pub pinned: Option<bool>,
}

/// Options for listing entities.
#[derive(Debug, Clone, Default)]
pub struct ListOpts {
    pub entity_type: Option<EntityType>,
    pub min_facts: Option<u32>,
    pub sort_by: EntitySort,
    pub limit: Option<usize>,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum EntitySort {
    Name,
    #[default]
    UpdatedAt,
    FactCount,
}

/// Options for traversing the graph.
#[derive(Debug, Clone, Default)]
pub struct TraverseOpts {
    /// Maximum depth (default: 2).
    pub depth: u32,
    /// Relation types to follow.
    pub relation_types: Option<Vec<RelationType>>,
    /// Direction (forward/reverse/both).
    pub direction: TraverseDirection,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum TraverseDirection {
    #[default]
    Forward,
    Reverse,
    Both,
}

/// Options for `WikiGraph::auto_relate` — discovers entity-to-entity
/// `related` edges from semantic similarity between facts.
#[derive(Debug, Clone)]
pub struct AutoRelateOpts {
    /// Minimum hybrid-search score to count two facts as similar.
    /// Default `0.0` (off) — top_k is the primary cutoff. Score scale
    /// depends on the active retrieval path: ~0.01-0.04 for RRF
    /// fusion (HNSW + BM25), 1-5 for BM25-only fallback. Tune per
    /// wiki if top_k alone is too noisy.
    pub threshold: f32,
    /// Top-K similar facts to inspect per source fact (excluding the
    /// fact itself). Default 3.
    pub top_k: usize,
    /// If true, evaluate pairs but don't write edges.
    pub dry_run: bool,
}

impl Default for AutoRelateOpts {
    fn default() -> Self {
        Self {
            threshold: 0.0,
            top_k: 3,
            dry_run: false,
        }
    }
}

/// Statistics returned by `auto_relate`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoRelateStats {
    pub facts_processed: usize,
    pub pairs_evaluated: usize,
    pub edges_created: usize,
    pub edges_skipped_same_entity: usize,
    pub edges_skipped_existing: usize,
}

/// Result of a graph traversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraverseResult {
    /// Entities found during traversal.
    pub entities: Vec<EntitySummary>,
    /// Relations found.
    pub relations: Vec<RelationRecord>,
    /// Total nodes visited.
    pub visited_count: usize,
}

/// A single step in a path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathStep {
    pub from: EntityId,
    pub relation_type: RelationType,
    pub to: EntityId,
}

/// Search result item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub fact_id: FactId,
    pub content: String,
    pub fact_type: FactType,
    pub entity_names: Vec<String>,
    pub source: Option<String>,
    pub score: f32,
    pub rank: usize,
    /// When the fact was inserted into the store (epoch ms).
    #[serde(default)]
    pub created_at: u64,
    /// When the fact was actually observed/decided in the real world (epoch ms).
    /// Sourced from frontmatter or `--observed-at`. Optional.
    #[serde(default)]
    pub observed_at: Option<u64>,
    /// The search session this result belongs to (if tracked).
    #[cfg(feature = "semantic")]
    pub session_id: Option<String>,
}

/// Options for search.
#[derive(Debug, Clone, Default)]
pub struct SearchOpts {
    pub limit: Option<usize>,
    pub min_confidence: Option<f32>,
    pub entity_filter: Option<Vec<EntityId>>,
    pub bm25_weight: f32,
    pub semantic_weight: f32,
    /// Lower-bound timestamp (inclusive, epoch ms). Compares against
    /// `observed_at` if present, else `created_at`.
    pub since: Option<u64>,
    /// Upper-bound timestamp (inclusive, epoch ms). Same source as `since`.
    pub until: Option<u64>,
    /// Search session to attribute results to (enables feedback tracking).
    #[cfg(feature = "semantic")]
    pub session_id: Option<String>,
    /// If `true`, exclude superseded facts.
    pub current_only: bool,
    /// "As of" timestamp (epoch ms). When set, the fact is included
    /// only if it (a) existed at that point — `created_at <= as_of` —
    /// and (b) was *still current* then — `superseded_at` is `None`
    /// or `> as_of`. Lets the caller ask "what did we believe was
    /// true on YYYY-MM-DD?" without manually walking the supersede
    /// chain.
    pub as_of: Option<u64>,
    /// Skip the embedding-model load entirely and run pure BM25.
    /// `WikiGraph::hybrid_search` honours this flag and short-circuits
    /// to `WikiGraph::search` before touching `embed_provider`. Cuts
    /// cold-start latency on a fresh CLI spawn from ~1s to ~70ms at
    /// the cost of semantic recall (BM25 token matching only).
    /// `false` (default) preserves the previous hybrid behaviour.
    pub bm25_only: bool,
}

/// Options for listing facts.
#[derive(Debug, Clone, Default)]
pub struct FactListOpts {
    pub fact_type: Option<FactType>,
    pub entity_id: Option<EntityId>,
    pub min_confidence: Option<f32>,
    pub limit: Option<usize>,
    pub offset: usize,
    /// Lower-bound timestamp (inclusive, epoch ms). Compares against
    /// `observed_at` if present, else `created_at`.
    pub since: Option<u64>,
    /// Upper-bound timestamp (inclusive, epoch ms). Same source as `since`.
    pub until: Option<u64>,
    /// If `true`, exclude superseded facts (those with `superseded_at` set).
    pub current_only: bool,
    /// "As of" timestamp — see `SearchOpts::as_of`.
    pub as_of: Option<u64>,
}

/// Retrieval strategy for `WikiGraph::query`. Inspired by LightRAG.
///
/// - `Naive`: hybrid search only — no entity resolution, no traverse, no recent.
///   Fastest; equivalent to calling `hybrid_search` directly. Use when you just
///   want top-K facts.
/// - `Local`: entity-centric — resolve the topic to an entity, return
///   immediate neighbors + facts. **Skips** the global hybrid search. Use
///   when you know the topic is an entity name and want its surroundings.
/// - `Hybrid` (default): search + entity + traverse(depth) + recent. The
///   "best of both" mode that matches the original `wg query` behavior.
/// - `Global`: broad scan — search + entity + traverse(deeper) + every fact
///   on the resolved entity (no recency cap). Use for "what does the wiki
///   know about X overall?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryMode {
    Naive,
    Local,
    #[default]
    Hybrid,
    Global,
}

impl QueryMode {
    /// Parse a string into a QueryMode (case-insensitive). Unknown → Hybrid.
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "naive" => QueryMode::Naive,
            "local" => QueryMode::Local,
            "global" => QueryMode::Global,
            _ => QueryMode::Hybrid,
        }
    }
}

impl std::fmt::Display for QueryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryMode::Naive => write!(f, "naive"),
            QueryMode::Local => write!(f, "local"),
            QueryMode::Hybrid => write!(f, "hybrid"),
            QueryMode::Global => write!(f, "global"),
        }
    }
}

impl serde::Serialize for QueryMode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for QueryMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(QueryMode::parse(&String::deserialize(d)?))
    }
}

/// Options for `WikiGraph::query` — a unified context fetch (search + traverse + recent facts).
#[derive(Debug, Clone)]
pub struct QueryOpts {
    /// Max search hits to include (default 10).
    pub search_limit: usize,
    /// Traverse depth when the topic resolves to an entity (default 2).
    pub depth: u32,
    /// Max recent facts to include (default 10).
    pub recent_limit: usize,
    /// Lower-bound timestamp for search/recent (epoch ms). `None` = no bound.
    pub since: Option<u64>,
    /// If `true`, exclude superseded facts from search and recent_facts.
    pub current_only: bool,
    /// Retrieval strategy. `Hybrid` by default.
    pub mode: QueryMode,
    /// Skip the embedding-model load. Same semantics as
    /// `SearchOpts::bm25_only` — flows down into the hybrid_search
    /// step. `false` (default) preserves the previous behaviour.
    pub bm25_only: bool,
}

impl Default for QueryOpts {
    fn default() -> Self {
        Self {
            search_limit: 10,
            depth: 2,
            recent_limit: 10,
            since: None,
            current_only: false,
            mode: QueryMode::default(),
            bm25_only: false,
        }
    }
}

/// Result of a unified `WikiGraph::query` call.
///
/// Composes hybrid search + entity resolution + graph traversal + recent
/// facts in a single pass. Designed for LLM agents that need a coherent
/// context dossier without making 3-4 round trips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// The topic that was queried.
    pub topic: String,
    /// The resolved entity, if `topic` matched an entity name or alias.
    pub entity: Option<EntityRecord>,
    /// Top hybrid-search hits (BM25 + semantic).
    pub search: Vec<SearchResult>,
    /// Related entities reachable from the resolved entity (empty if `entity` is None).
    pub related: Vec<EntitySummary>,
    /// Recent facts attached to the resolved entity (empty if `entity` is None).
    pub recent_facts: Vec<FactRecord>,
}

/// Store statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoreStats {
    pub entity_count: u64,
    pub fact_count: u64,
    pub relation_count: u64,
    pub total_size_bytes: u64,
    pub last_ingest_at: Option<u64>,
}

/// Options for `WikiGraph::overview` — first-impression snapshot of a wiki.
#[derive(Debug, Clone)]
pub struct OverviewOpts {
    /// Top-N entities to surface globally and per entity_type bucket.
    pub top_n_entities: usize,
    /// Window (in days) for the `recent_fact_count` field.
    pub recent_days: u64,
}

impl Default for OverviewOpts {
    fn default() -> Self {
        Self {
            top_n_entities: 10,
            recent_days: 7,
        }
    }
}

/// Per-entity-type bucket for `OverviewResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityTypeBucket {
    pub entity_type: EntityType,
    pub count: u64,
    /// Top entities of this type by fact_count (capped at `top_n_entities`).
    pub top_examples: Vec<EntitySummary>,
}

/// Per-fact-type bucket for `OverviewResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactTypeBucket {
    pub fact_type: FactType,
    pub count: u64,
}

/// First-impression snapshot of a wiki — what an agent sees on
/// `wg_overview`. Designed to answer "what's in this wiki?" in one
/// MCP round-trip without per-entity / per-fact follow-ups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewResult {
    pub stats: StoreStats,
    /// Entities grouped by entity_type, sorted by bucket size.
    pub entity_types: Vec<EntityTypeBucket>,
    /// Fact-type distribution, sorted by count.
    pub fact_types: Vec<FactTypeBucket>,
    /// Globally top entities by fact_count (capped at `top_n_entities`).
    pub top_entities: Vec<EntitySummary>,
    /// Entities with zero attached facts.
    pub orphan_entity_count: u64,
    /// Facts created within the last `recent_days` window.
    pub recent_fact_count: u64,
    /// Facts that are still current (not superseded).
    pub current_fact_count: u64,
    /// Facts that are pinned ("always-loaded" tier).
    pub pinned_fact_count: u64,
}

/// Export scope.
#[derive(Debug, Clone, Copy, Default)]
pub enum ExportScope {
    #[default]
    All,
    Entities,
    Relations,
    Facts,
}

/// Export statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportStats {
    pub entities_exported: u64,
    pub relations_exported: u64,
    pub facts_exported: u64,
}

/// Import statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportStats {
    pub entities_imported: u64,
    pub relations_imported: u64,
    pub facts_imported: u64,
    pub errors: u64,
}

/// Lint issue severity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LintSeverity {
    Error,
    Warning,
    Info,
}

impl std::fmt::Display for LintSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintSeverity::Error => write!(f, "error"),
            LintSeverity::Warning => write!(f, "warning"),
            LintSeverity::Info => write!(f, "info"),
        }
    }
}

/// A single lint issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintIssue {
    pub severity: LintSeverity,
    pub code: String,
    pub message: String,
    pub entity_id: Option<EntityId>,
    pub fact_id: Option<FactId>,
}

/// Lint report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LintReport {
    pub issues: Vec<LintIssue>,
    pub entity_count: u64,
    pub fact_count: u64,
    pub relation_count: u64,
}

#[cfg(feature = "semantic")]
mod semantic_types {
    /// Vector record format for semantic search.
    ///
    /// Binary format:
    /// - `bytes[0]`: version (u8, current: 1)
    /// - `bytes[1..3]`: dimensions (u16 LE, current: 256)
    /// - `bytes[3]`: dtype (u8, 0=f32, 1=f16, 2=i8)
    /// - `bytes[4..]`: little-endian vector data
    #[derive(Debug, Clone)]
    pub struct VectorRecord {
        pub version: u8,
        pub dimensions: u16,
        pub dtype: VectorDType,
        pub data: Vec<u8>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum VectorDType {
        F32,
        F16,
        I8,
    }

    impl VectorRecord {
        pub const CURRENT_VERSION: u8 = 1;
        pub const CURRENT_DIMENSIONS: u16 = 256;

        /// Encode to binary format.
        pub fn encode(&self) -> Vec<u8> {
            let mut bytes = Vec::with_capacity(4 + self.data.len());
            bytes.push(self.version);
            bytes.extend_from_slice(&self.dimensions.to_le_bytes());
            bytes.push(self.dtype as u8);
            bytes.extend_from_slice(&self.data);
            bytes
        }

        /// Decode from binary format.
        pub fn decode(bytes: &[u8]) -> Option<Self> {
            if bytes.len() < 4 {
                return None;
            }
            let version = bytes[0];
            if version != Self::CURRENT_VERSION {
                return None;
            }
            let dimensions = u16::from_le_bytes([bytes[1], bytes[2]]);
            let dtype = match bytes[3] {
                0 => VectorDType::F32,
                1 => VectorDType::F16,
                2 => VectorDType::I8,
                _ => return None,
            };
            let data = bytes[4..].to_vec();
            Some(Self {
                version,
                dimensions,
                dtype,
                data,
            })
        }
    }
}

/// Search session — records a single search query for feedback tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSession {
    pub id: String,
    pub query: String,
    pub timestamp: u64,
    pub result_count: usize,
}

/// Search feedback — records user feedback on a search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFeedback {
    pub session_id: String,
    pub fact_id: FactId,
    pub helpful: bool,
    pub timestamp: u64,
}

/// Domain adapter evaluation report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptEvalReport {
    pub total_feedback: usize,
    pub helpful_count: usize,
    pub skipped_count: usize,
    pub precision_at_10: f32,
    pub recall_boost: f32,
}

/// Domain adapter training result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptResult {
    pub feedback_used: usize,
    pub helpful_count: usize,
    pub generation: u32,
}

/// Current state of the domain adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptStatus {
    pub has_adapter: bool,
    pub feedback_count: usize,
    pub generation: u32,
    pub ready: bool,
}

#[cfg(feature = "semantic")]
pub use semantic_types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_record_new_has_no_observed_at() {
        let f = FactRecord::new("x".into(), FactType::Note, vec![]);
        assert!(f.observed_at.is_none());
        assert!(f.created_at > 0);
    }

    #[test]
    fn fact_record_serde_roundtrip_with_observed_at() {
        let mut f = FactRecord::new("x".into(), FactType::Decision, vec![]);
        f.observed_at = Some(1_700_000_000_000);
        let bytes = serde_json::to_vec(&f).unwrap();
        let back: FactRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.observed_at, Some(1_700_000_000_000));
        assert_eq!(back.content, "x");
    }

    #[test]
    fn fact_record_deserializes_legacy_record_without_observed_at() {
        // Simulate a record written by an older wg version (no observed_at field).
        let legacy = serde_json::json!({
            "id": FactId::new(),
            "content": "legacy",
            "fact_type": "note",
            "entity_ids": [],
            "tags": [],
            "source": null,
            "source_confidence": 0.5,
            "relevance_score": 0.5,
            "created_at": 1_700_000_000_000u64,
            "updated_at": 1_700_000_000_000u64,
            "access_count": 0,
            "last_accessed_at": 1_700_000_000_000u64,
        });
        let bytes = serde_json::to_vec(&legacy).unwrap();
        let f: FactRecord = serde_json::from_slice(&bytes).unwrap();
        assert!(f.observed_at.is_none());
        assert_eq!(f.content, "legacy");
    }

    #[test]
    fn fact_update_sets_observed_at() {
        let mut f = FactRecord::new("x".into(), FactType::Note, vec![]);
        let upd = FactUpdate {
            observed_at: Some(1_700_000_000_000),
            ..Default::default()
        };
        f.update(upd);
        assert_eq!(f.observed_at, Some(1_700_000_000_000));
    }
}
