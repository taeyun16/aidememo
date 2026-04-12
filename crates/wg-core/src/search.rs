//! Search engine for WikiGraph.
//!
//! Provides BM25 keyword search and hybrid semantic search.

use crate::config::Config;
use crate::error::{Result, WgError};
use crate::graph::Graph;
use crate::store::Store;
use crate::types::*;

/// BM25 search engine state.
struct IndexState {
    /// BM25 search engine instance keyed by FactId.
    engine: bm25::SearchEngine<FactId>,
    /// Whether the index needs rebuilding.
    dirty: bool,
    /// Rebuild generation counter.
    generation: u64,
}

impl IndexState {
    fn new() -> Self {
        Self {
            engine: bm25::SearchEngineBuilder::<FactId>::with_avgdl(256.0).build(),
            dirty: false,
            generation: 0,
        }
    }
}

/// Search engine for WikiGraph.
pub struct SearchEngine<'a> {
    store: &'a Store,
    config: &'a Config,
    index: parking_lot::RwLock<IndexState>,
}

impl<'a> SearchEngine<'a> {
    /// Create a new search engine.
    pub fn new(store: &'a Store, config: &'a Config) -> Self {
        let index = Self::build_index(store);
        Self {
            store,
            config,
            index: parking_lot::RwLock::new(index),
        }
    }

    /// Build the BM25 index from all facts.
    fn build_index(store: &Store) -> IndexState {
        let mut state = IndexState::new();

        let facts = match store.fact_list(FactListOpts {
            limit: Some(100000),
            ..Default::default()
        }) {
            Ok(facts) => facts,
            Err(_) => return state,
        };

        for fact in facts {
            // Build document text from content + entity names + tags
            let mut text = fact.content.clone();

            // Add entity names
            for entity_id in &fact.entity_ids {
                if let Ok(entity) = store.entity_get_by_id(*entity_id) {
                    text.push(' ');
                    text.push_str(&entity.name);
                }
            }

            // Add tags
            for tag in &fact.tags {
                text.push(' ');
                text.push_str(tag);
            }

            state.engine.upsert(bm25::Document::new(fact.id, text));
        }

        state.dirty = false;
        state.generation += 1;
        state
    }

    /// Rebuild the index if dirty.
    fn ensure_index(&self) {
        let mut index = self.index.write();
        if index.dirty {
            let new_index = Self::build_index(self.store);
            *index = new_index;
        }
    }

    /// Search for facts matching a query.
    pub fn search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>> {
        self.ensure_index();

        let index = self.index.read();
        // Perform BM25 search
        let bm25_results: Vec<bm25::SearchResult<FactId>> =
            index.engine.search(query, opts.limit.unwrap_or(10));

        let mut results = Vec::new();

        for (rank, bm25_result) in bm25_results.into_iter().enumerate() {
            let fact_id = bm25_result.document.id;
            let score = bm25_result.score;

            if let Ok(fact) = self.store.fact_get(&fact_id) {
                // Apply min confidence filter
                if let Some(min_conf) = opts.min_confidence {
                    if fact.source_confidence < min_conf {
                        continue;
                    }
                }

                // Apply entity filter
                if let Some(ref entity_filter) = opts.entity_filter {
                    if !fact
                        .entity_ids
                        .iter()
                        .any(|eid| entity_filter.contains(eid))
                    {
                        continue;
                    }
                }

                // Get entity names for display
                let entity_names: Vec<String> = fact
                    .entity_ids
                    .iter()
                    .filter_map(|eid| self.store.entity_get_by_id(*eid).ok())
                    .map(|e| e.name)
                    .collect();

                // Calculate combined score
                let combined_score =
                    score * (0.6 * fact.source_confidence + 0.4 * fact.relevance_score);

                results.push(SearchResult {
                    fact_id,
                    content: fact.content,
                    fact_type: fact.fact_type,
                    entity_names,
                    source: fact.source,
                    score: combined_score,
                    rank: rank + 1,
                });
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
        // First, traverse to find entities in scope
        let traverse_result = Graph::new(self.store).traverse(
            start,
            TraverseOpts {
                depth,
                relation_types: None,
                direction: TraverseDirection::Forward,
            },
        )?;

        // Create entity filter from traversal result
        let entity_ids: Vec<EntityId> = traverse_result.entities.iter().map(|e| e.id).collect();

        // Search with entity filter
        let mut search_opts = opts;
        search_opts.entity_filter = Some(entity_ids);

        self.search(query, search_opts)
    }
}

#[cfg(feature = "semantic")]
mod semantic {
    use super::*;

    /// Hybrid search combining BM25 and semantic vectors.
    pub struct HybridSearchEngine<'a> {
        store: &'a Store,
        config: &'a Config,
        bm25_index: parking_lot::RwLock<IndexState>,
    }

    impl<'a> HybridSearchEngine<'a> {
        pub fn new(store: &'a Store, config: &'a Config) -> Self {
            let bm25_index = SearchEngine::build_index(store);
            Self {
                store,
                config,
                bm25_index: parking_lot::RwLock::new(bm25_index),
            }
        }

        /// Search using RRF (Reciprocal Rank Fusion) combining BM25 and semantic.
        pub fn hybrid_search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>> {
            // BM25 results
            let bm25_results = self.bm25_search(query, opts.limit.unwrap_or(10))?;

            // Semantic results (if model is available)
            let semantic_results = self.semantic_search(query, opts.limit.unwrap_or(10)).ok();

            // RRF fusion
            let limit = opts.limit.unwrap_or(10) as usize;
            let fused = self.rrf_fusion(
                &bm25_results,
                semantic_results.as_deref(),
                opts.bm25_weight,
                opts.semantic_weight,
                limit,
            );

            Ok(fused)
        }

        fn bm25_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
            let index = self.bm25_index.read();

            let bm25_results: Vec<bm25::SearchResult<FactId>> = index.engine.search(query, limit);
            let mut results = Vec::new();

            for (rank, bm25_result) in bm25_results.into_iter().enumerate() {
                let fact_id = bm25_result.document.id;
                let score = bm25_result.score;

                if let Ok(fact) = self.store.fact_get(&fact_id) {
                    results.push(SearchResult {
                        fact_id,
                        content: fact.content.clone(),
                        fact_type: fact.fact_type,
                        entity_names: fact
                            .entity_ids
                            .iter()
                            .filter_map(|eid| self.store.entity_get_by_id(*eid).ok())
                            .map(|e| e.name)
                            .collect(),
                        source: fact.source.clone(),
                        score,
                        rank: rank + 1,
                    });
                }
            }

            Ok(results)
        }

        fn semantic_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
            // This would use Model2Vec to encode query and compute cosine similarity
            // For now, return empty results if semantic feature is enabled but model not loaded
            Err(WgError::SearchFailed(
                "Semantic model not yet implemented".to_string(),
            ))
        }

