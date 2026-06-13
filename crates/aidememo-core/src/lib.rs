//! AideMemo — local-first memory SDK for coding agents.

#[cfg(not(any(feature = "sqlite", feature = "redb")))]
compile_error!("aidememo-core requires at least one storage backend feature: `sqlite` or `redb`.");

pub mod adapt;
pub mod archive;
pub mod backend;
pub mod config;
pub mod embedding;
pub mod error;
pub mod extract;
pub mod extract_structured;
pub mod fuzzy;
pub mod graph;
pub mod index;
pub mod ingest;
pub mod lint;
pub mod migrate;
pub mod relations;
#[cfg(feature = "semantic")]
pub mod rerank;
#[cfg(feature = "s3")]
pub mod s3;
pub mod search;
#[cfg(feature = "sqlite")]
pub mod sqlite_store;
#[cfg(feature = "redb")]
pub mod store;
pub mod sync;
pub mod time;
pub mod types;
#[cfg(feature = "semantic")]
pub mod vector_index;
#[cfg(feature = "s3")]
pub mod wal;

use parking_lot::RwLock;
use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "semantic")]
use std::sync::OnceLock;

pub use config::{Config, ProjectConfig};
pub use error::{AideMemoError, Result};
pub use ingest::{IngestStats, ParsedFile, Section, Wikilink};

// Re-export ulid for external use
pub use ulid;

// Re-export store and graph components
use backend::{StoreBackend, StoreKind};
use graph::Graph;

/// AideMemo instance.
///
/// Thread-safe, can be shared across multiple operations.
pub struct AideMemo {
    // Use interior mutability pattern - Store itself uses Arc<Database>
    // For mutable operations, we use RwLock
    store: Arc<RwLock<StoreKind>>,
    /// Absolute path to the store file. Captured at `open` time so
    /// sidecars (HNSW index, i8 quant cache, etc.) live next to it
    /// regardless of how the caller spelled the path.
    store_path: std::path::PathBuf,
    config: Arc<Config>,
    /// Tier 7-A: lazy-loaded embedding provider, reused across all
    /// search calls. Without this `load_provider` was called per
    /// query, paying the Model2Vec model load cost (≈1 GB virtual
    /// allocation, hundreds of ms of wall time) every time.
    #[cfg(feature = "semantic")]
    provider: OnceLock<Arc<dyn embedding::EmbeddingProvider>>,
    /// Tier 7-A: LRU of recently-seen query embeddings. LLM agents
    /// often repeat the same topic across turns; cached query vectors
    /// skip the inference entirely.
    #[cfg(feature = "semantic")]
    query_embed_cache: Arc<parking_lot::Mutex<lru::LruCache<String, Vec<f32>>>>,
    /// Tier 7-C: in-memory cache of fact embeddings, quantized to i8
    /// (4× smaller than the f32 originals). Built lazily on first
    /// search-touch of each fact. Without this every search re-ran
    /// Model2Vec inference for every candidate; with it the second
    /// search onwards pays only the SIMD cosine cost.
    /// Invalidated on fact_add / fact_update / fact_delete.
    #[cfg(feature = "semantic")]
    fact_embed_cache: Arc<
        parking_lot::RwLock<std::collections::HashMap<types::FactId, search::QuantizedEmbedding>>,
    >,
    /// Tier 8: HNSW ANN index over fact embeddings. Loaded lazily
    /// from `wiki.hnsw.bin` next to the store file on first
    /// search; rebuilt on demand via `vector_index_rebuild()` or
    /// when the sidecar's model name doesn't match the active
    /// provider. `None` means "not built yet" → fall back to the
    /// BM25-prefilter path so the system still works.
    #[cfg(feature = "semantic")]
    vector_index: Arc<parking_lot::RwLock<Option<vector_index::HnswIndex>>>,
    /// Cached BM25 inverted index, shared across every search on
    /// this `AideMemo`. Without this each `hybrid_search` call
    /// constructed a fresh `SearchEngine` and rebuilt the index
    /// from scratch — at 10K facts that was ~800 ms per query.
    /// Mutations (`add_fact`, `fact_update`, `fact_delete`,
    /// `entity_add`, `entity_rename`, `entity_delete`, `ingest`)
    /// flip `dirty = true`; `SearchEngine::ensure_index` rebuilds
    /// on the next search and clears the flag.
    bm25_index: Arc<parking_lot::RwLock<index::Bm25IndexState>>,
    /// Optional cross-encoder reranker that runs after RRF fusion
    /// when `rerank.provider` is set. Lazy-initialized like
    /// `provider`: `None` until the first `hybrid_search` call,
    /// either `Some(reranker)` or stays absent thereafter (we
    /// remember "configured-but-failed-to-construct" as `None` so
    /// we don't keep retrying). When the user disables rerank in
    /// config the field stays `None` and `apply_rerank` is never
    /// called.
    #[cfg(feature = "semantic")]
    reranker: OnceLock<Option<Arc<dyn rerank::Reranker>>>,
    /// Cold-tier sibling. Lazy-opened on first archive_facts /
    /// include_archive search call. None on cold AideMemos (an
    /// archive doesn't get its own archive — invariant) and on
    /// hot AideMemos that have never archived anything yet.
    cold_sibling: parking_lot::Mutex<Option<Arc<AideMemo>>>,
    /// True for cold-tier AideMemos so they refuse to recursively
    /// open another cold (would create nested cold-tier files).
    is_cold: bool,
}

impl AideMemo {
    /// Access the store (read-only or write via RwLock).
    pub fn store(&self) -> &Arc<RwLock<StoreKind>> {
        &self.store
    }
}

impl AideMemo {
    // === Lifecycle ===

    /// Open or create a AideMemo store at the given path.
    pub fn open(path: &Path, config: Config) -> Result<Self> {
        Self::open_inner(path, config, false)
    }

