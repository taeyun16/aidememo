//! Search engine for WikiGraph.
//!
//! Provides BM25 keyword search and hybrid semantic search.

use crate::config::Config;
use crate::error::Result;
use crate::graph::Graph;
use crate::index::{Bm25IndexState, build_bm25_index};
use crate::store::Store;
use crate::types::*;
use parking_lot::RwLock;

/// Search engine for WikiGraph.
pub struct SearchEngine<'a> {
    store: &'a Store,
    config: &'a Config,
    index: RwLock<Bm25IndexState>,
}

impl<'a> SearchEngine<'a> {
    /// Create a new search engine.
    pub fn new(store: &'a Store, config: &'a Config) -> Self {
        let index = build_bm25_index(store);
        Self {
            store,
            config,
            index: RwLock::new(index),
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

        let index = self.index.read();
        let bm25_results: Vec<bm25::SearchResult<FactId>> = index.engine.search(query, limit);

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

                if !matches_time_window(&fact, opts.since, opts.until) {
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

/// Check whether a fact's timestamp falls within `[since, until]` (inclusive).
/// Prefers `observed_at` (real-world time) over `created_at` (DB insertion).
fn matches_time_window(fact: &FactRecord, since: Option<u64>, until: Option<u64>) -> bool {
    if since.is_none() && until.is_none() {
        return true;
    }
    let ts = fact.observed_at.unwrap_or(fact.created_at);
    if let Some(s) = since {
        if ts < s {
            return false;
        }
    }
    if let Some(u) = until {
        if ts > u {
            return false;
        }
    }
    true
}

#[cfg(feature = "semantic")]
fn build_search_result(
    store: &Store,
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
        score,
        rank,
        created_at: fact.created_at,
        observed_at: fact.observed_at,
        session_id,
    }
}

#[cfg(not(feature = "semantic"))]
fn build_search_result(
    store: &Store,
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
    /// from `WikiGraph::hybrid_search`, which reuses a singleton provider +
    /// query-embedding cache. This convenience wrapper exists for tests and
    /// one-off callers (bindings, scripts) that don't want to plumb a context.
    pub fn hybrid_search(
        store: &Store,
        query: &str,
        opts: SearchOpts,
    ) -> Result<Vec<SearchResult>> {
        let provider = crate::embedding::load_provider(store.config())?;
        let cache = Mutex::new(LruCache::new(
            std::num::NonZeroUsize::new(8).expect("non-zero"),
        ));
        let fact_cache = RwLock::new(HashMap::new());
        hybrid_search_with_ctx(store, query, opts, &*provider, &cache, &fact_cache)
    }

    /// Hybrid search with a caller-owned provider + query-embedding cache.
    /// Used by `WikiGraph` to avoid reloading the Model2Vec model on every
    /// search and to memoize repeated queries.
    pub fn hybrid_search_with_ctx(
        store: &Store,
        query: &str,
        opts: SearchOpts,
        provider: &dyn EmbeddingProvider,
        query_cache: &Mutex<LruCache<String, Vec<f32>>>,
        fact_cache: &RwLock<HashMap<FactId, QuantizedEmbedding>>,
    ) -> Result<Vec<SearchResult>> {
        let config = store.config();
        let engine = SearchEngine::new(store, config);

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
        let bm25_results = engine.search(query, bm25_opts)?;

        // Tier 7-D: graph-aware prefilter. Take the entities surfaced
        // by BM25 hits, walk N hops outward, and pull facts attached
        // to those neighbors into the candidate pool. wg's graph is
        // the differentiator — semantic neighborhoods + relation
        // structure together catch matches that pure keyword overlap
        // misses (e.g. a fact about "PostgreSQL replication" is a
        // strong candidate when the user searched for "Redis" and
        // those entities are linked in the graph).
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

        let semantic_results = semantic_search(
            store,
            query,
            &opts,
            provider,
            query_cache,
            fact_cache,
            semantic_candidates.as_deref(),
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

    fn semantic_search(
        store: &Store,
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
                let v = provider.embed(query)?;
                cache.put(query.to_string(), v.clone());
                v
            }
        };

        // Tier 7-B: when BM25 has narrowed the universe down to a
        // candidate slate, hydrate just those FactRecords. Otherwise
        // fall back to scanning every fact (maintains old behavior
        // when prefilter=0 or when no candidates are available).
        let mut facts: Vec<FactRecord> = match candidates {
            Some(ids) if !ids.is_empty() => ids
                .iter()
                .filter_map(|id| store.fact_get(id).ok())
                .collect(),
            _ => store.fact_list(FactListOpts {
                limit: None,
                offset: 0,
                since: opts.since,
                until: opts.until,
                current_only: opts.current_only,
                ..Default::default()
            })?,
        };

        let min_confidence = opts.min_confidence.unwrap_or(config.search.min_trust);

        facts.retain(|fact| {
            fact.source_confidence >= min_confidence
                && matches_entity_filter(fact, opts.entity_filter.as_ref())
                && (opts.since.is_none() && opts.until.is_none()
                    || matches_time_window(fact, opts.since, opts.until))
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
            let new_embeddings = provider.embed_batch(&miss_texts)?;
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

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
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
    fn graph_expand_candidates(
        store: &Store,
        bm25_results: &[SearchResult],
        depth: u32,
        fact_cap: usize,
        existing: &[FactId],
    ) -> Result<Vec<FactId>> {
        if depth == 0 || fact_cap == 0 {
            return Ok(Vec::new());
        }

        let already: HashSet<FactId> = existing.iter().copied().collect();

        // 1. Pull the seed entity set from BM25 hits.
        let mut seed_entities: HashSet<EntityId> = HashSet::new();
        for r in bm25_results {
            if let Ok(fact) = store.fact_get(&r.fact_id) {
                for eid in fact.entity_ids {
                    seed_entities.insert(eid);
                }
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

    fn rrf_fusion(
        store: &Store,
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
            score: f32,
        }

        let mut scores: HashMap<FactId, FusedEntry> = HashMap::new();

        for result in bm25_results {
            let fused_score = bm25_weight / (RRF_K + result.rank as f32);
            scores
                .entry(result.fact_id)
                .and_modify(|entry| entry.score += fused_score)
                .or_insert_with(|| FusedEntry {
                    result: result.clone(),
                    score: fused_score,
                });
        }

        if let Some(semantic_results) = semantic_results {
            for result in semantic_results {
                let fused_score = semantic_weight / (RRF_K + result.rank as f32);
                scores
                    .entry(result.fact_id)
                    .and_modify(|entry| entry.score += fused_score)
                    .or_insert_with(|| FusedEntry {
                        result: result.clone(),
                        score: fused_score,
                    });
            }
        }

        let mut entries: Vec<FusedEntry> = scores.into_values().collect();
        entries.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.result.rank.cmp(&b.result.rank))
        });

        let mut results = Vec::new();
        for (rank, entry) in entries.into_iter().take(limit).enumerate() {
            let mut result = entry.result;
            if let Ok(fact) = store.fact_get(&result.fact_id) {
                result.content = fact.content;
                result.fact_type = fact.fact_type;
                result.entity_names = fact
                    .entity_ids
                    .iter()
                    .filter_map(|eid| store.entity_get_by_id(*eid).ok())
                    .map(|entity| entity.name)
                    .collect();
                result.source = fact.source;
                result.created_at = fact.created_at;
                result.observed_at = fact.observed_at;
            }
            result.score = entry.score;
            result.rank = rank + 1;
            results.push(result);
        }

        results
    }
}

// `embed_text` was an explicit fn re-export; embedding is now done
// through `crate::embedding::EmbeddingProvider::embed`. External callers
// that need a one-shot embed should call:
//   let p = wg_core::embedding::load_provider(config)?;
//   let v = p.embed(text)?;
#[cfg(feature = "semantic")]
pub use semantic::{hybrid_search, hybrid_search_with_ctx};

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_test_store() -> (Store, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let config = Config::default();
        let store = Store::open(&path, config).unwrap();
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
                source_confidence: Some(0.7),
                observed_at: None,
            })
            .unwrap();

        let config = Config::default();
        let engine = SearchEngine::new(&store, &config);

        let results = engine
            .search("high availability", SearchOpts::default())
            .unwrap();
        assert!(!results.is_empty());

        let results = engine
            .search("nonexistent query xyz", SearchOpts::default())
            .unwrap();
        assert!(results.len() <= config.search.default_limit);
    }
}
