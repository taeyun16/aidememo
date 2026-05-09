//! WikiGraph — Structured index engine for LLM wikis.

pub mod adapt;
pub mod archive;
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
pub mod store;
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
pub use error::{Result, WgError};
pub use ingest::{IngestStats, ParsedFile, Section, Wikilink};

// Re-export ulid for external use
pub use ulid;

// Re-export store and graph components
use graph::Graph;
use store::Store;

/// WikiGraph instance.
///
/// Thread-safe, can be shared across multiple operations.
pub struct WikiGraph {
    // Use interior mutability pattern - Store itself uses Arc<Database>
    // For mutable operations, we use RwLock
    store: Arc<RwLock<Store>>,
    /// Absolute path to the redb file. Captured at `open` time so
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
    /// from `wiki.hnsw.bin` next to the redb store on first
    /// search; rebuilt on demand via `vector_index_rebuild()` or
    /// when the sidecar's model name doesn't match the active
    /// provider. `None` means "not built yet" → fall back to the
    /// BM25-prefilter path so the system still works.
    #[cfg(feature = "semantic")]
    vector_index: Arc<parking_lot::RwLock<Option<vector_index::HnswIndex>>>,
    /// Cached BM25 inverted index, shared across every search on
    /// this `WikiGraph`. Without this each `hybrid_search` call
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
    /// include_archive search call. None on cold WikiGraphs (an
    /// archive doesn't get its own archive — invariant) and on
    /// hot WikiGraphs that have never archived anything yet.
    cold_sibling: parking_lot::Mutex<Option<Arc<WikiGraph>>>,
    /// True for cold-tier WikiGraphs so they refuse to recursively
    /// open another cold (would create `<x>.cold.redb.cold.redb`).
    is_cold: bool,
}

impl WikiGraph {
    /// Access the store (read-only or write via RwLock).
    pub fn store(&self) -> &Arc<RwLock<Store>> {
        &self.store
    }
}

impl WikiGraph {
    // === Lifecycle ===

    /// Open or create a WikiGraph store at the given path.
    pub fn open(path: &Path, config: Config) -> Result<Self> {
        Self::open_inner(path, config, false)
    }