    fn open_inner(path: &Path, config: Config, is_cold: bool) -> Result<Self> {
        let store = StoreKind::open(path, config.clone())?;
        Ok(Self {
            store: Arc::new(RwLock::new(store)),
            store_path: path.to_path_buf(),
            config: Arc::new(config),
            #[cfg(feature = "semantic")]
            provider: OnceLock::new(),
            #[cfg(feature = "semantic")]
            query_embed_cache: Arc::new(parking_lot::Mutex::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(256).expect("non-zero"),
            ))),
            #[cfg(feature = "semantic")]
            fact_embed_cache: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            #[cfg(feature = "semantic")]
            vector_index: Arc::new(parking_lot::RwLock::new(None)),
            bm25_index: Arc::new(parking_lot::RwLock::new(index::Bm25IndexState::new())),
            #[cfg(feature = "semantic")]
            reranker: OnceLock::new(),
            cold_sibling: parking_lot::Mutex::new(None),
            is_cold,
        })
    }

    /// Lazy-open the cold-tier sibling AideMemo. Returns the same
    /// `Arc<AideMemo>` on every call; opens the backend-specific cold file
    /// and runs schema init the first time. Returns `None` when
    /// called on a cold AideMemo (cold's-cold is forbidden).
    pub fn cold(&self) -> Result<Option<Arc<AideMemo>>> {
        if self.is_cold {
            return Ok(None);
        }
        let mut guard = self.cold_sibling.lock();
        if let Some(c) = guard.as_ref() {
            return Ok(Some(c.clone()));
        }
        let cold_path = {
            let store = self.store.read();
            archive::cold_path_for_backend(
                std::path::Path::new(&store.config().store.path),
                &store.config().store.backend,
            )
        };
        let mut cfg = (*self.config).clone();
        cfg.store.path = cold_path.to_string_lossy().into_owned();
        let cold = AideMemo::open_inner(&cold_path, cfg, true)?;
        let arc = Arc::new(cold);
        *guard = Some(arc.clone());
        Ok(Some(arc))
    }

    /// Lazy-load (or return) the configured reranker. Returns `None`
    /// when reranking is disabled in config (`rerank.provider = ""`)
    /// or when the reranker construction failed — in the latter case
    /// we cache the `None` so we don't keep retrying every search.
    #[cfg(feature = "semantic")]
    fn reranker(&self) -> Option<Arc<dyn rerank::Reranker>> {
        if let Some(slot) = self.reranker.get() {
            return slot.clone();
        }
        let resolved = match rerank::load_reranker(&self.config) {
            Ok(Some(r)) => Some(Arc::from(r)),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("reranker disabled — failed to construct: {e}");
                None
            }
        };
        let _ = self.reranker.set(resolved.clone());
        resolved
    }

    /// Mark the cached BM25 inverted index as stale. Call from any
    /// op that changes a fact's BM25 doc text — content + entity
    /// names + tags — so the next search rebuilds before scoring.
    fn bm25_mark_dirty(&self) {
        self.bm25_index.write().dirty = true;
    }

    /// `bm25_mark_dirty` for cross-module callers in the same crate
    /// (currently `sync::sync_import`). Public-in-crate so the sync
    /// path can flip the dirty bit after upserting facts via
    /// `Store::fact_upsert_record`.
    pub(crate) fn bm25_mark_dirty_pub(&self) {
        self.bm25_mark_dirty();
    }

    /// Hand the inner store Arc to a same-crate caller. Today only
    /// `sync::sync_export` / `sync::sync_import` reach in here;
    /// nothing outside the crate gets a Store handle.
    pub(crate) fn store_handle(&self) -> &Arc<RwLock<StoreKind>> {
        &self.store
    }

    /// Lazy-load (or return) the embedding provider. Cached for the
    /// lifetime of the AideMemo.
    #[cfg(feature = "semantic")]
    fn embed_provider(&self) -> Result<Arc<dyn embedding::EmbeddingProvider>> {
        if let Some(p) = self.provider.get() {
            return Ok(p.clone());
        }
        let provider: Arc<dyn embedding::EmbeddingProvider> =
            Arc::from(embedding::load_provider(&self.config)?);
        // OnceLock::set returns Err if already initialized — race-safe.
        let _ = self.provider.set(provider);
        Ok(self
            .provider
            .get()
            .expect("just set or already populated")
            .clone())
    }

    /// Where the HNSW sidecar lives. Sits next to the store file
    /// (using the path the caller passed to `open`, not the config
    /// path which may be a default placeholder).
    #[cfg(feature = "semantic")]
    fn hnsw_sidecar_path(&self) -> std::path::PathBuf {
        self.store_path.with_extension("hnsw.bin")
    }

    /// Embed an arbitrary string via the configured provider.
    /// Public surface so callers (e.g. the bench harness) can use the
    /// same model aidememo uses for hybrid search to score per-fact relevance
    /// against a query — no need to instantiate their own embedder.
    /// Returns an error if `semantic` feature is off or the provider
    /// fails to load.
    #[cfg(feature = "semantic")]
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let provider = self.embed_provider()?;
        provider.embed(text)
    }

    /// Cosine similarity between two equal-length embedding vectors.
    /// Returns 0 when either vector has zero norm. Pure-Rust scalar
    /// loop — kept here so bench / tooling can stay dependency-free.
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let mut dot = 0.0_f32;
        let mut na = 0.0_f32;
        let mut nb = 0.0_f32;
        for i in 0..a.len() {
            dot += a[i] * b[i];
            na += a[i] * a[i];
            nb += b[i] * b[i];
        }
        if na == 0.0 || nb == 0.0 {
            return 0.0;
        }
        dot / (na.sqrt() * nb.sqrt())
    }

    /// Get-or-load-or-build the HNSW index. Three paths:
    ///   1. already loaded → return Arc clone
    ///   2. sidecar exists + model matches → load from disk
    ///   3. otherwise → return None (caller falls back to BM25 prefilter)
    ///
    /// Loading is cheap (bincode of a graph + Vec<f32> table). This
    /// is the lazy entry point — `vector_index_rebuild` is the
    /// explicit one.
    #[cfg(feature = "semantic")]
    fn vector_index_get(
        &self,
        provider: &dyn embedding::EmbeddingProvider,
    ) -> Option<std::sync::Arc<parking_lot::RwLock<Option<vector_index::HnswIndex>>>> {
        // Already loaded?
        if self.vector_index.read().is_some() {
            return Some(self.vector_index.clone());
        }
        // Try sidecar
        let path = self.hnsw_sidecar_path();
        match vector_index::HnswIndex::load_from(&path) {
            Ok(Some(idx)) if idx.matches_provider(&provider.name(), provider.dimension()) => {
                *self.vector_index.write() = Some(idx);
                Some(self.vector_index.clone())
            }
            _ => None,
        }
    }

    /// Build the HNSW index from every fact in the store, persist
    /// it as `wiki.hnsw.bin`, and replace the in-memory copy.
    /// Idempotent + safe to call repeatedly; cost is dominated by
    /// the embedding inference (≈1 ms per fact for model2vec, much
    /// more for HTTP-based providers).
    ///
    /// Reuses cached vectors from an existing sidecar when the
    /// model and dimension still match — embedding inference is the
    /// most expensive stage (~38% of rebuild on 5500-fact corpora),
    /// so reusing untouched fact embeddings drops a no-op rebuild
    /// from 3.7s to ~2.3s for model2vec providers and proportionally
    /// more for HTTP-served transformers.
    #[cfg(feature = "semantic")]
    pub fn vector_index_rebuild(&self) -> Result<usize> {
        self.vector_index_rebuild_with_opts(types::VectorRebuildOpts::default())
            .map(|s| s.facts_indexed)
    }

    /// Same as `vector_index_rebuild`, but accepts options. Set
    /// `opts.current_only = true` after `consolidate_gac` / supersede
    /// passes to drop superseded facts from the rebuilt HNSW index —
    /// the most direct way to keep the index size proportional to the
    /// representative set instead of the raw fact count.
    ///
    /// Trade-off: when `current_only` is on, `as_of` historical
    /// searches that need a superseded fact will fall back to the
    /// BM25-only path (HNSW won't return it). Default off preserves
    /// the existing time-travel surface.
    #[cfg(feature = "semantic")]
    pub fn vector_index_rebuild_with_opts(
        &self,
        opts: types::VectorRebuildOpts,
    ) -> Result<types::VectorRebuildStats> {
        let provider = self.embed_provider()?;
        let all_facts = self.fact_list(FactListOpts {
            limit: None,
            ..Default::default()
        })?;
        let total = all_facts.len();
        let facts: Vec<_> = if opts.current_only {
            all_facts
                .into_iter()
                .filter(|f| f.superseded_at.is_none())
                .collect()
        } else {
            all_facts
        };
        let superseded_skipped = total - facts.len();

        if facts.is_empty() {
            *self.vector_index.write() = None;
            // Remove a stale sidecar if it exists; otherwise harmless.
            let _ = std::fs::remove_file(self.hnsw_sidecar_path());
            return Ok(types::VectorRebuildStats {
                facts_indexed: 0,
                superseded_skipped,
            });
        }

        // Pull cached vectors off the existing sidecar (if any) so we
        // only re-embed facts the cache hasn't seen. Mismatched model
        // / dim invalidates the whole cache — cosine across two
        // different embedding spaces is meaningless.
        let cached: std::collections::HashMap<types::FactId, Vec<f32>> = {
            let in_memory = self.vector_index.read();
            if let Some(idx) = in_memory.as_ref() {
                if idx.matches_provider(&provider.name(), provider.dimension()) {
                    idx.extract_vectors()
                } else {
                    std::collections::HashMap::new()
                }
            } else {
                drop(in_memory);
                vector_index::HnswIndex::load_from(&self.hnsw_sidecar_path())
                    .ok()
                    .flatten()
                    .filter(|idx| idx.matches_provider(&provider.name(), provider.dimension()))
                    .map(|idx| idx.extract_vectors())
                    .unwrap_or_default()
            }
        };

        let mut entries: Vec<(types::FactId, Vec<f32>)> = Vec::with_capacity(facts.len());
        let mut to_embed: Vec<(types::FactId, String)> = Vec::new();
        for fact in facts {
            if let Some(v) = cached.get(&fact.id) {
                entries.push((fact.id, v.clone()));
            } else {
                to_embed.push((fact.id, fact.content));
            }
        }

        if !to_embed.is_empty() {
            let texts: Vec<String> = to_embed.iter().map(|(_, t)| t.clone()).collect();
            let embeddings = provider.embed_batch(&texts)?;
            for ((id, _), v) in to_embed.into_iter().zip(embeddings) {
                entries.push((id, v));
            }
        }

        let count = entries.len();
        let idx = vector_index::HnswIndex::build(&provider.name(), provider.dimension(), entries);
        idx.save_to(&self.hnsw_sidecar_path())?;
        *self.vector_index.write() = Some(idx);
        Ok(types::VectorRebuildStats {
            facts_indexed: count,
            superseded_skipped,
        })
    }

    /// Close the AideMemo store.
    pub fn close(self) -> Result<()> {
        // redb automatically closes when the database is dropped
        Ok(())
    }

    // === Entity Operations ===

    /// Add a new entity.
    pub fn entity_add(&self, input: EntityInput) -> Result<EntityId> {
        let id = self.store.write().entity_add(input)?;
        self.bm25_mark_dirty();
        Ok(id)
    }

    /// Get an entity by name.
    pub fn entity_get(&self, name: &str) -> Result<EntityRecord> {
        self.store.read().entity_get(name)
    }

    /// Get an entity by ID.
    pub fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord> {
        self.store.read().entity_get_by_id(id)
    }

    /// Update an entity.
    pub fn entity_update(&self, name: &str, input: EntityUpdate) -> Result<()> {
        self.store.write().entity_update(name, input)?;
        // Conservative: any field could be `name`, which is in the
        // BM25 doc text for every fact that references this entity.
        self.bm25_mark_dirty();
        Ok(())
    }

    /// List entities with options.
    pub fn entity_list(&self, opts: ListOpts) -> Result<Vec<EntitySummary>> {
        self.store.read().entity_list(opts)
    }

    /// Delete an entity.
    pub fn entity_delete(&self, name: &str) -> Result<()> {
        self.store.write().entity_delete(name)?;
        self.bm25_mark_dirty();
        Ok(())
    }

    /// Rename an entity (alias for entity_update with name change).
    pub fn entity_rename(&self, old_name: &str, new_name: &str) -> Result<()> {
        self.store.write().entity_update(
            old_name,
            EntityUpdate {
                name: Some(new_name.to_string()),
                ..Default::default()
            },
        )?;
        self.bm25_mark_dirty();
        Ok(())
    }

    /// Set the "compiled truth" summary prose for an entity.
    ///
    /// Pass `""` to clear an existing summary, or any non-empty text to set
    /// it. Updates `summary_updated_at` to now in either direction.
    pub fn entity_describe(&self, name: &str, summary: &str) -> Result<()> {
        self.store.write().entity_update(
            name,
            EntityUpdate {
                summary: Some(summary.to_string()),
                ..Default::default()
            },
        )
    }

    /// Add an alias to an entity.
    pub fn entity_alias_add(&self, name: &str, alias: &str) -> Result<()> {
        let record = self.store.read().entity_get(name)?;
        let mut updated_aliases = record.aliases.clone();
        updated_aliases.push(alias.to_string());
        self.store.write().entity_update(
            name,
            EntityUpdate {
                aliases: Some(updated_aliases),
                ..Default::default()
            },
        )
    }

    /// Resolve an entity name (or alias) to an ID.
    pub fn resolve_entity(&self, name: &str) -> Result<EntityId> {
        self.store.read().resolve_entity(name)
    }

    /// Suggest similar entity names for fuzzy matching.
    pub fn suggest_similar_entities(&self, name: &str) -> Result<Vec<String>> {
        self.store.read().suggest_similar_entities(name)
    }

    // === Fact Operations ===

    /// Add a new fact.
    /// Run the heuristic conversation-to-fact extractor against a
    /// chunk of text. Returns ranked [`extract::ExtractCandidate`]s
    /// the agent can review before persisting via `fact_add_many`.
    /// See `crates/aidememo-core/src/extract.rs` for the scoring rules.
    pub fn extract_candidates(
        &self,
        text: &str,
        max_candidates: usize,
    ) -> Result<Vec<extract::ExtractCandidate>> {
        let store = self.store.read();
        extract::extract_candidates(text, &*store, max_candidates)
    }

    /// LLM-aided extractor — same return shape as
    /// [`Self::extract_candidates`], but routes through the chat
    /// completions endpoint configured by `extract.provider`. Returns
    /// an error if the provider is unset; callers should fall back to
    /// the heuristic in that case.
    #[cfg(feature = "semantic")]
    pub fn extract_candidates_llm(
        &self,
        text: &str,
        max_candidates: usize,
    ) -> Result<Vec<extract::ExtractCandidate>> {
        let store = self.store.read();
        extract::extract_candidates_llm(text, &*store, &self.config.extract, max_candidates)
    }

    /// Top-N currently-pinned (`pinned=true` AND not superseded)
    /// facts, sorted by `last_accessed_at` descending. The agent's
    /// "always loaded" tier — Letta-style memory hierarchy without
    /// the runtime.
    pub fn pinned_facts(&self, limit: usize) -> Result<Vec<types::FactRecord>> {
        self.store.read().pinned_facts(limit)
    }

    /// Toggle the pinned flag on a single fact. Wraps `fact_update`
    /// with only the `pinned` field set so call sites don't have to
    /// build a full FactUpdate just to flip one boolean.
    pub fn fact_pin(&self, id: &FactId, pinned: bool) -> Result<()> {
        self.store.write().fact_update(
            id,
            types::FactUpdate {
                pinned: Some(pinned),
                ..Default::default()
            },
        )
    }

    pub fn add_fact(&self, input: FactInput) -> Result<FactId> {
        let id = {
            let mut store = self.store.write();
            store.fact_add(input)?
        };
        // New fact text → no cached embedding to keep, but the BM25
        // index will need rebuilding too. The fact_embed_cache only
        // holds inferred-once vectors keyed by FactId; new IDs simply
        // get computed lazily on first search.
        self.bm25_mark_dirty();
        // Auto-supersede any existing decision/convention fact on the
        // same entity — atomic types are mutually exclusive per entity
        // by design (mirrors OMEGA's "newer decision auto-resolves").
        // Off by default so the historical "every fact_add creates a
        // new fact" contract holds; opt in via
        // `lifecycle.auto_supersede_atomic_types = true`.
        if self.config.lifecycle.auto_supersede_atomic_types {
            let _ = self.maybe_resolve_atomic_conflict(&id);
        }
        Ok(id)
    }

    /// If the just-inserted fact is an atomic type (decision /
    /// convention) attached to one or more entities, mark any other
    /// not-yet-superseded fact of the same type on the same entity
    /// as superseded by this new one. The intent: the agent's
    /// "current state" view of an entity (`current_only=true`) only
    /// ever shows the most recent decision per entity.
    ///
    /// Best-effort: errors are returned to the caller, but the new
    /// fact is already inserted, so callers may choose to log and
    /// continue.
    fn maybe_resolve_atomic_conflict(&self, new_id: &FactId) -> Result<()> {
        let new_fact = self.fact_get(new_id)?;
        // Skip when this insert was a dedup-collapsed return — the
        // returned id matches an existing record whose entities /
        // type we don't want to touch.
        let is_atomic = matches!(
            new_fact.fact_type,
            FactType::Decision | FactType::Convention
        );
        if !is_atomic || new_fact.entity_ids.is_empty() {
            return Ok(());
        }
        let mut to_supersede: Vec<FactId> = Vec::new();
        for entity_id in &new_fact.entity_ids {
            let opts = types::FactListOpts {
                fact_type: Some(new_fact.fact_type),
                entity_id: Some(*entity_id),
                min_confidence: None,
                source_id: None,
                limit: None,
                offset: 0,
                since: None,
                until: None,
                current_only: true,
                as_of: None,
            };
            let candidates = self.store.read().fact_list(opts)?;
            for c in candidates {
                if c.id != *new_id && !to_supersede.contains(&c.id) {
                    to_supersede.push(c.id);
                }
            }
        }
        if to_supersede.is_empty() {
            return Ok(());
        }
        for old in &to_supersede {
            self.fact_supersede(old, new_id)?;
        }
        Ok(())
    }

    /// Backwards-compatible alias for add_fact.
    pub fn fact_add(&self, input: FactInput) -> Result<FactId> {
        self.add_fact(input)
    }

    /// Insert N facts in one backend transaction when supported. Use this for
    /// bulk imports so commit overhead is paid once for the batch instead of
    /// once per fact.
    /// All-or-nothing: a serialization or write failure aborts the
    /// transaction and no facts land. Returned ids are in the same
    /// order as the inputs.
    pub fn fact_add_many(&self, inputs: Vec<FactInput>) -> Result<Vec<FactId>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let ids = {
            let mut store = self.store.write();
            store.fact_add_many(inputs)?
        };
        // BM25 re-uses the same dirty mark whether the batch added
        // 1 or 1000 facts; the next search rebuilds against the
        // post-batch state.
        self.bm25_mark_dirty();
        Ok(ids)
    }

    /// Get a fact by ID.
    pub fn fact_get(&self, id: &FactId) -> Result<FactRecord> {
        match self.store.read().fact_get(id) {
            Ok(fact) => Ok(fact),
            Err(hot_err) if !self.is_cold => {
                let cold_path =
                    archive::cold_path_for_backend(&self.store_path, &self.config.store.backend);
                if !cold_path.exists() {
                    return Err(hot_err);
                }
                match self.cold()? {
                    Some(cold) => cold.fact_get(id).or(Err(hot_err)),
                    None => Err(hot_err),
                }
            }
            Err(err) => Err(err),
        }
    }

    /// Update a fact.
    pub fn fact_update(&self, id: &FactId, input: FactUpdate) -> Result<()> {
        self.store.write().fact_update(id, input)?;
        // Tier 7-C: content may have changed → invalidate the cached
        // embedding so the next search re-quantizes from fresh inference.
        #[cfg(feature = "semantic")]
        {
            self.fact_embed_cache.write().remove(id);
        }
        self.bm25_mark_dirty();
        Ok(())
    }

    /// Delete a fact.
    pub fn fact_delete(&self, id: &FactId) -> Result<()> {
        self.store.write().fact_delete(id)?;
        #[cfg(feature = "semantic")]
        {
            self.fact_embed_cache.write().remove(id);
        }
        self.bm25_mark_dirty();
        Ok(())
    }

    /// Record feedback for a fact.
    pub fn fact_feedback(&self, id: &FactId, helpful: bool) -> Result<()> {
        self.store.write().fact_feedback(id, helpful)
    }

    /// Move facts from the hot store to the cold-tier archive. redb stores use
    /// `<store>.cold.redb`; SQLite stores use `<store>.cold.sqlite`. Returns
    /// the number actually moved
    /// (silently skips ids not in hot — they may already be archived).
    /// See `crates/aidememo-core/src/archive.rs` for the design notes
    /// (cold preserves FactId, content-hash dedup, hot delete after
    /// cold commit). After this returns, BM25 / semantic indexes are
    /// marked dirty so the next search rebuilds against the smaller
    /// hot pool. Cold-side index update lands in stage 3.
    pub fn archive_facts(&self, fact_ids: &[FactId]) -> Result<usize> {
        if self.is_cold {
            return Err(AideMemoError::InvalidInput(
                "archive_facts called on a cold AideMemo (no nested archives)".into(),
            ));
        }
        let hot_ids = self.store.read().existing_fact_ids(fact_ids)?;
        if hot_ids.is_empty() {
            return Ok(0);
        }
        let cold = self.cold()?.ok_or_else(|| {
            AideMemoError::InvalidInput("archive_facts called without a cold sibling".into())
        })?;
        let moved = {
            let mut store = self.store.write();
            let mut cold_store = cold.store.write();
            store.fact_archive_to(&mut *cold_store, &hot_ids)?
        };
        if moved > 0 {
            self.bm25_mark_dirty();
            // Cold's BM25 / semantic indexes need a rebuild too — the
            // backend-level archive transfer writes raw fact records, bypassing
            // the regular fact_add path that flips the dirty bit.
            cold.bm25_mark_dirty();
        }
        Ok(moved)
    }

    /// Mark `old_id` as superseded by `new_id`. Sets `old.superseded_at = now`
    /// and `old.superseded_by = new_id`. Errors if either ID doesn't exist or
    /// `old_id` is already superseded.
    pub fn fact_supersede(&self, old_id: &FactId, new_id: &FactId) -> Result<()> {
        let old = self.store.read().fact_get(old_id)?;
        if old.superseded_at.is_some() {
            return Err(AideMemoError::InvalidInput(format!(
                "fact {old_id} already superseded"
            )));
        }
        // Verify the replacement exists in the hot store. Archived facts are
        // readable through fact_get for audit, but write-side timeline changes
        // operate only on the live tier.
        let _ = self.store.read().fact_get(new_id)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.fact_update(
            old_id,
            FactUpdate {
                content: None,
                fact_type: None,
                tags: None,
                source: None,
                source_id: None,
                observed_at: None,
                superseded_at: Some(now),
                superseded_by: Some(*new_id),
                pinned: None,
            },
        )
    }

    /// Record a search session.
    pub fn search_session_add(&self, session: &SearchSession) -> Result<()> {
        self.store.write().search_session_add(session)
    }

    /// Record feedback for a search result.
    pub fn search_feedback_add(&self, feedback: &SearchFeedback) -> Result<()> {
        self.store.write().search_feedback_add(feedback)
    }

    /// List facts with options.
    pub fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>> {
        self.store.read().fact_list(opts)
    }

    /// Count facts with `superseded_at` set. Used by `aidememo doctor` to
    /// detect a stale HNSW sidecar after `consolidate` (the sidecar
    /// keeps superseded facts indexed unless rebuilt with
    /// `vector-rebuild --current-only`).
    pub fn fact_count_superseded(&self) -> Result<usize> {
        let facts = self.store.read().fact_list(FactListOpts::default())?;
        Ok(facts.iter().filter(|f| f.superseded_at.is_some()).count())
    }

    // === Relation Operations ===

    /// Add a new relation.
    pub fn relation_add(&self, input: RelationInput) -> Result<()> {
        self.store.write().relation_add(input)
    }

    /// Auto-relate sweep: for every fact in the store, find the top-K
    /// semantically-similar OTHER facts (cosine ≥ `threshold`) and add
    /// a `related` relation between every distinct entity pair across
    /// the two facts. Mirrors OMEGA's "auto-relate top-3 ≥ 0.45" pass:
    /// a one-shot graph-richening operation users invoke after bulk
    /// ingest, not a per-fact-add hook (the per-add cost would be
    /// prohibitive on big wikis).
    ///
    /// Idempotent — re-running just refreshes existing edges using the
    /// backend's relation uniqueness key `{source}\0{rel_type}\0{target}`.
    /// Pure same-entity pairs are skipped; pairs already linked by a
    /// non-`related` relation are also skipped so domain-specific
    /// edges (`uses`, `decided_by`, …) don't get downgraded.
    #[cfg(feature = "semantic")]
    pub fn auto_relate(&self, opts: AutoRelateOpts) -> Result<AutoRelateStats> {
        use std::collections::HashSet;
        let limit = opts.top_k.saturating_add(1).max(2);
        let threshold = opts.threshold;
        let facts = self.fact_list(crate::types::FactListOpts {
            limit: None,
            ..Default::default()
        })?;
        let mut stats = AutoRelateStats {
            facts_processed: 0,
            pairs_evaluated: 0,
            edges_created: 0,
            edges_skipped_same_entity: 0,
            edges_skipped_existing: 0,
        };

        // Cache existing relations so repeated lookups don't re-query
        // the backend per pair. Set of (source_id, target_id) regardless of
        // rel_type — aidememo's auto-relate refuses to overwrite a richer
        // edge with a generic `related`.
        let mut existing: HashSet<(types::EntityId, types::EntityId)> = HashSet::new();
        for r in self.store.read().relations_list_all()? {
            existing.insert((r.source_id, r.target_id));
        }

        for fact in &facts {
            stats.facts_processed += 1;
            // Find similar facts. We use hybrid_search with the fact's
            // own content as query; the first hit will usually BE the
            // fact itself, so we filter it out before inspecting hits.
            let results = self.hybrid_search(
                &fact.content,
                crate::types::SearchOpts {
                    limit: Some(limit),
                    bm25_only: false,
                    current_only: true,
                    ..Default::default()
                },
            )?;
            for hit in results {
                if hit.fact_id == fact.id {
                    continue;
                }
                if hit.score < threshold {
                    continue;
                }
                let other = self.fact_get(&hit.fact_id)?;
                for src in &fact.entity_ids {
                    for tgt in &other.entity_ids {
                        stats.pairs_evaluated += 1;
                        if src == tgt {
                            stats.edges_skipped_same_entity += 1;
                            continue;
                        }
                        let pair = (*src, *tgt);
                        if existing.contains(&pair) {
                            stats.edges_skipped_existing += 1;
                            continue;
                        }
                        if !opts.dry_run {
                            let src_name = self.entity_get_by_id(*src)?.name;
                            let tgt_name = self.entity_get_by_id(*tgt)?.name;
                            self.relation_add(crate::types::RelationInput {
                                source: src_name,
                                target: tgt_name,
                                relation_type: crate::types::RelationType::new("related"),
                                weight: Some(hit.score),
                                evidence: Some(vec![format!("auto-relate via fact {}", fact.id)]),
                            })?;
                        }
                        existing.insert(pair);
                        stats.edges_created += 1;
                    }
                }
            }
        }
        Ok(stats)
    }

    /// Pairwise semantic-dedup pass over current facts. For every
    /// pair whose embeddings have cosine ≥ `opts.semantic_threshold`,
    /// the older fact (smaller `created_at`) is marked superseded by
    /// the newer one. Idempotent — re-running on a wiki that's
    /// already been consolidated finds no new pairs.
    ///
    /// Mirrors OMEGA's compaction step (newer wins, older flows into
    /// history). Designed for periodic batch use, not a per-write
    /// hook — embedding every fact is O(N) and pairwise comparison is
    /// O(N²), which is fine for the 1k-10k-fact range typical of an
    /// agent wiki but expensive at LongMemEval scale (50k+ facts).
    ///
    /// `dry_run = true` returns the same stats but writes nothing —
    /// use it to tune the threshold without committing.
    #[cfg(feature = "semantic")]
    pub fn consolidate_semantic(
        &self,
        opts: types::ConsolidateOpts,
    ) -> Result<types::ConsolidateStats> {
        use std::collections::{HashMap, HashSet};
        let mut stats = types::ConsolidateStats::default();
        if opts.semantic_threshold <= 0.0 && opts.ttl_days_by_type.is_empty() {
            return Ok(stats);
        }

        let facts = self.fact_list(types::FactListOpts {
            current_only: true,
            limit: None,
            ..Default::default()
        })?;
        stats.facts_processed = facts.len();
        let mut superseded: HashSet<types::FactId> = HashSet::new();

        if opts.semantic_threshold > 0.0 && facts.len() >= 2 {
            // Embed every fact once. The embedding provider already caches
            // its model load and the LRU caches handle repeated calls
            // cheaply, but we still want a single explicit map here so the
            // pairwise loop is pure cosine arithmetic.
            let provider = self.embed_provider()?;
            let mut embeds: HashMap<types::FactId, Vec<f32>> = HashMap::with_capacity(facts.len());
            for f in &facts {
                let v = provider.embed(&f.content)?;
                embeds.insert(f.id, v);
            }

            // Pairwise cosine. Older fact (smaller created_at) loses;
            // ties broken by smaller ULID so the result is deterministic.
            let mut to_apply: Vec<(types::FactId, types::FactId, f32)> = Vec::new();
            let mut max_cos: f32 = 0.0;
            for i in 0..facts.len() {
                if superseded.contains(&facts[i].id) {
                    continue;
                }
                for j in (i + 1)..facts.len() {
                    if superseded.contains(&facts[j].id) {
                        continue;
                    }
                    let cos = cosine_f32(&embeds[&facts[i].id], &embeds[&facts[j].id]);
                    if cos > max_cos {
                        max_cos = cos;
                    }
                    if cos < opts.semantic_threshold {
                        continue;
                    }
                    let (older, newer) = if facts[i].created_at < facts[j].created_at
                        || (facts[i].created_at == facts[j].created_at
                            && facts[i].id.0.to_string() < facts[j].id.0.to_string())
                    {
                        (facts[i].id, facts[j].id)
                    } else {
                        (facts[j].id, facts[i].id)
                    };
                    superseded.insert(older);
                    to_apply.push((older, newer, cos));
                }
            }

            stats.pairs_found = to_apply.len();
            stats.max_cosine = max_cos;

            if !opts.dry_run {
                for (old, new, _) in &to_apply {
                    // Skip if a prior pair in this same pass already
                    // superseded `old` against a different `new` — the
                    // first newer wins.
                    if let Ok(rec) = self.fact_get(old) {
                        if rec.superseded_at.is_some() {
                            continue;
                        }
                        self.fact_supersede(old, new)?;
                        stats.supersedes_applied += 1;
                    }
                }
            }
        }

        // ── TTL pass — independent of semantic dedup ───────────────
        // For every fact whose type has a configured TTL, mark it
        // superseded if its `created_at` is older than `now - ttl`.
        // This is expiry, not replacement: `superseded_by` stays None,
        // mirroring OMEGA's typed forgetting (session summaries
        // expire to None; lessons/preferences are permanent because
        // they're not in the TTL map).
        if !opts.ttl_days_by_type.is_empty() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            for fact in &facts {
                if superseded.contains(&fact.id) {
                    continue;
                }
                if fact.superseded_at.is_some() {
                    continue;
                }
                let type_str = fact.fact_type.to_string();
                let Some(&ttl_days) = opts.ttl_days_by_type.get(&type_str) else {
                    continue;
                };
                // ttl_days = 0 means "expire immediately" — `cutoff_ms`
                // becomes `now_ms`, so any fact with `created_at <
                // now_ms` (i.e. anything not literally just-inserted)
                // expires. Types not present in the map stay
                // permanent — that's the "not listed" branch above.
                let cutoff_ms = now_ms.saturating_sub(ttl_days * 24 * 60 * 60 * 1000);
                if fact.created_at < cutoff_ms {
                    if !opts.dry_run {
                        self.fact_update(
                            &fact.id,
                            types::FactUpdate {
                                content: None,
                                fact_type: None,
                                tags: None,
                                source: None,
                                source_id: None,
                                observed_at: None,
                                superseded_at: Some(now_ms),
                                superseded_by: None,
                                pinned: None,
                            },
                        )?;
                    }
                    stats.expired_applied += 1;
                }
            }
        }
        Ok(stats)
    }

    /// GAC (Geometry-Aware Consolidation) analysis pass. Stage 2a:
    /// dry-run only — embeds every current fact, single-link
    /// clusters at cosine ≥ θ, computes within-cluster mean cosine
    /// distance d̄, classifies each multi-fact cluster as tight
    /// (d̄ < θ' = 1 - θ, safe to compress to centroid) or spread
    /// (needs medoid+budget routing per the paper).
    ///
    /// Stage 2a returns `GacStats` only; no fact mutation. Stage 2b
    /// will gate `tight` clusters' supersede + `spread` clusters'
    /// cold-tier archive on the same opts.dry_run flag.
    #[cfg(feature = "semantic")]
    pub fn consolidate_gac(&self, opts: types::GacOpts) -> Result<types::GacStats> {
        use std::collections::HashMap;
        let mut stats = types::GacStats {
            theta: opts.theta,
            ..Default::default()
        };
        if opts.theta <= 0.0 || opts.theta > 1.0 {
            return Err(AideMemoError::InvalidInput(format!(
                "GacOpts.theta must be in (0, 1], got {}",
                opts.theta
            )));
        }

        let all_current = self.fact_list(types::FactListOpts {
            current_only: true,
            limit: None,
            ..Default::default()
        })?;
        let total_current = all_current.len();
        let facts: Vec<_> = if opts.protected_types.is_empty() {
            all_current
        } else {
            all_current
                .into_iter()
                .filter(|f| !opts.protected_types.contains(&f.fact_type))
                .collect()
        };
        stats.protected_skipped = total_current - facts.len();
        stats.facts_processed = facts.len();
        if facts.len() < 2 {
            stats.n_clusters = facts.len();
            stats.n_singletons = facts.len();
            return Ok(stats);
        }

        let provider = self.embed_provider()?;
        let mut embeds: HashMap<types::FactId, Vec<f32>> = HashMap::with_capacity(facts.len());
        for f in &facts {
            embeds.insert(f.id, provider.embed(&f.content)?);
        }

        // Single-link clustering via union-find. Two facts join the
        // same cluster when their cosine similarity ≥ θ. n² in fact
        // count — fine at the scale `consolidate` is expected to run
        // (periodic batch, hundreds-to-low-thousands of facts).
        let n = facts.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &mut [usize], x: usize) -> usize {
            let mut r = x;
            while parent[r] != r {
                r = parent[r];
            }
            // path-compress
            let mut cur = x;
            while parent[cur] != r {
                let nxt = parent[cur];
                parent[cur] = r;
                cur = nxt;
            }
            r
        }
        for i in 0..n {
            for j in (i + 1)..n {
                let cos = cosine_f32(&embeds[&facts[i].id], &embeds[&facts[j].id]);
                if cos >= opts.theta {
                    let ri = find(&mut parent, i);
                    let rj = find(&mut parent, j);
                    if ri != rj {
                        parent[ri] = rj;
                    }
                }
            }
        }

        // Group facts by their root.
        let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            let r = find(&mut parent, i);
            clusters.entry(r).or_default().push(i);
        }

        let theta_prime = 1.0 - opts.theta;
        let mut max_dbar: f32 = 0.0;
        let mut max_cluster_size: usize = 0;

        // Per-cluster routing decisions, deferred so dry-run shares
        // the same code path that mutation uses. Each entry:
        //   (representative_idx, [losers_to_supersede], [losers_to_archive])
        // After classification we either drop the lists (dry-run) or
        // dispatch supersede / archive_facts in two passes.
        struct ClusterDecision {
            representative: usize,
            supersede_losers: Vec<usize>,
            archive_losers: Vec<usize>,
            tight: bool,
        }
        let mut decisions: Vec<ClusterDecision> = Vec::new();

        for members in clusters.values() {
            stats.n_clusters += 1;
            if members.len() < 2 {
                stats.n_singletons += 1;
                continue;
            }
            stats.n_multi_clusters += 1;
            if members.len() > max_cluster_size {
                max_cluster_size = members.len();
            }

            // Within-cluster mean cosine distance + per-member
            // mean distance to peers (used to pick the medoid).
            let m = members.len();
            let mut sum_dist: f32 = 0.0;
            let mut pair_count: usize = 0;
            let mut per_member_sum: Vec<f32> = vec![0.0; m];
            for a in 0..m {
                for b in (a + 1)..m {
                    let cos = cosine_f32(
                        &embeds[&facts[members[a]].id],
                        &embeds[&facts[members[b]].id],
                    );
                    let d = 1.0 - cos;
                    sum_dist += d;
                    pair_count += 1;
                    per_member_sum[a] += d;
                    per_member_sum[b] += d;
                }
            }
            let d_bar = if pair_count == 0 {
                0.0
            } else {
                sum_dist / pair_count as f32
            };
            if d_bar > max_dbar {
                max_dbar = d_bar;
            }
            let tight = d_bar < theta_prime;
            if tight {
                stats.tight_clusters += 1;
                stats.tight_facts += m;
            } else {
                stats.spread_clusters += 1;
                stats.spread_facts += m;
            }

            // Tight: representative = newest fact in the cluster
            // (matches the paper's "centroid is cheap" claim while
            // keeping the same newer-wins invariant the existing
            // pairwise consolidate uses, so stage 2b reproduces
            // existing behaviour for tight clusters).
            //
            // Spread: representative = medoid (member with the
            // smallest mean cosine distance to peers). Residual
            // budget keeps the next `spread_residual_budget` most
            // distant members on top of the medoid; the rest get
            // archived to cold.
            let representative_local: usize;
            let mut survivors: Vec<usize>;
            if tight {
                let newest_local = (0..m)
                    .max_by_key(|&i| facts[members[i]].created_at)
                    .unwrap_or(0);
                representative_local = newest_local;
                survivors = vec![newest_local];
            } else {
                let medoid_local = (0..m)
                    .min_by(|&a, &b| {
                        per_member_sum[a]
                            .partial_cmp(&per_member_sum[b])
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .unwrap_or(0);
                representative_local = medoid_local;
                survivors = vec![medoid_local];
                if opts.spread_residual_budget > 0 {
                    // Pick the budget most-distant-from-medoid
                    // members as residuals. They preserve cluster
                    // diversity the medoid alone can't represent.
                    let mut others: Vec<usize> = (0..m).filter(|&i| i != medoid_local).collect();
                    others.sort_by(|&a, &b| {
                        per_member_sum[b]
                            .partial_cmp(&per_member_sum[a])
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for &idx in others.iter().take(opts.spread_residual_budget) {
                        survivors.push(idx);
                    }
                }
            }

            // Build the loser lists: every member not in survivors
            // is either superseded (use_cold_tier=false) or archived
            // (use_cold_tier=true).
            let mut supersede_losers = Vec::new();
            let mut archive_losers = Vec::new();
            for (i, &member_idx) in members.iter().enumerate().take(m) {
                if survivors.contains(&i) {
                    continue;
                }
                if opts.use_cold_tier {
                    archive_losers.push(member_idx);
                } else {
                    supersede_losers.push(member_idx);
                }
            }
            decisions.push(ClusterDecision {
                representative: members[representative_local],
                supersede_losers,
                archive_losers,
                tight,
            });
        }
        stats.max_dbar = max_dbar;
        stats.max_cluster_size = max_cluster_size;

        if !opts.dry_run {
            // Pass 1: supersede losers. Older fact's `superseded_by`
            // points at the cluster representative.
            for d in &decisions {
                let new_id = facts[d.representative].id;
                for &loser_idx in &d.supersede_losers {
                    let old_id = facts[loser_idx].id;
                    if old_id == new_id {
                        continue;
                    }
                    if let Ok(rec) = self.fact_get(&old_id) {
                        if rec.superseded_at.is_some() {
                            continue;
                        }
                        if self.fact_supersede(&old_id, &new_id).is_ok() {
                            if d.tight {
                                stats.tight_collapsed += 1;
                            } else {
                                stats.spread_archived += 1;
                            }
                        }
                    }
                }
            }
            // Pass 2: archive losers (only when use_cold_tier=true).
            // Collect ids first so we can call archive_facts in a
            // single batch — that path already takes care of
            // BM25-mark-dirty + cold-side BM25 dirty.
            if opts.use_cold_tier {
                let mut to_archive: Vec<types::FactId> = Vec::new();
                for d in &decisions {
                    for &loser_idx in &d.archive_losers {
                        to_archive.push(facts[loser_idx].id);
                    }
                }
                if !to_archive.is_empty() {
                    let moved = self.archive_facts(&to_archive)?;
                    stats.archived_to_cold = moved;
                    // tight_collapsed / spread_archived breakdown is
                    // by routing decision, not by destination; the
                    // archived bucket may include both. We approximate
                    // by attributing per-cluster: walk decisions again.
                    let mut tight_archived = 0usize;
                    let mut spread_archived = 0usize;
                    for d in &decisions {
                        for _ in &d.archive_losers {
                            if d.tight {
                                tight_archived += 1;
                            } else {
                                spread_archived += 1;
                            }
                        }
                    }
                    stats.tight_collapsed += tight_archived;
                    stats.spread_archived += spread_archived;
                }
            }
        }
        Ok(stats)
    }

    /// Remove a relation.
    pub fn relation_remove(&self, source: &str, target: &str, rel_type: &str) -> Result<()> {
        self.store.write().relation_remove(source, target, rel_type)
    }

    /// Get relations for an entity.
    pub fn relations_get(
        &self,
        entity: &str,
        direction: TraverseDirection,
    ) -> Result<Vec<RelationRecord>> {
        self.store.read().relations_get(entity, direction)
    }

    // === Graph Operations ===

    /// Traverse the graph from a starting entity.
    pub fn traverse(&self, start: &str, opts: TraverseOpts) -> Result<TraverseResult> {
        let store = self.store.read();
        let graph = Graph::new(&*store);
        graph.traverse(start, opts)
    }

    /// Find a path between two entities.
    pub fn path_find(&self, from: &str, to: &str) -> Result<Option<Vec<PathStep>>> {
        let store = self.store.read();
        let graph = Graph::new(&*store);
        graph.path_find(from, to)
    }

    // === Ingest ===

    /// Ingest markdown files from a wiki root directory.
    ///
    /// Parses frontmatter, wikilinks, and heading-anchored sections,
    /// then writes entities, relations, and facts to the store.
    pub fn ingest(&self, wiki_root: &Path, incremental: bool) -> Result<ingest::IngestStats> {
        let stats = {
            let mut store = self.store.write();
            ingest::ingest_wiki(wiki_root, &mut *store, incremental)?
        };
        self.bm25_mark_dirty();
        // Auto-rebuild the HNSW index if the user opted into the
        // "hnsw" semantic path. Failure is non-fatal — the BM25
        // fallback in hybrid_search will still serve results, and
        // operators can retry with `aidememo vector-rebuild` or by
        // re-ingesting.
        #[cfg(feature = "semantic")]
        if self.config.search.semantic_index == "hnsw" {
            if let Err(e) = self.vector_index_rebuild() {
                tracing::warn!("HNSW index rebuild after ingest failed: {e}");
            }
        }
        Ok(stats)
    }

    // === Search ===

    /// Search for facts matching a query.
    #[cfg(feature = "semantic")]
    pub fn search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>> {
        use search::SearchEngine;
        let want_archive = opts.include_archive && !self.is_cold;
        let limit = opts.limit.unwrap_or(self.config.search.default_limit);
        let mut results = {
            let store = self.store.read();
            let engine = SearchEngine::new(&*store, &self.config, &self.bm25_index);
            engine.search(query, opts.clone())?
        };
        if want_archive {
            self.merge_archive_results(query, &mut results, opts, limit, false)?;
        }
        Ok(results)
    }

    /// Pull cold-tier results in to fill out the hot result list, up
    /// to `limit`. Hot results keep their order; cold supplies
    /// fillers that don't already appear (dedup on fact_id). Used by
    /// both `search` and `hybrid_search` so the merge logic is in one
    /// place. `use_hybrid=true` routes the cold call through
    /// `hybrid_search`; otherwise plain `search`.
    #[cfg(feature = "semantic")]
    fn merge_archive_results(
        &self,
        query: &str,
        results: &mut Vec<SearchResult>,
        opts: SearchOpts,
        limit: usize,
        use_hybrid: bool,
    ) -> Result<()> {
        if results.len() >= limit {
            return Ok(());
        }
        let cold = match self.cold()? {
            Some(c) => c,
            None => return Ok(()),
        };
        let mut cold_opts = opts;
        cold_opts.include_archive = false; // never recurse
        let cold_results = if use_hybrid {
            cold.hybrid_search(query, cold_opts)?
        } else {
            cold.search(query, cold_opts)?
        };
        let existing: std::collections::HashSet<FactId> =
            results.iter().map(|r| r.fact_id).collect();
        for cr in cold_results {
            if results.len() >= limit {
                break;
            }
            if existing.contains(&cr.fact_id) {
                continue;
            }
            results.push(cr);
        }
        for (i, r) in results.iter_mut().enumerate() {
            r.rank = i + 1;
        }
        Ok(())
    }

    /// Search using hybrid BM25 + semantic ranking.
    #[cfg(feature = "semantic")]
    #[tracing::instrument(level = "debug", skip(self, opts), fields(query_len = query.len()))]
    pub fn hybrid_search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>> {
        // Lazy fast path. `bm25_only` (or zero semantic weight in
        // config) tells us we don't need the embedding model — skip
        // the provider load and run pure BM25. This saves ~700-900ms
        // of cold-start tax on a fresh CLI spawn.
        if opts.bm25_only || self.config.search.semantic_weight == 0.0 {
            tracing::debug!(reason = "bm25_only", "lazy fast path");
            return self.search(query, opts);
        }
        let provider_start = std::time::Instant::now();
        let provider = self.embed_provider()?;
        tracing::debug!(
            ms = provider_start.elapsed().as_secs_f64() * 1000.0,
            "embed_provider loaded"
        );

        // Two-stage retrieval: when a reranker is configured, ask for
        // a wider candidate pool than the user wants in the final
        // result. The reranker can then promote correct evidence from
        // ranks 11..K into the user's top-10. This is the standard
        // pattern (cf. Pinecone's "rerankers and two-stage retrieval"
        // and OMEGA's 5-stage pipeline). Without this, even a perfect
        // reranker can only re-order what RRF already cut to the
        // user's `limit` — capping the upside.
        let user_limit = opts.limit.unwrap_or(self.config.search.default_limit);
        // Capture the include_archive flag before opts moves into the
        // engine — we need it back for the cold-tier merge step.
        let include_archive = opts.include_archive;
        let archive_opts = opts.clone();
        let opts = if self.config.rerank.provider.trim().is_empty() {
            opts
        } else {
            let wider = user_limit.max(self.config.rerank.top_k).max(20);
            SearchOpts {
                limit: Some(wider),
                ..opts
            }
        };

        // Try the HNSW path when configured. If the sidecar is
        // missing, model-mismatched, or fails to load for any
        // reason, fall through to the BM25-prefilter path so the
        // search still works (just without the +recall benefit).
        // Operators can run `aidememo vector-rebuild` to fix the sidecar.
        let mut results = if self.config.search.semantic_index == "hnsw" && {
            let _ = self.vector_index_get(provider.as_ref());
            self.vector_index.read().is_some()
        } {
            let guard = self.vector_index.read();
            let idx = guard.as_ref().expect("checked is_some above");
            let store = self.store.read();
            search::hybrid_search_with_hnsw(
                &*store,
                query,
                opts,
                provider.as_ref(),
                &self.query_embed_cache,
                idx,
                &self.bm25_index,
            )?
        } else {
            if self.config.search.semantic_index == "hnsw" {
                // Index unavailable — log via tracing and fall through.
                // We don't error out because BM25 prefilter is a valid
                // fallback that produces useful results.
                tracing::warn!(
                    sidecar = %self.hnsw_sidecar_path().display(),
                    "semantic_index=hnsw configured but no sidecar; \
                     falling back to BM25 prefilter. Run `aidememo vector-rebuild`.",
                );
            }
            let store = self.store.read();
            search::hybrid_search_with_ctx(
                &*store,
                query,
                opts,
                provider.as_ref(),
                &self.query_embed_cache,
                &self.fact_embed_cache,
                &self.bm25_index,
            )?
        };

        // Optional cross-encoder rerank of the top-K. `apply_rerank`
        // logs and falls through on reranker failure, so a flaky
        // reranker degrades to RRF order rather than failing the
        // search.
        if let Some(reranker) = self.reranker() {
            let top_k = self.config.rerank.top_k;
            rerank::apply_rerank(&mut results, query, reranker.as_ref(), top_k);
            // Now trim back to the user's original limit. The
            // reranker may have promoted a previously-rank-11 hit
            // into the top-10; truncating after the reorder is what
            // realises that gain.
            results.truncate(user_limit);
        }

        // Stage 3 of the cold-tier work: when the caller asked for
        // archive-included search, top up from cold to fill any
        // remaining slots up to user_limit. Reuse the captured flag
        // (opts itself moved into the engine above).
        if include_archive && !self.is_cold {
            let mut cold_opts = archive_opts;
            cold_opts.limit = Some(user_limit);
            cold_opts.bm25_only = false;
            cold_opts.include_archive = false;
            self.merge_archive_results(query, &mut results, cold_opts, user_limit, true)?;
        }

        Ok(results)
    }

    /// Search with graph traversal scope.
    #[cfg(feature = "semantic")]
    pub fn search_with_traverse(
        &self,
        query: &str,
        start: &str,
        depth: u32,
        opts: SearchOpts,
    ) -> Result<Vec<SearchResult>> {
        use search::SearchEngine;
        let store = self.store.read();
        let engine = SearchEngine::new(&*store, &self.config, &self.bm25_index);
        engine.search_with_traverse(query, start, depth, opts)
    }

    /// Search (BM25 only, no semantic features).
    #[cfg(not(feature = "semantic"))]
    pub fn search(&self, _query: &str, _opts: SearchOpts) -> Result<Vec<SearchResult>> {
        Err(AideMemoError::SearchFailed(
            "BM25 search requires the 'semantic' feature".to_string(),
        ))
    }

    // === Query (unified context fetch) ===

    /// Fetch a coherent context dossier for `topic` in a single call.
    ///
    /// Always runs hybrid search. If `topic` resolves to an entity (by name
    /// or alias), additionally returns the entity record, related entities
    /// reachable within `opts.depth` hops, and the most recent facts attached
    /// to that entity.
    ///
    /// This collapses what would otherwise be 3–4 separate calls (search →
    /// entity_get → traverse → fact_list) into one round trip — useful for
    /// LLM agents and MCP clients minimizing context-window spend.
    #[cfg(feature = "semantic")]
    pub fn query(&self, topic: &str, opts: types::QueryOpts) -> Result<types::QueryResult> {
        use types::QueryMode;

        // 1. Hybrid search — every mode except `Local` runs it.
        let search = if opts.mode == QueryMode::Local {
            Vec::new()
        } else {
            self.hybrid_search(
                topic,
                SearchOpts {
                    limit: Some(opts.search_limit),
                    since: opts.since,
                    current_only: opts.current_only,
                    bm25_only: opts.bm25_only,
                    source_id: opts.source_id.clone(),
                    ..Default::default()
                },
            )?
        };

        // 2. Entity resolution — every mode except `Naive` runs it.
        let entity = if opts.mode == QueryMode::Naive {
            None
        } else {
            self.entity_get(topic).ok()
        };

        // 3. Traverse depth — Global widens, Local narrows.
        let depth = match opts.mode {
            QueryMode::Local => opts.depth.max(1),
            QueryMode::Global => opts.depth.max(4),
            _ => opts.depth,
        };

        // 4. Recent facts — Global drops the recency cap.
        let recent_limit = match opts.mode {
            QueryMode::Global => 200, // big-but-bounded
            _ => opts.recent_limit,
        };
        let recent_since = match opts.mode {
            QueryMode::Global => None,
            _ => opts.since,
        };

        let (related, recent_facts) = if let Some(ref e) = entity {
            let traverse = self.traverse(
                &e.name,
                TraverseOpts {
                    depth,
                    relation_types: None,
                    direction: TraverseDirection::Both,
                },
            )?;
            let recent = self.fact_list(FactListOpts {
                entity_id: Some(e.id),
                limit: Some(recent_limit),
                since: recent_since,
                current_only: opts.current_only,
                source_id: opts.source_id.clone(),
                ..Default::default()
            })?;
            (traverse.entities, recent)
        } else {
            (Vec::new(), Vec::new())
        };

        Ok(types::QueryResult {
            topic: topic.to_string(),
            entity,
            search,
            related,
            recent_facts,
        })
    }

    /// Start a tracked workflow from a sparse issue / ticket.
    ///
    /// This is the in-process equivalent of `aidememo workflow start` and MCP
    /// `aidememo_workflow_start`: create a session entity, store the incoming
    /// ticket as a `question` fact, then return a context pack with scoped
    /// search, lessons, errors, and decisions.
    #[cfg(feature = "semantic")]
    pub fn workflow_start(
        &self,
        title: &str,
        opts: types::WorkflowStartOpts,
    ) -> Result<types::WorkflowStartPack> {
        let title = title.trim();
        if title.is_empty() {
            return Err(AideMemoError::InvalidInput(
                "workflow title required".into(),
            ));
        }

        let body = opts
            .body
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let source = opts
            .source
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let source_id = opts
            .source_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let session_name = format!("session-{}", ulid::Ulid::new());
        let session_entity_id = self.entity_add(EntityInput {
            name: session_name.clone(),
            entity_type: Some(EntityType::parse("session")),
            source_page: Some(source.unwrap_or(title).to_string()),
            ..Default::default()
        })?;

        let mut ticket_content = format!("Workflow ticket: {title}");
        if let Some(body) = body {
            ticket_content.push_str("\n\n");
            ticket_content.push_str(body);
        }

        let ticket_fact_id = self.add_fact(FactInput {
            content: ticket_content,
            fact_type: Some(FactType::Question),
            entity_ids: Some(vec![session_entity_id]),
            tags: Some(vec!["workflow-start".into(), "ticket".into()]),
            source: source.map(str::to_string),
            source_id: source_id.map(str::to_string),
            source_confidence: Some(1.0),
            observed_at: None,
        })?;

        let query_text = if let Some(body) = body {
            format!("{title}\n\n{body}")
        } else {
            title.to_string()
        };
        let context = self.query(
            &query_text,
            types::QueryOpts {
                search_limit: opts.limit,
                depth: opts.depth,
                recent_limit: opts.recent_limit,
                since: None,
                current_only: true,
                mode: types::QueryMode::Hybrid,
                bm25_only: opts.bm25_only,
                source_id: source_id.map(str::to_string),
            },
        )?;

        let typed_hits = self
            .hybrid_search(
                &query_text,
                SearchOpts {
                    limit: Some(30),
                    current_only: true,
                    bm25_only: opts.bm25_only,
                    source_id: source_id.map(str::to_string),
                    ..Default::default()
                },
            )
            .unwrap_or_default();
        let prior_lessons = typed_hits
            .iter()
            .filter(|hit| hit.fact_type == FactType::Lesson)
            .take(5)
            .cloned()
            .collect();
        let prior_errors = typed_hits
            .iter()
            .filter(|hit| hit.fact_type == FactType::Error)
            .take(5)
            .cloned()
            .collect();
        let relevant_decisions = typed_hits
            .iter()
            .filter(|hit| hit.fact_type == FactType::Decision)
            .take(5)
            .cloned()
            .collect();

        Ok(types::WorkflowStartPack {
            session_id: session_name.clone(),
            export: format!("export AIDEMEMO_SESSION_ID={session_name}"),
            title: title.to_string(),
            source: source.map(str::to_string),
            source_id: source_id.map(str::to_string),
            ticket_fact_id: ticket_fact_id.to_string(),
            context,
            prior_lessons,
            prior_errors,
            relevant_decisions,
        })
    }

    // === Lint ===

    /// Run graph health checks.
    pub fn lint(&self) -> Result<Vec<LintIssue>> {
        use crate::lint::LintEngine;
        let store = self.store.read();
        let engine = LintEngine::new(&*store);
        Ok(engine.lint()?.issues)
    }

    // === Adapt (semantic-adapt feature) ===

    /// Train the domain adapter using all search feedback.
    #[cfg(feature = "semantic-adapt")]
    pub fn adapt_train(&self) -> Result<crate::types::AdaptResult> {
        let mut store = self.store.write();
        store.adapt_train()
    }

    /// Get current adapter status.
    #[cfg(feature = "semantic-adapt")]
    pub fn adapt_status(&self) -> Result<crate::types::AdaptStatus> {
        let store = self.store.read();
        store.adapt_status()
    }

    /// Evaluate the adapter on all feedback.
    #[cfg(feature = "semantic-adapt")]
    pub fn adapt_eval(&self) -> Result<crate::types::AdaptEvalReport> {
        let store = self.store.read();
        store.adapt_eval()
    }

    // === Import/Export ===

    /// Export data to JSONL.
    pub fn export_jsonl(
        &self,
        writer: &mut dyn std::io::Write,
        scope: ExportScope,
    ) -> Result<ExportStats> {
        use crate::migrate::Exporter;
        let store = self.store.read();
        let exporter = Exporter::new(&*store);
        exporter.export_jsonl(writer, scope)
    }

    /// Import data from JSONL.
    pub fn import_jsonl(&mut self, reader: &mut dyn std::io::Read) -> Result<ImportStats> {
        use crate::migrate::Importer;
        let mut store = self.store.write();
        let mut importer = Importer::new(&mut *store);
        importer.import_jsonl(reader)
    }

    // === Statistics ===

    /// Get store statistics.
    pub fn stats(&self) -> Result<StoreStats> {
        self.store.read().stats()
    }

    /// First-impression overview of the wiki — designed to answer
    /// "what's in here?" in one MCP round-trip. Aggregates the
    /// existing `stats` / `entity_list` / `fact_list` outputs into a
    /// structured snapshot: entity-type buckets with top examples,
    /// fact-type distribution, top-N central entities, recent
    /// activity, current/pinned/orphan counts.
    ///
    /// Reads everything in a couple of full scans — fine for typical
    /// wikis (<100 k facts). Not meant to be called per-turn; agents
    /// invoke it once at session start or on "summarise this wiki"
    /// prompts.
    pub fn overview(&self, opts: types::OverviewOpts) -> Result<types::OverviewResult> {
        use std::collections::BTreeMap;

        let stats = self.stats()?;

        // Entity scan — sort by fact_count descending in a single pass.
        let entities = self.entity_list(types::ListOpts {
            sort_by: types::EntitySort::FactCount,
            limit: None,
            ..Default::default()
        })?;
        let mut orphan_entity_count: u64 = 0;
        let mut by_type: BTreeMap<String, EntityTypeBucketAcc> = BTreeMap::new();
        for e in &entities {
            if e.fact_count == 0 {
                orphan_entity_count += 1;
            }
            let key = e.entity_type.to_string();
            let bucket = by_type.entry(key).or_insert_with(|| EntityTypeBucketAcc {
                entity_type: e.entity_type.clone(),
                count: 0,
                top_examples: Vec::new(),
            });
            bucket.count += 1;
            if bucket.top_examples.len() < opts.top_n_entities {
                // entity_list is already sorted by fact_count desc, so
                // pushing in iteration order yields top-N per bucket.
                bucket.top_examples.push(e.clone());
            }
        }
        let mut entity_types: Vec<types::EntityTypeBucket> = by_type
            .into_values()
            .map(|b| types::EntityTypeBucket {
                entity_type: b.entity_type,
                count: b.count,
                top_examples: b.top_examples,
            })
            .collect();
        entity_types.sort_by_key(|b| std::cmp::Reverse(b.count));
        let top_entities: Vec<types::EntitySummary> =
            entities.into_iter().take(opts.top_n_entities).collect();

        // Fact scan — count by type, count current/pinned, count
        // recent within `recent_days`.
        let facts = self.fact_list(types::FactListOpts {
            limit: None,
            ..Default::default()
        })?;
        let cutoff_ms: u64 = if opts.recent_days == 0 {
            0
        } else {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            now_ms.saturating_sub(opts.recent_days * 24 * 60 * 60 * 1_000)
        };
        let mut fact_types_acc: BTreeMap<String, FactTypeBucketAcc> = BTreeMap::new();
        let mut current_fact_count: u64 = 0;
        let mut pinned_fact_count: u64 = 0;
        let mut recent_fact_count: u64 = 0;
        for f in &facts {
            let key = f.fact_type.to_string();
            let bucket = fact_types_acc
                .entry(key)
                .or_insert_with(|| FactTypeBucketAcc {
                    fact_type: f.fact_type,
                    count: 0,
                });
            bucket.count += 1;
            if f.superseded_at.is_none() {
                current_fact_count += 1;
            }
            if f.pinned {
                pinned_fact_count += 1;
            }
            if f.created_at >= cutoff_ms {
                recent_fact_count += 1;
            }
        }
        let mut fact_types: Vec<types::FactTypeBucket> = fact_types_acc
            .into_values()
            .map(|b| types::FactTypeBucket {
                fact_type: b.fact_type,
                count: b.count,
            })
            .collect();
        fact_types.sort_by_key(|b| std::cmp::Reverse(b.count));

        Ok(types::OverviewResult {
            stats,
            entity_types,
            fact_types,
            top_entities,
            orphan_entity_count,
            recent_fact_count,
            current_fact_count,
            pinned_fact_count,
        })
    }

    /// Get the configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get the store path captured when this graph was opened.
    pub fn store_path(&self) -> &std::path::Path {
        &self.store_path
    }
}

struct EntityTypeBucketAcc {
    entity_type: types::EntityType,
    count: u64,
    top_examples: Vec<types::EntitySummary>,
}

struct FactTypeBucketAcc {
    fact_type: types::FactType,
    count: u64,
}

/// f32 cosine similarity via simsimd. Returns 0.0 on size mismatch
/// or empty input. Range: 0 (orthogonal) → 1 (identical).
#[cfg(feature = "semantic")]
fn cosine_f32(a: &[f32], b: &[f32]) -> f32 {
    use simsimd::SpatialSimilarity;
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    match f32::cosine(a, b) {
        Some(distance) => (1.0 - distance) as f32,
        None => 0.0,
    }
}

// Re-export types for convenience
pub use types::{
    AdaptEvalReport, AdaptResult, AdaptStatus, AutoRelateOpts, AutoRelateStats, ConsolidateOpts,
    ConsolidateStats, EntityId, EntityInput, EntityRecord, EntitySort, EntitySummary, EntityType,
    EntityTypeBucket, EntityUpdate, ExportScope, ExportStats, FactId, FactInput, FactListOpts,
    FactRecord, FactType, FactTypeBucket, FactUpdate, ImportStats, LintIssue, LintReport,
    LintSeverity, ListOpts, OverviewOpts, OverviewResult, PathStep, QueryMode, QueryOpts,
    QueryResult, RelationInput, RelationRecord, RelationType, SearchFeedback, SearchOpts,
    SearchResult, SearchSession, StoreStats, TraverseDirection, TraverseOpts, TraverseResult,
    WorkflowStartOpts, WorkflowStartPack,
};

#[cfg(feature = "semantic")]
pub use types::VectorRecord;

#[cfg(all(test, feature = "sqlite"))]
mod sqlite_backend_tests {
    use super::*;
    use tempfile::tempdir;

    fn sqlite_config(path: &std::path::Path) -> Config {
        let mut config = Config::default();
        config.store.backend = "sqlite".to_string();
        config.store.path = path.to_string_lossy().into_owned();
        config
    }

    #[cfg(feature = "redb")]
    fn libsqlite_config(path: &std::path::Path) -> Config {
        let mut config = Config::default();
        config.store.backend = "libsqlite".to_string();
        config.store.path = path.to_string_lossy().into_owned();
        config
    }

    #[cfg(feature = "redb")]
    fn redb_config(path: &std::path::Path) -> Config {
        let mut config = Config::default();
        config.store.backend = "redb".to_string();
        config.store.path = path.to_string_lossy().into_owned();
        config
    }

    fn fresh_sqlite() -> (AideMemo, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wiki.sqlite");
        let wiki = AideMemo::open(&path, sqlite_config(&path)).unwrap();
        (wiki, dir)
    }

    #[test]
    fn aidememo_sqlite_runs_core_public_api() {
        let (wiki, _dir) = fresh_sqlite();
        let redis = wiki
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                aliases: Some(vec!["redis-server".to_string()]),
                tags: Some(vec!["cache".to_string()]),
                ..Default::default()
            })
            .unwrap();
        let sentinel = wiki
            .entity_add(EntityInput {
                name: "Sentinel".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        wiki.relation_add(RelationInput {
            source: "Sentinel".to_string(),
            target: "Redis".to_string(),
            relation_type: RelationType::new("monitors"),
            weight: Some(1.0),
            evidence: Some(vec!["sqlite-test".to_string()]),
        })
        .unwrap();
        let fact = wiki
            .fact_add(FactInput {
                content: "Redis stores hot cache keys".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![redis, sentinel]),
                source_confidence: Some(1.0),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(wiki.entity_get("redis-server").unwrap().id, redis);
        assert_eq!(wiki.fact_get(&fact).unwrap().fact_type, FactType::Decision);
        assert_eq!(wiki.stats().unwrap().relation_count, 1);

        let traversed = wiki
            .traverse(
                "Sentinel",
                TraverseOpts {
                    depth: 1,
                    direction: TraverseDirection::Forward,
                    relation_types: None,
                },
            )
            .unwrap();
        assert!(
            traversed
                .entities
                .iter()
                .any(|entity| entity.name == "Redis")
        );
        assert!(wiki.lint().unwrap().is_empty());
        let overview = wiki.overview(OverviewOpts::default()).unwrap();
        assert_eq!(overview.stats.fact_count, 1);

        let moved = wiki.archive_facts(&[fact]).unwrap();
        assert_eq!(moved, 1);
        assert_eq!(
            wiki.fact_get(&fact).unwrap().content,
            "Redis stores hot cache keys"
        );
        let cold = wiki.cold().unwrap().unwrap();
        assert!(cold.store_path.ends_with("wiki.sqlite.cold.sqlite"));
        assert_eq!(
            cold.fact_get(&fact).unwrap().content,
            "Redis stores hot cache keys"
        );
    }

    #[test]
    fn aidememo_sqlite_fact_supersede_requires_hot_facts() {
        let (wiki, _dir) = fresh_sqlite();
        let entity = wiki
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                ..Default::default()
            })
            .unwrap();
        let archived = wiki
            .fact_add(FactInput {
                content: "archived Redis decision".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![entity]),
                ..Default::default()
            })
            .unwrap();
        let replacement = wiki
            .fact_add(FactInput {
                content: "live Redis decision".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![entity]),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(wiki.archive_facts(&[archived]).unwrap(), 1);
        assert_eq!(
            wiki.fact_get(&archived).unwrap().content,
            "archived Redis decision"
        );
        let err = wiki
            .fact_supersede(&archived, &replacement)
            .expect_err("archived old fact must not be superseded from cold tier");
        assert!(format!("{err}").contains(&archived.to_string()));

        let live_before = wiki
            .store()
            .read()
            .fact_get(&replacement)
            .expect("live replacement remains hot");
        assert!(live_before.superseded_at.is_none());

        let live = wiki
            .fact_add(FactInput {
                content: "another live Redis decision".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![entity]),
                ..Default::default()
            })
            .unwrap();
        let err = wiki
            .fact_supersede(&live, &archived)
            .expect_err("archived replacement fact must not be used as live successor");
        assert!(format!("{err}").contains(&archived.to_string()));
        let live_after = wiki
            .store()
            .read()
            .fact_get(&live)
            .expect("live fact remains hot");
        assert!(live_after.superseded_at.is_none());
    }

    #[cfg(feature = "semantic")]
    #[test]
    fn sqlite_consolidate_ttl_runs_with_semantic_threshold_disabled() {
        let (wiki, _dir) = fresh_sqlite();
        let entity = wiki
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                ..Default::default()
            })
            .unwrap();
        let fact = wiki
            .fact_add(FactInput {
                content: "short lived Redis note".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![entity]),
                ..Default::default()
            })
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(2));

        let mut opts = ConsolidateOpts {
            semantic_threshold: 0.0,
            dry_run: true,
            ..Default::default()
        };
        opts.ttl_days_by_type.insert("note".to_string(), 0);

        let dry_run = wiki.consolidate_semantic(opts.clone()).unwrap();
        assert_eq!(dry_run.facts_processed, 1);
        assert_eq!(dry_run.pairs_found, 0);
        assert_eq!(dry_run.expired_applied, 1);
        assert!(wiki.fact_get(&fact).unwrap().superseded_at.is_none());

        opts.dry_run = false;
        let applied = wiki.consolidate_semantic(opts).unwrap();
        assert_eq!(applied.facts_processed, 1);
        assert_eq!(applied.pairs_found, 0);
        assert_eq!(applied.expired_applied, 1);
        let expired = wiki.fact_get(&fact).unwrap();
        assert!(expired.superseded_at.is_some());
        assert!(expired.superseded_by.is_none());
    }

    #[cfg(feature = "redb")]
    #[test]
    fn archive_contract_matches_redb_sqlite_and_libsqlite_public_api() {
        fn seed(wiki: &AideMemo) -> FactId {
            let entity = wiki
                .entity_add(EntityInput {
                    name: "Redis".to_string(),
                    entity_type: Some(EntityType::Technology),
                    ..Default::default()
                })
                .unwrap();
            wiki.fact_add(FactInput {
                content: "Redis archive contract fact".to_string(),
                fact_type: Some(FactType::Claim),
                entity_ids: Some(vec![entity]),
                source_confidence: Some(1.0),
                ..Default::default()
            })
            .unwrap()
        }

        fn assert_archive_contract(wiki: &AideMemo, id: FactId, cold_suffix: &str) {
            assert_eq!(wiki.archive_facts(&[id]).unwrap(), 1);
            assert!(
                wiki.store().read().fact_get(&id).is_err(),
                "archive must remove the fact from the hot store",
            );
            assert_eq!(
                wiki.fact_get(&id).unwrap().content,
                "Redis archive contract fact"
            );
            let cold = wiki.cold().unwrap().unwrap();
            assert!(cold.store_path.ends_with(cold_suffix));
            assert_eq!(
                cold.fact_get(&id).unwrap().content,
                "Redis archive contract fact"
            );
            assert_eq!(wiki.archive_facts(&[id]).unwrap(), 0);
        }

        let dir = tempdir().unwrap();
        let redb_path = dir.path().join("wiki.redb");
        let sqlite_path = dir.path().join("wiki.sqlite");
        let libsqlite_path = dir.path().join("wiki.libsqlite");

        let redb = AideMemo::open(&redb_path, redb_config(&redb_path)).unwrap();
        let sqlite = AideMemo::open(&sqlite_path, sqlite_config(&sqlite_path)).unwrap();
        let libsqlite = AideMemo::open(&libsqlite_path, libsqlite_config(&libsqlite_path)).unwrap();

        let redb_fact = seed(&redb);
        let sqlite_fact = seed(&sqlite);
        let libsqlite_fact = seed(&libsqlite);

        assert_archive_contract(&redb, redb_fact, "wiki.redb.cold.redb");
        assert_archive_contract(&sqlite, sqlite_fact, "wiki.sqlite.cold.sqlite");
        assert_archive_contract(&libsqlite, libsqlite_fact, "wiki.libsqlite.cold.sqlite");
    }

    #[cfg(feature = "redb")]
    #[test]
    fn sqlite_matches_redb_for_mutation_feedback_and_relation_contract() {
        #[derive(Debug, PartialEq)]
        struct Snapshot {
            entity_names: Vec<String>,
            fact_contents: Vec<String>,
            pinned_contents: Vec<String>,
            primary_relevance_milli: i32,
            redis_fact_count: u32,
            relation_count: u64,
            search_feedback_count: usize,
        }

        fn exercise(wiki: &AideMemo) -> Snapshot {
            let redis = wiki
                .entity_add(EntityInput {
                    name: "Redis".to_string(),
                    entity_type: Some(EntityType::Technology),
                    aliases: Some(vec!["redis-cache".to_string()]),
                    ..Default::default()
                })
                .unwrap();
            let cache = wiki
                .entity_add(EntityInput {
                    name: "CacheLayer".to_string(),
                    entity_type: Some(EntityType::Custom("component".to_string())),
                    ..Default::default()
                })
                .unwrap();
            wiki.entity_add(EntityInput {
                name: "ScratchEntity".to_string(),
                entity_type: Some(EntityType::Custom("scratch".to_string())),
                ..Default::default()
            })
            .unwrap();
            wiki.entity_rename("ScratchEntity", "RenamedScratch")
                .unwrap();
            assert!(wiki.entity_get("ScratchEntity").is_err());
            assert_eq!(
                wiki.entity_get("RenamedScratch").unwrap().name,
                "RenamedScratch"
            );
            wiki.entity_delete("RenamedScratch").unwrap();
            assert!(wiki.entity_get("RenamedScratch").is_err());

            let ids = wiki
                .fact_add_many(vec![
                    FactInput {
                        content: "Redis mutation contract primary fact".to_string(),
                        fact_type: Some(FactType::Claim),
                        entity_ids: Some(vec![redis, cache]),
                        source_confidence: Some(1.0),
                        ..Default::default()
                    },
                    FactInput {
                        content: "Redis mutation contract delete candidate".to_string(),
                        fact_type: Some(FactType::Note),
                        entity_ids: Some(vec![redis]),
                        source_confidence: Some(1.0),
                        ..Default::default()
                    },
                ])
                .unwrap();
            assert_eq!(ids.len(), 2);
            wiki.fact_delete(&ids[1]).unwrap();
            assert!(wiki.fact_get(&ids[1]).is_err());

            wiki.fact_pin(&ids[0], true).unwrap();
            wiki.fact_feedback(&ids[0], false).unwrap();
            wiki.search_session_add(&SearchSession {
                id: "session-mutation-contract".to_string(),
                query: "redis mutation".to_string(),
                timestamp: 42,
                result_count: 1,
            })
            .unwrap();
            wiki.search_feedback_add(&SearchFeedback {
                session_id: "session-mutation-contract".to_string(),
                fact_id: ids[0],
                helpful: true,
                timestamp: 43,
            })
            .unwrap();

            wiki.relation_add(RelationInput {
                source: "Redis".to_string(),
                target: "CacheLayer".to_string(),
                relation_type: RelationType::new("uses"),
                weight: Some(1.0),
                evidence: Some(vec!["mutation-contract".to_string()]),
            })
            .unwrap();
            assert_eq!(
                wiki.relations_get("Redis", TraverseDirection::Forward)
                    .unwrap()
                    .len(),
                1
            );
            wiki.relation_remove("Redis", "CacheLayer", "uses").unwrap();
            assert!(
                wiki.relations_get("Redis", TraverseDirection::Forward)
                    .unwrap()
                    .is_empty()
            );

            let mut entity_names: Vec<String> = wiki
                .entity_list(ListOpts::default())
                .unwrap()
                .into_iter()
                .map(|entity| entity.name)
                .collect();
            entity_names.sort();

            let mut fact_contents: Vec<String> = wiki
                .fact_list(FactListOpts::default())
                .unwrap()
                .into_iter()
                .map(|fact| fact.content)
                .collect();
            fact_contents.sort();

            let pinned_contents = wiki
                .pinned_facts(10)
                .unwrap()
                .into_iter()
                .map(|fact| fact.content)
                .collect();
            let primary = wiki.fact_get(&ids[0]).unwrap();
            let redis_id = wiki.resolve_entity("redis-cache").unwrap();
            let store = wiki.store().read();

            Snapshot {
                entity_names,
                fact_contents,
                pinned_contents,
                primary_relevance_milli: (primary.relevance_score * 1000.0).round() as i32,
                redis_fact_count: store.count_entity_facts(&redis_id).unwrap(),
                relation_count: wiki.stats().unwrap().relation_count,
                search_feedback_count: store.search_feedback_count().unwrap(),
            }
        }

        let dir = tempdir().unwrap();
        let redb_path = dir.path().join("wiki.redb");
        let sqlite_path = dir.path().join("wiki.sqlite");

        let redb = AideMemo::open(&redb_path, redb_config(&redb_path)).unwrap();
        let sqlite = AideMemo::open(&sqlite_path, sqlite_config(&sqlite_path)).unwrap();

        assert_eq!(exercise(&redb), exercise(&sqlite));
    }

    #[test]
    fn aidememo_sqlite_exports_imports_and_syncs() {
        let (wiki, dir) = fresh_sqlite();
        let redis = wiki
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let fact = wiki
            .fact_add(FactInput {
                content: "Redis import export fixture".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis]),
                source_confidence: Some(1.0),
                ..Default::default()
            })
            .unwrap();

        let mut exported = Vec::new();
        let export_stats = wiki
            .export_jsonl(&mut exported, ExportScope::All)
            .expect("export");
        assert_eq!(export_stats.entities_exported, 1);
        assert_eq!(export_stats.facts_exported, 1);

        let import_path = dir.path().join("import.sqlite");
        let mut imported = AideMemo::open(&import_path, sqlite_config(&import_path)).unwrap();
        let mut reader = exported.as_slice();
        let import_stats = imported.import_jsonl(&mut reader).expect("import");
        assert_eq!(import_stats.entities_imported, 1);
        assert_eq!(import_stats.facts_imported, 1);
        assert_eq!(imported.entity_get_by_id(redis).unwrap().name, "Redis");
        let imported_fact = imported.fact_get(&fact).unwrap();
        assert_eq!(imported_fact.content, "Redis import export fixture");
        assert_eq!(imported_fact.entity_ids, vec![redis]);

        let mut replay = exported.as_slice();
        let replay_stats = imported.import_jsonl(&mut replay).expect("re-import");
        assert_eq!(replay_stats.entities_imported, 0);
        assert_eq!(replay_stats.relations_imported, 0);
        assert_eq!(replay_stats.facts_imported, 0);
        assert_eq!(replay_stats.errors, 0);

        let mut delta = Vec::new();
        let emitted = wiki
            .sync_export(
                sync::SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut delta,
            )
            .expect("sync export");
        assert!(emitted >= 2);
        let sync_path = dir.path().join("sync.sqlite");
        let synced = AideMemo::open(&sync_path, sqlite_config(&sync_path)).unwrap();
        let sync_stats = synced
            .sync_import(std::str::from_utf8(&delta).unwrap())
            .expect("sync import");
        assert_eq!(sync_stats.entities_inserted, 1);
        assert_eq!(sync_stats.facts_inserted, 1);
    }

    #[test]
    fn aidememo_sqlite_ingests_markdown() {
        let (wiki, dir) = fresh_sqlite();
        let wiki_root = dir.path().join("wiki");
        std::fs::create_dir_all(&wiki_root).unwrap();
        std::fs::write(
            wiki_root.join("Redis.md"),
            "Redis references [[Sentinel]].\n\n## Decision: Cache\n\nUse Redis for cache.\n",
        )
        .unwrap();

        let stats = wiki.ingest(&wiki_root, false).unwrap();
        assert_eq!(stats.files_scanned, 1);
        assert_eq!(stats.facts_added, 1);
        assert_eq!(wiki.stats().unwrap().last_ingest_at.is_some(), true);
    }
}

