//! redb storage layer for WikiGraph.
//!
//! Provides persistent storage for entities, relations, facts, and metadata.
//! Uses ULID-based canonical keys with name/alias secondary indexes.

use crate::config::Config;
use crate::error::{Result, WgError};
use crate::types::*;
use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use std::sync::Arc;
use ulid::Ulid;

// Table definitions
pub(crate) const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
pub(crate) const ENTITIES_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("entities");
pub(crate) const ENTITY_BY_NAME_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("entity_by_name");
pub(crate) const RELATIONS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("relations");
pub(crate) const RELATIONS_REV_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("relations_rev");
pub(crate) const FACTS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("facts");
pub(crate) const FACT_BY_ENTITY_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("fact_by_entity");
pub(crate) const SEARCH_SESSIONS_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("search_sessions");
pub(crate) const SEARCH_FEEDBACK_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("search_feedback");

// Schema version
const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Shared database state.
pub struct Store {
    db: Arc<Database>,
    config: Arc<Config>,
}

// Implement Send + Sync since Arc<Database> is thread-safe
unsafe impl Send for Store {}
unsafe impl Sync for Store {}

impl Store {
    /// Access the store configuration.
    #[allow(dead_code)]
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Open a write transaction with the durability level configured
    /// in `store.durability`. Defaults to `Immediate` (per-commit
    /// fsync); `Eventual` is honored when the user has explicitly
    /// opted in. An unrecognized value falls back to `Immediate` —
    /// the safe choice — and `set` validation already prevents
    /// other strings from landing in the config in the first place.
    fn begin_write(&self) -> Result<redb::WriteTransaction> {
        let mut txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;
        let durability = match self.config.store.durability.as_str() {
            "eventual" => redb::Durability::Eventual,
            _ => redb::Durability::Immediate,
        };
        txn.set_durability(durability);
        Ok(txn)
    }

