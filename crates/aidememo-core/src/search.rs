//! Search engine for AideMemo.
//!
//! Provides BM25 keyword search and hybrid semantic search.

use crate::backend::StoreBackend;
use crate::config::Config;
use crate::error::Result;
use crate::graph::Graph;
use crate::index::{Bm25IndexState, build_bm25_index};
use crate::types::*;
use parking_lot::RwLock;

/// Search engine for AideMemo.
///
/// Borrows its BM25 state from the caller (typically `AideMemo`),
/// so multiple `SearchEngine` instances created during a single
/// `AideMemo`'s lifetime share the same cached inverted index.
/// `ensure_index` rebuilds only when the cache is marked dirty by a
/// preceding mutation.
pub struct SearchEngine<'a, B: StoreBackend + ?Sized> {
    store: &'a B,
    config: &'a Config,
    index: &'a RwLock<Bm25IndexState>,
}

impl<'a, B: StoreBackend + ?Sized> SearchEngine<'a, B> {
    /// Create a new search engine bound to a caller-owned BM25 cache.
    pub fn new(store: &'a B, config: &'a Config, index: &'a RwLock<Bm25IndexState>) -> Self {
        Self {
            store,
            config,
            index,
        }
    }

    /// Rebuild the index if dirty.
    fn ensure_index(&self) {
        let mut index = self.index.write();
        if index.dirty {
            *index = build_bm25_index(self.store);
        }
    }

    /// Search for facts matching a query.
    pub fn search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>> {
        self.ensure_index();

        let limit = opts.limit.unwrap_or(self.config.search.default_limit);
        let min_confidence = opts.min_confidence.unwrap_or(self.config.search.min_trust);

        let candidate_limit = limit.max(limit.saturating_mul(8).min(512));
        let index = self.index.read();
        let bm25_results: Vec<bm25::SearchResult<FactId>> =
            index.engine.search(query, candidate_limit);

        let mut results = Vec::new();

        for (rank, bm25_result) in bm25_results.into_iter().enumerate() {
            let fact_id = bm25_result.document.id;
            let score = bm25_result.score;

            if let Ok(fact) = self.store.fact_get(&fact_id) {
                if fact.source_confidence < min_confidence {
                    continue;
                }

                if !matches_entity_filter(&fact, opts.entity_filter.as_ref()) {
                    continue;
                }

                if !matches_source_id(&fact, opts.source_id.as_ref()) {
                    continue;
                }

                if !matches_time_window(&fact, opts.since, opts.until) {
                    continue;
                }

                if !matches_as_of(&fact, opts.as_of) {
                    continue;
                }

                if opts.current_only && fact.superseded_at.is_some() {
                    continue;
                }

                #[cfg(feature = "semantic")]
                {
                    results.push(build_search_result(
                        self.store,
                        fact,
                        fact_id,
                        score,
                        rank + 1,
                        opts.session_id.clone(),
                    ));
                }
                #[cfg(not(feature = "semantic"))]
                {
                    results.push(build_search_result(
                        self.store,
                        fact,
                        fact_id,
                        score,
                        rank + 1,
                    ));
                }
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.content.cmp(&b.content))
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.source_id.cmp(&b.source_id))
                .then_with(|| a.fact_type.to_string().cmp(&b.fact_type.to_string()))
                .then_with(|| a.entity_names.cmp(&b.entity_names))
                .then_with(|| a.observed_at.cmp(&b.observed_at))
                .then_with(|| a.fact_id.0.cmp(&b.fact_id.0))
        });
        for (rank, result) in results.iter_mut().enumerate() {
            result.rank = rank + 1;
        }
        results.truncate(limit);

        Ok(results)
    }

    /// Search with graph traversal scope.
    pub fn search_with_traverse(
        &self,
        query: &str,
        start: &str,
        depth: u32,
        opts: SearchOpts,
    ) -> Result<Vec<SearchResult>> {
        let traverse_result = Graph::new(self.store).traverse(
            start,
            TraverseOpts {
                depth,
                relation_types: None,
                direction: TraverseDirection::Forward,
            },
        )?;

        let entity_ids: Vec<EntityId> = traverse_result.entities.iter().map(|e| e.id).collect();

        let mut search_opts = opts;
        search_opts.entity_filter = Some(entity_ids);

        self.search(query, search_opts)
    }
}

fn matches_entity_filter(fact: &FactRecord, entity_filter: Option<&Vec<EntityId>>) -> bool {
    match entity_filter {
        Some(filter) => fact.entity_ids.iter().any(|eid| filter.contains(eid)),
        None => true,
    }
}

fn matches_source_id(fact: &FactRecord, source_id: Option<&String>) -> bool {
    match source_id {
        Some(source_id) => fact.source_id.as_deref() == Some(source_id.as_str()),
        None => true,
    }
}

/// Check whether a fact's timestamp falls within `[since, until]` (inclusive).
/// Prefers `observed_at` (real-world time) over `created_at` (DB insertion).
fn matches_time_window(fact: &FactRecord, since: Option<u64>, until: Option<u64>) -> bool {
    if since.is_none() && until.is_none() {
        return true;
    }
    let ts = fact.observed_at.unwrap_or(fact.created_at);
    if let Some(s) = since
        && ts < s
    {
        return false;
    }
    if let Some(u) = until
        && ts > u
    {
        return false;
    }
    true
}

/// "As of" check — fact must have existed and still been current at
/// `as_of`. Used by `--as-of` queries to walk back the timeline
/// without manually following supersede chains.
fn matches_as_of(fact: &FactRecord, as_of: Option<u64>) -> bool {
    let Some(as_of) = as_of else { return true };
    if fact.created_at > as_of {
        return false;
    }
    if let Some(superseded_at) = fact.superseded_at
        && superseded_at <= as_of
    {
        return false;
    }
    true
}

#[cfg(feature = "semantic")]
fn build_search_result(
    store: &(impl StoreBackend + ?Sized),
    fact: FactRecord,
    fact_id: FactId,
    score: f32,
    rank: usize,
    session_id: Option<String>,
) -> SearchResult {
    let entity_names: Vec<String> = fact
        .entity_ids
        .iter()
        .filter_map(|eid| store.entity_get_by_id(*eid).ok())
        .map(|e| e.name)
        .collect();

    SearchResult {
        fact_id,
        content: fact.content,
        fact_type: fact.fact_type,
        entity_names,
        source: fact.source,
        source_id: fact.source_id,
        actor_id: fact.actor_id,
        score,
        rank,
        created_at: fact.created_at,
        observed_at: fact.observed_at,
        session_id,
    }
}