    fn open_inner(path: &Path, config: Config, is_cold: bool) -> Result<Self> {
        let store = Store::open(path, config.clone())?;
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

    /// Lazy-open the cold-tier sibling WikiGraph. Returns the same
    /// `Arc<WikiGraph>` on every call; opens the cold redb file
    /// and runs schema init the first time. Returns `None` when
    /// called on a cold WikiGraph (cold's-cold is forbidden).
    pub fn cold(&self) -> Result<Option<Arc<WikiGraph>>> {
        if self.is_cold {
            return Ok(None);
        }
        let mut guard = self.cold_sibling.lock();
        if let Some(c) = guard.as_ref() {
            return Ok(Some(c.clone()));
        }
        let cold_path = {
            let store = self.store.read();
            archive::cold_path_for(std::path::Path::new(&store.config().store.path))
        };
        let mut cfg = (*self.config).clone();
        cfg.store.path = cold_path.to_string_lossy().into_owned();
        let cold = WikiGraph::open_inner(&cold_path, cfg, true)?;
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

    /// Lazy-load (or return) the embedding provider. Cached for the
    /// lifetime of the WikiGraph.
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

    /// Where the HNSW sidecar lives. Sits next to the redb store
    /// (using the path the caller passed to `open`, not the config
    /// path which may be a default placeholder).
    #[cfg(feature = "semantic")]
    fn hnsw_sidecar_path(&self) -> std::path::PathBuf {
        self.store_path.with_extension("hnsw.bin")
    }

    /// Embed an arbitrary string via the configured provider.
    /// Public surface so callers (e.g. the bench harness) can use the
    /// same model wg uses for hybrid search to score per-fact relevance
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

    /// Close the WikiGraph store.
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
    /// See `crates/wg-core/src/extract.rs` for the scoring rules.
    pub fn extract_candidates(
        &self,
        text: &str,
        max_candidates: usize,
    ) -> Result<Vec<extract::ExtractCandidate>> {
        extract::extract_candidates(text, &self.store.read(), max_candidates)
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
        extract::extract_candidates_llm(
            text,
            &self.store.read(),
            &self.config.extract,
            max_candidates,
        )
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

    /// Insert N facts in one redb write transaction. Use this for
    /// bulk imports — the per-commit fsync (≈3-5 ms on macOS APFS)
    /// is paid once for the batch instead of once per fact.
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
        self.store.read().fact_get(id)
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

    /// Move facts from the hot store to the cold-tier archive
    /// (`<store>.cold.redb`). Returns the number actually moved
    /// (silently skips ids not in hot — they may already be archived).
    /// See `crates/wg-core/src/archive.rs` for the design notes
    /// (cold preserves FactId, content-hash dedup, hot delete after
    /// cold commit). After this returns, BM25 / semantic indexes are
    /// marked dirty so the next search rebuilds against the smaller
    /// hot pool. Cold-side index update lands in stage 3.
    pub fn archive_facts(&self, fact_ids: &[FactId]) -> Result<usize> {
        if self.is_cold {
            return Err(WgError::InvalidInput(
                "archive_facts called on a cold WikiGraph (no nested archives)".into(),
            ));
        }
        let moved = {
            let mut store = self.store.write();
            store.archive_facts(fact_ids)?
        };
        if moved > 0 {
            self.bm25_mark_dirty();
            // Cold's BM25 / semantic indexes need a rebuild too — the
            // raw inserts that cold_insert_archived does don't go
            // through the regular fact_add path that flips the dirty
            // bit. Open the sibling lazily so single-archive workflows
            // don't pay the open cost when search-with-archive isn't
            // used.
            if let Ok(Some(cold)) = self.cold() {
                cold.bm25_mark_dirty();
            }
        }
        Ok(moved)
    }

    /// Mark `old_id` as superseded by `new_id`. Sets `old.superseded_at = now`
    /// and `old.superseded_by = new_id`. Errors if either ID doesn't exist or
    /// `old_id` is already superseded.
    pub fn fact_supersede(&self, old_id: &FactId, new_id: &FactId) -> Result<()> {
        let old = self.fact_get(old_id)?;
        if old.superseded_at.is_some() {
            return Err(WgError::InvalidInput(format!(
                "fact {old_id} already superseded"
            )));
        }
        // Verify the new fact exists.
        let _ = self.fact_get(new_id)?;
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

    /// Count facts with `superseded_at` set. Used by `wg doctor` to
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
    /// Idempotent — re-running just refreshes existing edges (the redb
    /// key is `{source}\0{rel_type}\0{target}` so writes overwrite).
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
        // redb per pair. Set of (source_id, target_id) regardless of
        // rel_type — wg's auto-relate refuses to overwrite a richer
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
        if opts.semantic_threshold <= 0.0 {
            return Ok(stats);
        }

        let facts = self.fact_list(types::FactListOpts {
            current_only: true,
            limit: None,
            ..Default::default()
        })?;
        stats.facts_processed = facts.len();
        if facts.len() < 2 {
            return Ok(stats);
        }

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
        let mut superseded: HashSet<types::FactId> = HashSet::new();
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
            return Err(WgError::InvalidInput(format!(
                "GacOpts.theta must be in (0, 1], got {}",
                opts.theta
            )));
        }

        let facts = self.fact_list(types::FactListOpts {
            current_only: true,
            limit: None,
            ..Default::default()
        })?;
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
            for i in 0..m {
                if survivors.contains(&i) {
                    continue;
                }
                if opts.use_cold_tier {
                    archive_losers.push(members[i]);
                } else {
                    supersede_losers.push(members[i]);
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
        let graph = Graph::new(&store);
        graph.traverse(start, opts)
    }

    /// Find a path between two entities.
    pub fn path_find(&self, from: &str, to: &str) -> Result<Option<Vec<PathStep>>> {
        let store = self.store.read();
        let graph = Graph::new(&store);
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
            ingest::ingest_wiki(wiki_root, &mut store, incremental)?
        };
        self.bm25_mark_dirty();
        // Auto-rebuild the HNSW index if the user opted into the
        // "hnsw" semantic path. Failure is non-fatal — the BM25
        // fallback in hybrid_search will still serve results, and
        // operators can retry with `wg vector-rebuild` or by
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
            let engine = SearchEngine::new(&store, &self.config, &self.bm25_index);
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
        // Operators can run `wg vector-rebuild` to fix the sidecar.
        let mut results = if self.config.search.semantic_index == "hnsw" && {
            let _ = self.vector_index_get(provider.as_ref());
            self.vector_index.read().is_some()
        } {
            let guard = self.vector_index.read();
            let idx = guard.as_ref().expect("checked is_some above");
            let store = self.store.read();
            search::hybrid_search_with_hnsw(
                &store,
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
                     falling back to BM25 prefilter. Run `wg vector-rebuild`.",
                );
            }
            let store = self.store.read();
            search::hybrid_search_with_ctx(
                &store,
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
            let cold_opts = SearchOpts {
                limit: Some(user_limit),
                bm25_only: false,
                include_archive: false,
                ..SearchOpts::default()
            };
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
        let engine = SearchEngine::new(&store, &self.config, &self.bm25_index);
        engine.search_with_traverse(query, start, depth, opts)
    }

    /// Search (BM25 only, no semantic features).
    #[cfg(not(feature = "semantic"))]
    pub fn search(&self, _query: &str, _opts: SearchOpts) -> Result<Vec<SearchResult>> {
        Err(WgError::SearchFailed(
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

    // === Lint ===

    /// Run graph health checks.
    pub fn lint(&self) -> Result<Vec<LintIssue>> {
        use crate::lint::LintEngine;
        let store = self.store.read();
        let engine = LintEngine::new(&store);
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
        let exporter = Exporter::new(&store);
        exporter.export_jsonl(writer, scope)
    }

    /// Import data from JSONL.
    pub fn import_jsonl(&mut self, reader: &mut dyn std::io::Read) -> Result<ImportStats> {
        use crate::migrate::Importer;
        let mut store = self.store.write();
        let mut importer = Importer::new(&mut store);
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
};

#[cfg(feature = "semantic")]
pub use types::VectorRecord;

#[cfg(all(test, feature = "semantic"))]
mod query_tests {
    use super::*;
    use tempfile::tempdir;

    fn fresh_wiki() -> (WikiGraph, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let wiki = WikiGraph::open(&dir.path().join("test.redb"), Config::default()).unwrap();
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
