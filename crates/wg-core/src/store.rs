//! redb storage layer for WikiGraph.
//!
//! Provides persistent storage for entities, relations, facts, and metadata.
//! Uses ULID-based canonical keys with name/alias secondary indexes.

use crate::config::Config;
use crate::error::{Result, WgError};
use crate::types::*;
use parking_lot::RwLock;
use redb::{
    Database, ReadableMultimapTable, ReadableTable, ReadableTableMetadata, TableDefinition,
};
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
    /// Open or create a WikiGraph store at the given path.
    pub fn open(path: &Path, config: Config) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WgError::StoreOpen {
                path: path.to_path_buf(),
                source: Box::new(e),
            })?;
        }

        let db = Database::create(path).map_err(|e| WgError::StoreOpen {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;

        let store = Self {
            db: Arc::new(db),
            config: Arc::new(config),
        };

        // Initialize schema if needed
        store.init_schema()?;

        Ok(store)
    }

    /// Initialize schema (create tables if they don't exist).
    fn init_schema(&self) -> Result<()> {
        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Create tables
        write_txn.open_table(META_TABLE).ok();
        write_txn.open_table(ENTITIES_TABLE).ok();
        write_txn.open_table(ENTITY_BY_NAME_TABLE).ok();
        write_txn.open_table(RELATIONS_TABLE).ok();
        write_txn.open_table(RELATIONS_REV_TABLE).ok();
        write_txn.open_table(FACTS_TABLE).ok();
        write_txn.open_table(FACT_BY_ENTITY_TABLE).ok();

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
        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;
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

        if let Some(aliases) = input.aliases {
            record.aliases = aliases;
        }
        if let Some(tags) = input.tags {
            record.tags = tags;
        }
        if let Some(source_page) = input.source_page {
            record.source_page = Some(source_page);
        }

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Check if entity with same name exists
        {
            let mut by_name =
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

        let record_bytes = entities
            .get(id.as_bytes().as_slice())
            .map_err(|e| WgError::StoreRead {
                table: "entities",
                key: id.to_string(),
                source: Box::new(e),
            })?
            .ok_or(WgError::EntityIdNotFound(id.to_string()))?;

        serde_json::from_slice(record_bytes.value()).map_err(|e| WgError::Deserialize {
            context: format!("entity {:?}", id),
            source: e,
        })
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

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

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

        let facts = read_txn
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
            let (_key, value) = entry.map_err(|e| WgError::StoreRead {
                table: "entities",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;

            let record: EntityRecord =
                serde_json::from_slice(value.value()).map_err(|e| WgError::Deserialize {
                    context: "entity list".to_string(),
                    source: e,
                })?;

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
            EntitySort::FactCount => results.sort_by(|a, b| b.fact_count.cmp(&a.fact_count)),
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

        let prefix = format!("{}\0", entity_id.to_string());
        let count = fact_by_entity
            .iter()
            .map_err(|e| WgError::StoreRead {
                table: "fact_by_entity",
                key: "<iter>".to_string(),
                source: Box::new(e),
            })?
            .filter_map(|entry| entry.ok())
            .filter(|(k, _)| {
                let key_str = k.value();
                key_str.starts_with(&prefix)
            })
            .count() as u32;

        Ok(count)
    }

    /// Delete an entity.
    pub fn entity_delete(&mut self, name: &str) -> Result<()> {
        let record = self.entity_get(name)?;
        let id = record.id;

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

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
        let id = FactId::new();
        let mut record = FactRecord::new(
            input.content.clone(),
            input.fact_type.unwrap_or_default(),
            input.entity_ids.unwrap_or_default(),
        );

        if let Some(tags) = input.tags {
            record.tags = tags;
        }
        if let Some(source) = input.source {
            record.source = Some(source);
        }
        if let Some(confidence) = input.source_confidence {
            record.source_confidence = confidence;
        }

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Serialize record
        let record_bytes = serde_json::to_vec(&record).map_err(|e| WgError::Serialize {
            context: format!("fact {:?}", id),
            source: e,
        })?;

        // Insert into facts table
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

        // Insert into fact_by_entity index for each entity
        {
            let mut fact_by_entity =
                write_txn
                    .open_table(FACT_BY_ENTITY_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_by_entity",
                        key: "insert".to_string(),
                        source: Box::new(e),
                    })?;

            for entity_id in &record.entity_ids {
                let key = format!("{}\0{}", entity_id.to_string(), id.to_string());
                fact_by_entity
                    .insert(&key as &str, id.as_bytes().as_slice())
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_by_entity",
                        key: key,
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "facts",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(id)
    }

    /// Get a fact by ID.
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

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

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
        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

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
                let key = format!("{}\0{}", entity_id.to_string(), id.to_string());
                fact_by_entity
                    .remove(&key as &str)
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_by_entity",
                        key: key,
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
            },
        )?;

        Ok(())
    }

    // === Relation Operations ===

    /// Build a relation key.
    fn relation_key(
        source_id: &EntityId,
        rel_type: &RelationType,
        target_id: &EntityId,
    ) -> Vec<u8> {
        format!(
            "{}\0{}\0{}",
            source_id.to_string(),
            rel_type.0,
            target_id.to_string()
        )
        .into_bytes()
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

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

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

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;

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

                let prefix = format!("{}\0", entity_id.to_string());

                for entry in relations.iter().map_err(|e| WgError::StoreRead {
                    table: "relations",
                    key: "<iter>".to_string(),
                    source: Box::new(e),
                })? {
                    let (key, value) = entry.map_err(|e| WgError::StoreRead {
                        table: "relations",
                        key: "<entry>".to_string(),
                        source: Box::new(e),
                    })?;

                    let key_str = String::from_utf8_lossy(key.value());
                    if key_str.starts_with(&prefix) {
                        let record: RelationRecord = serde_json::from_slice(value.value())
                            .map_err(|e| WgError::Deserialize {
                                context: "relation get".to_string(),
                                source: e,
                            })?;
                        results.push(record);
                    }
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

                let prefix = format!("{}\0", entity_id.to_string());

                for entry in relations_rev.iter().map_err(|e| WgError::StoreRead {
                    table: "relations_rev",
                    key: "<iter>".to_string(),
                    source: Box::new(e),
                })? {
                    let (key, value) = entry.map_err(|e| WgError::StoreRead {
                        table: "relations_rev",
                        key: "<entry>".to_string(),
                        source: Box::new(e),
                    })?;

                    let key_str = String::from_utf8_lossy(key.value());
                    if key_str.starts_with(&prefix) {
                        let record: RelationRecord = serde_json::from_slice(value.value())
                            .map_err(|e| WgError::Deserialize {
                                context: "relation get rev".to_string(),
                                source: e,
                            })?;
                        results.push(record);
                    }
                }
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
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
                    entity_type: None,
                    aliases: None,
                    tags: None,
                    source_page: None,
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
}
