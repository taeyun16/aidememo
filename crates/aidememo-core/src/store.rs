//! redb storage layer for AideMemo.
//!
//! Provides persistent storage for entities, relations, facts, and metadata.
//! Uses ULID-based canonical keys with name/alias secondary indexes.

use crate::config::Config;
use crate::error::{AideMemoError, Result};
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
/// Index from `normalized_source_id + NUL + sha256(content)` → first FactId
/// that committed that exact content in the source namespace. Powers
/// source-aware exact-dedup at write time: byte-identical facts in one source
/// collapse, while independent sources retain separate provenance and ids.
/// Modeled after OMEGA's SHA-256 dedup pass — see
/// `docs/MEASUREMENTS.md`.
pub(crate) const FACT_CONTENT_HASH_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("fact_content_hash");
pub(crate) const SEARCH_SESSIONS_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("search_sessions");
pub(crate) const SEARCH_FEEDBACK_TABLE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("search_feedback");

// Schema version
const CURRENT_SCHEMA_VERSION: u32 = 3;

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

    /// Access the shared `Arc<Config>` — used by the archive module to
    /// clone hot's config when opening the cold sibling store.
    pub(crate) fn config_arc(&self) -> &Arc<Config> {
        &self.config
    }

    /// Access the shared `Arc<Database>` — used by the archive module
    /// to begin a read transaction without re-implementing the helper.
    pub(crate) fn db_arc(&self) -> &Arc<Database> {
        &self.db
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;
        let durability = match self.config.store.durability.as_str() {
            "eventual" => redb::Durability::Eventual,
            _ => redb::Durability::Immediate,
        };
        txn.set_durability(durability);
        Ok(txn)
    }

    /// Open or create a AideMemo store at the given path.
    #[tracing::instrument(level = "debug", skip(config), fields(path = %path.display()))]
    pub fn open(path: &Path, mut config: Config) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AideMemoError::StoreOpen {
                path: path.to_path_buf(),
                source: Box::new(e),
            })?;
        }

        // Sync config.store.path to the actual path so downstream code
        // (cold-tier path derivation, lint reports, stats) sees the real
        // file we opened, not the default that the caller forgot to
        // update. CLI's --store flag and any explicit `Store::open(p,
        // ...)` caller pass `path` as truth; this keeps the two views
        // consistent.
        config.store.path = path.to_string_lossy().into_owned();

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
                    return Err(AideMemoError::StoreOpen {
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
        write_txn.open_table(FACT_CONTENT_HASH_TABLE).ok();
        write_txn.open_table(SEARCH_SESSIONS_TABLE).ok();
        write_txn.open_table(SEARCH_FEEDBACK_TABLE).ok();

        // Set schema version if not exists
        {
            let mut meta =
                write_txn
                    .open_table(META_TABLE)
                    .map_err(|e| AideMemoError::StoreRead {
                        table: "meta",
                        key: "schema_version".to_string(),
                        source: Box::new(e),
                    })?;

            let version_key = "schema_version";
            if meta
                .get(version_key)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "meta",
                    key: version_key.to_string(),
                    source: Box::new(e),
                })?
                .is_none()
            {
                meta.insert(version_key, CURRENT_SCHEMA_VERSION.to_le_bytes().as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "meta",
                        key: version_key.to_string(),
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
            table: "meta",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        self.migrate_schema()
    }

    fn migrate_schema(&self) -> Result<()> {
        match self.schema_version()? {
            CURRENT_SCHEMA_VERSION => Ok(()),
            1 => {
                self.migrate_fact_dedup_scope_v2()?;
                self.migrate_relation_scope_v3()
            }
            2 => self.migrate_relation_scope_v3(),
            version => Err(AideMemoError::UnsupportedSchemaVersion(version)),
        }
    }

    /// Rebuild the legacy global hash index as a source-aware index. The facts
    /// table is authoritative: rebuilding also repairs stale entries left by
    /// older update/delete paths and canonicalises SDK-provided source ids.
    fn migrate_fact_dedup_scope_v2(&self) -> Result<()> {
        let mut records = {
            let read_txn = self
                .db
                .begin_read()
                .map_err(|e| AideMemoError::TransactionBegin {
                    source: Box::new(e),
                })?;
            let facts = read_txn
                .open_table(FACTS_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "facts",
                    key: "<dedup migration>".to_string(),
                    source: Box::new(e),
                })?;
            let mut records = Vec::new();
            for entry in facts.iter().map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<dedup migration iter>".to_string(),
                source: Box::new(e),
            })? {
                let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                    table: "facts",
                    key: "<dedup migration entry>".to_string(),
                    source: Box::new(e),
                })?;
                let record = serde_json::from_slice::<FactRecord>(value.value()).map_err(|e| {
                    AideMemoError::Deserialize {
                        context: "fact dedup migration".to_string(),
                        source: e,
                    }
                })?;
                records.push(record);
            }
            records
        };

        let mut canonical_by_key = std::collections::HashMap::new();
        for record in &mut records {
            record.source_id = normalize_source_id(record.source_id.as_deref());
            let key = fact_dedup_key(record.source_id.as_deref(), &record.content);
            canonical_by_key.entry(key).or_insert(record.id);
        }

        let write_txn = self.begin_write()?;
        {
            let mut facts =
                write_txn
                    .open_table(FACTS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "facts",
                        key: "<dedup migration>".to_string(),
                        source: Box::new(e),
                    })?;
            for record in &records {
                let bytes = serde_json::to_vec(record).map_err(|e| AideMemoError::Serialize {
                    context: format!("fact dedup migration {}", record.id),
                    source: e,
                })?;
                facts
                    .insert(record.id.as_bytes().as_slice(), bytes.as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "facts",
                        key: record.id.to_string(),
                        source: Box::new(e),
                    })?;
            }
        }
        {
            let mut index = write_txn.open_table(FACT_CONTENT_HASH_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "fact_content_hash",
                    key: "<dedup migration>".to_string(),
                    source: Box::new(e),
                }
            })?;
            index
                .retain(|_, _| false)
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "fact_content_hash",
                    key: "<dedup migration clear>".to_string(),
                    source: Box::new(e),
                })?;
            for (key, id) in canonical_by_key {
                index
                    .insert(key.as_str(), id.as_bytes().as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "fact_content_hash",
                        key,
                        source: Box::new(e),
                    })?;
            }
        }
        {
            let mut meta =
                write_txn
                    .open_table(META_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "meta",
                        key: "schema_version".to_string(),
                        source: Box::new(e),
                    })?;
            meta.insert("schema_version", 2_u32.to_le_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "meta",
                    key: "schema_version".to_string(),
                    source: Box::new(e),
                })?;
        }
        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
            table: "meta",
            key: "dedup migration commit".to_string(),
            source: Box::new(e),
        })?;
        Ok(())
    }

    /// Rebuild relation indexes so the owning source namespace participates
    /// in edge identity. Legacy v1/v2 records deserialize with `None` and stay
    /// in the unscoped namespace.
    fn migrate_relation_scope_v3(&self) -> Result<()> {
        let mut records = {
            let read_txn = self
                .db
                .begin_read()
                .map_err(|e| AideMemoError::TransactionBegin {
                    source: Box::new(e),
                })?;
            let relations =
                read_txn
                    .open_table(RELATIONS_TABLE)
                    .map_err(|e| AideMemoError::StoreRead {
                        table: "relations",
                        key: "<scope migration>".to_string(),
                        source: Box::new(e),
                    })?;
            let mut records = Vec::new();
            for entry in relations.iter().map_err(|e| AideMemoError::StoreRead {
                table: "relations",
                key: "<scope migration iter>".to_string(),
                source: Box::new(e),
            })? {
                let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                    table: "relations",
                    key: "<scope migration entry>".to_string(),
                    source: Box::new(e),
                })?;
                records.push(
                    serde_json::from_slice::<RelationRecord>(value.value()).map_err(|e| {
                        AideMemoError::Deserialize {
                            context: "relation scope migration".to_string(),
                            source: e,
                        }
                    })?,
                );
            }
            records
        };
        for record in &mut records {
            record.scope_source_id = normalize_source_id(record.scope_source_id.as_deref());
        }

        let write_txn = self.begin_write()?;
        {
            let mut relations =
                write_txn
                    .open_table(RELATIONS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "relations",
                        key: "<scope migration>".to_string(),
                        source: Box::new(e),
                    })?;
            relations
                .retain(|_, _| false)
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "relations",
                    key: "<scope migration clear>".to_string(),
                    source: Box::new(e),
                })?;
            for record in &records {
                let key = Self::relation_key(
                    &record.source_id,
                    &record.relation_type,
                    &record.target_id,
                    record.scope_source_id.as_deref(),
                );
                let bytes = serde_json::to_vec(record).map_err(|e| AideMemoError::Serialize {
                    context: "relation scope migration".to_string(),
                    source: e,
                })?;
                relations
                    .insert(key.as_slice(), bytes.as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "relations",
                        key: String::from_utf8_lossy(&key).to_string(),
                        source: Box::new(e),
                    })?;
            }
        }
        {
            let mut reverse = write_txn.open_table(RELATIONS_REV_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "relations_rev",
                    key: "<scope migration>".to_string(),
                    source: Box::new(e),
                }
            })?;
            reverse
                .retain(|_, _| false)
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "relations_rev",
                    key: "<scope migration clear>".to_string(),
                    source: Box::new(e),
                })?;
            for record in &records {
                let key = Self::relation_key(
                    &record.target_id,
                    &record.relation_type,
                    &record.source_id,
                    record.scope_source_id.as_deref(),
                );
                let bytes = serde_json::to_vec(record).map_err(|e| AideMemoError::Serialize {
                    context: "relation scope migration reverse".to_string(),
                    source: e,
                })?;
                reverse
                    .insert(key.as_slice(), bytes.as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "relations_rev",
                        key: String::from_utf8_lossy(&key).to_string(),
                        source: Box::new(e),
                    })?;
            }
        }
        {
            let mut meta =
                write_txn
                    .open_table(META_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "meta",
                        key: "schema_version".to_string(),
                        source: Box::new(e),
                    })?;
            meta.insert(
                "schema_version",
                CURRENT_SCHEMA_VERSION.to_le_bytes().as_slice(),
            )
            .map_err(|e| AideMemoError::StoreWrite {
                table: "meta",
                key: "schema_version".to_string(),
                source: Box::new(e),
            })?;
        }
        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
            table: "meta",
            key: "relation scope migration commit".to_string(),
            source: Box::new(e),
        })?;
        Ok(())
    }

    /// Get the schema version.
    pub fn schema_version(&self) -> Result<u32> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let meta = read_txn
            .open_table(META_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "meta",
                key: "schema_version".to_string(),
                source: Box::new(e),
            })?;

        let version_key = "schema_version";
        let version = meta
            .get(version_key)
            .map_err(|e| AideMemoError::StoreRead {
                table: "meta",
                key: version_key.to_string(),
                source: Box::new(e),
            })?
            .ok_or(AideMemoError::SchemaVersionMismatch {
                found: 0,
                expected: CURRENT_SCHEMA_VERSION,
            })?;

        let bytes: [u8; 4] = version
            .value()
            .try_into()
            .map_err(|_| AideMemoError::Internal("invalid schema_version bytes".to_string()))?;

        Ok(u32::from_le_bytes(bytes))
    }

    /// Get store statistics.
    pub fn stats(&self) -> Result<StoreStats> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let entities =
            read_txn
                .open_table(ENTITIES_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entities",
                    key: "<all>".to_string(),
                    source: Box::new(e),
                })?;

        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let relations =
            read_txn
                .open_table(RELATIONS_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "relations",
                    key: "<all>".to_string(),
                    source: Box::new(e),
                })?;

        let meta = read_txn
            .open_table(META_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "meta",
                key: "stats".to_string(),
                source: Box::new(e),
            })?;

        let last_ingest_at = meta
            .get("last_ingest_at")
            .map_err(|e| AideMemoError::StoreRead {
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
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entities",
                    key: "<iter>".to_string(),
                    source: Box::new(e),
                })?
                .count() as u64,
            fact_count: facts
                .iter()
                .map_err(|e| AideMemoError::StoreRead {
                    table: "facts",
                    key: "<iter>".to_string(),
                    source: Box::new(e),
                })?
                .count() as u64,
            relation_count: relations
                .iter()
                .map_err(|e| AideMemoError::StoreRead {
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
            .map_err(|e| AideMemoError::StoreWrite {
                table: "meta",
                key: "last_ingest_at".to_string(),
                source: Box::new(e),
            })?;
        let now = crate::time::current_epoch_ms();
        meta.insert("last_ingest_at", now.to_le_bytes().as_slice())
            .map_err(|e| AideMemoError::StoreWrite {
                table: "meta",
                key: "last_ingest_at".to_string(),
                source: Box::new(e),
            })?;
        drop(meta);
        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
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
            let by_name = write_txn.open_table(ENTITY_BY_NAME_TABLE).map_err(|e| {
                AideMemoError::StoreRead {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                }
            })?;

            if by_name
                .get(&record.name_lower as &str)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                })?
                .is_some()
            {
                return Err(AideMemoError::EntityAlreadyExists { name: input.name });
            }
        }

        // Serialize record
        let record_bytes = serde_json::to_vec(&record).map_err(|e| AideMemoError::Serialize {
            context: format!("entity {:?}", id),
            source: e,
        })?;

        // Insert into entities table
        {
            let mut entities =
                write_txn
                    .open_table(ENTITIES_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "entities",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

            entities
                .insert(id.as_bytes().as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }

        // Insert into name index
        {
            let mut by_name = write_txn.open_table(ENTITY_BY_NAME_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                }
            })?;

            by_name
                .insert(&record.name_lower as &str, id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                })?;

            // Also index aliases
            for alias in &record.aliases {
                let alias_lower = alias.to_lowercase();
                by_name
                    .insert(&alias_lower as &str, id.as_bytes().as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "entity_by_name",
                        key: alias_lower,
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Look up ID by name
        let by_name =
            read_txn
                .open_table(ENTITY_BY_NAME_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entity_by_name",
                    key: name.to_lowercase(),
                    source: Box::new(e),
                })?;

        let id_bytes = by_name
            .get(name.to_lowercase().as_str())
            .map_err(|e| AideMemoError::StoreRead {
                table: "entity_by_name",
                key: name.to_lowercase(),
                source: Box::new(e),
            })?
            .ok_or_else(|| {
                let suggestions = self.suggest_similar_entities(name).unwrap_or_default();
                AideMemoError::entity_not_found(name.to_string(), suggestions)
            })?;

        let id = EntityId(Ulid::from_bytes(id_bytes.value().try_into().map_err(
            |_| AideMemoError::Internal("invalid entity id bytes".to_string()),
        )?));

        // Get the entity record
        let entities =
            read_txn
                .open_table(ENTITIES_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;

        let record_bytes = entities
            .get(id.as_bytes().as_slice())
            .map_err(|e| AideMemoError::StoreRead {
                table: "entities",
                key: id.to_string(),
                source: Box::new(e),
            })?
            .ok_or_else(|| AideMemoError::EntityIdNotFound(id.to_string()))?;

        serde_json::from_slice(record_bytes.value()).map_err(|e| AideMemoError::Deserialize {
            context: format!("entity {:?}", id),
            source: e,
        })
    }

    /// Get an entity by ID.
    pub fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let entities =
            read_txn
                .open_table(ENTITIES_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;

        let record_bytes =
            entities
                .get(id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;

        if let Some(record_bytes) = record_bytes {
            let mut record: EntityRecord =
                serde_json::from_slice(record_bytes.value()).map_err(|e| {
                    AideMemoError::Deserialize {
                        context: format!("entity {:?}", id),
                        source: e,
                    }
                })?;
            record.id = id;
            return Ok(record);
        }

        // Compatibility fallback for legacy rows whose stored JSON id doesn't match the table key.
        for entry in entities.iter().map_err(|e| AideMemoError::StoreRead {
            table: "entities",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                table: "entities",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;

            let mut record: EntityRecord =
                serde_json::from_slice(value.value()).map_err(|e| AideMemoError::Deserialize {
                    context: format!("entity {:?}", id),
                    source: e,
                })?;

            if record.id == id {
                record.id = id;
                return Ok(record);
            }
        }

        Err(AideMemoError::EntityIdNotFound(id.to_string()))
    }

    /// Update an entity.
    pub fn entity_update(&mut self, name: &str, input: EntityUpdate) -> Result<()> {
        let _read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Get current record
        let record = self.entity_get(name)?;

        let mut updated = record.clone();
        updated.update(input);

        let write_txn = self.begin_write()?;

        // Serialize updated record
        let record_bytes = serde_json::to_vec(&updated).map_err(|e| AideMemoError::Serialize {
            context: format!("entity update {:?}", record.id),
            source: e,
        })?;

        // Update entities table
        {
            let mut entities =
                write_txn
                    .open_table(ENTITIES_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "entities",
                        key: record.id.to_string(),
                        source: Box::new(e),
                    })?;

            entities
                .insert(record.id.as_bytes().as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entities",
                    key: record.id.to_string(),
                    source: Box::new(e),
                })?;
        }

        // Update name index if name changed
        if record.name_lower != updated.name_lower {
            let mut by_name = write_txn.open_table(ENTITY_BY_NAME_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: "update".to_string(),
                    source: Box::new(e),
                }
            })?;

            // Remove old name
            by_name
                .remove(record.name_lower.as_str())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower,
                    source: Box::new(e),
                })?;

            // Add new name
            by_name
                .insert(&updated.name_lower as &str, record.id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: updated.name_lower.clone(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let entities =
            read_txn
                .open_table(ENTITIES_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entities",
                    key: "<all>".to_string(),
                    source: Box::new(e),
                })?;

        let _facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let mut results: Vec<EntitySummary> = Vec::new();

        for entry in entities.iter().map_err(|e| AideMemoError::StoreRead {
            table: "entities",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                table: "entities",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;

            let mut record: EntityRecord =
                serde_json::from_slice(value.value()).map_err(|e| AideMemoError::Deserialize {
                    context: "entity list".to_string(),
                    source: e,
                })?;

            let id = match key.value().try_into() {
                Ok(bytes) => EntityId(Ulid::from_bytes(bytes)),
                Err(_) => record.id,
            };
            record.id = id;

            // Apply filters
            if let Some(ref entity_type) = opts.entity_type
                && &record.entity_type != entity_type
            {
                continue;
            }

            // Count facts for this entity
            let fact_count = self.count_entity_facts_internal(&read_txn, &record.id)?;

            if let Some(min_facts) = opts.min_facts
                && fact_count < min_facts
            {
                continue;
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
                .map_err(|e| AideMemoError::StoreRead {
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
            .map_err(|e| AideMemoError::StoreRead {
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
            .map_err(|e| AideMemoError::TransactionBegin {
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
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "entities",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

            entities
                .remove(id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }

        // Remove from name index
        {
            let mut by_name = write_txn.open_table(ENTITY_BY_NAME_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: "delete".to_string(),
                    source: Box::new(e),
                }
            })?;

            by_name
                .remove(record.name_lower.as_str())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower,
                    source: Box::new(e),
                })?;

            for alias in &record.aliases {
                by_name.remove(alias.to_lowercase().as_str()).map_err(|e| {
                    AideMemoError::StoreWrite {
                        table: "entity_by_name",
                        key: alias.to_lowercase(),
                        source: Box::new(e),
                    }
                })?;
            }
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let entities =
            read_txn
                .open_table(ENTITIES_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entities",
                    key: "<all>".to_string(),
                    source: Box::new(e),
                })?;

        let mut suggestions = Vec::new();
        let name_lower = name.to_lowercase();

        for entry in entities.iter().map_err(|e| AideMemoError::StoreRead {
            table: "entities",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                table: "entities",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;

            let record: EntityRecord =
                serde_json::from_slice(value.value()).map_err(|e| AideMemoError::Deserialize {
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let by_name =
            read_txn
                .open_table(ENTITY_BY_NAME_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entity_by_name",
                    key: name.to_lowercase(),
                    source: Box::new(e),
                })?;

        by_name
            .get(name.to_lowercase().as_str())
            .map_err(|e| AideMemoError::StoreRead {
                table: "entity_by_name",
                key: name.to_lowercase(),
                source: Box::new(e),
            })?
            .ok_or_else(|| {
                let suggestions = self.suggest_similar_entities(name).unwrap_or_default();
                AideMemoError::entity_not_found(name.to_string(), suggestions)
            })
            .map(|v| {
                let bytes: [u8; 16] = v
                    .value()
                    .try_into()
                    .map_err(|_| AideMemoError::Internal("invalid entity id bytes".to_string()))
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
            .ok_or_else(|| AideMemoError::InvalidInput("fact_add_many returned no ids".to_string()))
    }

    /// Insert N facts in a single redb write transaction. Amortizes
    /// the per-commit fsync over the whole batch — at the same scale,
    /// one-by-one `fact_add` pays ~3-5 ms per fsync on macOS APFS;
    /// `fact_add_many` pays it once for the whole vec.
    ///
    /// **Exact-content dedup**: each input's normalized `source_id` and
    /// SHA-256 content hash are compared against the `fact_content_hash`
    /// index. Same-source duplicates return the existing ID, including
    /// duplicates inside one batch. Identical content from another source
    /// remains independent so its provenance is not lost.
    ///
    /// All-or-nothing for the inserts that DO need to land: a
    /// serialization or write failure aborts the transaction and no
    /// facts land. The returned `Vec<FactId>` is in the same order
    /// as `inputs` — duplicate slots get the existing id.
    pub fn fact_add_many(&mut self, inputs: Vec<FactInput>) -> Result<Vec<FactId>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 1: build every input's source-aware dedup key, look up matches in
        // a single read transaction, and resolve each input slot to either
        // (a) a `Pending` new record or (b) an `Existing` fact id.
        let dedup_keys: Vec<String> = inputs
            .iter()
            .map(|input| fact_dedup_key(input.source_id.as_deref(), &input.content))
            .collect();
        let mut existing_by_key: std::collections::HashMap<String, FactId> =
            std::collections::HashMap::new();
        {
            let read_txn = self
                .db
                .begin_read()
                .map_err(|e| AideMemoError::TransactionBegin {
                    source: Box::new(e),
                })?;
            // The hash table didn't exist in pre-dedup wikis; treat
            // "table not found" the same as "no hits" so we never crash
            // on a legacy store.
            if let Ok(table) = read_txn.open_table(FACT_CONTENT_HASH_TABLE) {
                for key in &dedup_keys {
                    if existing_by_key.contains_key(key.as_str()) {
                        continue;
                    }
                    if let Ok(Some(v)) = table.get(key.as_str()) {
                        let bytes = v.value();
                        if bytes.len() == 16 {
                            let mut arr = [0u8; 16];
                            arr.copy_from_slice(bytes);
                            let id = FactId(ulid::Ulid::from_bytes(arr));
                            existing_by_key.insert(key.clone(), id);
                        }
                    }
                }
            }
        }

        // Phase 2: build records to insert, deduping inside the batch
        // too. `resolved_ids[i]` mirrors the input order. When dedup
        // hits an existing fact but the new input adds entity_ids the
        // existing record didn't carry, queue an entity-merge so the
        // graph index picks up the new attachments (otherwise the
        // re-ingest silently drops the new entity link).
        let mut resolved_ids: Vec<FactId> = Vec::with_capacity(inputs.len());
        let mut records: Vec<FactRecord> = Vec::new();
        let mut record_dedup_keys: Vec<String> = Vec::new();
        let mut batch_seen: std::collections::HashMap<String, FactId> =
            std::collections::HashMap::new();
        // existing_id -> set of new entity_ids to merge in
        let mut entity_merges: std::collections::HashMap<FactId, Vec<EntityId>> =
            std::collections::HashMap::new();

        for (input, dedup_key) in inputs.into_iter().zip(dedup_keys.iter()) {
            // Pre-existing in store? Reuse the id, skip the insert,
            // but queue any new entity_ids for merge.
            if let Some(id) = existing_by_key.get(dedup_key.as_str()) {
                resolved_ids.push(*id);
                if let Some(eids) = input.entity_ids
                    && !eids.is_empty()
                {
                    entity_merges.entry(*id).or_default().extend(eids);
                }
                continue;
            }
            // Already queued earlier in this batch? Reuse that id.
            if let Some(id) = batch_seen.get(dedup_key) {
                resolved_ids.push(*id);
                if let Some(eids) = input.entity_ids
                    && !eids.is_empty()
                {
                    entity_merges.entry(*id).or_default().extend(eids);
                }
                continue;
            }
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
            record.source_id = normalize_source_id(input.source_id.as_deref());
            if let Some(actor_id) = input.actor_id {
                record.actor_id = Some(actor_id);
            }
            if let Some(confidence) = input.source_confidence {
                record.source_confidence = confidence;
            }
            if let Some(observed_at) = input.observed_at {
                record.observed_at = Some(observed_at);
            }
            batch_seen.insert(dedup_key.clone(), id);
            records.push(record);
            record_dedup_keys.push(dedup_key.clone());
            resolved_ids.push(id);
        }

        // Nothing actually new AND no entity merges queued — all inputs
        // collapsed to existing facts with no new entity links. Skip
        // the write transaction entirely.
        if records.is_empty() && entity_merges.is_empty() {
            return Ok(resolved_ids);
        }

        let write_txn = self.begin_write()?;

        {
            let mut facts =
                write_txn
                    .open_table(FACTS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "facts",
                        key: "<batch>".to_string(),
                        source: Box::new(e),
                    })?;
            let mut fact_by_entity = write_txn.open_table(FACT_BY_ENTITY_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "fact_by_entity",
                    key: "<batch>".to_string(),
                    source: Box::new(e),
                }
            })?;
            let mut content_hash_index =
                write_txn.open_table(FACT_CONTENT_HASH_TABLE).map_err(|e| {
                    AideMemoError::StoreWrite {
                        table: "fact_content_hash",
                        key: "<batch>".to_string(),
                        source: Box::new(e),
                    }
                })?;

            for (record, dedup_key) in records.iter().zip(record_dedup_keys.iter()) {
                let id = record.id;
                let record_bytes =
                    serde_json::to_vec(record).map_err(|e| AideMemoError::Serialize {
                        context: format!("fact {:?}", id),
                        source: e,
                    })?;

                facts
                    .insert(id.as_bytes().as_slice(), record_bytes.as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "facts",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

                content_hash_index
                    .insert(dedup_key.as_str(), id.as_bytes().as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "fact_content_hash",
                        key: dedup_key.clone(),
                        source: Box::new(e),
                    })?;

                for entity_id in &record.entity_ids {
                    let key = format!("{}\0{}", entity_id, id);
                    fact_by_entity
                        .insert(&key as &str, id.as_bytes().as_slice())
                        .map_err(|e| AideMemoError::StoreWrite {
                            table: "fact_by_entity",
                            key,
                            source: Box::new(e),
                        })?;
                }
            }

            // Phase 3: process entity merges for dedup-hit existing
            // facts. Read each existing record, append any new
            // entity_ids that aren't already present, write the
            // updated record back, and add fact_by_entity index rows.
            // Skip records we just inserted (their entity_ids are
            // already authoritative).
            for (existing_id, new_eids) in entity_merges.iter() {
                if records.iter().any(|r| r.id == *existing_id) {
                    continue;
                }
                let raw = match facts.get(existing_id.as_bytes().as_slice()) {
                    Ok(Some(v)) => v.value().to_vec(),
                    _ => continue,
                };
                let mut record: FactRecord = match serde_json::from_slice(&raw) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let mut changed = false;
                for eid in new_eids {
                    if !record.entity_ids.contains(eid) {
                        record.entity_ids.push(*eid);
                        let key = format!("{}\0{}", eid, existing_id);
                        fact_by_entity
                            .insert(&key as &str, existing_id.as_bytes().as_slice())
                            .map_err(|e| AideMemoError::StoreWrite {
                                table: "fact_by_entity",
                                key,
                                source: Box::new(e),
                            })?;
                        changed = true;
                    }
                }
                if changed {
                    let bytes =
                        serde_json::to_vec(&record).map_err(|e| AideMemoError::Serialize {
                            context: format!("fact {:?}", existing_id),
                            source: e,
                        })?;
                    facts
                        .insert(existing_id.as_bytes().as_slice(), bytes.as_slice())
                        .map_err(|e| AideMemoError::StoreWrite {
                            table: "facts",
                            key: existing_id.to_string(),
                            source: Box::new(e),
                        })?;
                }
            }
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
            table: "facts",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(resolved_ids)
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;
        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: id.to_string(),
                source: Box::new(e),
            })?;

        let record_bytes = facts
            .get(id.as_bytes().as_slice())
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: id.to_string(),
                source: Box::new(e),
            })?
            .ok_or(AideMemoError::FactNotFound(id.to_string()))?;

        let mut record: FactRecord = serde_json::from_slice(record_bytes.value()).map_err(|e| {
            AideMemoError::Deserialize {
                context: format!("fact {:?}", id),
                source: e,
            }
        })?;

        // Record access
        record.record_access();

        Ok(record)
    }

    fn fact_matches_list_opts(record: &FactRecord, opts: &FactListOpts) -> bool {
        if let Some(ref fact_type) = opts.fact_type
            && &record.fact_type != fact_type
        {
            return false;
        }

        if let Some(min_confidence) = opts.min_confidence
            && record.source_confidence < min_confidence
        {
            return false;
        }

        if let Some(entity_id) = opts.entity_id
            && !record.entity_ids.contains(&entity_id)
        {
            return false;
        }

        if let Some(ref source_id) = opts.source_id
            && record.source_id.as_deref() != Some(source_id.as_str())
        {
            return false;
        }

        // Time filter: prefer observed_at (real-world time) over created_at (DB insertion).
        if opts.since.is_some() || opts.until.is_some() {
            let ts = record.observed_at.unwrap_or(record.created_at);
            if let Some(since) = opts.since
                && ts < since
            {
                return false;
            }
            if let Some(until) = opts.until
                && ts > until
            {
                return false;
            }
        }

        if opts.current_only && record.superseded_at.is_some() {
            return false;
        }

        // As-of filter: include this fact only if it (a) existed at the
        // as_of time and (b) wasn't superseded yet then.
        if let Some(as_of) = opts.as_of {
            if record.created_at > as_of {
                return false;
            }
            if let Some(superseded_at) = record.superseded_at
                && superseded_at <= as_of
            {
                return false;
            }
        }

        true
    }

    /// Update a fact.
    pub fn fact_update(&mut self, id: &FactId, input: FactUpdate) -> Result<()> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        // Get current record
        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: id.to_string(),
                source: Box::new(e),
            })?;

        let record_bytes = facts
            .get(id.as_bytes().as_slice())
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: id.to_string(),
                source: Box::new(e),
            })?
            .ok_or(AideMemoError::FactNotFound(id.to_string()))?;

        let mut record: FactRecord = serde_json::from_slice(record_bytes.value()).map_err(|e| {
            AideMemoError::Deserialize {
                context: format!("fact update {:?}", id),
                source: e,
            }
        })?;

        let old_dedup_key = fact_dedup_key(record.source_id.as_deref(), &record.content);
        record.update(input);
        record.source_id = normalize_source_id(record.source_id.as_deref());
        let new_dedup_key = fact_dedup_key(record.source_id.as_deref(), &record.content);

        drop(record_bytes);
        drop(facts);
        drop(read_txn);

        let write_txn = self.begin_write()?;

        let record_bytes = serde_json::to_vec(&record).map_err(|e| AideMemoError::Serialize {
            context: format!("fact update {:?}", id),
            source: e,
        })?;

        {
            let mut index = write_txn.open_table(FACT_CONTENT_HASH_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "fact_content_hash",
                    key: new_dedup_key.clone(),
                    source: Box::new(e),
                }
            })?;
            let indexed_id = index
                .get(new_dedup_key.as_str())
                .map_err(|e| AideMemoError::StoreRead {
                    table: "fact_content_hash",
                    key: new_dedup_key.clone(),
                    source: Box::new(e),
                })?
                .map(|value| value.value().to_vec());
            if indexed_id
                .as_deref()
                .is_some_and(|bytes| bytes != id.as_bytes().as_slice())
            {
                return Err(AideMemoError::InvalidInput(format!(
                    "fact content already exists in source namespace for {id}"
                )));
            }
            if old_dedup_key != new_dedup_key {
                let old_indexed_id = index
                    .get(old_dedup_key.as_str())
                    .map_err(|e| AideMemoError::StoreRead {
                        table: "fact_content_hash",
                        key: old_dedup_key.clone(),
                        source: Box::new(e),
                    })?
                    .map(|value| value.value().to_vec());
                if old_indexed_id.as_deref() == Some(id.as_bytes().as_slice()) {
                    index.remove(old_dedup_key.as_str()).map_err(|e| {
                        AideMemoError::StoreWrite {
                            table: "fact_content_hash",
                            key: old_dedup_key.clone(),
                            source: Box::new(e),
                        }
                    })?;
                }
            }
            index
                .insert(new_dedup_key.as_str(), id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "fact_content_hash",
                    key: new_dedup_key,
                    source: Box::new(e),
                })?;
        }

        {
            let mut facts =
                write_txn
                    .open_table(FACTS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "facts",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

            facts
                .insert(id.as_bytes().as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
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
            let facts =
                write_txn
                    .open_table(FACTS_TABLE)
                    .map_err(|e| AideMemoError::StoreRead {
                        table: "facts",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

            let record_bytes = facts
                .get(id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreRead {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?
                .ok_or(AideMemoError::FactNotFound(id.to_string()))?;

            serde_json::from_slice(record_bytes.value()).map_err(|e| {
                AideMemoError::Deserialize {
                    context: format!("fact delete {:?}", id),
                    source: e,
                }
            })?
        };

        // Remove from facts table
        {
            let mut facts =
                write_txn
                    .open_table(FACTS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "facts",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;

            facts
                .remove(id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }

        // Remove from fact_by_entity index
        {
            let mut fact_by_entity = write_txn.open_table(FACT_BY_ENTITY_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "fact_by_entity",
                    key: "delete".to_string(),
                    source: Box::new(e),
                }
            })?;

            for entity_id in &record.entity_ids {
                let key = format!("{}\0{}", entity_id, id);
                fact_by_entity
                    .remove(&key as &str)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "fact_by_entity",
                        key,
                        source: Box::new(e),
                    })?;
            }
        }

        // Remove the source-aware exact-dedup entry so a later re-add does
        // not resolve to the deleted FactId. Only remove when this record is
        // still the index's canonical target (legacy sync data may contain
        // another same-key record).
        {
            let dedup_key = fact_dedup_key(record.source_id.as_deref(), &record.content);
            let mut index = write_txn.open_table(FACT_CONTENT_HASH_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "fact_content_hash",
                    key: dedup_key.clone(),
                    source: Box::new(e),
                }
            })?;
            let indexed_id = index
                .get(dedup_key.as_str())
                .map_err(|e| AideMemoError::StoreRead {
                    table: "fact_content_hash",
                    key: dedup_key.clone(),
                    source: Box::new(e),
                })?
                .map(|value| value.value().to_vec());
            if indexed_id.as_deref() == Some(id.as_bytes().as_slice()) {
                index
                    .remove(dedup_key.as_str())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "fact_content_hash",
                        key: dedup_key,
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
            table: "facts",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;

        Ok(())
    }

    /// List facts with options.
    /// Return every fact tagged `pinned=true`, sorted by
    /// `last_accessed_at` descending. Skips superseded facts so a
    /// pinned-then-retired decision doesn't keep showing up. Bounded
    /// by `limit` to keep the warmup envelope size predictable.
    pub fn pinned_facts(&self, limit: usize) -> Result<Vec<FactRecord>> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;
        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;
        let mut out: Vec<FactRecord> = Vec::new();
        for entry in facts.iter().map_err(|e| AideMemoError::StoreRead {
            table: "facts",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;
            let record: FactRecord =
                serde_json::from_slice(value.value()).map_err(|e| AideMemoError::Deserialize {
                    context: "pinned facts".to_string(),
                    source: e,
                })?;
            if record.pinned && record.superseded_at.is_none() {
                out.push(record);
            }
        }
        out.sort_by_key(|f| std::cmp::Reverse(f.last_accessed_at));
        out.truncate(limit);
        Ok(out)
    }

    pub fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<all>".to_string(),
                source: Box::new(e),
            })?;

        let mut results: Vec<FactRecord> = Vec::new();

        if let Some(entity_id) = opts.entity_id {
            let fact_by_entity = read_txn.open_table(FACT_BY_ENTITY_TABLE).map_err(|e| {
                AideMemoError::StoreRead {
                    table: "fact_by_entity",
                    key: entity_id.to_string(),
                    source: Box::new(e),
                }
            })?;
            let lower = format!("{}\0", entity_id);
            let upper = format!("{}\u{1}", entity_id);

            for entry in fact_by_entity
                .range::<&str>(lower.as_str()..upper.as_str())
                .map_err(|e| AideMemoError::StoreRead {
                    table: "fact_by_entity",
                    key: "<range>".to_string(),
                    source: Box::new(e),
                })?
            {
                let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                    table: "fact_by_entity",
                    key: "<entry>".to_string(),
                    source: Box::new(e),
                })?;
                let id_bytes = value.value();
                if id_bytes.len() != 16 {
                    continue;
                }
                let mut raw_id = [0u8; 16];
                raw_id.copy_from_slice(id_bytes);
                let fact_id = FactId(Ulid::from_bytes(raw_id));

                let Some(record_bytes) = facts.get(fact_id.as_bytes().as_slice()).map_err(|e| {
                    AideMemoError::StoreRead {
                        table: "facts",
                        key: fact_id.to_string(),
                        source: Box::new(e),
                    }
                })?
                else {
                    continue;
                };

                let record: FactRecord =
                    serde_json::from_slice(record_bytes.value()).map_err(|e| {
                        AideMemoError::Deserialize {
                            context: "fact list".to_string(),
                            source: e,
                        }
                    })?;

                if Self::fact_matches_list_opts(&record, &opts) {
                    results.push(record);
                }
            }
        } else {
            for entry in facts.iter().map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<iter>".to_string(),
                source: Box::new(e),
            })? {
                let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                    table: "facts",
                    key: "<entry>".to_string(),
                    source: Box::new(e),
                })?;

                let record: FactRecord = serde_json::from_slice(value.value()).map_err(|e| {
                    AideMemoError::Deserialize {
                        context: "fact list".to_string(),
                        source: e,
                    }
                })?;

                if Self::fact_matches_list_opts(&record, &opts) {
                    results.push(record);
                }
            }
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
        record.updated_at =
            crate::time::current_epoch_ms().max(record.updated_at.saturating_add(1));

        let _ = self.fact_upsert_record(record)?;

        Ok(())
    }

    /// Add a search session record.
    pub fn search_session_add(&mut self, session: &SearchSession) -> Result<()> {
        let write_txn = self.begin_write()?;

        let mut table =
            write_txn
                .open_table(SEARCH_SESSIONS_TABLE)
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "search_sessions",
                    key: session.id.clone(),
                    source: Box::new(e),
                })?;

        let bytes = serde_json::to_vec(session).map_err(|e| AideMemoError::Serialize {
            context: "search_session".to_string(),
            source: e,
        })?;

        table
            .insert(session.id.as_str(), bytes.as_slice())
            .map_err(|e| AideMemoError::StoreWrite {
                table: "search_sessions",
                key: session.id.clone(),
                source: Box::new(e),
            })?;
        drop(table);

        write_txn
            .commit()
            .map_err(|e| AideMemoError::Internal(format!("transaction commit failed: {}", e)))?;

        Ok(())
    }

    /// Add a search feedback record.
    pub fn search_feedback_add(&mut self, feedback: &SearchFeedback) -> Result<()> {
        let write_txn = self.begin_write()?;

        let mut table =
            write_txn
                .open_table(SEARCH_FEEDBACK_TABLE)
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "search_feedback",
                    key: format!("{}:{}", feedback.session_id, feedback.fact_id).to_string(),
                    source: Box::new(e),
                })?;

        let bytes = serde_json::to_vec(feedback).map_err(|e| AideMemoError::Serialize {
            context: "search_feedback".to_string(),
            source: e,
        })?;

        table
            .insert(
                format!("{}:{}", feedback.session_id, feedback.fact_id).as_str(),
                bytes.as_slice(),
            )
            .map_err(|e| AideMemoError::StoreWrite {
                table: "search_feedback",
                key: format!("{}:{}", feedback.session_id, feedback.fact_id),
                source: Box::new(e),
            })?;
        drop(table);

        write_txn
            .commit()
            .map_err(|e| AideMemoError::Internal(format!("transaction commit failed: {}", e)))?;

        Ok(())
    }

    /// Count total feedback entries.
    pub fn search_feedback_count(&self) -> Result<usize> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let table =
            read_txn
                .open_table(SEARCH_FEEDBACK_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "search_feedback",
                    key: "<all>".to_string(),
                    source: Box::new(e),
                })?;

        let iter = table.iter().map_err(|e| AideMemoError::StoreRead {
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
        scope_source_id: Option<&str>,
    ) -> Vec<u8> {
        format!(
            "{}\0{}\0{}\0{}",
            source_id,
            scope_source_id.unwrap_or(""),
            rel_type.0,
            target_id
        )
        .into_bytes()
    }

    // === Sync helpers (Phase 2) ===
    //
    // ID-preserving idempotent inserters. Used by `AideMemo::sync_import`
    // when applying a delta from a remote aidememo. Differ from the ordinary
    // `entity_add` / `fact_add_many` paths in three ways:
    //   1. The donor's ULID is preserved (not re-allocated locally), so
    //      pre-existing fact references continue to resolve.
    //   2. Idempotent — if the ID already exists locally, return Ok(false)
    //      and write nothing (Phase 2 sync uses "first writer wins" by ID,
    //      no LWW merge yet).
    //   3. Skip the dedup-by-content-hash path: the donor already made
    //      that decision when ingesting; we trust it.
    //
    // Phase 2.5 will add: name-conflict resolution for entities (alias the
    // local entity to the donor ULID rather than skipping), and LWW
    // supersede merging for facts.

    /// Insert an `EntityRecord` preserving its ULID. Returns `Ok(true)` on
    /// fresh insert, `Ok(false)` if an entity with the same ULID already
    /// exists locally (idempotent skip).
    ///
    /// Name conflict (same name, different ULID) currently *also* skips —
    /// the donor's record drops on the floor and any facts referencing
    /// the donor's entity_id will be orphaned in the local store. Phase
    /// 2.5 will replace this with alias resolution. Acceptable for the
    /// pull-only Phase 2 MVP because the upstream is the only writer
    /// agents touch, so name collisions don't arise in normal use.
    pub fn entity_upsert_record(&mut self, record: EntityRecord) -> Result<bool> {
        let id = record.id;

        // LWW by `updated_at`: if local copy exists and isn't older
        // than the incoming, skip. Otherwise overwrite. This is the
        // Phase 2.5 change from "skip if id exists" — needed so
        // entity_describe / supersede / pin updates land on records
        // the downstream already pulled.
        if let Ok(local) = self.entity_get_by_id(id)
            && local.updated_at >= record.updated_at
        {
            return Ok(false);
        }

        let write_txn = self.begin_write()?;

        // Name-conflict guard — only relevant for FRESH inserts (no
        // local copy yet). When updating an existing record, the name
        // index already points at this same id so the lookup will hit
        // ourselves and we proceed.
        let is_update = {
            let entities =
                write_txn
                    .open_table(ENTITIES_TABLE)
                    .map_err(|e| AideMemoError::StoreRead {
                        table: "entities",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;
            entities
                .get(id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?
                .is_some()
        };
        if !is_update {
            let by_name = write_txn.open_table(ENTITY_BY_NAME_TABLE).map_err(|e| {
                AideMemoError::StoreRead {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                }
            })?;
            if by_name
                .get(&record.name_lower as &str)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                })?
                .is_some()
            {
                return Ok(false);
            }
        }

        let record_bytes = serde_json::to_vec(&record).map_err(|e| AideMemoError::Serialize {
            context: format!("entity {:?}", id),
            source: e,
        })?;

        {
            let mut entities =
                write_txn
                    .open_table(ENTITIES_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "entities",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;
            entities
                .insert(id.as_bytes().as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entities",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }
        {
            let mut by_name = write_txn.open_table(ENTITY_BY_NAME_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                }
            })?;
            by_name
                .insert(&record.name_lower as &str, id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "entity_by_name",
                    key: record.name_lower.clone(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
            table: "sync_upsert",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;
        Ok(true)
    }

    /// Insert a `FactRecord` preserving its ULID. Returns `Ok(true)`
    /// when the local copy ends up reflecting the incoming record
    /// (fresh insert OR LWW overwrite), `Ok(false)` when the local
    /// copy was already at-or-after the incoming `updated_at`. A same-source
    /// exact duplicate with another id is skipped, while matching content in
    /// another source remains independent.
    pub fn fact_upsert_record(&mut self, mut record: FactRecord) -> Result<bool> {
        record.source_id = normalize_source_id(record.source_id.as_deref());
        let id = record.id;
        let local = self.fact_get(&id).ok();
        if local
            .as_ref()
            .is_some_and(|local| local.updated_at >= record.updated_at)
        {
            return Ok(false);
        }

        let dedup_key = fact_dedup_key(record.source_id.as_deref(), &record.content);
        let record_bytes = serde_json::to_vec(&record).map_err(|e| AideMemoError::Serialize {
            context: format!("fact {:?}", id),
            source: e,
        })?;

        let write_txn = self.begin_write()?;
        {
            let mut content_hash_index =
                write_txn.open_table(FACT_CONTENT_HASH_TABLE).map_err(|e| {
                    AideMemoError::StoreWrite {
                        table: "fact_content_hash",
                        key: dedup_key.clone(),
                        source: Box::new(e),
                    }
                })?;
            let indexed_id = content_hash_index
                .get(dedup_key.as_str())
                .map_err(|e| AideMemoError::StoreRead {
                    table: "fact_content_hash",
                    key: dedup_key.clone(),
                    source: Box::new(e),
                })?
                .map(|value| value.value().to_vec());
            if indexed_id
                .as_deref()
                .is_some_and(|bytes| bytes != id.as_bytes().as_slice())
            {
                return Ok(false);
            }

            if let Some(local) = &local {
                let old_key = fact_dedup_key(local.source_id.as_deref(), &local.content);
                if old_key != dedup_key {
                    let old_indexed_id = content_hash_index
                        .get(old_key.as_str())
                        .map_err(|e| AideMemoError::StoreRead {
                            table: "fact_content_hash",
                            key: old_key.clone(),
                            source: Box::new(e),
                        })?
                        .map(|value| value.value().to_vec());
                    if old_indexed_id.as_deref() == Some(id.as_bytes().as_slice()) {
                        content_hash_index.remove(old_key.as_str()).map_err(|e| {
                            AideMemoError::StoreWrite {
                                table: "fact_content_hash",
                                key: old_key,
                                source: Box::new(e),
                            }
                        })?;
                    }
                }
            }
            content_hash_index
                .insert(dedup_key.as_str(), id.as_bytes().as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "fact_content_hash",
                    key: dedup_key.clone(),
                    source: Box::new(e),
                })?;
        }
        {
            let mut facts =
                write_txn
                    .open_table(FACTS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "facts",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;
            facts
                .insert(id.as_bytes().as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "facts",
                    key: id.to_string(),
                    source: Box::new(e),
                })?;
        }
        {
            let mut fact_by_entity = write_txn.open_table(FACT_BY_ENTITY_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "fact_by_entity",
                    key: id.to_string(),
                    source: Box::new(e),
                }
            })?;
            for entity_id in &record.entity_ids {
                let key = format!("{}\0{}", entity_id, id);
                fact_by_entity
                    .insert(&key as &str, id.as_bytes().as_slice())
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "fact_by_entity",
                        key,
                        source: Box::new(e),
                    })?;
            }
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
            table: "sync_upsert",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;
        Ok(true)
    }

    /// Insert a `RelationRecord`. Returns `Ok(true)` on fresh insert,
    /// `Ok(false)` if the (source, type, target) tuple is already
    /// present locally. Relations are key-shaped (no ULID), so
    /// idempotency is by tuple, not by ID.
    pub fn relation_upsert_record(&mut self, mut record: RelationRecord) -> Result<bool> {
        record.scope_source_id = normalize_source_id(record.scope_source_id.as_deref());
        let key = Self::relation_key(
            &record.source_id,
            &record.relation_type,
            &record.target_id,
            record.scope_source_id.as_deref(),
        );

        let record_bytes = serde_json::to_vec(&record).map_err(|e| AideMemoError::Serialize {
            context: "relation".to_string(),
            source: e,
        })?;

        // Identical replays are idempotent. A differing canonical upstream
        // record replaces both indexes so weight/evidence changes propagate.
        {
            let read_txn = self
                .db
                .begin_read()
                .map_err(|e| AideMemoError::TransactionBegin {
                    source: Box::new(e),
                })?;
            if let Ok(table) = read_txn.open_table(RELATIONS_TABLE)
                && let Some(existing) =
                    table
                        .get(key.as_slice())
                        .map_err(|e| AideMemoError::StoreRead {
                            table: "relations",
                            key: String::from_utf8_lossy(&key).to_string(),
                            source: Box::new(e),
                        })?
                && existing.value() == record_bytes.as_slice()
            {
                return Ok(false);
            }
        }

        let rev_key = Self::relation_key(
            &record.target_id,
            &record.relation_type,
            &record.source_id,
            record.scope_source_id.as_deref(),
        );
        let write_txn = self.begin_write()?;
        {
            let mut relations =
                write_txn
                    .open_table(RELATIONS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "relations",
                        key: String::from_utf8_lossy(&key).to_string(),
                        source: Box::new(e),
                    })?;
            relations
                .insert(key.as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "relations",
                    key: String::from_utf8_lossy(&key).to_string(),
                    source: Box::new(e),
                })?;
        }
        {
            let mut relations_rev = write_txn.open_table(RELATIONS_REV_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "relations_rev",
                    key: String::from_utf8_lossy(&rev_key).to_string(),
                    source: Box::new(e),
                }
            })?;
            relations_rev
                .insert(rev_key.as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "relations_rev",
                    key: String::from_utf8_lossy(&rev_key).to_string(),
                    source: Box::new(e),
                })?;
        }
        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
            table: "sync_upsert",
            key: "commit".to_string(),
            source: Box::new(e),
        })?;
        Ok(true)
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
            scope_source_id: normalize_source_id(input.scope_source_id.as_deref()),
            relation_type: rel_type.clone(),
            weight,
            evidence: input.evidence.unwrap_or_default(),
            created_at: crate::time::current_epoch_ms(),
        };

        let write_txn = self.begin_write()?;

        // Serialize record
        let record_bytes = serde_json::to_vec(&record).map_err(|e| AideMemoError::Serialize {
            context: "relation".to_string(),
            source: e,
        })?;

        // Insert into relations table
        let key = Self::relation_key(
            &source_id,
            &rel_type,
            &target_id,
            record.scope_source_id.as_deref(),
        );
        {
            let mut relations =
                write_txn
                    .open_table(RELATIONS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "relations",
                        key: String::from_utf8_lossy(&key).to_string(),
                        source: Box::new(e),
                    })?;

            relations
                .insert(key.as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "relations",
                    key: String::from_utf8_lossy(&key).to_string(),
                    source: Box::new(e),
                })?;
        }

        // Insert into relations_rev table (reverse key)
        let rev_key = Self::relation_key(
            &target_id,
            &rel_type,
            &source_id,
            record.scope_source_id.as_deref(),
        );
        {
            let mut relations_rev = write_txn.open_table(RELATIONS_REV_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "relations_rev",
                    key: String::from_utf8_lossy(&rev_key).to_string(),
                    source: Box::new(e),
                }
            })?;

            relations_rev
                .insert(rev_key.as_slice(), record_bytes.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "relations_rev",
                    key: String::from_utf8_lossy(&rev_key).to_string(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
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

        let key = Self::relation_key(&source_id, &rel_type, &target_id, None);
        let rev_key = Self::relation_key(&target_id, &rel_type, &source_id, None);

        {
            let mut relations =
                write_txn
                    .open_table(RELATIONS_TABLE)
                    .map_err(|e| AideMemoError::StoreWrite {
                        table: "relations",
                        key: String::from_utf8_lossy(&key).to_string(),
                        source: Box::new(e),
                    })?;

            relations
                .remove(key.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "relations",
                    key: String::from_utf8_lossy(&key).to_string(),
                    source: Box::new(e),
                })?;
        }

        {
            let mut relations_rev = write_txn.open_table(RELATIONS_REV_TABLE).map_err(|e| {
                AideMemoError::StoreWrite {
                    table: "relations_rev",
                    key: String::from_utf8_lossy(&rev_key).to_string(),
                    source: Box::new(e),
                }
            })?;

            relations_rev
                .remove(rev_key.as_slice())
                .map_err(|e| AideMemoError::StoreWrite {
                    table: "relations_rev",
                    key: String::from_utf8_lossy(&rev_key).to_string(),
                    source: Box::new(e),
                })?;
        }

        write_txn.commit().map_err(|e| AideMemoError::StoreWrite {
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
            .map_err(|e| AideMemoError::TransactionBegin {
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
                        .map_err(|e| AideMemoError::StoreRead {
                            table: "relations",
                            key: "<all>".to_string(),
                            source: Box::new(e),
                        })?;

                for entry in relations
                    .range::<&[u8]>(lower.as_slice()..upper.as_slice())
                    .map_err(|e| AideMemoError::StoreRead {
                        table: "relations",
                        key: "<range>".to_string(),
                        source: Box::new(e),
                    })?
                {
                    let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                        table: "relations",
                        key: "<entry>".to_string(),
                        source: Box::new(e),
                    })?;

                    let record: RelationRecord =
                        serde_json::from_slice(value.value()).map_err(|e| {
                            AideMemoError::Deserialize {
                                context: "relation get".to_string(),
                                source: e,
                            }
                        })?;
                    results.push(record);
                }
            }
            TraverseDirection::Reverse => {
                let relations_rev = read_txn.open_table(RELATIONS_REV_TABLE).map_err(|e| {
                    AideMemoError::StoreRead {
                        table: "relations_rev",
                        key: "<all>".to_string(),
                        source: Box::new(e),
                    }
                })?;

                for entry in relations_rev
                    .range::<&[u8]>(lower.as_slice()..upper.as_slice())
                    .map_err(|e| AideMemoError::StoreRead {
                        table: "relations_rev",
                        key: "<range>".to_string(),
                        source: Box::new(e),
                    })?
                {
                    let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                        table: "relations_rev",
                        key: "<entry>".to_string(),
                        source: Box::new(e),
                    })?;

                    let record: RelationRecord =
                        serde_json::from_slice(value.value()).map_err(|e| {
                            AideMemoError::Deserialize {
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;
        let relations =
            read_txn
                .open_table(RELATIONS_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "relations",
                    key: "<all>".to_string(),
                    source: Box::new(e),
                })?;

        let mut results = Vec::new();
        for entry in relations.iter().map_err(|e| AideMemoError::StoreRead {
            table: "relations",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })? {
            let (_key, value) = entry.map_err(|e| AideMemoError::StoreRead {
                table: "relations",
                key: "<entry>".to_string(),
                source: Box::new(e),
            })?;
            let record: RelationRecord =
                serde_json::from_slice(value.value()).map_err(|e| AideMemoError::Deserialize {
                    context: "relations_list_all".to_string(),
                    source: e,
                })?;
            results.push(record);
        }
        Ok(results)
    }
}

/// SHA-256 hex digest of the content portion of the dedup key. We hash raw
/// bytes (no content normalisation or lowercasing), so punctuation-different
/// content does not collide; that belongs to the semantic-dedup pass.
fn sha256_hex(s: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(s.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Source-aware exact-dedup key. A NUL separator is unambiguous because the
/// content digest is fixed-width lowercase hex; the source namespace itself
/// remains case-sensitive.
fn fact_dedup_key(source_id: Option<&str>, content: &str) -> String {
    let source_id = normalize_source_id(source_id).unwrap_or_default();
    format!("{source_id}\0{}", sha256_hex(content))
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
    fn fact_exact_dedup_is_scoped_by_normalized_source_id() {
        let mut store = create_test_store();
        let content = "shared release decision";

        let alpha = store
            .fact_add(FactInput {
                content: content.to_string(),
                source_id: Some("  alpha  ".to_string()),
                actor_id: Some("codex:alpha".to_string()),
                ..Default::default()
            })
            .unwrap();
        let beta = store
            .fact_add(FactInput {
                content: content.to_string(),
                source_id: Some("beta".to_string()),
                actor_id: Some("codex:beta".to_string()),
                ..Default::default()
            })
            .unwrap();
        let alpha_again = store
            .fact_add(FactInput {
                content: content.to_string(),
                source_id: Some("alpha".to_string()),
                actor_id: Some("other-writer".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_ne!(alpha, beta, "different sources retain independent ids");
        assert_eq!(alpha_again, alpha, "same normalized source still dedups");
        let alpha_record = store.fact_get(&alpha).unwrap();
        let beta_record = store.fact_get(&beta).unwrap();
        assert_eq!(alpha_record.source_id.as_deref(), Some("alpha"));
        assert_eq!(alpha_record.actor_id.as_deref(), Some("codex:alpha"));
        assert_eq!(beta_record.source_id.as_deref(), Some("beta"));
        assert_eq!(beta_record.actor_id.as_deref(), Some("codex:beta"));
        assert_eq!(store.stats().unwrap().fact_count, 2);
    }

    #[test]
    fn sync_upsert_keeps_same_content_from_different_sources() {
        let mut store = create_test_store();
        let mut alpha = FactRecord::new(
            "synced decision".to_string(),
            FactType::Decision,
            Vec::new(),
        );
        alpha.source_id = Some("alpha".to_string());
        alpha.actor_id = Some("agent-a".to_string());
        let mut beta = FactRecord::new(
            "synced decision".to_string(),
            FactType::Decision,
            Vec::new(),
        );
        beta.source_id = Some("beta".to_string());
        beta.actor_id = Some("agent-b".to_string());

        assert!(store.fact_upsert_record(alpha.clone()).unwrap());
        assert!(store.fact_upsert_record(beta.clone()).unwrap());
        assert_eq!(store.fact_get(&alpha.id).unwrap().actor_id, alpha.actor_id);
        assert_eq!(store.fact_get(&beta.id).unwrap().actor_id, beta.actor_id);
        assert_eq!(store.stats().unwrap().fact_count, 2);
    }

    #[test]
    fn fact_delete_releases_source_aware_dedup_key() {
        let mut store = create_test_store();
        let input = || FactInput {
            content: "archivable fact".to_string(),
            source_id: Some("alpha".to_string()),
            ..Default::default()
        };
        let original = store.fact_add(input()).unwrap();
        store.fact_delete(&original).unwrap();
        let replacement = store.fact_add(input()).unwrap();

        assert_ne!(replacement, original);
        assert_eq!(
            store.fact_get(&replacement).unwrap().content,
            "archivable fact"
        );
    }

    #[test]
    fn open_migrates_legacy_global_content_hash_index() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("legacy.redb");
        let content = "legacy shared content";
        let (alpha, beta) = {
            let mut store = Store::open(&path, Config::default()).unwrap();
            let alpha = store
                .fact_add(FactInput {
                    content: content.to_string(),
                    source_id: Some("alpha".to_string()),
                    actor_id: Some("legacy-a".to_string()),
                    ..Default::default()
                })
                .unwrap();
            let beta = store
                .fact_add(FactInput {
                    content: content.to_string(),
                    source_id: Some("beta".to_string()),
                    actor_id: Some("legacy-b".to_string()),
                    ..Default::default()
                })
                .unwrap();
            (alpha, beta)
        };

        // Recreate the v1 metadata/index shape: one global content-hash entry.
        let db = Database::create(&path).unwrap();
        let write_txn = db.begin_write().unwrap();
        {
            let mut meta = write_txn.open_table(META_TABLE).unwrap();
            meta.insert("schema_version", 1_u32.to_le_bytes().as_slice())
                .unwrap();
        }
        {
            let mut index = write_txn.open_table(FACT_CONTENT_HASH_TABLE).unwrap();
            index.retain(|_, _| false).unwrap();
            index
                .insert(sha256_hex(content).as_str(), alpha.as_bytes().as_slice())
                .unwrap();
        }
        write_txn.commit().unwrap();
        drop(db);

        let mut store = Store::open(&path, Config::default()).unwrap();
        assert_eq!(store.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
        let alpha_again = store
            .fact_add(FactInput {
                content: content.to_string(),
                source_id: Some("alpha".to_string()),
                ..Default::default()
            })
            .unwrap();
        let beta_again = store
            .fact_add(FactInput {
                content: content.to_string(),
                source_id: Some("beta".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(alpha_again, alpha);
        assert_eq!(beta_again, beta);
        assert_eq!(store.stats().unwrap().fact_count, 2);
    }

    #[test]
    fn open_migrates_legacy_relation_keys_to_source_scope() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("legacy-relations.redb");
        let (source_id, target_id) = {
            let mut store = Store::open(&path, Config::default()).unwrap();
            let source_id = store
                .entity_add(EntityInput {
                    name: "SharedSource".to_string(),
                    ..Default::default()
                })
                .unwrap();
            let target_id = store
                .entity_add(EntityInput {
                    name: "SharedTarget".to_string(),
                    ..Default::default()
                })
                .unwrap();
            (source_id, target_id)
        };

        let legacy_record = RelationRecord {
            source_id,
            target_id,
            scope_source_id: None,
            relation_type: RelationType::new("links"),
            weight: 1.0,
            evidence: vec!["legacy proof".to_string()],
            created_at: 42,
        };
        let mut legacy_value = serde_json::to_value(&legacy_record).unwrap();
        legacy_value
            .as_object_mut()
            .unwrap()
            .remove("scope_source_id");
        let legacy_bytes = serde_json::to_vec(&legacy_value).unwrap();
        let forward_key = format!("{}\0{}\0{}", source_id, "links", target_id).into_bytes();
        let reverse_key = format!("{}\0{}\0{}", target_id, "links", source_id).into_bytes();

        let db = Database::create(&path).unwrap();
        let write_txn = db.begin_write().unwrap();
        {
            let mut relations = write_txn.open_table(RELATIONS_TABLE).unwrap();
            relations.retain(|_, _| false).unwrap();
            relations
                .insert(forward_key.as_slice(), legacy_bytes.as_slice())
                .unwrap();
        }
        {
            let mut reverse = write_txn.open_table(RELATIONS_REV_TABLE).unwrap();
            reverse.retain(|_, _| false).unwrap();
            reverse
                .insert(reverse_key.as_slice(), legacy_bytes.as_slice())
                .unwrap();
        }
        {
            let mut meta = write_txn.open_table(META_TABLE).unwrap();
            meta.insert("schema_version", 2_u32.to_le_bytes().as_slice())
                .unwrap();
        }
        write_txn.commit().unwrap();
        drop(db);

        let mut store = Store::open(&path, Config::default()).unwrap();
        assert_eq!(store.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
        for (scope, evidence) in [("alpha", "alpha proof"), ("beta", "beta proof")] {
            store
                .relation_add(RelationInput {
                    source: "SharedSource".to_string(),
                    target: "SharedTarget".to_string(),
                    scope_source_id: Some(scope.to_string()),
                    relation_type: RelationType::new("links"),
                    weight: Some(1.0),
                    evidence: Some(vec![evidence.to_string()]),
                })
                .unwrap();
        }
        let relations = store
            .relations_get("SharedSource", TraverseDirection::Forward)
            .unwrap();
        assert_eq!(relations.len(), 3);
        assert!(relations.iter().any(|relation| {
            relation.scope_source_id.is_none() && relation.evidence == ["legacy proof"]
        }));
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
        assert!(matches!(err, AideMemoError::StoreOpen { .. }));
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
                source_id: None,
                actor_id: None,
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
                    source_id: None,
                    actor_id: None,
                    observed_at: None,
                    superseded_at: None,
                    superseded_by: None,
                    pinned: None,
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
    fn fact_list_filters_by_source_id() {
        let mut store = create_test_store();
        let entity_id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        for (content, source_id) in [
            ("Redis alpha source fact", "alpha"),
            ("Redis beta source fact", "beta"),
            ("Redis alpha second fact", "alpha"),
        ] {
            store
                .fact_add(FactInput {
                    content: content.to_string(),
                    fact_type: Some(FactType::Note),
                    entity_ids: Some(vec![entity_id]),
                    source_id: Some(source_id.to_string()),
                    ..Default::default()
                })
                .unwrap();
        }

        let alpha = store
            .fact_list(FactListOpts {
                source_id: Some("alpha".to_string()),
                ..Default::default()
            })
            .unwrap();
        let beta = store
            .fact_list(FactListOpts {
                source_id: Some("beta".to_string()),
                ..Default::default()
            })
            .unwrap();
        let missing = store
            .fact_list(FactListOpts {
                source_id: Some("missing".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(alpha.len(), 2);
        assert!(
            alpha
                .iter()
                .all(|f| f.source_id.as_deref() == Some("alpha"))
        );
        assert_eq!(beta.len(), 1);
        assert!(beta.iter().all(|f| f.source_id.as_deref() == Some("beta")));
        assert!(missing.is_empty());
    }

    #[cfg(feature = "semantic-adapt")]
    #[test]
    fn adapt_train_status_roundtrips_persisted_adapter() {
        let mut store = create_test_store();
        let fact_id = FactId(ulid::Ulid::new());
        store
            .search_feedback_add(&SearchFeedback {
                session_id: "session-1".to_string(),
                fact_id,
                helpful: true,
                timestamp: 1,
            })
            .unwrap();

        let train = store.adapt_train().unwrap();
        assert_eq!(train.feedback_used, 1);
        assert_eq!(train.helpful_count, 1);
        assert_eq!(train.generation, 1);

        let status = store.adapt_status().unwrap();
        assert_eq!(status.feedback_count, 1);
        assert!(status.has_adapter);
        assert_eq!(status.generation, 1);
        assert!(status.ready);

        let loaded = store.load_adapter().unwrap();
        assert!(!loaded.is_empty());
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
                scope_source_id: None,
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
    fn fact_list_entity_filter_preserves_existing_filters() {
        let mut store = create_test_store();
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        store
            .entity_add(EntityInput {
                name: "Postgres".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let redis_id = store.resolve_entity("Redis").unwrap();
        let postgres_id = store.resolve_entity("Postgres").unwrap();

        let redis_alpha = store
            .fact_add(FactInput {
                content: "redis alpha note".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis_id]),
                source_id: Some("alpha".to_string()),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .unwrap();
        let redis_decision = store
            .fact_add(FactInput {
                content: "redis alpha decision".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![redis_id]),
                source_id: Some("alpha".to_string()),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .unwrap();
        let redis_beta = store
            .fact_add(FactInput {
                content: "redis beta note".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis_id]),
                source_id: Some("beta".to_string()),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .unwrap();
        store
            .fact_add(FactInput {
                content: "postgres alpha note".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![postgres_id]),
                source_id: Some("alpha".to_string()),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .unwrap();

        let filtered = store
            .fact_list(FactListOpts {
                entity_id: Some(redis_id),
                fact_type: Some(FactType::Note),
                source_id: Some("alpha".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, redis_alpha);

        store
            .fact_update(
                &redis_beta,
                FactUpdate {
                    superseded_at: Some(crate::time::current_epoch_ms()),
                    superseded_by: Some(redis_decision),
                    ..Default::default()
                },
            )
            .unwrap();
        let current = store
            .fact_list(FactListOpts {
                entity_id: Some(redis_id),
                current_only: true,
                ..Default::default()
            })
            .unwrap();
        let current_ids: std::collections::HashSet<_> = current.iter().map(|f| f.id).collect();
        assert!(current_ids.contains(&redis_alpha));
        assert!(current_ids.contains(&redis_decision));
        assert!(!current_ids.contains(&redis_beta));

        let paged = store
            .fact_list(FactListOpts {
                entity_id: Some(redis_id),
                limit: Some(1),
                offset: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(paged.len(), 1);
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
    fn fact_add_dedup_merges_new_entity_ids() {
        // Re-ingesting the same content with a different entity must
        // (a) return the same fact id, (b) merge the new entity into
        // the fact's entity_ids, and (c) make the fact discoverable
        // via the second entity's index — otherwise an agent that
        // re-ingests "I ride bikes" first under #Health then under
        // #Cycling would silently lose the #Cycling link.
        let mut store = create_test_store();
        store
            .entity_add(EntityInput {
                name: "Health".to_string(),
                entity_type: Some(EntityType::Custom("topic".into())),
                ..Default::default()
            })
            .unwrap();
        store
            .entity_add(EntityInput {
                name: "Cycling".to_string(),
                entity_type: Some(EntityType::Custom("topic".into())),
                ..Default::default()
            })
            .unwrap();
        let health_id = store.resolve_entity("Health").unwrap();
        let cycling_id = store.resolve_entity("Cycling").unwrap();

        let id1 = store
            .fact_add(FactInput {
                content: "I ride bikes on weekends".into(),
                entity_ids: Some(vec![health_id]),
                ..Default::default()
            })
            .unwrap();
        let id2 = store
            .fact_add(FactInput {
                content: "I ride bikes on weekends".into(),
                entity_ids: Some(vec![cycling_id]),
                ..Default::default()
            })
            .unwrap();

        // Same content collapses to one fact.
        assert_eq!(id1, id2, "same content must dedup to one fact id");

        // Both entities now claim the fact.
        let rec = store.fact_get(&id1).unwrap();
        assert!(rec.entity_ids.contains(&health_id));
        assert!(rec.entity_ids.contains(&cycling_id));
        assert_eq!(rec.entity_ids.len(), 2, "no duplicate entity ids");

        // fact_by_entity index has both rows.
        assert_eq!(store.count_entity_facts(&health_id).unwrap(), 1);
        assert_eq!(store.count_entity_facts(&cycling_id).unwrap(), 1);

        // Re-ingesting under Health a second time is a no-op (no
        // duplicate index row, no duplicate entity id in the record).
        let id3 = store
            .fact_add(FactInput {
                content: "I ride bikes on weekends".into(),
                entity_ids: Some(vec![health_id]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(id3, id1);
        let rec2 = store.fact_get(&id1).unwrap();
        assert_eq!(rec2.entity_ids.len(), 2);
        assert_eq!(store.count_entity_facts(&health_id).unwrap(), 1);
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
            source_id: None,
            actor_id: None,
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let table =
            read_txn
                .open_table(SEARCH_FEEDBACK_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "search_feedback",
                    key: "<all>".to_string(),
                    source: Box::new(e),
                })?;

        let mut feedback_pairs = Vec::new();
        let iter = table.iter().map_err(|e| AideMemoError::StoreRead {
            table: "search_feedback",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })?;

        for item in iter {
            let (_, value) = item.map_err(|e| AideMemoError::StoreRead {
                table: "search_feedback",
                key: "<item>".to_string(),
                source: Box::new(e),
            })?;
            let fb: crate::types::SearchFeedback =
                serde_json::from_slice(value.value()).map_err(|e| AideMemoError::Deserialize {
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
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let table =
            read_txn
                .open_table(SEARCH_FEEDBACK_TABLE)
                .map_err(|e| AideMemoError::StoreRead {
                    table: "search_feedback",
                    key: "<all>".to_string(),
                    source: Box::new(e),
                })?;

        let mut feedback_pairs = Vec::new();
        let iter = table.iter().map_err(|e| AideMemoError::StoreRead {
            table: "search_feedback",
            key: "<iter>".to_string(),
            source: Box::new(e),
        })?;

        for item in iter {
            let (_, value) = item.map_err(|e| AideMemoError::StoreRead {
                table: "search_feedback",
                key: "<item>".to_string(),
                source: Box::new(e),
            })?;
            let fb: crate::types::SearchFeedback =
                serde_json::from_slice(value.value()).map_err(|e| AideMemoError::Deserialize {
                    context: "search_feedback".to_string(),
                    source: e,
                })?;
            feedback_pairs.push((fb.fact_id.to_string(), fb.helpful));
        }

        drop(read_txn);

        let adapter = self.load_adapter()?;
        Ok(adapter.evaluate(&feedback_pairs, 10))
    }

    /// Load the adapter from meta bytes, or return a fresh one. Public so
    /// the search engine can pull it on the hot path without a second
    /// `Store` plumbing.
    pub fn load_adapter(&self) -> Result<crate::adapt::DomainAdapter> {
        match self.meta_get::<crate::adapt::DomainAdapter>("adapter_state")? {
            Some(adapter) => Ok(adapter),
            None => Ok(crate::adapt::DomainAdapter::new()),
        }
    }

    /// Get a meta value as bytes.
    fn meta_get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;

        let meta = read_txn
            .open_table(META_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "meta",
                key: key.to_string(),
                source: Box::new(e),
            })?;

        match meta.get(key).map_err(|e| AideMemoError::StoreRead {
            table: "meta",
            key: key.to_string(),
            source: Box::new(e),
        })? {
            Some(value) => {
                let bytes = value.value();
                let val: T =
                    serde_json::from_slice(bytes).map_err(|e| AideMemoError::Deserialize {
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
            .map_err(|e| AideMemoError::StoreWrite {
                table: "meta",
                key: key.to_string(),
                source: Box::new(e),
            })?;

        meta.insert(key, value)
            .map_err(|e| AideMemoError::StoreWrite {
                table: "meta",
                key: key.to_string(),
                source: Box::new(e),
            })?;
        drop(meta);

        write_txn
            .commit()
            .map_err(|e| AideMemoError::Internal(format!("meta set commit failed: {}", e)))?;

        Ok(())
    }
}