#[cfg(not(feature = "semantic"))]
fn build_search_result(
    store: &(impl StoreBackend + ?Sized),
    fact: FactRecord,
    fact_id: FactId,
    score: f32,
    rank: usize,
) -> SearchResult {
    let entity_names: Vec<String> = fact
        .entity_ids
        .iter()
        .filter_map(|eid| store.entity_get_by_id(*eid).ok())
        .map(|e| e.name)
        .collect();

    SearchResult {
        fact_id,
        content: fact.content,
        fact_type: fact.fact_type,
        entity_names,
        source: fact.source,
        source_id: fact.source_id,
        actor_id: fact.actor_id,
        score,
        rank,
        created_at: fact.created_at,
        observed_at: fact.observed_at,
    }
}

/// Tier 7-C: fact embedding stored as i8 + a per-vector scale.
///
/// Per-vector scaling lets us keep the dynamic range of each fact
/// independent (some embeddings have outlier dimensions). Cosine
/// is invariant to positive scale, so as long as we apply the same
/// quantization to the query side, similarity rankings are preserved
/// up to quantization noise.
#[cfg(feature = "semantic")]
#[derive(Debug, Clone)]
pub struct QuantizedEmbedding {
    pub data: Vec<i8>,
}

#[cfg(feature = "semantic")]
impl QuantizedEmbedding {
    /// Quantize an f32 vector to i8 using max-abs scaling. Empty input
    /// yields an empty vector.
    pub fn from_f32(v: &[f32]) -> Self {
        if v.is_empty() {
            return Self { data: Vec::new() };
        }
        let max = v.iter().fold(0f32, |acc, x| acc.max(x.abs()));
        if max <= 0.0 {
            return Self {
                data: vec![0; v.len()],
            };
        }
        let scale = 127.0 / max;
        let data = v
            .iter()
            .map(|x| (x * scale).round().clamp(-127.0, 127.0) as i8)
            .collect();
        Self { data }
    }
}

#[cfg(feature = "semantic")]
mod semantic {
    use super::*;
    use crate::embedding::EmbeddingProvider;
    use lru::LruCache;
    use parking_lot::{Mutex, RwLock};
    use std::cmp::Ordering;
    use std::collections::{HashMap, HashSet};

    /// Cosine similarity between two i8 vectors via simsimd. Returns 0.0
    /// on size mismatch or empty input. Range mirrors the f32 path:
    /// 0 (orthogonal) → 1 (identical).
    fn cosine_i8(a: &[i8], b: &[i8]) -> f32 {
        use simsimd::SpatialSimilarity;
        if a.is_empty() || b.is_empty() || a.len() != b.len() {
            return 0.0;
        }
        match i8::cosine(a, b) {
            Some(distance) => (1.0 - distance) as f32,
            None => 0.0,
        }
    }

    /// Hybrid search combining BM25 and semantic vectors.
    ///
    /// Loads a fresh provider on every call. Prefer `hybrid_search_with_ctx`
    /// from `AideMemo::hybrid_search`, which reuses a singleton provider +
    /// query-embedding cache. This convenience wrapper exists for tests and
    /// one-off callers (bindings, scripts) that don't want to plumb a context.
    pub fn hybrid_search<B: StoreBackend + ?Sized>(
        store: &B,
        query: &str,
        opts: SearchOpts,
    ) -> Result<Vec<SearchResult>> {
        let provider = crate::embedding::load_provider(store.config())?;
        let cache = Mutex::new(LruCache::new(
            std::num::NonZeroUsize::new(8).expect("non-zero"),
        ));
        let fact_cache = RwLock::new(HashMap::new());
        let bm25_index = RwLock::new(Bm25IndexState::new());
        hybrid_search_with_ctx(
            store,
            query,
            opts,
            &*provider,
            &cache,
            &fact_cache,
            &bm25_index,
        )
    }

    /// Hybrid search with a caller-owned provider + query-embedding cache.
    /// Used by `AideMemo` to avoid reloading the Model2Vec model on every
    /// search and to memoize repeated queries.
    pub fn hybrid_search_with_ctx<B: StoreBackend + ?Sized>(
        store: &B,
        query: &str,
        opts: SearchOpts,
        provider: &dyn EmbeddingProvider,
        query_cache: &Mutex<LruCache<String, Vec<f32>>>,
        fact_cache: &RwLock<HashMap<FactId, QuantizedEmbedding>>,
        bm25_index: &RwLock<Bm25IndexState>,
    ) -> Result<Vec<SearchResult>> {
        // Phase timings: emitted as DEBUG-level tracing events so they
        // show up under `RUST_LOG=aidememo_core=debug aidememo search …`. The
        // legacy AIDEMEMO_SEARCH_PROFILE env still works (it eprintln's the
        // same numbers for users who want a self-contained dump
        // without configuring a tracing subscriber).
        let profile = std::env::var("AIDEMEMO_SEARCH_PROFILE").is_ok();
        let phase = |label: &str, t0: std::time::Instant| {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            tracing::debug!(scope = "bm25-prefilter", phase = label, ms, "phase");
            if profile {
                eprintln!("[search/bm25-prefilter] {label}: {ms:.2}ms");
            }
        };

        let config = store.config();
        let engine = SearchEngine::new(store, config, bm25_index);

        // Tier 7-B: pull a wider BM25 candidate slate than the final
        // limit. The semantic re-ranker scores only these candidates,
        // capping per-query embedding inference at `semantic_prefilter`
        // facts instead of the whole store. Final RRF fusion still uses
        // BM25 results too, so any candidate ranked highly by either
        // signal can win.
        let limit = opts.limit.unwrap_or(config.search.default_limit);
        let prefilter = config.search.semantic_prefilter;
        let bm25_opts = SearchOpts {
            limit: Some(limit.max(prefilter).max(1)),
            ..opts.clone()
        };
        let t = std::time::Instant::now();
        let bm25_results = engine.search(query, bm25_opts)?;
        phase("bm25_search", t);

        // Tier 7-D: graph-aware prefilter. Take the entities surfaced
        // by BM25 hits, walk N hops outward, and pull facts attached
        // to those neighbors into the candidate pool. aidememo's graph is
        // the differentiator — semantic neighborhoods + relation
        // structure together catch matches that pure keyword overlap
        // misses (e.g. a fact about "PostgreSQL replication" is a
        // strong candidate when the user searched for "Redis" and
        // those entities are linked in the graph).
        let t = std::time::Instant::now();
        let semantic_candidates: Option<Vec<FactId>> = if prefilter > 0 {
            let mut ids: Vec<FactId> = bm25_results
                .iter()
                .take(prefilter)
                .map(|r| r.fact_id)
                .collect();
            if config.search.graph_prefilter {
                let extra = graph_expand_candidates(
                    store,
                    &bm25_results,
                    config.search.graph_depth,
                    config.search.graph_fact_cap,
                    &ids,
                )?;
                ids.extend(extra);
            }
            Some(ids)
        } else {
            None
        };
        phase("graph_prefilter", t);

        let t = std::time::Instant::now();
        let semantic_results = semantic_search(
            store,
            query,
            &opts,
            provider,
            query_cache,
            fact_cache,
            semantic_candidates.as_deref(),
        )?;
        phase("semantic_search", t);

        let bm25_weight = effective_weight(opts.bm25_weight, config.search.bm25_weight);
        let semantic_weight = effective_weight(opts.semantic_weight, config.search.semantic_weight);

        let t = std::time::Instant::now();
        let fused = rrf_fusion(
            store,
            &bm25_results,
            Some(semantic_results.as_slice()),
            bm25_weight,
            semantic_weight,
            limit,
        );
        phase("rrf_fusion", t);

        Ok(fused)
    }

