//! Storage backend trait shared by the redb store and experimental backends.
//!
//! This captures the store surface that higher-level graph, ingest, search, and
//! lint code need in order to run against redb or an experimental backend.

use std::path::Path;

use crate::config::Config;
use crate::error::{AideMemoError, Result};
#[cfg(feature = "redb")]
use crate::store::Store;
use crate::types::{
    EntityId, EntityInput, EntityRecord, EntityUpdate, FactId, FactInput, FactListOpts, FactRecord,
    FactUpdate, ListOpts, RelationInput, RelationRecord, SearchFeedback, SearchSession, StoreStats,
    TraverseDirection,
};

/// Runtime-selected store backend.
pub enum StoreKind {
    #[cfg(feature = "redb")]
    Redb(Store),
    #[cfg(feature = "sqlite")]
    Sqlite(crate::sqlite_store::SqliteStore),
}

impl StoreKind {
    /// Open the backend selected by `config.store.backend`.
    pub fn open(path: &Path, config: Config) -> Result<Self> {
        match config.store.backend.trim().to_lowercase().as_str() {
            "" | "sqlite" | "libsqlite" => {
                #[cfg(feature = "sqlite")]
                {
                    Ok(Self::Sqlite(crate::sqlite_store::SqliteStore::open(
                        path, config,
                    )?))
                }
                #[cfg(not(feature = "sqlite"))]
                {
                    Err(AideMemoError::InvalidInput(
                        "store.backend = sqlite requires the `sqlite` Cargo feature".to_string(),
                    ))
                }
            }
            "redb" => {
                #[cfg(feature = "redb")]
                {
                    Ok(Self::Redb(Store::open(path, config)?))
                }
                #[cfg(not(feature = "redb"))]
                {
                    Err(AideMemoError::InvalidInput(
                        "store.backend = redb requires the `redb` Cargo feature".to_string(),
                    ))
                }
            }
            other => Err(AideMemoError::InvalidInput(format!(
                "unsupported store.backend '{other}'"
            ))),
        }
    }

    /// Returns true when this handle is backed by redb.
    pub fn is_redb(&self) -> bool {
        #[cfg(feature = "redb")]
        {
            return matches!(self, Self::Redb(_));
        }
        #[cfg(not(feature = "redb"))]
        {
            false
        }
    }
}

/// Storage contract shared by redb and experimental backends.
pub trait StoreBackend {
    /// Open or create a backend store at `path`.
    fn open(path: &Path, config: Config) -> Result<Self>
    where
        Self: Sized;

    /// Return basic store counts and size metadata.
    fn stats(&self) -> Result<StoreStats>;

    /// Return the effective backend configuration.
    fn config(&self) -> &Config;

    /// Mark the store as freshly ingested.
    fn set_last_ingest_at(&self) -> Result<()>;

    /// Insert one entity and return its id.
    fn entity_add(&mut self, input: EntityInput) -> Result<EntityId>;

    /// Fetch an entity by name or alias.
    fn entity_get(&self, name: &str) -> Result<EntityRecord>;

    /// Resolve an entity name or alias to an id.
    fn resolve_entity(&self, name: &str) -> Result<EntityId>;

    /// Fetch an entity by id.
    fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord>;

    /// Update one entity.
    fn entity_update(&mut self, name: &str, input: EntityUpdate) -> Result<()>;

    /// List entities with the existing AideMemo filters.
    fn entity_list(&self, opts: ListOpts) -> Result<Vec<crate::types::EntitySummary>>;

    /// Delete one entity.
    fn entity_delete(&mut self, name: &str) -> Result<()>;

    /// Insert or update a raw entity record while preserving its id.
    fn entity_upsert_record(&mut self, record: EntityRecord) -> Result<bool>;

    /// Suggest similar entity names for typo recovery.
    fn suggest_similar_entities(&self, name: &str) -> Result<Vec<String>>;

    /// Count facts attached to one entity.
    fn count_entity_facts(&self, entity_id: &EntityId) -> Result<u32>;

    /// Insert one fact.
    fn fact_add(&mut self, input: FactInput) -> Result<FactId>;

    /// Insert many facts in one backend transaction when supported.
    fn fact_add_many(&mut self, inputs: Vec<FactInput>) -> Result<Vec<FactId>>;

