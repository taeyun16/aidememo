//! BM25 in-memory index builder and state.

use crate::store::Store;
use crate::types::{FactId, FactListOpts};

/// In-memory BM25 index state.
///
/// Public so that callers (notably `AideMemo`, but also bindings
/// and benchmarks) can own one and pass `&RwLock<Bm25IndexState>`
/// into `SearchEngine::new`. Internals stay private — this is just
/// a handle to a cached inverted index keyed by `FactId`.
pub struct Bm25IndexState {
    /// BM25 search engine instance keyed by FactId.
    pub(crate) engine: bm25::SearchEngine<FactId>,
    /// Whether the index needs rebuilding.
    pub dirty: bool,
    /// Rebuild generation counter.
    pub(crate) generation: u64,
}

impl Bm25IndexState {
    /// Create an empty BM25 index state.
    ///
    /// Starts marked `dirty: true` so the first search via
    /// `SearchEngine::ensure_index` triggers a full rebuild.
    /// `build_bm25_index` flips `dirty` back to `false` once the
    /// engine is populated.
    pub fn new() -> Self {
        Self {
            engine: bm25::SearchEngineBuilder::<FactId>::with_avgdl(256.0).build(),
            dirty: true,
            generation: 0,
        }
    }
}

impl Default for Bm25IndexState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the BM25 index from all facts in the store.
pub(crate) fn build_bm25_index(store: &Store) -> Bm25IndexState {
    let mut state = Bm25IndexState::new();

    let facts = match store.fact_list(FactListOpts {
        limit: None,
        ..Default::default()
    }) {
        Ok(facts) => facts,
        Err(_) => return state,
    };

    for fact in facts {
        // Build document text from content + entity names + tags
        let mut text = fact.content;

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
