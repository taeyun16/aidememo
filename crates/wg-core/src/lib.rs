//! WikiGraph — Structured index engine for LLM wikis.

pub mod config;
pub mod error;
pub mod fuzzy;
pub mod graph;
pub mod index;
pub mod ingest;
pub mod lint;
pub mod migrate;
pub mod search;
pub mod store;
pub mod types;
pub mod adapt;
#[cfg(feature = "s3")]
pub mod s3;
#[cfg(feature = "s3")]
pub mod wal;

use parking_lot::RwLock;
use std::path::Path;
use std::sync::Arc;

pub use config::Config;
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
    config: Arc<Config>,
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
        let store = Store::open(path, config.clone())?;
        Ok(Self {
            store: Arc::new(RwLock::new(store)),
            config: Arc::new(config),
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
        self.store.write().entity_add(input)
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
        self.store.write().entity_update(name, input)
    }

    /// List entities with options.
    pub fn entity_list(&self, opts: ListOpts) -> Result<Vec<EntitySummary>> {
        self.store.read().entity_list(opts)
    }

    /// Delete an entity.
    pub fn entity_delete(&self, name: &str) -> Result<()> {
        self.store.write().entity_delete(name)
    }

    /// Rename an entity (alias for entity_update with name change).
    pub fn entity_rename(&self, old_name: &str, new_name: &str) -> Result<()> {
        self.store.write().entity_update(
            old_name,
            EntityUpdate {
                name: Some(new_name.to_string()),
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
    pub fn add_fact(&self, input: FactInput) -> Result<FactId> {
        let mut store = self.store.write();
        store.fact_add(input)
    }

    /// Backwards-compatible alias for add_fact.
    pub fn fact_add(&self, input: FactInput) -> Result<FactId> {
        self.add_fact(input)
    }

    /// Get a fact by ID.
    pub fn fact_get(&self, id: &FactId) -> Result<FactRecord> {
        self.store.read().fact_get(id)
    }

    /// Update a fact.
    pub fn fact_update(&self, id: &FactId, input: FactUpdate) -> Result<()> {
        self.store.write().fact_update(id, input)
    }

    /// Delete a fact.
    pub fn fact_delete(&self, id: &FactId) -> Result<()> {
        self.store.write().fact_delete(id)
    }

    /// Record feedback for a fact.
    pub fn fact_feedback(&self, id: &FactId, helpful: bool) -> Result<()> {
        self.store.write().fact_feedback(id, helpful)
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

    // === Relation Operations ===

    /// Add a new relation.
    pub fn relation_add(&self, input: RelationInput) -> Result<()> {
        self.store.write().relation_add(input)
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
    pub fn ingest(&mut self, wiki_root: &Path, incremental: bool) -> Result<ingest::IngestStats> {
        let mut store = self.store.write();
        ingest::ingest_wiki(wiki_root, &mut store, incremental)
    }

    // === Search ===

    /// Search for facts matching a query.
    #[cfg(feature = "semantic")]
    pub fn search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>> {
        use search::SearchEngine;
        let store = self.store.read();
        let engine = SearchEngine::new(&*store, &self.config);
        engine.search(query, opts)
    }

    /// Search using hybrid BM25 + semantic ranking.
    #[cfg(feature = "semantic")]
    pub fn hybrid_search(&self, query: &str, opts: SearchOpts) -> Result<Vec<SearchResult>> {
        let store = self.store.read();
        search::hybrid_search(&*store, query, opts)
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
        let engine = SearchEngine::new(&*store, &self.config);
        engine.search_with_traverse(query, start, depth, opts)
    }

    /// Search (BM25 only, no semantic features).
    #[cfg(not(feature = "semantic"))]
    pub fn search(&self, _query: &str, _opts: SearchOpts) -> Result<Vec<SearchResult>> {
        Err(WgError::SearchFailed(
            "BM25 search requires the 'semantic' feature".to_string(),
        ))
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

    /// Get the configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }
}

// Re-export types for convenience
pub use types::{
    AdaptEvalReport, AdaptResult, AdaptStatus, EntityId, EntityInput, EntityRecord, EntitySort,
    EntitySummary, EntityType, EntityUpdate, ExportScope, ExportStats, FactId, FactInput,
    FactListOpts, FactRecord, FactType, FactUpdate, ImportStats, LintIssue, LintReport,
    LintSeverity, ListOpts, PathStep, RelationInput, RelationRecord, RelationType,
    SearchFeedback, SearchOpts, SearchResult, SearchSession, StoreStats, TraverseDirection,
    TraverseOpts, TraverseResult,
};

#[cfg(feature = "semantic")]
pub use types::VectorRecord;