    /// Fetch one fact by id.
    fn fact_get(&self, id: &FactId) -> Result<FactRecord>;

    /// Fetch many facts, preserving input order and skipping missing ids.
    fn fact_get_many(&self, ids: &[FactId]) -> Result<Vec<FactRecord>>;

    /// List facts with the existing AideMemo filters.
    fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>>;

    /// Update fact metadata/content.
    fn fact_update(&mut self, id: &FactId, input: FactUpdate) -> Result<()>;

    /// Delete one fact.
    fn fact_delete(&mut self, id: &FactId) -> Result<()>;

    /// Insert or update a raw fact record while preserving its id.
    fn fact_upsert_record(&mut self, record: FactRecord) -> Result<bool>;

    /// Return currently pinned facts.
    fn pinned_facts(&self, limit: usize) -> Result<Vec<FactRecord>>;

    /// Record direct feedback on a fact.
    fn fact_feedback(&mut self, id: &FactId, helpful: bool) -> Result<()>;

    /// Record a search session.
    fn search_session_add(&mut self, session: &SearchSession) -> Result<()>;

    /// Record feedback against a search session result.
    fn search_feedback_add(&mut self, feedback: &SearchFeedback) -> Result<()>;

    /// Count search feedback entries.
    fn search_feedback_count(&self) -> Result<usize>;

    /// Train the search adapter from feedback.
    #[cfg(feature = "semantic-adapt")]
    fn adapt_train(&mut self) -> Result<crate::types::AdaptResult>;

    /// Return adapter status.
    #[cfg(feature = "semantic-adapt")]
    fn adapt_status(&self) -> Result<crate::types::AdaptStatus>;

    /// Evaluate the current adapter.
    #[cfg(feature = "semantic-adapt")]
    fn adapt_eval(&self) -> Result<crate::types::AdaptEvalReport>;

    /// Load persisted adapter state.
    #[cfg(feature = "semantic-adapt")]
    fn load_adapter(&self) -> Result<crate::adapt::DomainAdapter>;

    /// Insert or replace one relation.
    fn relation_add(&mut self, input: RelationInput) -> Result<()>;

    /// Insert a raw relation record.
    fn relation_upsert_record(&mut self, record: RelationRecord) -> Result<bool>;

    /// Remove one relation.
    fn relation_remove(&mut self, source: &str, target: &str, rel_type: &str) -> Result<()>;

    /// Get relations for an entity name.
    fn relations_get(
        &self,
        entity_name: &str,
        direction: TraverseDirection,
    ) -> Result<Vec<RelationRecord>>;

    /// Get relations for an entity id.
    fn relations_get_by_id(
        &self,
        entity_id: &EntityId,
        direction: TraverseDirection,
    ) -> Result<Vec<RelationRecord>> {
        let entity = self.entity_get_by_id(*entity_id)?;
        self.relations_get(&entity.name, direction)
    }

    /// Return every forward relation once.
    fn relations_list_all(&self) -> Result<Vec<RelationRecord>>;
}

#[cfg(feature = "redb")]
impl StoreBackend for Store {
    fn open(path: &Path, config: Config) -> Result<Self> {
        Store::open(path, config)
    }

    fn stats(&self) -> Result<StoreStats> {
        Store::stats(self)
    }

    fn config(&self) -> &Config {
        Store::config(self)
    }

    fn set_last_ingest_at(&self) -> Result<()> {
        Store::set_last_ingest_at(self)
    }

    fn entity_add(&mut self, input: EntityInput) -> Result<EntityId> {
        Store::entity_add(self, input)
    }

    fn entity_get(&self, name: &str) -> Result<EntityRecord> {
        Store::entity_get(self, name)
    }

    fn resolve_entity(&self, name: &str) -> Result<EntityId> {
        Store::resolve_entity(self, name)
    }

    fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord> {
        Store::entity_get_by_id(self, id)
    }

    fn entity_update(&mut self, name: &str, input: EntityUpdate) -> Result<()> {
        Store::entity_update(self, name, input)
    }

    fn entity_list(&self, opts: ListOpts) -> Result<Vec<crate::types::EntitySummary>> {
        Store::entity_list(self, opts)
    }

    fn entity_delete(&mut self, name: &str) -> Result<()> {
        Store::entity_delete(self, name)
    }