        fn rrf_fusion(
            &self,
            bm25_results: &[SearchResult],
            semantic_results: Option<&[SearchResult]>,
            bm25_weight: f32,
            semantic_weight: f32,
            limit: usize,
        ) -> Vec<SearchResult> {
            use std::collections::HashMap;

            const RRF_K: f32 = 60.0; // RRF constant

            let mut scores: HashMap<FactId, f32> = HashMap::new();

            // BM25 scores
            for result in bm25_results {
                let rrf_score = bm25_weight / (RRF_K + result.rank as f32);
                *scores.entry(result.fact_id).or_insert(0.0) += rrf_score;
            }

            // Semantic scores
            if let Some(semantic_results) = semantic_results {
                for result in semantic_results {
                    let rrf_score = semantic_weight / (RRF_K + result.rank as f32);
                    *scores.entry(result.fact_id).or_insert(0.0) += rrf_score;
                }
            }

            // Sort by combined score
            let mut sorted: Vec<_> = scores.into_iter().collect();
            sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            // Fetch full results (limited)
            let mut results = Vec::new();
            for (rank, (fact_id, score)) in sorted.into_iter().enumerate().take(limit) {
                if let Ok(fact) = self.store.fact_get(&fact_id) {
                    results.push(SearchResult {
                        fact_id,
                        content: fact.content,
                        fact_type: fact.fact_type,
                        entity_names: fact
                            .entity_ids
                            .iter()
                            .filter_map(|eid| self.store.entity_get_by_id(*eid).ok())
                            .map(|e| e.name)
                            .collect(),
                        source: fact.source,
                        score,
                        rank: rank + 1,
                    });
                }
            }

            results
        }
    }
}

#[cfg(feature = "semantic")]
pub use semantic::HybridSearchEngine;

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

        // Create entities
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                tags: Some(vec!["cache".to_string(), "infra".to_string()]),
                ..Default::default()
            })
            .unwrap();

        let redis_id = store.resolve_entity("Redis").unwrap();

        // Create facts
        store
            .fact_add(FactInput {
                content: "Redis Sentinel provides high availability".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![redis_id]),
                tags: Some(vec!["ha".to_string()]),
                source: Some("entities/redis.md".to_string()),
                source_confidence: Some(0.8),
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
            })
            .unwrap();

        let config = Config::default();
        let engine = SearchEngine::new(&store, &config);

        // Search
        let results = engine
            .search("high availability", SearchOpts::default())
            .unwrap();
        assert!(!results.is_empty());

        // Search with no matches
        let results = engine
            .search("nonexistent query xyz", SearchOpts::default())
            .unwrap();
        // Results may be empty or contain low-scoring results
    }
}