    /// Open or create a WikiGraph store at the given path.
    #[tracing::instrument(level = "debug", skip(config), fields(path = %path.display()))]
    pub fn open(path: &Path, config: Config) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WgError::StoreOpen {
                path: path.to_path_buf(),
                source: Box::new(e),
            })?;
        }

        let db = Self::open_with_retry(path, config.store.lock_retry_ms)?;

        let store = Self {
            db: Arc::new(db),
            config: Arc::new(config),
        };

        // Initialize schema if needed
        store.init_schema()?;

        Ok(store)
    }

    /// Open the redb file, retrying on lock contention when configured.
    /// Polls every 100 ms up to `retry_ms`. retry_ms=0 → original
    /// fail-fast behaviour.
    fn open_with_retry(path: &Path, retry_ms: u64) -> Result<Database> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(retry_ms);
        loop {
            match Database::create(path) {
                Ok(db) => return Ok(db),
                Err(e) if Self::is_lock_error(&e) && std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(WgError::StoreOpen {
                        path: path.to_path_buf(),
                        source: Box::new(e),
                    });
                }
            }
        }
    }

    /// Best-effort detection of redb's "another process holds the lock"
    /// error. redb 2.x reports it as `DatabaseError::DatabaseAlreadyOpen`,
    /// but variant names have shifted across releases — match the stable
    /// substring of the rendered message instead so we keep working
    /// across minor bumps.
    fn is_lock_error(err: &redb::DatabaseError) -> bool {
        let msg = err.to_string();
        msg.contains("Database already open") || msg.contains("Cannot acquire lock")
    }

    /// Initialize schema (create tables if they don't exist).
    fn init_schema(&self) -> Result<()> {
        let write_txn = self.begin_write()?;

        // Create tables
        write_txn.open_table(META_TABLE).ok();
        write_txn.open_table(ENTITIES_TABLE).ok();
        write_txn.open_table(ENTITY_BY_NAME_TABLE).ok();
        write_txn.open_table(RELATIONS_TABLE).ok();
        write_txn.open_table(RELATIONS_REV_TABLE).ok();
        write_txn.open_table(FACTS_TABLE).ok();
        write_txn.open_table(FACT_BY_ENTITY_TABLE).ok();
        write_txn.open_table(SEARCH_SESSIONS_TABLE).ok();
        write_txn.open_table(SEARCH_FEEDBACK_TABLE).ok();

        // Set schema version if not exists
        {
            let mut meta = write_txn
                .open_table(META_TABLE)
                .map_err(|e| WgError::StoreRead {
                    table: "meta",
                    key: "schema_version".to_string(),
                    source: Box::new(e),
                })?;

            let version_key = "schema_version";
            if meta
                .get(version_key)
                .map_err(|e| WgError::StoreRead {
                    table: "meta",
                    key: version_key.to_string(),
                    source: Box::new(e),
                })?
                .is_none()
            {
                meta.insert(version_key, CURRENT_SCHEMA_VERSION.to_le_bytes().as_slice())
                    .map_err(|e| WgError::StoreWrite {
                        table: "meta",
                        key: version_key.to_string(),
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "meta",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(())
    }

    /// Get the schema version.
    pub fn schema_version(&self) -> Result<u32> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let meta = read_txn
            .open_table(META_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "meta",
                key: "schema_version".to_string(),
                source: Box::new(e),
            })?;

        let version_key = "schema_version";
        let version = meta
            .get(version_key)
            .map_err(|e| WgError::StoreRead {
                table: "meta",
                key: version_key.to_string(),
                source: Box::new(e),
            })?
            .ok_or(WgError::SchemaVersionMismatch {
                found: 0,
                expected: CURRENT_SCHEMA_VERSION,
            })?;

        let bytes: [u8; 4] = version
            .value()
            .try_into()
            .map_err(|_| WgError::Internal("invalid schema_version bytes".to_string()))?;

        Ok(u32::from_le_bytes(bytes))
    }

    /// Get store statistics.
    pub fn stats(&self) -> Result<StoreStats> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let entities = read_txn
            .open_table(ENTITIES_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "entities",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let relations = read_txn
            .open_table(RELATIONS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "relations",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let meta = read_txn
            .open_table(META_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "meta",
                key: "stats".to_string(),
                source: Box::new(e),
            })?;

        let last_ingest_at = meta
            .get("last_ingest_at")
            .map_err(|e| WgError::StoreRead {
                table: "meta",
                key: "last_ingest_at".to_string(),
                source: Box::new(e),
            })?
            .map(|v| {
                let bytes: [u8; 8] = v.value().try_into().unwrap_or_default();
                u64::from_le_bytes(bytes)
            });

        // Get file size
        let path = Path::new(&self.config.store.path);
        let total_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

        Ok(StoreStats {
            entity_count: entities
                .iter()
                .map_err(|e| WgError::StoreRead {
                    table: "entities",
                    key: "<iter>".to_string(),
                    source: Box::new(e),
                })?
                .count() as u64,
            fact_count: facts
                .iter()
                .map_err(|e| WgError::StoreRead {
                    table: "facts",
                    key: "<iter>".to_string(),
                    source: Box::new(e),
                })?
                .count() as u64,
            relation_count: relations
                .iter()
                .map_err(|e| WgError::StoreRead {
                    table: "relations",
                    key: "<iter>".to_string(),
                    source: Box::new(e),
                })?
                .count() as u64,
            total_size_bytes: total_size,
            last_ingest_at,
        })
    }

    /// Update the last_ingest_at timestamp in meta.
    pub fn set_last_ingest_at(&self) -> Result<()> {
        let write_txn = self.begin_write()?;
        let mut meta = write_txn
            .open_table(META_TABLE)
            .map_err(|e| WgError::StoreWrite {
                table: "meta",
                key: "last_ingest_at".to_string(),
                source: Box::new(e),
            })?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        meta.insert("last_ingest_at", now.to_le_bytes().as_slice())
            .map_err(|e| WgError::StoreWrite {
                table: "meta",
                key: "last_ingest_at".to_string(),
                source: Box::new(e),
            })?;
        drop(meta);
        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "meta",
            key: "last_ingest_at".to_string(),
            source: Box::new(e),
        })?;
        Ok(())
    }

    // === Entity Operations ===

    /// Add a new entity.
    pub fn entity_add(&mut self, input: EntityInput) -> Result<EntityId> {
        let id = EntityId::new();
        let mut record =
            EntityRecord::new(input.name.clone(), input.entity_type.unwrap_or_default());
        // Use the same id — don't let EntityRecord generate a different one
        record.id = id;

        if let Some(aliases) = input.aliases {
            record.aliases = aliases;
        }
        if let Some(tags) = input.tags {
            record.tags = tags;
        }
        if let Some(source_page) = input.source_page {
            record.source_page = Some(source_page);
        }

        let write_txn = self.begin_write()?;

        // Check if entity with same name exists
        {
            let by_name =
                write_txn
                    .open_table(ENTITY_BY_NAME_TABLE)
                    .map_err(|e| WgError::StoreRead {
                        table: "entity_by_name",
                        key: record.name_lower.clone(),
                        source: Box::new(e),
                    })?;

            if by_name
                .get(&record.name_lower as &str)
                .map_err(|e| WgError::StoreRead {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                })?
                .is_some()
            {
                return Err(WgError::EntityAlreadyExists { name: input.name });
            }
        }

        // Serialize record
        let record_bytes = serde_json::to_vec(&record).map_err(|e| WgError::Serialize {
            context: format!("entity {:?}", id),
            source: e,
        })?;

        // Insert into entities table
        {
            let mut entities =
                write_txn
                    .open_table(ENTITIES_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "entities",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

            entities
                .insert(id.as_bytes().as_slice(), record_bytes.as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }

        // Insert into name index
        {
            let mut by_name =
                write_txn
                    .open_table(ENTITY_BY_NAME_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "entity_by_name",
                        key: record.name_lower.clone(),
                        source: Box::new(e),
                    })?;

            by_name
                .insert(&record.name_lower as &str, id.as_bytes().as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                })?;

            // Also index aliases
            for alias in &record.aliases {
                let alias_lower = alias.to_lowercase();
                by_name
                    .insert(&alias_lower as &str, id.as_bytes().as_slice())
                    .map_err(|e| WgError::StoreWrite {
                        table: "entity_by_name",
                        key: alias_lower,
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "entities",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(id)
    }

    /// Get an entity by name.
    pub fn entity_get(&self, name: &str) -> Result<EntityRecord> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Look up ID by name
        let by_name =
            read_txn
                .open_table(ENTITY_BY_NAME_TABLE)
                .map_err(|e| WgError::StoreRead {
                    table: "entity_by_name",
                    key: name.to_lowercase(),
                    source: Box::new(e),
                })?;

        let id_bytes = by_name
            .get(name.to_lowercase().as_str())
            .map_err(|e| WgError::StoreRead {
                table: "entity_by_name",
                key: name.to_lowercase(),
                source: Box::new(e),
            })?
            .ok_or_else(|| {
                let suggestions = self.suggest_similar_entities(name).unwrap_or_default();
                WgError::entity_not_found(name.to_string(), suggestions)
            })?;

        let id =
            EntityId(Ulid::from_bytes(id_bytes.value().try_into().map_err(
                |_| WgError::Internal("invalid entity id bytes".to_string()),
            )?));

        // Get the entity record
        let entities = read_txn
            .open_table(ENTITIES_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "entities",
                key: id.to_string(),
                source: Box::new(e),
            })?;

        let record_bytes = entities
            .get(id.as_bytes().as_slice())
            .map_err(|e| WgError::StoreRead {
                table: "entities",
                key: id.to_string(),
                source: Box::new(e),
            })?
            .ok_or_else(|| WgError::EntityIdNotFound(id.to_string()))?;

        serde_json::from_slice(record_bytes.value()).map_err(|e| WgError::Deserialize {
            context: format!("entity {:?}", id),
            source: e,
        })
    }

    /// Get an entity by ID.
    pub fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let entities = read_txn
            .open_table(ENTITIES_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "entities",
                key: id.to_string(),
                source: Box::new(e),
            })?;

        let record_bytes =
            entities
                .get(id.as_bytes().as_slice())
                .map_err(|e| WgError::StoreRead {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;

        if let Some(record_bytes) = record_bytes {
            let mut record: EntityRecord =
                serde_json::from_slice(record_bytes.value()).map_err(|e| WgError::Deserialize {
                    context: format!("entity {:?}", id),
                    source: e,
                })?;
            record.id = id;
            return Ok(record);
        }

        // Compatibility fallback for legacy rows whose stored JSON id doesn't match the table key.
        for entry in entities.iter().map_err(|e| WgError::StoreRead {
            table: "entities",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (_key, value) = entry.map_err(|e| WgError::StoreRead {
                table: "entities",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;

            let mut record: EntityRecord =
                serde_json::from_slice(value.value()).map_err(|e| WgError::Deserialize {
                    context: format!("entity {:?}", id),
                    source: e,
                })?;

            if record.id == id {
                record.id = id;
                return Ok(record);
            }
        }

        Err(WgError::EntityIdNotFound(id.to_string()))
    }

    /// Update an entity.
    pub fn entity_update(&mut self, name: &str, input: EntityUpdate) -> Result<()> {
        let _read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Get current record
        let record = self.entity_get(name)?;

        let mut updated = record.clone();
        updated.update(input);

        let write_txn = self.begin_write()?;

        // Serialize updated record
        let record_bytes = serde_json::to_vec(&updated).map_err(|e| WgError::Serialize {
            context: format!("entity update {:?}", record.id),
            source: e,
        })?;

        // Update entities table
        {
            let mut entities =
                write_txn
                    .open_table(ENTITIES_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "entities",
                        key: record.id.to_string(),
                        source: Box::new(e),
                    })?;

            entities
                .insert(record.id.as_bytes().as_slice(), record_bytes.as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "entities",
                    key: record.id.to_string(),
                    source: Box::new(e),
                })?;
        }

        // Update name index if name changed
        if record.name_lower != updated.name_lower {
            let mut by_name =
                write_txn
                    .open_table(ENTITY_BY_NAME_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "entity_by_name",
                        key: "update".to_string(),
                        source: Box::new(e),
                    })?;

            // Remove old name
            by_name
                .remove(record.name_lower.as_str())
                .map_err(|e| WgError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower,
                    source: Box::new(e),
                })?;

            // Add new name
            by_name
                .insert(&updated.name_lower as &str, record.id.as_bytes().as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "entity_by_name",
                    key: updated.name_lower.clone(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "entities",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(())
    }

    /// List entities with options.
    pub fn entity_list(&self, opts: ListOpts) -> Result<Vec<EntitySummary>> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let entities = read_txn
            .open_table(ENTITIES_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "entities",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let _facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let mut results: Vec<EntitySummary> = Vec::new();

        for entry in entities.iter().map_err(|e| WgError::StoreRead {
            table: "entities",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (key, value) = entry.map_err(|e| WgError::StoreRead {
                table: "entities",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;

            let mut record: EntityRecord =
                serde_json::from_slice(value.value()).map_err(|e| WgError::Deserialize {
                    context: "entity list".to_string(),
                    source: e,
                })?;

            let id = match key.value().try_into() {
                Ok(bytes) => EntityId(Ulid::from_bytes(bytes)),
                Err(_) => record.id,
            };
            record.id = id;

            // Apply filters
            if let Some(ref entity_type) = opts.entity_type {
                if &record.entity_type != entity_type {
                    continue;
                }
            }

            // Count facts for this entity
            let fact_count = self.count_entity_facts_internal(&read_txn, &record.id)?;

            if let Some(min_facts) = opts.min_facts {
                if fact_count < min_facts {
                    continue;
                }
            }

            results.push(EntitySummary {
                id: record.id,
                name: record.name,
                entity_type: record.entity_type,
                fact_count,
                tags: record.tags,
            });
        }

        // Apply sorting
        match opts.sort_by {
            EntitySort::Name => results.sort_by(|a, b| a.name.cmp(&b.name)),
            EntitySort::UpdatedAt => results.sort_by(|a, b| {
                let a_rec = self.entity_get_by_id(a.id).ok();
                let b_rec = self.entity_get_by_id(b.id).ok();
                b_rec
                    .and_then(|r| r.updated_at.checked_sub(r.created_at))
                    .cmp(&a_rec.and_then(|r| r.updated_at.checked_sub(r.created_at)))
            }),
            EntitySort::FactCount => results.sort_by_key(|e| std::cmp::Reverse(e.fact_count)),
        }

        // Apply pagination
        let offset = opts.offset;
        let limit = opts.limit.unwrap_or(usize::MAX);
        results = results.into_iter().skip(offset).take(limit).collect();

        Ok(results)
    }

    fn count_entity_facts_internal(
        &self,
        txn: &redb::ReadTransaction,
        entity_id: &EntityId,
    ) -> Result<u32> {
        let fact_by_entity =
            txn.open_table(FACT_BY_ENTITY_TABLE)
                .map_err(|e| WgError::StoreRead {
                    table: "fact_by_entity",
                    key: entity_id.to_string(),
                    source: Box::new(e),
                })?;

        // Range scan via the `{entity_id}\0` prefix → `{entity_id}\x01`
        // upper bound (one byte past the separator). Sub-linear in the
        // total fact count: redb only walks the entries actually owned
        // by this entity. Earlier code did a full `iter()` + prefix
        // filter, which was O(total facts).
        let lower = format!("{}\0", entity_id);
        let upper = format!("{}\u{1}", entity_id);
        let count = fact_by_entity
            .range::<&str>(lower.as_str()..upper.as_str())
            .map_err(|e| WgError::StoreRead {
                table: "fact_by_entity",
                key: "<range>".to_string(),
                source: Box::new(e),
            })?
            .filter_map(|entry| entry.ok())
            .count() as u32;

        Ok(count)
    }

    /// Count facts attached to an entity using the `fact_by_entity`
    /// secondary index. Public so graph traversal and other read paths
    /// can avoid scanning the full facts table just to get a count.
    pub fn count_entity_facts(&self, entity_id: &EntityId) -> Result<u32> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;
        self.count_entity_facts_internal(&read_txn, entity_id)
    }

    /// Delete an entity.
    pub fn entity_delete(&mut self, name: &str) -> Result<()> {
        let record = self.entity_get(name)?;
        let id = record.id;

        let write_txn = self.begin_write()?;

        // Remove from entities table
        {
            let mut entities =
                write_txn
                    .open_table(ENTITIES_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "entities",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

            entities
                .remove(id.as_bytes().as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }

        // Remove from name index
        {
            let mut by_name =
                write_txn
                    .open_table(ENTITY_BY_NAME_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "entity_by_name",
                        key: "delete".to_string(),
                        source: Box::new(e),
                    })?;

            by_name
                .remove(record.name_lower.as_str())
                .map_err(|e| WgError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower,
                    source: Box::new(e),
                })?;

            for alias in &record.aliases {
                by_name
                    .remove(alias.to_lowercase().as_str())
                    .map_err(|e| WgError::StoreWrite {
                        table: "entity_by_name",
                        key: alias.to_lowercase(),
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "entities",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(())
    }

    /// Suggest similar entity names for fuzzy matching.
    pub fn suggest_similar_entities(&self, name: &str) -> Result<Vec<String>> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let entities = read_txn
            .open_table(ENTITIES_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "entities",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let mut suggestions = Vec::new();
        let name_lower = name.to_lowercase();

        for entry in entities.iter().map_err(|e| WgError::StoreRead {
            table: "entities",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (_key, value) = entry.map_err(|e| WgError::StoreRead {
                table: "entities",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;

            let record: EntityRecord =
                serde_json::from_slice(value.value()).map_err(|e| WgError::Deserialize {
                    context: "suggest entities".to_string(),
                    source: e,
                })?;

            // Use trigram similarity
            let similarity = trigram::similarity(&name_lower, &record.name_lower);
            if similarity > 0.5 {
                suggestions.push((record.name.clone(), similarity));
            }

            // Check aliases
            for alias in &record.aliases {
                let similarity = trigram::similarity(&name_lower, &alias.to_lowercase());
                if similarity > 0.5 {
                    suggestions.push((record.name.clone(), similarity));
                    break;
                }
            }
        }

        // Sort by similarity and return top 5
        suggestions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(suggestions
            .into_iter()
            .take(5)
            .map(|(name, _)| name)
            .collect())
    }

    /// Resolve an entity name to an ID.
    pub fn resolve_entity(&self, name: &str) -> Result<EntityId> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let by_name =
            read_txn
                .open_table(ENTITY_BY_NAME_TABLE)
                .map_err(|e| WgError::StoreRead {
                    table: "entity_by_name",
                    key: name.to_lowercase(),
                    source: Box::new(e),
                })?;

        by_name
            .get(name.to_lowercase().as_str())
            .map_err(|e| WgError::StoreRead {
                table: "entity_by_name",
                key: name.to_lowercase(),
                source: Box::new(e),
            })?
            .ok_or_else(|| {
                let suggestions = self.suggest_similar_entities(name).unwrap_or_default();
                WgError::entity_not_found(name.to_string(), suggestions)
            })
            .map(|v| {
                let bytes: [u8; 16] = v
                    .value()
                    .try_into()
                    .map_err(|_| WgError::Internal("invalid entity id bytes".to_string()))
                    .unwrap();
                EntityId(Ulid::from_bytes(bytes))
            })
    }

    // === Fact Operations ===

    /// Add a new fact.
    pub fn fact_add(&mut self, input: FactInput) -> Result<FactId> {
        // Single-item path delegates to the batch path so there's
        // exactly one place that knows how to write a fact.
        let mut ids = self.fact_add_many(vec![input])?;
        ids.pop()
            .ok_or_else(|| WgError::InvalidInput("fact_add_many returned no ids".to_string()))
    }

    /// Insert N facts in a single redb write transaction. Amortizes
    /// the per-commit fsync over the whole batch — at the same scale,
    /// one-by-one `fact_add` pays ~3-5 ms per fsync on macOS APFS;
    /// `fact_add_many` pays it once for the whole vec.
    ///
    /// All-or-nothing: a serialization or write failure aborts the
    /// transaction and no facts land. The returned `Vec<FactId>` is
    /// in the same order as `inputs`.
    pub fn fact_add_many(&mut self, inputs: Vec<FactInput>) -> Result<Vec<FactId>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let mut records: Vec<FactRecord> = Vec::with_capacity(inputs.len());
        for input in inputs {
            let id = FactId::new();
            let mut record = FactRecord::new(
                input.content.clone(),
                input.fact_type.unwrap_or_default(),
                input.entity_ids.unwrap_or_default(),
            );
            record.id = id;

            if let Some(tags) = input.tags {
                record.tags = tags;
            }
            if let Some(source) = input.source {
                record.source = Some(source);
            }
            if let Some(confidence) = input.source_confidence {
                record.source_confidence = confidence;
            }
            if let Some(observed_at) = input.observed_at {
                record.observed_at = Some(observed_at);
            }
            records.push(record);
        }

        let write_txn = self.begin_write()?;

        {
            let mut facts = write_txn
                .open_table(FACTS_TABLE)
                .map_err(|e| WgError::StoreWrite {
                    table: "facts",
                    key: "<batch>".to_string(),
                    source: Box::new(e),
                })?;
            let mut fact_by_entity =
                write_txn
                    .open_table(FACT_BY_ENTITY_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_by_entity",
                        key: "<batch>".to_string(),
                        source: Box::new(e),
                    })?;

            for record in &records {
                let id = record.id;
                let record_bytes = serde_json::to_vec(record).map_err(|e| WgError::Serialize {
                    context: format!("fact {:?}", id),
                    source: e,
                })?;

                facts
                    .insert(id.as_bytes().as_slice(), record_bytes.as_slice())
                    .map_err(|e| WgError::StoreWrite {
                        table: "facts",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

                for entity_id in &record.entity_ids {
                    let key = format!("{}\0{}", entity_id, id);
                    fact_by_entity
                        .insert(&key as &str, id.as_bytes().as_slice())
                        .map_err(|e| WgError::StoreWrite {
                            table: "fact_by_entity",
                            key,
                            source: Box::new(e),
                        })?;
                }
            }
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "facts",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(records.into_iter().map(|r| r.id).collect())
    }

    /// Get a fact by ID.
    /// Fetch many facts by id in a single read transaction.
    ///
    /// Used by the search path to hydrate a BM25 / HNSW candidate slate
    /// without opening one redb txn per id (each `begin_read` +
    /// `open_table` pair is ~20 µs of overhead, so 64 candidates was
    /// ~2 ms of pure transaction setup). Returned records preserve
    /// input order; missing ids are silently skipped.
    pub fn fact_get_many(&self, ids: &[FactId]) -> Result<Vec<FactRecord>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;
        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: "<fact_get_many>".to_string(),
                source: Box::new(e),
            })?;

        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            let entry = match facts.get(id.as_bytes().as_slice()) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(_) => continue,
            };
            let mut record: FactRecord = match serde_json::from_slice(entry.value()) {
                Ok(r) => r,
                Err(_) => continue,
            };
            record.record_access();
            results.push(record);
        }
        Ok(results)
    }

    pub fn fact_get(&self, id: &FactId) -> Result<FactRecord> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: id.to_string(),
                source: Box::new(e),
            })?;

        let record_bytes = facts
            .get(id.as_bytes().as_slice())
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: id.to_string(),
                source: Box::new(e),
            })?
            .ok_or(WgError::FactNotFound(id.to_string()))?;

        let mut record: FactRecord =
            serde_json::from_slice(record_bytes.value()).map_err(|e| WgError::Deserialize {
                context: format!("fact {:?}", id),
                source: e,
            })?;

        // Record access
        record.record_access();

        Ok(record)
    }

    /// Update a fact.
    pub fn fact_update(&mut self, id: &FactId, input: FactUpdate) -> Result<()> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Get current record
        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: id.to_string(),
                source: Box::new(e),
            })?;

        let record_bytes = facts
            .get(id.as_bytes().as_slice())
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: id.to_string(),
                source: Box::new(e),
            })?
            .ok_or(WgError::FactNotFound(id.to_string()))?;

        let mut record: FactRecord =
            serde_json::from_slice(record_bytes.value()).map_err(|e| WgError::Deserialize {
                context: format!("fact update {:?}", id),
                source: e,
            })?;

        record.update(input);

        let write_txn = self.begin_write()?;

        let record_bytes = serde_json::to_vec(&record).map_err(|e| WgError::Serialize {
            context: format!("fact update {:?}", id),
            source: e,
        })?;

        {
            let mut facts = write_txn
                .open_table(FACTS_TABLE)
                .map_err(|e| WgError::StoreWrite {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;

            facts
                .insert(id.as_bytes().as_slice(), record_bytes.as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "facts",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(())
    }

    /// Delete a fact.
    pub fn fact_delete(&mut self, id: &FactId) -> Result<()> {
        let write_txn = self.begin_write()?;

        // Get current record to find entity IDs
        let record: FactRecord = {
            let facts = write_txn
                .open_table(FACTS_TABLE)
                .map_err(|e| WgError::StoreRead {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;

            let record_bytes = facts
                .get(id.as_bytes().as_slice())
                .map_err(|e| WgError::StoreRead {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?
                .ok_or(WgError::FactNotFound(id.to_string()))?;

            serde_json::from_slice(record_bytes.value()).map_err(|e| WgError::Deserialize {
                context: format!("fact delete {:?}", id),
                source: e,
            })?
        };

        // Remove from facts table
        {
            let mut facts = write_txn
                .open_table(FACTS_TABLE)
                .map_err(|e| WgError::StoreWrite {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;

            facts
                .remove(id.as_bytes().as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }

        // Remove from fact_by_entity index
        {
            let mut fact_by_entity =
                write_txn
                    .open_table(FACT_BY_ENTITY_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_by_entity",
                        key: "delete".to_string(),
                        source: Box::new(e),
                    })?;

            for entity_id in &record.entity_ids {
                let key = format!("{}\0{}", entity_id, id);
                fact_by_entity
                    .remove(&key as &str)
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_by_entity",
                        key,
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "facts",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(())
    }

    /// List facts with options.
    pub fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let mut results: Vec<FactRecord> = Vec::new();

        for entry in facts.iter().map_err(|e| WgError::StoreRead {
            table: "facts",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (_key, value) = entry.map_err(|e| WgError::StoreRead {
                table: "facts",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;

            let record: FactRecord =
                serde_json::from_slice(value.value()).map_err(|e| WgError::Deserialize {
                    context: "fact list".to_string(),
                    source: e,
                })?;

            // Apply filters
            if let Some(ref fact_type) = opts.fact_type {
                if &record.fact_type != fact_type {
                    continue;
                }
            }

            if let Some(min_confidence) = opts.min_confidence {
                if record.source_confidence < min_confidence {
                    continue;
                }
            }

            if let Some(entity_id) = opts.entity_id {
                if !record.entity_ids.contains(&entity_id) {
                    continue;
                }
            }

            // Time filter: prefer observed_at (real-world time) over created_at (DB insertion).
            if opts.since.is_some() || opts.until.is_some() {
                let ts = record.observed_at.unwrap_or(record.created_at);
                if let Some(since) = opts.since {
                    if ts < since {
                        continue;
                    }
                }
                if let Some(until) = opts.until {
                    if ts > until {
                        continue;
                    }
                }
            }

            // Current-only filter: skip facts that have been superseded.
            if opts.current_only && record.superseded_at.is_some() {
                continue;
            }

            // As-of filter: include this fact only if it (a) existed
            // at the as_of time and (b) wasn't superseded yet then.
            if let Some(as_of) = opts.as_of {
                if record.created_at > as_of {
                    continue;
                }
                if let Some(superseded_at) = record.superseded_at
                    && superseded_at <= as_of
                {
                    continue;
                }
            }

            results.push(record);
        }

        // Apply pagination
        let offset = opts.offset;
        let limit = opts.limit.unwrap_or(usize::MAX);
        results = results.into_iter().skip(offset).take(limit).collect();

        Ok(results)
    }

    /// Record fact feedback.
    pub fn fact_feedback(&mut self, id: &FactId, helpful: bool) -> Result<()> {
        let mut record = self.fact_get(id)?;

        // Update relevance score based on feedback
        if helpful {
            record.relevance_score = (record.relevance_score + 0.10).min(1.0);
        } else {
            record.relevance_score = (record.relevance_score - 0.15).max(0.0);
        }

        self.fact_update(
            id,
            FactUpdate {
                content: None,
                fact_type: None,
                tags: None,
                source: None,
                observed_at: None,
                superseded_at: None,
                superseded_by: None,
            },
        )?;

        Ok(())
    }

    /// Add a search session record.
    pub fn search_session_add(&mut self, session: &SearchSession) -> Result<()> {
        let write_txn = self.begin_write()?;

        let mut table =
            write_txn
                .open_table(SEARCH_SESSIONS_TABLE)
                .map_err(|e| WgError::StoreWrite {
                    table: "search_sessions",
                    key: session.id.clone(),
                    source: Box::new(e),
                })?;

        let bytes = serde_json::to_vec(session).map_err(|e| WgError::Serialize {
            context: "search_session".to_string(),
            source: e,
        })?;

        table
            .insert(session.id.as_str(), bytes.as_slice())
            .map_err(|e| WgError::StoreWrite {
                table: "search_sessions",
                key: session.id.clone(),
                source: Box::new(e),
            })?;
        drop(table);

        write_txn
            .commit()
            .map_err(|e| WgError::Internal(format!("transaction commit failed: {}", e)))?;

        Ok(())
    }

    /// Add a search feedback record.
    pub fn search_feedback_add(&mut self, feedback: &SearchFeedback) -> Result<()> {
        let write_txn = self.begin_write()?;

        let mut table =
            write_txn
                .open_table(SEARCH_FEEDBACK_TABLE)
                .map_err(|e| WgError::StoreWrite {
                    table: "search_feedback",
                    key: format!("{}:{}", feedback.session_id, feedback.fact_id).to_string(),
                    source: Box::new(e),
                })?;

        let bytes = serde_json::to_vec(feedback).map_err(|e| WgError::Serialize {
            context: "search_feedback".to_string(),
            source: e,
        })?;

        table
            .insert(
                format!("{}:{}", feedback.session_id, feedback.fact_id).as_str(),
                bytes.as_slice(),
            )
            .map_err(|e| WgError::StoreWrite {
                table: "search_feedback",
                key: format!("{}:{}", feedback.session_id, feedback.fact_id),
                source: Box::new(e),
            })?;
        drop(table);

        write_txn
            .commit()
            .map_err(|e| WgError::Internal(format!("transaction commit failed: {}", e)))?;

        Ok(())
    }

    /// Count total feedback entries.
    pub fn search_feedback_count(&self) -> Result<usize> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let table = read_txn
            .open_table(SEARCH_FEEDBACK_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "search_feedback",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let iter = table.iter().map_err(|e| WgError::StoreRead {
            table: "search_feedback",
            key: "<count>".to_string(),
            source: Box::new(e),
        })?;
        let count = iter.fold(0, |acc, _| acc + 1);
        Ok(count)
    }

    // === Relation Operations ===

    /// Build a relation key.
    fn relation_key(
        source_id: &EntityId,
        rel_type: &RelationType,
        target_id: &EntityId,
    ) -> Vec<u8> {
        format!("{}\0{}\0{}", source_id, rel_type.0, target_id).into_bytes()
    }

    /// Add a new relation.
    pub fn relation_add(&mut self, input: RelationInput) -> Result<()> {
        let source_id = self.resolve_entity(&input.source)?;
        let target_id = self.resolve_entity(&input.target)?;
        let rel_type = input.relation_type;
        let weight = input.weight.unwrap_or(1.0);

        let record = RelationRecord {
            source_id,
            target_id,
            relation_type: rel_type.clone(),
            weight,
            evidence: input.evidence.unwrap_or_default(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };

        let write_txn = self.begin_write()?;

        // Serialize record
        let record_bytes = serde_json::to_vec(&record).map_err(|e| WgError::Serialize {
            context: "relation".to_string(),
            source: e,
        })?;

        // Insert into relations table
        let key = Self::relation_key(&source_id, &rel_type, &target_id);
        {
            let mut relations =
                write_txn
                    .open_table(RELATIONS_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "relations",
                        key: String::from_utf8_lossy(&key).to_string(),
                        source: Box::new(e),
                    })?;

            relations
                .insert(key.as_slice(), record_bytes.as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "relations",
                    key: String::from_utf8_lossy(&key).to_string(),
                    source: Box::new(e),
                })?;
        }

        // Insert into relations_rev table (reverse key)
        let rev_key = Self::relation_key(&target_id, &rel_type, &source_id);
        {
            let mut relations_rev =
                write_txn
                    .open_table(RELATIONS_REV_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "relations_rev",
                        key: String::from_utf8_lossy(&rev_key).to_string(),
                        source: Box::new(e),
                    })?;

            relations_rev
                .insert(rev_key.as_slice(), record_bytes.as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "relations_rev",
                    key: String::from_utf8_lossy(&rev_key).to_string(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "relations",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(())
    }

    /// Remove a relation.
    pub fn relation_remove(&mut self, source: &str, target: &str, rel_type: &str) -> Result<()> {
        let source_id = self.resolve_entity(source)?;
        let target_id = self.resolve_entity(target)?;
        let rel_type = RelationType::new(rel_type);

        let write_txn = self.begin_write()?;

        let key = Self::relation_key(&source_id, &rel_type, &target_id);
        let rev_key = Self::relation_key(&target_id, &rel_type, &source_id);

        {
            let mut relations =
                write_txn
                    .open_table(RELATIONS_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "relations",
                        key: String::from_utf8_lossy(&key).to_string(),
                        source: Box::new(e),
                    })?;

            relations
                .remove(key.as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "relations",
                    key: String::from_utf8_lossy(&key).to_string(),
                    source: Box::new(e),
                })?;
        }

        {
            let mut relations_rev =
                write_txn
                    .open_table(RELATIONS_REV_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "relations_rev",
                        key: String::from_utf8_lossy(&rev_key).to_string(),
                        source: Box::new(e),
                    })?;

            relations_rev
                .remove(rev_key.as_slice())
                .map_err(|e| WgError::StoreWrite {
                    table: "relations_rev",
                    key: String::from_utf8_lossy(&rev_key).to_string(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "relations",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(())
    }

    /// Get relations for an entity.
    pub fn relations_get(
        &self,
        entity_name: &str,
        direction: TraverseDirection,
    ) -> Result<Vec<RelationRecord>> {
        let entity_id = self.resolve_entity(entity_name)?;

        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let mut results = Vec::new();

        // Range bounds for `{entity_id}\0...` prefix scan. Encode the
        // upper bound as `{entity_id}\x01` — one byte past the `\0`
        // separator, which is greater than every key starting with
        // `{entity_id}\0` regardless of suffix. Doing this with redb's
        // `range()` keeps the scan sub-linear; the previous `iter()` +
        // prefix-filter was O(total relations).
        let lower = format!("{}\0", entity_id).into_bytes();
        let upper = format!("{}\u{1}", entity_id).into_bytes();

        match direction {
            TraverseDirection::Forward | TraverseDirection::Both => {
                let relations =
                    read_txn
                        .open_table(RELATIONS_TABLE)
                        .map_err(|e| WgError::StoreRead {
                            table: "relations",
                            key: "<all>".to_string(),
                            source: Box::new(e),
                        })?;

                for entry in relations
                    .range::<&[u8]>(lower.as_slice()..upper.as_slice())
                    .map_err(|e| WgError::StoreRead {
                        table: "relations",
                        key: "<range>".to_string(),
                        source: Box::new(e),
                    })?
                {
                    let (_key, value) = entry.map_err(|e| WgError::StoreRead {
                        table: "relations",
                        key: "<entry>".to_string(),
                        source: Box::new(e),
                    })?;

                    let record: RelationRecord =
                        serde_json::from_slice(value.value()).map_err(|e| {
                            WgError::Deserialize {
                                context: "relation get".to_string(),
                                source: e,
                            }
                        })?;
                    results.push(record);
                }
            }
            TraverseDirection::Reverse => {
                let relations_rev =
                    read_txn
                        .open_table(RELATIONS_REV_TABLE)
                        .map_err(|e| WgError::StoreRead {
                            table: "relations_rev",
                            key: "<all>".to_string(),
                            source: Box::new(e),
                        })?;

                for entry in relations_rev
                    .range::<&[u8]>(lower.as_slice()..upper.as_slice())
                    .map_err(|e| WgError::StoreRead {
                        table: "relations_rev",
                        key: "<range>".to_string(),
                        source: Box::new(e),
                    })?
                {
                    let (_key, value) = entry.map_err(|e| WgError::StoreRead {
                        table: "relations_rev",
                        key: "<entry>".to_string(),
                        source: Box::new(e),
                    })?;

                    let record: RelationRecord =
                        serde_json::from_slice(value.value()).map_err(|e| {
                            WgError::Deserialize {
                                context: "relation get rev".to_string(),
                                source: e,
                            }
                        })?;
                    results.push(record);
                }
            }
        }

        Ok(results)
    }

    /// Return every relation in the store as a single list. Used by
    /// `LintEngine` to walk the graph once instead of running per-
    /// entity `relations_get` calls.
    pub fn relations_list_all(&self) -> Result<Vec<RelationRecord>> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;
        let relations = read_txn
            .open_table(RELATIONS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "relations",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let mut results = Vec::new();
        for entry in relations.iter().map_err(|e| WgError::StoreRead {
            table: "relations",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (_key, value) = entry.map_err(|e| WgError::StoreRead {
                table: "relations",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;
            let record: RelationRecord =
                serde_json::from_slice(value.value()).map_err(|e| WgError::Deserialize {
                    context: "relations_list_all".to_string(),
                    source: e,
                })?;
            results.push(record);
        }
        Ok(results)
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_test_store() -> Store {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let config = Config::default();
        Store::open(&path, config).unwrap()
    }

    #[test]
    fn open_with_retry_zero_fails_fast_when_locked() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("locked.redb");
        // Hold an exclusive open. Default config has lock_retry_ms=0
        // so the second open should fail fast (well under 1s).
        let _holder = Store::open(&path, Config::default()).unwrap();
        let start = std::time::Instant::now();
        let err = match Store::open(&path, Config::default()) {
            Ok(_) => panic!("second open should have hit the lock"),
            Err(e) => e,
        };
        let elapsed_ms = start.elapsed().as_millis();
        assert!(
            elapsed_ms < 200,
            "fail-fast should be < 200ms, took {elapsed_ms}ms"
        );
        assert!(matches!(err, WgError::StoreOpen { .. }));
    }

    #[test]
    fn open_with_retry_succeeds_when_holder_drops() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("retried.redb");
        let path_clone = path.clone();

        // Hold the lock, then release after 200 ms in a thread.
        let holder = Store::open(&path, Config::default()).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            drop(holder);
        });

        // Try to open with 2-second retry budget — should succeed
        // once the holder drops.
        let mut config = Config::default();
        config.store.lock_retry_ms = 2000;
        let start = std::time::Instant::now();
        let store = match Store::open(&path_clone, config) {
            Ok(s) => s,
            Err(e) => panic!("retry should succeed, got {e:?}"),
        };
        let elapsed_ms = start.elapsed().as_millis();
        // Took at least one retry-poll (100 ms) but well under the
        // 2-second budget.
        assert!(
            (100..1500).contains(&(elapsed_ms as u64)),
            "expected ~200-1000ms wait, got {elapsed_ms}ms"
        );
        drop(store);
        releaser.join().unwrap();
    }

    #[test]
    fn test_entity_crud() {
        let mut store = create_test_store();

        // Create entity
        let id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                aliases: Some(vec!["redis-server".to_string()]),
                tags: Some(vec!["infra".to_string()]),
                source_page: Some("entities/redis.md".to_string()),
            })
            .unwrap();

        assert!(id.0.to_bytes() != [0u8; 16]);

        // Get entity
        let record = store.entity_get("Redis").unwrap();
        assert_eq!(record.name, "Redis");
        assert_eq!(record.entity_type, EntityType::Technology);

        // Get by ID
        let record2 = store.entity_get_by_id(id).unwrap();
        assert_eq!(record.name, record2.name);

        // List entities
        let entities = store.entity_list(ListOpts::default()).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "Redis");

        // Update entity
        store
            .entity_update(
                "Redis",
                EntityUpdate {
                    name: Some("Redis Server".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        let record3 = store.entity_get("Redis Server").unwrap();
        assert_eq!(record3.name, "Redis Server");

        // Delete entity
        store.entity_delete("Redis Server").unwrap();
        let result = store.entity_get("Redis Server");
        assert!(result.is_err());
    }

    #[test]
    fn test_fact_crud() {
        let mut store = create_test_store();

        // Create entity first
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        // Create fact
        let fact_id = store
            .fact_add(FactInput {
                content: "Redis Sentinel provides HA".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![store.resolve_entity("Redis").unwrap()]),
                tags: Some(vec!["ha".to_string()]),
                source: Some("entities/redis.md#ha".to_string()),
                source_confidence: Some(0.8),
                observed_at: None,
            })
            .unwrap();

        // Get fact
        let fact = store.fact_get(&fact_id).unwrap();
        assert_eq!(fact.content, "Redis Sentinel provides HA");
        assert_eq!(fact.source_confidence, 0.8);

        // List facts
        let facts = store.fact_list(FactListOpts::default()).unwrap();
        assert_eq!(facts.len(), 1);

        // Update fact
        store
            .fact_update(
                &fact_id,
                FactUpdate {
                    content: Some("Redis Sentinel provides HA with automatic failover".to_string()),
                    fact_type: None,
                    tags: None,
                    source: None,
                    observed_at: None,
                    superseded_at: None,
                    superseded_by: None,
                },
            )
            .unwrap();

        let updated = store.fact_get(&fact_id).unwrap();
        assert!(updated.content.contains("automatic failover"));

        // Delete fact
        store.fact_delete(&fact_id).unwrap();
        let result = store.fact_get(&fact_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_relation_crud() {
        let mut store = create_test_store();

        // Create entities
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        store
            .entity_add(EntityInput {
                name: "Sentinel".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        // Create relation
        store
            .relation_add(RelationInput {
                source: "Sentinel".to_string(),
                target: "Redis".to_string(),
                relation_type: RelationType::new("monitors"),
                weight: Some(1.0),
                evidence: Some(vec!["entities/sentinel.md".to_string()]),
            })
            .unwrap();

        // Get relations
        let relations = store
            .relations_get("Sentinel", TraverseDirection::Forward)
            .unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].relation_type.0, "monitors");

        // Remove relation
        store
            .relation_remove("Sentinel", "Redis", "monitors")
            .unwrap();
        let relations_after = store
            .relations_get("Sentinel", TraverseDirection::Forward)
            .unwrap();
        assert_eq!(relations_after.len(), 0);
    }

    #[test]
    fn test_stats() {
        let mut store = create_test_store();

        let stats = store.stats().unwrap();
        assert_eq!(stats.entity_count, 0);
        assert_eq!(stats.fact_count, 0);
        assert_eq!(stats.relation_count, 0);

        // Add entity
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        let stats2 = store.stats().unwrap();
        assert_eq!(stats2.entity_count, 1);
    }

    #[test]
    fn fact_add_many_inserts_all_in_one_txn() {
        let mut store = create_test_store();
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let redis_id = store.resolve_entity("Redis").unwrap();

        let inputs: Vec<FactInput> = (0..10)
            .map(|i| FactInput {
                content: format!("fact {i}"),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis_id]),
                source_confidence: Some(0.5),
                ..Default::default()
            })
            .collect();

        let ids = store.fact_add_many(inputs).unwrap();
        assert_eq!(ids.len(), 10);

        // Each fact is stored and findable by id.
        for id in &ids {
            let record = store.fact_get(id).unwrap();
            assert!(record.content.starts_with("fact "));
        }

        // The fact_by_entity index has one entry per fact for Redis,
        // so count_entity_facts must return 10.
        let count = store.count_entity_facts(&redis_id).unwrap();
        assert_eq!(count, 10);
    }

    #[test]
    fn fact_get_many_returns_records_in_input_order_skipping_missing() {
        let mut store = create_test_store();
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let redis_id = store.resolve_entity("Redis").unwrap();

        let inputs: Vec<FactInput> = (0..3)
            .map(|i| FactInput {
                content: format!("fact {i}"),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis_id]),
                ..Default::default()
            })
            .collect();
        let ids = store.fact_add_many(inputs).unwrap();
        assert_eq!(ids.len(), 3);

        // Empty input → empty output, no txn opened.
        assert!(store.fact_get_many(&[]).unwrap().is_empty());

        // Order preserved.
        let reversed: Vec<FactId> = ids.iter().rev().cloned().collect();
        let records = store.fact_get_many(&reversed).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].id, ids[2]);
        assert_eq!(records[1].id, ids[1]);
        assert_eq!(records[2].id, ids[0]);

        // Missing ids are silently skipped, real ids still come through.
        let missing = FactId::new();
        let mixed = vec![ids[0], missing, ids[1]];
        let records = store.fact_get_many(&mixed).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, ids[0]);
        assert_eq!(records[1].id, ids[1]);
    }

    #[test]
    fn store_eventual_durability_writes_and_reads() {
        // Confirms the `store.durability = "eventual"` config path
        // doesn't break basic read-after-write within the same process
        // (the eventual flush only matters on power loss, which the
        // test environment can't simulate). Bench data lives in
        // `benchmarks/src/bin/fsync_probe.rs`.
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.store.durability = "eventual".into();
        let mut store = Store::open(&dir.path().join("test.redb"), config).unwrap();

        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let redis_id = store.resolve_entity("Redis").unwrap();

        let id = store
            .fact_add(FactInput {
                content: "test fact".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis_id]),
                ..Default::default()
            })
            .unwrap();
        let record = store.fact_get(&id).unwrap();
        assert_eq!(record.content, "test fact");
    }

    #[test]
    fn fact_add_many_empty_input_is_noop() {
        let mut store = create_test_store();
        let ids = store.fact_add_many(vec![]).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn test_fact_list_time_filter() {
        let mut store = create_test_store();

        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let redis_id = store.resolve_entity("Redis").unwrap();

        // Three facts spanning different observed_at times.
        let make = |content: &str, observed_at: Option<u64>| FactInput {
            content: content.to_string(),
            fact_type: Some(FactType::Decision),
            entity_ids: Some(vec![redis_id]),
            tags: None,
            source: None,
            source_confidence: Some(0.5),
            observed_at,
        };

        // 2024-01-01, 2024-06-15, 2024-12-31 (UTC midnight, ms)
        let jan = 1_704_067_200_000;
        let jun = 1_718_409_600_000;
        let dec = 1_735_603_200_000;

        store.fact_add(make("jan", Some(jan))).unwrap();
        store.fact_add(make("jun", Some(jun))).unwrap();
        store.fact_add(make("dec", Some(dec))).unwrap();
        // One with no observed_at — falls back to created_at (now), should be > dec.
        store.fact_add(make("now", None)).unwrap();

        // since-only: keeps jun, dec, now
        let r = store
            .fact_list(FactListOpts {
                since: Some(jun),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r.len(), 3);

        // until-only: keeps jan, jun
        let r = store
            .fact_list(FactListOpts {
                until: Some(jun),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r.len(), 2);

        // Window jun..=dec: keeps jun, dec
        let r = store
            .fact_list(FactListOpts {
                since: Some(jun),
                until: Some(dec),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn fact_list_as_of_hides_facts_superseded_before_the_cutoff() {
        let mut store = create_test_store();
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let redis_id = store.resolve_entity("Redis").unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let one_day = 24 * 60 * 60 * 1000;
        let yesterday = now.saturating_sub(one_day);
        let tomorrow = now + one_day;

        let a = store
            .fact_add(FactInput {
                content: "use Redis 6".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![redis_id]),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .unwrap();
        let b = store
            .fact_add(FactInput {
                content: "use Redis 7".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![redis_id]),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .unwrap();

        // Mark A superseded yesterday (in the past relative to the as_of cutoff).
        store
            .fact_update(
                &a,
                FactUpdate {
                    superseded_at: Some(yesterday),
                    superseded_by: Some(b),
                    ..Default::default()
                },
            )
            .unwrap();

        // as_of in the future: A is already invalidated, only B remains.
        let r = store
            .fact_list(FactListOpts {
                entity_id: Some(redis_id),
                as_of: Some(tomorrow),
                limit: Some(100),
                ..Default::default()
            })
            .unwrap();
        let contents: Vec<_> = r.iter().map(|f| f.content.as_str()).collect();
        assert!(contents.contains(&"use Redis 7"));
        assert!(!contents.contains(&"use Redis 6"));
    }
}

// === Adapt operations (semantic-adapt feature) ===

#[cfg(feature = "semantic-adapt")]
impl Store {
    /// Train the domain adapter using all available feedback.
    pub fn adapt_train(&mut self) -> Result<crate::types::AdaptResult> {
        use crate::adapt::DomainAdapter;

        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let table = read_txn
            .open_table(SEARCH_FEEDBACK_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "search_feedback",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let mut feedback_pairs = Vec::new();
        let iter = table.iter().map_err(|e| WgError::StoreRead {
            table: "search_feedback",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })?;

        for item in iter {
            let (_, value) = item.map_err(|e| WgError::StoreRead {
                table: "search_feedback",
                key: "<item>".to_string(),
                source: Box::new(e),
            })?;
            let fb: crate::types::SearchFeedback =
                serde_json::from_slice(value.value()).map_err(|e| WgError::Deserialize {
                    context: "search_feedback".to_string(),
                    source: e,
                })?;
            feedback_pairs.push((fb.fact_id.to_string(), fb.helpful));
        }

        drop(read_txn);

        let mut adapter = DomainAdapter::new();
        let result = adapter.train(&feedback_pairs);

        // Persist adapter state to meta
        let bytes = adapter.to_bytes()?;
        self.meta_set("adapter_state", &bytes)?;

        Ok(result)
    }

    /// Get the current adapter status and statistics.
    pub fn adapt_status(&self) -> Result<crate::types::AdaptStatus> {
        let feedback_count = self.search_feedback_count()?;
        let adapter = self.load_adapter()?;
        Ok(adapter.status(feedback_count))
    }

    /// Evaluate the adapter on all available feedback.
    pub fn adapt_eval(&self) -> Result<crate::types::AdaptEvalReport> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let table = read_txn
            .open_table(SEARCH_FEEDBACK_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "search_feedback",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let mut feedback_pairs = Vec::new();
        let iter = table.iter().map_err(|e| WgError::StoreRead {
            table: "search_feedback",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })?;

        for item in iter {
            let (_, value) = item.map_err(|e| WgError::StoreRead {
                table: "search_feedback",
                key: "<item>".to_string(),
                source: Box::new(e),
            })?;
            let fb: crate::types::SearchFeedback =
                serde_json::from_slice(value.value()).map_err(|e| WgError::Deserialize {
                    context: "search_feedback".to_string(),
                    source: e,
                })?;
            feedback_pairs.push((fb.fact_id.to_string(), fb.helpful));
        }

        drop(read_txn);

        let adapter = self.load_adapter()?;
        Ok(adapter.evaluate(&feedback_pairs, 10))
    }

    /// Load the adapter from meta bytes, or return a fresh one.
    fn load_adapter(&self) -> Result<crate::adapt::DomainAdapter> {
        match self.meta_get::<Vec<u8>>("adapter_state")? {
            Some(bytes) => crate::adapt::DomainAdapter::from_bytes(&bytes),
            None => Ok(crate::adapt::DomainAdapter::new()),
        }
    }

    /// Get a meta value as bytes.
    fn meta_get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        let meta = read_txn
            .open_table(META_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "meta",
                key: key.to_string(),
                source: Box::new(e),
            })?;

        match meta.get(key).map_err(|e| WgError::StoreRead {
            table: "meta",
            key: key.to_string(),
            source: Box::new(e),
        })? {
            Some(value) => {
                let bytes = value.value();
                let val: T = serde_json::from_slice(bytes).map_err(|e| WgError::Deserialize {
                    context: format!("meta/{}", key),
                    source: e,
                })?;
                Ok(Some(val))
            }
            None => Ok(None),
        }
    }

    /// Set a meta value from bytes.
    fn meta_set(&mut self, key: &str, value: &[u8]) -> Result<()> {
        let write_txn = self.begin_write()?;

        let mut meta = write_txn
            .open_table(META_TABLE)
            .map_err(|e| WgError::StoreWrite {
                table: "meta",
                key: key.to_string(),
                source: Box::new(e),
            })?;

        meta.insert(key, value).map_err(|e| WgError::StoreWrite {
            table: "meta",
            key: key.to_string(),
            source: Box::new(e),
        })?;
        drop(meta);

        write_txn
            .commit()
            .map_err(|e| WgError::Internal(format!("meta set commit failed: {}", e)))?;

        Ok(())
    }
}