    /// HNSW-backed hybrid search. Replaces the BM25 prefilter step
    /// with a vector-index lookup; everything downstream (semantic
    /// re-rank, RRF, graph prefilter) still applies. Same accuracy
    /// as `prefilter=0` (brute force) at lower latency, which is
    /// the whole point.
    pub fn hybrid_search_with_hnsw<B: StoreBackend + ?Sized>(
        store: &B,
        query: &str,
        opts: SearchOpts,
        provider: &dyn EmbeddingProvider,
        query_cache: &Mutex<LruCache<String, Vec<f32>>>,
        index: &crate::vector_index::HnswIndex,
        bm25_index: &RwLock<Bm25IndexState>,
    ) -> Result<Vec<SearchResult>> {
        // Phase timings — same scope name as hybrid_search_with_ctx
        // so users grepping `RUST_LOG=aidememo_core=debug` see one
        // consistent label for "the BM25/semantic blend ran here."
        let phase = |label: &str, t0: std::time::Instant| {
            tracing::debug!(
                scope = "hnsw-hybrid",
                phase = label,
                ms = t0.elapsed().as_secs_f64() * 1000.0,
                "phase",
            );
        };

        let config = store.config();
        let engine = SearchEngine::new(store, config, bm25_index);

        let limit = opts.limit.unwrap_or(config.search.default_limit);
        // We still run BM25 — its scores feed into RRF fusion alongside
        // semantic. HNSW only changes which facts the *semantic* side
        // scores, not which facts BM25 nominates.
        let bm25_opts = SearchOpts {
            limit: Some(limit),
            ..opts.clone()
        };
        let t = std::time::Instant::now();
        let bm25_results = engine.search(query, bm25_opts)?;
        phase("bm25", t);

        // Embed the query (cached) and pull top candidates from the
        // index. We over-fetch (cap × 2) to mirror the BM25 path's
        // habit of pulling more than `limit` so the re-rank has
        // headroom.
        let t = std::time::Instant::now();
        let query_embedding = {
            let mut cache = query_cache.lock();
            if let Some(v) = cache.get(query) {
                v.clone()
            } else {
                let v = provider.embed_query(query)?;
                cache.put(query.to_string(), v.clone());
                v
            }
        };
        phase("query_embed", t);
        let mut q_norm = query_embedding.clone();
        crate::vector_index::l2_normalize(&mut q_norm);
        let cap = config.search.semantic_prefilter.max(limit) * 2;
        let t = std::time::Instant::now();
        let hnsw_ids = index.search(&q_norm, cap);
        phase("hnsw_lookup", t);

        // Run the existing semantic_search on this candidate slate.
        // We reuse the same fact_embed_cache (it doubles as a query
        // cache for repeated facts) — but pass an empty one because
        // the HNSW path is the authoritative ranking and we don't
        // want quantized re-scoring to perturb it. The semantic
        // step still walks the candidates and scores them so that
        // RRF fusion has consistent ranks.
        let empty_cache: RwLock<HashMap<FactId, QuantizedEmbedding>> = RwLock::new(HashMap::new());
        let semantic_results = semantic_search(
            store,
            query,
            &opts,
            provider,
            query_cache,
            &empty_cache,
            Some(&hnsw_ids),
        )?;

        let bm25_weight = effective_weight(opts.bm25_weight, config.search.bm25_weight);
        let semantic_weight = effective_weight(opts.semantic_weight, config.search.semantic_weight);

        Ok(rrf_fusion(
            store,
            &bm25_results,
            Some(semantic_results.as_slice()),
            bm25_weight,
            semantic_weight,
            limit,
        ))
    }

