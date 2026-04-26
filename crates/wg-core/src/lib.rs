//! WikiGraph — Structured index engine for LLM wikis.

pub mod adapt;
pub mod config;
pub mod error;
pub mod fuzzy;
pub mod graph;
pub mod index;
pub mod ingest;
pub mod lint;
pub mod migrate;
pub mod relations;
#[cfg(feature = "s3")]
pub mod s3;
pub mod search;
pub mod store;
pub mod types;
#[cfg(feature = "s3")]
pub mod wal;

use parking_lot::RwLock;
use std::path::Path;
use std::sync::Arc;

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
    pub fn ingest(&self, wiki_root: &Path, incremental: bool) -> Result<ingest::IngestStats> {
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
        let search = self.hybrid_search(
            topic,
            SearchOpts {
                limit: Some(opts.search_limit),
                since: opts.since,
                current_only: opts.current_only,
                ..Default::default()
            },
        )?;

        let entity = self.entity_get(topic).ok();

        let (related, recent_facts) = if let Some(ref e) = entity {
            let traverse = self.traverse(
                &e.name,
                TraverseOpts {
                    depth: opts.depth,
                    relation_types: None,
                    direction: TraverseDirection::Both,
                },
            )?;
            let recent = self.fact_list(FactListOpts {
                entity_id: Some(e.id),
                limit: Some(opts.recent_limit),
                since: opts.since,
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
    LintSeverity, ListOpts, PathStep, QueryOpts, QueryResult, RelationInput, RelationRecord,
    RelationType, SearchFeedback, SearchOpts, SearchResult, SearchSession, StoreStats,
    TraverseDirection, TraverseOpts, TraverseResult,
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

    #[test]
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