    fn entity_upsert_record(&mut self, record: EntityRecord) -> Result<bool> {
        Store::entity_upsert_record(self, record)
    }

    fn suggest_similar_entities(&self, name: &str) -> Result<Vec<String>> {
        Store::suggest_similar_entities(self, name)
    }

    fn count_entity_facts(&self, entity_id: &EntityId) -> Result<u32> {
        Store::count_entity_facts(self, entity_id)
    }

    fn fact_add(&mut self, input: FactInput) -> Result<FactId> {
        Store::fact_add(self, input)
    }

    fn fact_add_many(&mut self, inputs: Vec<FactInput>) -> Result<Vec<FactId>> {
        Store::fact_add_many(self, inputs)
    }

    fn fact_get(&self, id: &FactId) -> Result<FactRecord> {
        Store::fact_get(self, id)
    }

    fn fact_get_many(&self, ids: &[FactId]) -> Result<Vec<FactRecord>> {
        Store::fact_get_many(self, ids)
    }

    fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>> {
        Store::fact_list(self, opts)
    }

    fn fact_update(&mut self, id: &FactId, input: FactUpdate) -> Result<()> {
        Store::fact_update(self, id, input)
    }

    fn fact_delete(&mut self, id: &FactId) -> Result<()> {
        Store::fact_delete(self, id)
    }

    fn fact_upsert_record(&mut self, record: FactRecord) -> Result<bool> {
        Store::fact_upsert_record(self, record)
    }

    fn pinned_facts(&self, limit: usize) -> Result<Vec<FactRecord>> {
        Store::pinned_facts(self, limit)
    }

    fn fact_feedback(&mut self, id: &FactId, helpful: bool) -> Result<()> {
        Store::fact_feedback(self, id, helpful)
    }

    fn search_session_add(&mut self, session: &SearchSession) -> Result<()> {
        Store::search_session_add(self, session)
    }

    fn search_feedback_add(&mut self, feedback: &SearchFeedback) -> Result<()> {
        Store::search_feedback_add(self, feedback)
    }