    fn semantic_search<B: StoreBackend + ?Sized>(
        store: &B,
        query: &str,
        opts: &SearchOpts,
        provider: &dyn EmbeddingProvider,
        query_cache: &Mutex<LruCache<String, Vec<f32>>>,
        fact_cache: &RwLock<HashMap<FactId, QuantizedEmbedding>>,
        candidates: Option<&[FactId]>,
    ) -> Result<Vec<SearchResult>> {
        let config = store.config();
        // LRU hit: Model2Vec inference for the query is the second-most
        // expensive op after fact embedding; for hot queries (LLM agents
        // often repeat the same topic across turns) caching pays off
        // immediately.
        let query_embedding = {
            let mut cache = query_cache.lock();
            if let Some(v) = cache.get(query) {
                v.clone()
            } else {
                let v = provider.embed_query(query)?;
                cache.put(query.to_string(), v.clone());
                v
            }
        };

        // Tier 7-B: when BM25 has narrowed the universe down to a
        // candidate slate, hydrate just those FactRecords. Otherwise
        // fall back to scanning every fact (maintains old behavior
        // when prefilter=0 or when no candidates are available).
        // `fact_get_many` opens a single redb read txn for the whole
        // slate — a per-call `fact_get` loop paid ~20 µs of txn
        // overhead per candidate (~2 ms total at typical 64-fact
        // prefilter sizes).
        let mut facts: Vec<FactRecord> = match candidates {
            Some(ids) if !ids.is_empty() => store.fact_get_many(ids)?,
            _ => store.fact_list(FactListOpts {
                limit: None,
                offset: 0,
                since: opts.since,
                until: opts.until,
                current_only: opts.current_only,
                as_of: opts.as_of,
                source_id: opts.source_id.clone(),
                ..Default::default()
            })?,
        };

        let min_confidence = opts.min_confidence.unwrap_or(config.search.min_trust);

        facts.retain(|fact| {
            fact.source_confidence >= min_confidence
                && matches_entity_filter(fact, opts.entity_filter.as_ref())
                && matches_source_id(fact, opts.source_id.as_ref())
                && (opts.since.is_none() && opts.until.is_none()
                    || matches_time_window(fact, opts.since, opts.until))
                && matches_as_of(fact, opts.as_of)
                && (!opts.current_only || fact.superseded_at.is_none())
        });

        if facts.is_empty() {
            return Ok(Vec::new());
        }

        // Tier 6-B: batch-fetch every entity referenced by any fact in this
        // search ONCE, into a HashMap. Without this we'd hit redb N×M times
        // (N facts × M entity_ids per fact). With it: one read per unique
        // entity, then constant-time lookup in fact_semantic_text.
        let mut unique_eids: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
        for f in &facts {
            for eid in &f.entity_ids {
                unique_eids.insert(*eid);
            }
        }
        let entity_names: std::collections::HashMap<EntityId, String> = unique_eids
            .into_iter()
            .filter_map(|eid| store.entity_get_by_id(eid).ok().map(|e| (eid, e.name)))
            .collect();

        // Tier 7-C: split facts into (a) those with a cached i8 embedding
        // and (b) those that need fresh inference. Only embed the misses,
        // then quantize+store them so subsequent searches are inference-free.
        let q_quantized = QuantizedEmbedding::from_f32(&query_embedding);

        let cached_lookup: HashMap<FactId, QuantizedEmbedding> = {
            let cache = fact_cache.read();
            facts
                .iter()
                .filter_map(|f| cache.get(&f.id).map(|q| (f.id, q.clone())))
                .collect()
        };

        let miss_indices: Vec<usize> = facts
            .iter()
            .enumerate()
            .filter_map(|(idx, f)| {
                if cached_lookup.contains_key(&f.id) {
                    None
                } else {
                    Some(idx)
                }
            })
            .collect();

        if !miss_indices.is_empty() {
            let miss_texts: Vec<String> = miss_indices
                .iter()
                .map(|&idx| fact_semantic_text_cached(&facts[idx], &entity_names))
                .collect();
            let new_embeddings = provider.embed_document_batch(&miss_texts)?;
            let mut writer = fact_cache.write();
            for (slot, embedding) in miss_indices.iter().zip(new_embeddings.iter()) {
                let fact_id = facts[*slot].id;
                writer.insert(fact_id, QuantizedEmbedding::from_f32(embedding));
            }
        }

        let cache_view = fact_cache.read();
        let mut scored: Vec<(usize, f32)> = facts
            .iter()
            .enumerate()
            .filter_map(|(idx, fact)| {
                cache_view
                    .get(&fact.id)
                    .map(|q| (idx, cosine_i8(&q_quantized.data, &q.data)))
            })
            .collect();
        drop(cache_view);

        // Deterministic tiebreak: BM25 score → content lex → fact_id.
        // The previous insertion-order tiebreak (a.0.cmp(&b.0)) depended
        // on store traversal order, which in turn depended on ULID
        // ordering — different across ingests of the same data, yielding
        // bench-time nondeterminism. Falling back to content first keeps
        // semantically-equivalent facts in a stable position.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| facts[a.0].content.cmp(&facts[b.0].content))
                .then_with(|| facts[a.0].id.0.cmp(&facts[b.0].id.0))
        });

        let mut results = Vec::with_capacity(scored.len());
        for (rank, (idx, score)) in scored.into_iter().enumerate() {
            let fact = facts[idx].clone();
            let fact_id = fact.id;
            results.push(build_search_result(
                store,
                fact,
                fact_id,
                score,
                rank + 1,
                None,
            ));
        }

        Ok(results)
    }

    // `load_model` and `embed_text` previously lived here and were used
    // directly by semantic_search. They've been replaced by the
    // `crate::embedding::EmbeddingProvider` trait — keeping a single,
    // pluggable seam (Model2Vec by default; OpenAI-compatible HTTP if
    // `model.provider = "openai"`).
    //
    // If you need the StaticModel directly (e.g. for adapt training),
    // construct a `Model2VecProvider` via `embedding::load_provider()` and
    // downcast — but right now nothing else in the crate needs raw access.

    /// Cached variant — used by `semantic_search` after one bulk
    /// entity fetch. The earlier `fact_semantic_text(store, fact)` would
    /// hit redb once per (fact, entity) pair, which dominated wall time
    /// on larger wikis. This version is pure HashMap lookups.
    fn fact_semantic_text_cached(
        fact: &FactRecord,
        entity_names: &std::collections::HashMap<EntityId, String>,
    ) -> String {
        let mut parts = vec![fact.content.clone()];

        if !fact.tags.is_empty() {
            parts.push(fact.tags.join(" "));
        }

        let names: Vec<&str> = fact
            .entity_ids
            .iter()
            .filter_map(|eid| entity_names.get(eid).map(|s| s.as_str()))
            .collect();
        if !names.is_empty() {
            parts.push(names.join(" "));
        }

        if let Some(source) = &fact.source {
            parts.push(source.clone());
        }

        parts.join("\n")
    }

    /// Tier 7-D: expand the candidate pool with facts attached to
    /// graph neighbors of the BM25 hit set. Caps the output at
    /// `fact_cap` to bound the worst case (a hub entity could
    /// transitively reach the entire graph).
    ///
    /// Returns FactIds **excluding** anything already in `existing`,
    /// so the caller can append without dedup work downstream.
    fn graph_expand_candidates<B: StoreBackend + ?Sized>(
        store: &B,
        bm25_results: &[SearchResult],
        depth: u32,
        fact_cap: usize,
        existing: &[FactId],
    ) -> Result<Vec<FactId>> {
        if depth == 0 || fact_cap == 0 {
            return Ok(Vec::new());
        }

        let already: HashSet<FactId> = existing.iter().copied().collect();

        // 1. Pull the seed entity set from BM25 hits — single batched
        //    fact_get for the whole slate.
        let seed_ids: Vec<FactId> = bm25_results.iter().map(|r| r.fact_id).collect();
        let seed_facts = store.fact_get_many(&seed_ids)?;
        let mut seed_entities: HashSet<EntityId> = HashSet::new();
        for fact in seed_facts {
            for eid in fact.entity_ids {
                seed_entities.insert(eid);
            }
        }
        if seed_entities.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Walk N hops out. Reuses the existing graph BFS rather
        //    than re-implementing it. Direction = Both because
        //    inbound/outbound relations are equally relevant for
        //    augmenting candidates.
        let graph = crate::graph::Graph::new(store);
        let mut neighbor_entities: HashSet<EntityId> = HashSet::new();
        for seed_id in &seed_entities {
            let entity = match store.entity_get_by_id(*seed_id) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let result = graph.traverse(
                &entity.name,
                TraverseOpts {
                    depth,
                    relation_types: None,
                    direction: TraverseDirection::Both,
                },
            )?;
            for e in result.entities {
                neighbor_entities.insert(e.id);
            }
        }

        // 3. Drop seeds — those facts already came in via BM25.
        for seed in &seed_entities {
            neighbor_entities.remove(seed);
        }
        if neighbor_entities.is_empty() {
            return Ok(Vec::new());
        }

        // 4. For each neighbor, pull a bounded slice of its facts.
        //    `per_entity` divides the global cap roughly evenly so
        //    a single hub doesn't monopolize the budget.
        let per_entity = (fact_cap / neighbor_entities.len().max(1)).max(1);
        let mut out: Vec<FactId> = Vec::with_capacity(fact_cap);
        for eid in neighbor_entities {
            if out.len() >= fact_cap {
                break;
            }
            let facts = store.fact_list(FactListOpts {
                entity_id: Some(eid),
                limit: Some(per_entity),
                ..Default::default()
            })?;
            for f in facts {
                if !already.contains(&f.id) && !out.contains(&f.id) {
                    out.push(f.id);
                    if out.len() >= fact_cap {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }

    fn effective_weight(opts_weight: f32, config_weight: f32) -> f32 {
        if opts_weight > 0.0 {
            opts_weight
        } else {
            config_weight
        }
    }

    fn rrf_fusion<B: StoreBackend + ?Sized>(
        store: &B,
        bm25_results: &[SearchResult],
        semantic_results: Option<&[SearchResult]>,
        bm25_weight: f32,
        semantic_weight: f32,
        limit: usize,
    ) -> Vec<SearchResult> {
        use std::collections::HashMap;

        const RRF_K: f32 = 60.0;

        #[derive(Clone)]
        struct FusedEntry {
            result: SearchResult,
            base_score: f32,
            weighted_score: f32,
        }

        let mut scores: HashMap<FactId, FusedEntry> = HashMap::new();

        for result in bm25_results {
            let fused_score = bm25_weight / (RRF_K + result.rank as f32);
            scores
                .entry(result.fact_id)
                .and_modify(|entry| entry.base_score += fused_score)
                .or_insert_with(|| FusedEntry {
                    result: result.clone(),
                    base_score: fused_score,
                    weighted_score: 0.0,
                });
        }

        if let Some(semantic_results) = semantic_results {
            for result in semantic_results {
                let fused_score = semantic_weight / (RRF_K + result.rank as f32);
                scores
                    .entry(result.fact_id)
                    .and_modify(|entry| entry.base_score += fused_score)
                    .or_insert_with(|| FusedEntry {
                        result: result.clone(),
                        base_score: fused_score,
                        weighted_score: 0.0,
                    });
            }
        }

        // Apply per-fact multiplicative weights (confidence × age decay)
        // BEFORE the take(limit) cut. We have to pull every candidate
        // anyway to populate `content` / `entity_names`, so amortising
        // the lookup here costs the same as the old single-pass fetch.
        let now_ms = current_epoch_ms();
        let cfg = store.config();
        let weight_by_confidence = cfg.search.weight_by_confidence;
        let tau_ms = cfg.search.time_decay_tau_ms;
        let type_weights = &cfg.search.fact_type_weights;
        let centrality_w = cfg.search.entity_centrality_weight;
        // Adapter is loaded once per fusion call (cheap — meta_get is a
        // single redb point lookup) and only consulted when training has
        // populated biases. Toggling `search.use_adapter` lets operators
        // run an un-adapted baseline without wiping the persisted state.
        // Gated behind `semantic-adapt` because `Store::load_adapter` and
        // `DomainAdapter` only compile under that feature.
        #[cfg(feature = "semantic-adapt")]
        let adapter: Option<crate::adapt::DomainAdapter> = if cfg.search.use_adapter {
            store.load_adapter().ok().filter(|a| !a.is_empty())
        } else {
            None
        };

        let mut entries: Vec<FusedEntry> = scores
            .into_values()
            .map(|mut entry| {
                let mut weight = 1.0_f32;
                if let Ok(fact) = store.fact_get(&entry.result.fact_id) {
                    if weight_by_confidence {
                        // Floor relevance at 0.1 so a hard "unhelpful"
                        // signal hurts ranking but doesn't bury a fact
                        // entirely (still discoverable when no other
                        // candidate exists).
                        weight *= fact.source_confidence.clamp(0.0, 1.0);
                        weight *= fact.relevance_score.clamp(0.1, 1.0);
                    }
                    let type_key = fact.fact_type.to_string();
                    // Decay-exempt types skip the time multiplier. Long-
                    // lived facts (decisions / conventions / patterns)
                    // shouldn't lose rank just for being old — the
                    // decision is still the decision. Note / question /
                    // claim continue to decay so stale chatter falls
                    // off the top.
                    let exempt = cfg.search.decay_exempt_types.contains(&type_key);
                    if tau_ms > 0 && !exempt {
                        let ts = fact.observed_at.unwrap_or(fact.created_at);
                        let age_ms = now_ms.saturating_sub(ts);
                        let decay = (-(age_ms as f64) / tau_ms as f64).exp() as f32;
                        weight *= decay;
                    }
                    // Per-fact_type ranking multiplier — decisions /
                    // conventions get boosted, questions deprioritised.
                    // Mirrors OMEGA's "decisions / lessons 2× weight"
                    // approach. `fact.fact_type.to_string()` is the same
                    // lowercase form used as the BTreeMap key (set in
                    // SearchConfig::default).
                    if let Some(w) = type_weights.get(&type_key) {
                        weight *= *w;
                    }
                    // Entity centrality boost: multi-fact "hub"
                    // entities (Postgres mentioned in 50 facts) carry
                    // their facts higher than long-tail entities
                    // (Acme corp mentioned once). Inspired by Zep /
                    // Graphiti's central-node ranking. Disabled by
                    // default (entity_centrality_weight = 0.0); turn
                    // on with `aidememo config set
                    // search.entity_centrality_weight 0.2`.
                    if centrality_w > 0.0 {
                        let max_fact_count = fact
                            .entity_ids
                            .iter()
                            .filter_map(|eid| store.count_entity_facts(eid).ok())
                            .max()
                            .unwrap_or(0);
                        if max_fact_count > 0 {
                            let log = (1.0 + max_fact_count as f32).log10();
                            weight *= 1.0 + centrality_w * log;
                        }
                    }
                    // Hydrate display fields while we have the fact in
                    // hand — saves a second `fact_get` later.
                    entry.result.content = fact.content;
                    entry.result.fact_type = fact.fact_type;
                    entry.result.entity_names = fact
                        .entity_ids
                        .iter()
                        .filter_map(|eid| store.entity_get_by_id(*eid).ok())
                        .map(|entity| entity.name)
                        .collect();
                    entry.result.source = fact.source;
                    entry.result.created_at = fact.created_at;
                    entry.result.observed_at = fact.observed_at;
                }
                #[cfg(feature = "semantic-adapt")]
                if let Some(ref adapter) = adapter {
                    weight *= adapter.weight_factor(&entry.result.fact_id.to_string());
                }
                entry.weighted_score = entry.base_score * weight;
                entry
            })
            .collect();

        entries.sort_by(|a, b| {
            b.weighted_score
                .partial_cmp(&a.weighted_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.result.rank.cmp(&b.result.rank))
                .then_with(|| a.result.content.cmp(&b.result.content))
                .then_with(|| a.result.fact_id.0.cmp(&b.result.fact_id.0))
        });

        let mut results = Vec::new();
        for (rank, entry) in entries.into_iter().take(limit).enumerate() {
            let mut result = entry.result;
            result.score = entry.weighted_score;
            result.rank = rank + 1;
            results.push(result);
        }

        results
    }
}

#[cfg(feature = "semantic")]
fn current_epoch_ms() -> u64 {
    // Re-export of the shared helper so the rest of search.rs doesn't
    // need to reach into the time module — and so the AIDEMEMO_NOW_MS pin
    // applies uniformly across hybrid_search + ingest.
    crate::time::current_epoch_ms()
}

// `embed_text` was an explicit fn re-export; embedding is now done
// through `crate::embedding::EmbeddingProvider::embed`. External callers
// that need a one-shot embed should call:
//   let p = aidememo_core::embedding::load_provider(config)?;
//   let v = p.embed(text)?;
#[cfg(feature = "semantic")]
pub use semantic::{hybrid_search, hybrid_search_with_ctx, hybrid_search_with_hnsw};

#[cfg(all(test, any(feature = "sqlite", feature = "redb")))]
mod tests {
    use super::*;
    use crate::backend::{StoreBackend, StoreKind};
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn test_store_path(
        dir: &tempfile::TempDir,
        stem: &str,
        mut config: Config,
    ) -> (PathBuf, Config) {
        if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
            config.store.backend = "redb".to_string();
        }
        let suffix = if config.store.backend == "redb" {
            "redb"
        } else {
            "sqlite"
        };
        let path = dir.path().join(format!("{stem}.{suffix}"));
        config.store.path = path.to_string_lossy().into_owned();
        (path, config)
    }

    fn create_test_store() -> (StoreKind, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let (path, config) = test_store_path(&dir, "test", Config::default());
        let store = StoreKind::open(&path, config).unwrap();
        (store, dir)
    }

    #[test]
    fn test_bm25_search() {
        let (mut store, _dir) = create_test_store();

        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                tags: Some(vec!["cache".to_string(), "infra".to_string()]),
                ..Default::default()
            })
            .unwrap();

        let redis_id = store.resolve_entity("Redis").unwrap();

        store
            .fact_add(FactInput {
                content: "Redis Sentinel provides high availability".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![redis_id]),
                tags: Some(vec!["ha".to_string()]),
                source: Some("entities/redis.md".to_string()),
                source_id: None,
                actor_id: None,
                source_confidence: Some(0.8),
                observed_at: None,
            })
            .unwrap();

        store
            .fact_add(FactInput {
                content: "Redis Cluster provides horizontal scaling".to_string(),
                fact_type: Some(FactType::Pattern),
                entity_ids: Some(vec![redis_id]),
                tags: Some(vec!["scaling".to_string()]),
                source: Some("entities/redis.md".to_string()),
                source_id: None,
                actor_id: None,
                source_confidence: Some(0.7),
                observed_at: None,
            })
            .unwrap();

        let config = Config::default();
        let bm25_state = RwLock::new(Bm25IndexState::new());
        let engine = SearchEngine::new(&store, &config, &bm25_state);

        let results = engine
            .search("high availability", SearchOpts::default())
            .unwrap();
        assert!(!results.is_empty());

        let results = engine
            .search("nonexistent query xyz", SearchOpts::default())
            .unwrap();
        assert!(results.len() <= config.search.default_limit);
    }

    #[test]
    fn bm25_search_filters_by_source_id() {
        let (mut store, _dir) = create_test_store();
        let redis_id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        for (content, source_id) in [
            ("Redis alpha tenant cache policy", "alpha"),
            ("Redis beta tenant cache policy", "beta"),
            ("Redis alpha tenant eviction policy", "alpha"),
        ] {
            store
                .fact_add(FactInput {
                    content: content.to_string(),
                    fact_type: Some(FactType::Note),
                    entity_ids: Some(vec![redis_id]),
                    source_id: Some(source_id.to_string()),
                    source_confidence: Some(1.0),
                    ..Default::default()
                })
                .unwrap();
        }

        let config = Config::default();
        let bm25_state = RwLock::new(Bm25IndexState::new());
        let engine = SearchEngine::new(&store, &config, &bm25_state);

        let alpha = engine
            .search(
                "Redis tenant policy",
                SearchOpts {
                    source_id: Some("alpha".to_string()),
                    limit: Some(10),
                    ..Default::default()
                },
            )
            .unwrap();
        let beta = engine
            .search(
                "Redis tenant policy",
                SearchOpts {
                    source_id: Some("beta".to_string()),
                    limit: Some(10),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(alpha.len(), 2);
        assert!(
            alpha
                .iter()
                .all(|r| r.source_id.as_deref() == Some("alpha"))
        );
        assert_eq!(beta.len(), 1);
        assert!(beta.iter().all(|r| r.source_id.as_deref() == Some("beta")));
    }

    #[test]
    fn bm25_search_tiebreaks_on_stable_fields() {
        let (mut store, _dir) = create_test_store();
        let redis_id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        for content in [
            "zeta redis cache token parity",
            "alpha redis cache token parity",
        ] {
            store
                .fact_add(FactInput {
                    content: content.to_string(),
                    fact_type: Some(FactType::Note),
                    entity_ids: Some(vec![redis_id]),
                    source_confidence: Some(1.0),
                    ..Default::default()
                })
                .unwrap();
        }

        let config = Config::default();
        let bm25_state = RwLock::new(Bm25IndexState::new());
        let engine = SearchEngine::new(&store, &config, &bm25_state);

        let results = engine
            .search(
                "redis cache token parity",
                SearchOpts {
                    limit: Some(2),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "alpha redis cache token parity");
        assert_eq!(results[1].content, "zeta redis cache token parity");
        assert_eq!(results[0].rank, 1);
        assert_eq!(results[1].rank, 2);
    }

    // semantic::hybrid_search initialises the embedding provider
    // even when semantic_index = "bm25", so it triggers a HuggingFace
    // model fetch. Skipped in CI; run locally with
    // `cargo test -p aidememo-core --features semantic -- --ignored`.
    #[cfg(feature = "semantic")]
    #[test]
    #[ignore = "downloads HF model — local only"]
    fn hybrid_search_weights_higher_confidence_higher() {
        // Two facts mention the same query terms; the high-confidence
        // one should rank above the low-confidence one. Without
        // weight_by_confidence the BM25 rank alone would tie or
        // invert (insertion-order tiebreak).
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        // Disable time decay so the only differentiator is confidence.
        config.search.time_decay_tau_ms = 0;
        config.search.weight_by_confidence = true;
        config.search.semantic_index = "bm25".to_string();
        let (path, config) = test_store_path(&dir, "test", config);
        let mut store = StoreKind::open(&path, config.clone()).unwrap();

        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let redis = store.resolve_entity("Redis").unwrap();

        let low = store
            .fact_add(FactInput {
                content: "low-confidence claim about redis cluster".to_string(),
                fact_type: Some(FactType::Claim),
                entity_ids: Some(vec![redis]),
                source_confidence: Some(0.2),
                ..Default::default()
            })
            .unwrap();
        let high = store
            .fact_add(FactInput {
                content: "high-confidence decision about redis cluster".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![redis]),
                source_confidence: Some(0.95),
                ..Default::default()
            })
            .unwrap();

        let results = semantic::hybrid_search(
            &store,
            "redis cluster",
            SearchOpts {
                limit: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
        let ids: Vec<_> = results.iter().map(|r| r.fact_id).collect();
        let pos_high = ids.iter().position(|i| *i == high).expect("high present");
        let pos_low = ids.iter().position(|i| *i == low).expect("low present");
        assert!(
            pos_high < pos_low,
            "high-confidence fact should rank ahead of low-confidence (high={pos_high} low={pos_low})",
        );
    }

    #[cfg(feature = "semantic")]
    #[test]
    #[ignore = "downloads HF model — local only"]
    fn hybrid_search_time_decay_demotes_old_facts() {
        // Two equally-confident facts, one fresh and one a year old
        // (via observed_at). With time_decay_tau_ms set to 30 days,
        // the older fact's weight is e^(-12) ≈ 0 — it must rank
        // behind the fresh one even when BM25 would tie.
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        config.search.time_decay_tau_ms = 30 * 24 * 60 * 60 * 1000; // 30 days
        config.search.weight_by_confidence = false; // isolate decay
        config.search.semantic_index = "bm25".to_string();
        let (path, config) = test_store_path(&dir, "test", config);
        let mut store = StoreKind::open(&path, config.clone()).unwrap();

        store
            .entity_add(EntityInput {
                name: "Postgres".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let pg = store.resolve_entity("Postgres").unwrap();

        let now_ms = current_epoch_ms();
        let one_year_ago = now_ms - 365 * 24 * 60 * 60 * 1000;

        let stale = store
            .fact_add(FactInput {
                content: "postgres logical replication is unstable".to_string(),
                fact_type: Some(FactType::Claim),
                entity_ids: Some(vec![pg]),
                source_confidence: Some(0.9),
                observed_at: Some(one_year_ago),
                ..Default::default()
            })
            .unwrap();
        let fresh = store
            .fact_add(FactInput {
                content: "postgres logical replication shipped to prod".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![pg]),
                source_confidence: Some(0.9),
                observed_at: Some(now_ms),
                ..Default::default()
            })
            .unwrap();

        let results = semantic::hybrid_search(
            &store,
            "postgres logical replication",
            SearchOpts {
                limit: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
        let ids: Vec<_> = results.iter().map(|r| r.fact_id).collect();
        let pos_fresh = ids.iter().position(|i| *i == fresh).expect("fresh present");
        let pos_stale = ids.iter().position(|i| *i == stale).expect("stale present");
        assert!(
            pos_fresh < pos_stale,
            "fresh fact should rank ahead of year-old fact under 30d τ (fresh={pos_fresh} stale={pos_stale})",
        );
    }

    #[cfg(feature = "semantic")]
    #[test]
    #[ignore = "downloads HF model — local only"]
    fn hybrid_search_decay_disabled_when_tau_is_zero() {
        // With time_decay_tau_ms=0, age must not affect ranking —
        // operators on an archival wiki want every fact treated
        // equally regardless of when it was observed.
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        config.search.time_decay_tau_ms = 0;
        config.search.weight_by_confidence = false;
        config.search.semantic_index = "bm25".to_string();
        let (path, config) = test_store_path(&dir, "test", config);
        let mut store = StoreKind::open(&path, config.clone()).unwrap();

        store
            .entity_add(EntityInput {
                name: "X".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let x = store.resolve_entity("X").unwrap();

        // Two facts, ancient and modern, identical content modulo a
        // disambiguator — without decay they should tie on BM25 score.
        let now_ms = current_epoch_ms();
        store
            .fact_add(FactInput {
                content: "needle alpha".to_string(),
                entity_ids: Some(vec![x]),
                observed_at: Some(now_ms - 10 * 365 * 24 * 60 * 60 * 1000),
                source_confidence: Some(0.5),
                ..Default::default()
            })
            .unwrap();
        store
            .fact_add(FactInput {
                content: "needle beta".to_string(),
                entity_ids: Some(vec![x]),
                observed_at: Some(now_ms),
                source_confidence: Some(0.5),
                ..Default::default()
            })
            .unwrap();

        let results = semantic::hybrid_search(
            &store,
            "needle",
            SearchOpts {
                limit: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
        // Both candidates returned; with τ=0 their weighted_score
        // equals their base RRF score (no exp() multiplier).
        assert!(results.len() >= 2, "both facts should be returned");
        let scores: Vec<_> = results.iter().map(|r| r.score).collect();
        // Equal base RRF scores remain equal-weighted; we only assert
        // the score is non-zero (decay didn't squash anything).
        assert!(scores.iter().all(|s| *s > 0.0));
    }

    #[cfg(all(feature = "semantic", feature = "semantic-adapt"))]
    #[test]
    #[ignore = "downloads HF model — local only"]
    fn hybrid_search_adapter_promotes_helpful_facts() {
        // Twin facts with identical content + confidence; without the
        // adapter they tie. Recording helpful feedback on one and
        // training the adapter must push it ahead of its twin.
        use crate::types::SearchFeedback;
        use ulid::Ulid;

        let dir = tempdir().unwrap();
        let mut config = Config::default();
        config.search.time_decay_tau_ms = 0;
        config.search.weight_by_confidence = false;
        config.search.semantic_index = "bm25".to_string();
        config.search.use_adapter = true;
        let (path, mut config) = test_store_path(&dir, "test", config);
        let mut store = StoreKind::open(&path, config.clone()).unwrap();

        store
            .entity_add(EntityInput {
                name: "Topic".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let topic = store.resolve_entity("Topic").unwrap();

        let twin_a = store
            .fact_add(FactInput {
                content: "alpha note about topic indexing".to_string(),
                entity_ids: Some(vec![topic]),
                source_confidence: Some(0.5),
                ..Default::default()
            })
            .unwrap();
        let twin_b = store
            .fact_add(FactInput {
                content: "beta note about topic indexing".to_string(),
                entity_ids: Some(vec![topic]),
                source_confidence: Some(0.5),
                ..Default::default()
            })
            .unwrap();

        let session_id = Ulid::new().to_string();
        for _ in 0..3 {
            store
                .search_feedback_add(&SearchFeedback {
                    session_id: session_id.clone(),
                    fact_id: twin_a,
                    helpful: true,
                    timestamp: 0,
                })
                .unwrap();
        }
        store.adapt_train().unwrap();

        let results = semantic::hybrid_search(
            &store,
            "topic indexing",
            SearchOpts {
                limit: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
        let pos_a = results.iter().position(|r| r.fact_id == twin_a);
        let pos_b = results.iter().position(|r| r.fact_id == twin_b);
        assert!(
            matches!((pos_a, pos_b), (Some(a), Some(b)) if a < b),
            "helpful-feedback fact should rank ahead of its untrained twin (a={pos_a:?} b={pos_b:?})",
        );

        // Bypass: with use_adapter=false the bias must not influence
        // ranking, so the original BM25 / RRF tie-break order returns.
        config.search.use_adapter = false;
        let store_no_adapter = StoreKind::open(&path, config).unwrap();
        let untouched = semantic::hybrid_search(
            &store_no_adapter,
            "topic indexing",
            SearchOpts {
                limit: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(untouched.len() >= 2);
    }

    #[cfg(feature = "semantic")]
    #[test]
    #[ignore = "downloads HF model — local only"]
    fn hybrid_search_decay_exempt_types_resist_aging() {
        // Two facts, both year-old. One typed Decision (default
        // exempt), one Note (default decays). With τ=30d the Note
        // should be crushed near zero, the Decision should hold its
        // weight, so the Decision wins ranking even though both are
        // equally old.
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        config.search.time_decay_tau_ms = 30 * 24 * 60 * 60 * 1000; // 30 days
        config.search.weight_by_confidence = false;
        config.search.semantic_index = "bm25".to_string();
        // decay_exempt_types defaults to {decision, convention, pattern} —
        // verify by relying on the default rather than over-configuring.
        let (path, config) = test_store_path(&dir, "test", config);
        let mut store = StoreKind::open(&path, config.clone()).unwrap();
        store
            .entity_add(EntityInput {
                name: "Topic".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let t = store.resolve_entity("Topic").unwrap();

        let now_ms = current_epoch_ms();
        let one_year_ago = now_ms - 365 * 24 * 60 * 60 * 1000;

        let old_note = store
            .fact_add(FactInput {
                content: "topic foo bar baz".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![t]),
                observed_at: Some(one_year_ago),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .unwrap();
        let old_decision = store
            .fact_add(FactInput {
                content: "topic foo bar quux".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![t]),
                observed_at: Some(one_year_ago),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .unwrap();

        let results = semantic::hybrid_search(
            &store,
            "topic foo bar",
            SearchOpts {
                limit: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
        let pos_decision = results
            .iter()
            .position(|r| r.fact_id == old_decision)
            .expect("decision present");
        let pos_note = results
            .iter()
            .position(|r| r.fact_id == old_note)
            .expect("note present");
        assert!(
            pos_decision < pos_note,
            "decay-exempt Decision should rank ahead of decayed Note despite equal age \
             (decision={pos_decision} note={pos_note})",
        );
    }
}