#[cfg(all(test, feature = "sqlite", feature = "semantic"))]
mod sqlite_backend_search_tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(feature = "redb")]
    fn seed_backend(wiki: &AideMemo) {
        let redis = wiki
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                aliases: Some(vec!["redis-server".to_string()]),
                tags: Some(vec!["cache".to_string()]),
                ..Default::default()
            })
            .unwrap();
        let sentinel = wiki
            .entity_add(EntityInput {
                name: "Sentinel".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        wiki.relation_add(RelationInput {
            source: "Sentinel".to_string(),
            target: "Redis".to_string(),
            relation_type: RelationType::new("monitors"),
            weight: Some(1.0),
            evidence: Some(vec!["parity".to_string()]),
        })
        .unwrap();
        wiki.fact_add(FactInput {
            content: "Redis handles hot cache keys".to_string(),
            fact_type: Some(FactType::Claim),
            entity_ids: Some(vec![redis]),
            source_confidence: Some(1.0),
            ..Default::default()
        })
        .unwrap();
        wiki.fact_add(FactInput {
            content: "Sentinel monitors Redis availability".to_string(),
            fact_type: Some(FactType::Note),
            entity_ids: Some(vec![sentinel]),
            source_confidence: Some(1.0),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn aidememo_sqlite_runs_bm25_search_and_query() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wiki.sqlite");
        let mut config = Config::default();
        config.store.backend = "sqlite".to_string();
        config.search.semantic_weight = 0.0;
        let wiki = AideMemo::open(&path, config).unwrap();
        let redis = wiki
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        wiki.fact_add(FactInput {
            content: "Redis handles hot cache keys".to_string(),
            fact_type: Some(FactType::Claim),
            entity_ids: Some(vec![redis]),
            source_confidence: Some(1.0),
            ..Default::default()
        })
        .unwrap();

        let hits = wiki
            .search(
                "hot cache",
                SearchOpts {
                    limit: Some(5),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(hits[0].entity_names, vec!["Redis".to_string()]);

        let result = wiki
            .query(
                "Redis",
                QueryOpts {
                    bm25_only: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(result.entity.is_some());
        assert_eq!(result.recent_facts.len(), 1);
    }

    #[cfg(feature = "redb")]
    #[test]
    fn sqlite_matches_redb_for_core_public_api_fixture() {
        let dir = tempdir().unwrap();
        let redb_path = dir.path().join("wiki.redb");
        let sqlite_path = dir.path().join("wiki.sqlite");

        let mut redb_config = Config::default();
        redb_config.store.backend = "redb".to_string();
        redb_config.store.path = redb_path.to_string_lossy().into_owned();
        redb_config.search.semantic_weight = 0.0;
        let redb = AideMemo::open(&redb_path, redb_config).unwrap();

        let mut sqlite_config = Config::default();
        sqlite_config.store.backend = "sqlite".to_string();
        sqlite_config.store.path = sqlite_path.to_string_lossy().into_owned();
        sqlite_config.search.semantic_weight = 0.0;
        let sqlite = AideMemo::open(&sqlite_path, sqlite_config).unwrap();

        seed_backend(&redb);
        seed_backend(&sqlite);

        assert_eq!(
            redb.stats().unwrap().entity_count,
            sqlite.stats().unwrap().entity_count
        );
        assert_eq!(
            redb.stats().unwrap().fact_count,
            sqlite.stats().unwrap().fact_count
        );
        assert_eq!(
            redb.stats().unwrap().relation_count,
            sqlite.stats().unwrap().relation_count
        );

        let fact_contents = |wiki: &AideMemo| {
            let mut contents: Vec<String> = wiki
                .fact_list(FactListOpts {
                    limit: None,
                    ..Default::default()
                })
                .unwrap()
                .into_iter()
                .map(|fact| fact.content)
                .collect();
            contents.sort();
            contents
        };
        assert_eq!(fact_contents(&redb), fact_contents(&sqlite));

        let traverse_names = |wiki: &AideMemo| {
            let mut names: Vec<String> = wiki
                .traverse(
                    "Sentinel",
                    TraverseOpts {
                        depth: 1,
                        direction: TraverseDirection::Forward,
                        relation_types: None,
                    },
                )
                .unwrap()
                .entities
                .into_iter()
                .map(|entity| entity.name)
                .collect();
            names.sort();
            names
        };
        assert_eq!(traverse_names(&redb), traverse_names(&sqlite));

        let search_contents = |wiki: &AideMemo| {
            wiki.search(
                "hot cache",
                SearchOpts {
                    limit: Some(3),
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|hit| hit.content)
            .collect::<Vec<_>>()
        };
        assert_eq!(search_contents(&redb), search_contents(&sqlite));
    }

    #[cfg(feature = "redb")]
    #[test]
    fn sqlite_import_preserves_redb_export_ids_for_migration_gate() {
        let dir = tempdir().unwrap();
        let redb_path = dir.path().join("source.redb");
        let sqlite_path = dir.path().join("target.sqlite");

        let mut redb_config = Config::default();
        redb_config.store.backend = "redb".to_string();
        redb_config.store.path = redb_path.to_string_lossy().into_owned();
        redb_config.search.semantic_weight = 0.0;
        let redb = AideMemo::open(&redb_path, redb_config).unwrap();
        seed_backend(&redb);

        let mut exported = Vec::new();
        redb.export_jsonl(&mut exported, ExportScope::All)
            .expect("redb export");

        let mut sqlite_config = Config::default();
        sqlite_config.store.backend = "sqlite".to_string();
        sqlite_config.store.path = sqlite_path.to_string_lossy().into_owned();
        sqlite_config.search.semantic_weight = 0.0;
        let mut sqlite = AideMemo::open(&sqlite_path, sqlite_config).unwrap();
        let mut reader = exported.as_slice();
        let import_stats = sqlite.import_jsonl(&mut reader).expect("sqlite import");
        assert_eq!(import_stats.entities_imported, 2);
        assert_eq!(import_stats.facts_imported, 2);
        assert_eq!(import_stats.errors, 0);

        let redb_stats = redb.stats().unwrap();
        let sqlite_stats = sqlite.stats().unwrap();
        assert_eq!(sqlite_stats.entity_count, redb_stats.entity_count);
        assert_eq!(sqlite_stats.fact_count, redb_stats.fact_count);
        assert_eq!(sqlite_stats.relation_count, redb_stats.relation_count);

        for summary in redb.entity_list(ListOpts::default()).unwrap() {
            let redb_entity = redb.entity_get_by_id(summary.id).unwrap();
            let sqlite_entity = sqlite.entity_get_by_id(summary.id).unwrap();
            assert_eq!(sqlite_entity.name, redb_entity.name);
            assert_eq!(sqlite_entity.aliases, redb_entity.aliases);
        }

        for fact in redb.fact_list(FactListOpts::default()).unwrap() {
            let imported = sqlite.fact_get(&fact.id).unwrap();
            assert_eq!(imported.content, fact.content);
            assert_eq!(imported.entity_ids, fact.entity_ids);
            assert_eq!(imported.fact_type, fact.fact_type);
        }

        let traverse_names = |wiki: &AideMemo| {
            let mut names: Vec<String> = wiki
                .traverse(
                    "Sentinel",
                    TraverseOpts {
                        depth: 1,
                        direction: TraverseDirection::Forward,
                        relation_types: None,
                    },
                )
                .unwrap()
                .entities
                .into_iter()
                .map(|entity| entity.name)
                .collect();
            names.sort();
            names
        };
        assert_eq!(traverse_names(&redb), traverse_names(&sqlite));

        let search_contents = |wiki: &AideMemo| {
            wiki.search(
                "hot cache",
                SearchOpts {
                    limit: Some(3),
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|hit| hit.content)
            .collect::<Vec<_>>()
        };
        assert_eq!(search_contents(&redb), search_contents(&sqlite));

        let mut replay = exported.as_slice();
        let replay_stats = sqlite.import_jsonl(&mut replay).expect("re-import");
        assert_eq!(replay_stats.entities_imported, 0);
        assert_eq!(replay_stats.relations_imported, 0);
        assert_eq!(replay_stats.facts_imported, 0);
        assert_eq!(replay_stats.errors, 0);
    }
}

#[cfg(all(test, feature = "semantic"))]
mod query_tests {
    use super::*;
    use tempfile::tempdir;

    fn fresh_wiki() -> (AideMemo, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let wiki = AideMemo::open(&dir.path().join("test.sqlite"), Config::default()).unwrap();
        (wiki, dir)
    }

    // Loads the configured embedding model from HuggingFace on first
    // run. Skipped in CI to avoid network + concurrent-blob-lock
    // races; run locally with `cargo test -- --ignored`.
    #[test]
    #[ignore = "downloads HF model — local only"]
    fn query_resolves_entity_and_returns_search_traverse_recent() {
        let (wiki, _dir) = fresh_wiki();

        wiki.entity_add(EntityInput {
            name: "Redis".into(),
            entity_type: Some(EntityType::Technology),
            tags: Some(vec!["cache".into()]),
            ..Default::default()
        })
        .unwrap();
        let redis_id = wiki.resolve_entity("Redis").unwrap();

        wiki.add_fact(FactInput {
            content: "Redis Sentinel provides high availability".into(),
            fact_type: Some(FactType::Decision),
            entity_ids: Some(vec![redis_id]),
            tags: None,
            source: None,
            source_id: None,
            source_confidence: None,
            observed_at: None,
        })
        .unwrap();

        let result = wiki.query("Redis", QueryOpts::default()).unwrap();

        assert_eq!(result.topic, "Redis");
        assert!(result.entity.is_some());
        assert_eq!(result.entity.as_ref().unwrap().name, "Redis");
        assert!(
            !result.recent_facts.is_empty(),
            "expected recent facts for Redis"
        );
    }

    #[test]
    #[ignore = "downloads HF model — local only"]
    fn query_unknown_topic_returns_search_only() {
        let (wiki, _dir) = fresh_wiki();
        let result = wiki
            .query("nonexistent-topic-xyz", QueryOpts::default())
            .unwrap();
        assert!(result.entity.is_none());
        assert!(result.related.is_empty());
        assert!(result.recent_facts.is_empty());
    }
}
