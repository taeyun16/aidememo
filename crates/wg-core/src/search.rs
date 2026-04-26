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

#[cfg(feature = "semantic")]
mod semantic {
    use super::*;
    use std::cmp::Ordering;
    use std::path::PathBuf;

    /// Hybrid search combining BM25 and semantic vectors.
    pub fn hybrid_search(
        store: &Store,
        query: &str,
        opts: SearchOpts,
    ) -> Result<Vec<SearchResult>> {
        let config = store.config();
        let engine = SearchEngine::new(store, config);

        let bm25_results = engine.search(query, opts.clone())?;
        let semantic_results = semantic_search(store, query, &opts)?;

        let bm25_weight = effective_weight(opts.bm25_weight, config.search.bm25_weight);
        let semantic_weight = effective_weight(opts.semantic_weight, config.search.semantic_weight);
        let limit = opts.limit.unwrap_or(config.search.default_limit);

        Ok(rrf_fusion(
            store,
            &bm25_results,
            Some(semantic_results.as_slice()),
            bm25_weight,
            semantic_weight,
            limit,
        ))
    }

    fn semantic_search(store: &Store, query: &str, opts: &SearchOpts) -> Result<Vec<SearchResult>> {
        let config = store.config();
        let provider = crate::embedding::load_provider(config)?;
        let query_embedding = provider.embed(query)?;

        let mut facts = store.fact_list(FactListOpts {
            limit: None,
            offset: 0,
            since: opts.since,
            until: opts.until,
            current_only: opts.current_only,
            ..Default::default()
        })?;

        let min_confidence = opts.min_confidence.unwrap_or(config.search.min_trust);

        facts.retain(|fact| {
            fact.source_confidence >= min_confidence
                && matches_entity_filter(fact, opts.entity_filter.as_ref())
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

        let texts: Vec<String> = facts
            .iter()
            .map(|fact| fact_semantic_text_cached(fact, &entity_names))
            .collect();
        let embeddings = provider.embed_batch(&texts)?;

        let mut scored: Vec<(usize, f32)> = facts
            .iter()
            .enumerate()
            .filter_map(|(idx, _fact)| {
                embeddings
                    .get(idx)
                    .map(|embedding| (idx, cosine_similarity(&query_embedding, embedding)))
            })
            .collect();

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

    /// Cosine similarity via simsimd — auto-dispatches to AVX2 / AVX-512 /
    /// NEON depending on CPU. Falls back to a portable scalar loop if
    /// simsimd returns `None` (e.g. mismatched lengths or empty input).
    ///
    /// simsimd reports cosine **distance**, not similarity, so we invert.
    /// Range: 0 (orthogonal) → 1 (identical).
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        use simsimd::SpatialSimilarity;
        if a.is_empty() || b.is_empty() || a.len() != b.len() {
            return 0.0;
        }
        match f32::cosine(a, b) {
            Some(distance) => (1.0 - distance) as f32,
            None => 0.0,
        }
    }

    fn effective_weight(opts_weight: f32, config_weight: f32) -> f32 {
        if opts_weight > 0.0 {
            opts_weight
        } else {
            config_weight
        }
    }

    fn expand_tilde(path: &str) -> PathBuf {
        if path == "~" {
            return home_dir().unwrap_or_else(|| PathBuf::from(path));
        }

        if let Some(rest) = path.strip_prefix("~/") {
            if let Some(home) = home_dir() {
                return home.join(rest);
            }
        }

        PathBuf::from(path)
    }

    fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
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

#[cfg(feature = "semantic")]
// `embed_text` was an explicit fn re-export; embedding is now done
// through `crate::embedding::EmbeddingProvider::embed`. External callers
// that need a one-shot embed should call:
//   let p = wg_core::embedding::load_provider(config)?;
//   let v = p.embed(text)?;
#[cfg(feature = "semantic")]
pub use semantic::hybrid_search;

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