    fn search_feedback_count(&self) -> Result<usize> {
        Store::search_feedback_count(self)
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_train(&mut self) -> Result<crate::types::AdaptResult> {
        Store::adapt_train(self)
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_status(&self) -> Result<crate::types::AdaptStatus> {
        Store::adapt_status(self)
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_eval(&self) -> Result<crate::types::AdaptEvalReport> {
        Store::adapt_eval(self)
    }

    #[cfg(feature = "semantic-adapt")]
    fn load_adapter(&self) -> Result<crate::adapt::DomainAdapter> {
        Store::load_adapter(self)
    }

    fn relation_add(&mut self, input: RelationInput) -> Result<()> {
        Store::relation_add(self, input)
    }

    fn relation_upsert_record(&mut self, record: RelationRecord) -> Result<bool> {
        Store::relation_upsert_record(self, record)
    }

    fn relation_remove(&mut self, source: &str, target: &str, rel_type: &str) -> Result<()> {
        Store::relation_remove(self, source, target, rel_type)
    }

    fn relations_get(
        &self,
        entity_name: &str,
        direction: TraverseDirection,
    ) -> Result<Vec<RelationRecord>> {
        Store::relations_get(self, entity_name, direction)
    }

    fn relations_list_all(&self) -> Result<Vec<RelationRecord>> {
        Store::relations_list_all(self)
    }
}

impl StoreBackend for StoreKind {
    fn open(path: &Path, config: Config) -> Result<Self> {
        StoreKind::open(path, config)
    }

    fn stats(&self) -> Result<StoreStats> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.stats(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.stats(),
        }
    }

    fn config(&self) -> &Config {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.config(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.config(),
        }
    }

    fn set_last_ingest_at(&self) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.set_last_ingest_at(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.set_last_ingest_at(),
        }
    }

    fn entity_add(&mut self, input: EntityInput) -> Result<EntityId> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.entity_add(input),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.entity_add(input),
        }
    }

    fn entity_get(&self, name: &str) -> Result<EntityRecord> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.entity_get(name),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.entity_get(name),
        }
    }

    fn resolve_entity(&self, name: &str) -> Result<EntityId> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.resolve_entity(name),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.resolve_entity(name),
        }
    }

    fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.entity_get_by_id(id),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.entity_get_by_id(id),
        }
    }

    fn entity_update(&mut self, name: &str, input: EntityUpdate) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.entity_update(name, input),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.entity_update(name, input),
        }
    }

    fn entity_list(&self, opts: ListOpts) -> Result<Vec<crate::types::EntitySummary>> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.entity_list(opts),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.entity_list(opts),
        }
    }

    fn entity_delete(&mut self, name: &str) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.entity_delete(name),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.entity_delete(name),
        }
    }

    fn entity_upsert_record(&mut self, record: EntityRecord) -> Result<bool> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.entity_upsert_record(record),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.entity_upsert_record(record),
        }
    }

    fn suggest_similar_entities(&self, name: &str) -> Result<Vec<String>> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.suggest_similar_entities(name),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.suggest_similar_entities(name),
        }
    }

    fn count_entity_facts(&self, entity_id: &EntityId) -> Result<u32> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.count_entity_facts(entity_id),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.count_entity_facts(entity_id),
        }
    }

    fn fact_add(&mut self, input: FactInput) -> Result<FactId> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_add(input),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_add(input),
        }
    }

    fn fact_add_many(&mut self, inputs: Vec<FactInput>) -> Result<Vec<FactId>> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_add_many(inputs),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_add_many(inputs),
        }
    }

    fn fact_get(&self, id: &FactId) -> Result<FactRecord> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_get(id),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_get(id),
        }
    }

    fn fact_get_many(&self, ids: &[FactId]) -> Result<Vec<FactRecord>> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_get_many(ids),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_get_many(ids),
        }
    }

    fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_list(opts),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_list(opts),
        }
    }

    fn fact_update(&mut self, id: &FactId, input: FactUpdate) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_update(id, input),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_update(id, input),
        }
    }

    fn fact_delete(&mut self, id: &FactId) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_delete(id),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_delete(id),
        }
    }

    fn fact_upsert_record(&mut self, record: FactRecord) -> Result<bool> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_upsert_record(record),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_upsert_record(record),
        }
    }

    fn pinned_facts(&self, limit: usize) -> Result<Vec<FactRecord>> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.pinned_facts(limit),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.pinned_facts(limit),
        }
    }

    fn fact_feedback(&mut self, id: &FactId, helpful: bool) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.fact_feedback(id, helpful),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.fact_feedback(id, helpful),
        }
    }

    fn search_session_add(&mut self, session: &SearchSession) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.search_session_add(session),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.search_session_add(session),
        }
    }

    fn search_feedback_add(&mut self, feedback: &SearchFeedback) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.search_feedback_add(feedback),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.search_feedback_add(feedback),
        }
    }

    fn search_feedback_count(&self) -> Result<usize> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.search_feedback_count(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.search_feedback_count(),
        }
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_train(&mut self) -> Result<crate::types::AdaptResult> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.adapt_train(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.adapt_train(),
        }
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_status(&self) -> Result<crate::types::AdaptStatus> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.adapt_status(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.adapt_status(),
        }
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_eval(&self) -> Result<crate::types::AdaptEvalReport> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.adapt_eval(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.adapt_eval(),
        }
    }

    #[cfg(feature = "semantic-adapt")]
    fn load_adapter(&self) -> Result<crate::adapt::DomainAdapter> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.load_adapter(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.load_adapter(),
        }
    }

    fn relation_add(&mut self, input: RelationInput) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.relation_add(input),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.relation_add(input),
        }
    }

    fn relation_upsert_record(&mut self, record: RelationRecord) -> Result<bool> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.relation_upsert_record(record),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.relation_upsert_record(record),
        }
    }

    fn relation_remove(&mut self, source: &str, target: &str, rel_type: &str) -> Result<()> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.relation_remove(source, target, rel_type),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.relation_remove(source, target, rel_type),
        }
    }

    fn relations_get(
        &self,
        entity_name: &str,
        direction: TraverseDirection,
    ) -> Result<Vec<RelationRecord>> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.relations_get(entity_name, direction),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.relations_get(entity_name, direction),
        }
    }

    fn relations_list_all(&self) -> Result<Vec<RelationRecord>> {
        match self {
            #[cfg(feature = "redb")]
            StoreKind::Redb(store) => store.relations_list_all(),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite(store) => store.relations_list_all(),
        }
    }
}
