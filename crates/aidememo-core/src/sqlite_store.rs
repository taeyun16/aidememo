//! SQLite storage backend.
//!
//! This is the default local backend and can be selected explicitly at runtime
//! with `store.backend = "sqlite"` or `store.backend = "libsqlite"`. It
//! implements the `StoreBackend` surface shared with the optional redb backend.
//! Remote libSQL/Turso semantics are intentionally out of scope for this local
//! backend.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};
use ulid::Ulid;

use crate::backend::StoreBackend;
use crate::config::Config;
use crate::error::{AideMemoError, Result};
use crate::types::{
    EntityId, EntityInput, EntityRecord, EntitySort, EntitySummary, EntityType, EntityUpdate,
    FactId, FactInput, FactListOpts, FactRecord, FactType, FactUpdate, ListOpts, RelationInput,
    RelationRecord, SearchFeedback, SearchSession, StoreStats, TraverseDirection,
};

/// SQLite-backed store.
pub struct SqliteStore {
    conn: Mutex<Connection>,
    config: Arc<Config>,
    path: PathBuf,
}

impl SqliteStore {
    /// Open or create a SQLite store.
    pub fn open(path: &Path, mut config: Config) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| AideMemoError::StoreOpen {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
        }
        config.store.path = path.to_string_lossy().into_owned();

        let conn = Connection::open(path).map_err(|source| AideMemoError::StoreOpen {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        // Keep SQLite's internal wait short, then let the outer retry loop add
        // jitter up to the configured total budget. This avoids synchronized
        // writer convoys when several agent profiles share one store.
        let sqlite_timeout_ms = config.store.lock_retry_ms.min(1_000);
        conn.busy_timeout(Duration::from_millis(sqlite_timeout_ms))
            .map_err(|source| sqlite_write("pragma", "busy_timeout", source))?;
        let store = Self {
            conn: Mutex::new(conn),
            config: Arc::new(config),
            path: path.to_path_buf(),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Access the backend configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    fn init_schema(&self) -> Result<()> {
        let synchronous = match self.config.store.durability.as_str() {
            "eventual" => "NORMAL",
            _ => "FULL",
        };
        let conn = self.conn.lock();
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|source| sqlite_write("pragma", "journal_mode", source))?;
        conn.pragma_update(None, "synchronous", synchronous)
            .map_err(|source| sqlite_write("pragma", "synchronous", source))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|source| sqlite_write("pragma", "foreign_keys", source))?;

        conn.execute_batch(
            r#"
                CREATE TABLE IF NOT EXISTS meta (
                    key TEXT PRIMARY KEY,
                    value BLOB NOT NULL
                );

                CREATE TABLE IF NOT EXISTS entities (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    name_lower TEXT NOT NULL UNIQUE,
                    entity_type TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    record_json BLOB NOT NULL
                );

                CREATE TABLE IF NOT EXISTS entity_names (
                    name_lower TEXT PRIMARY KEY,
                    entity_id TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS facts (
                    id TEXT PRIMARY KEY,
                    content_hash TEXT NOT NULL UNIQUE,
                    content TEXT NOT NULL,
                    fact_type TEXT NOT NULL,
                    source_confidence REAL NOT NULL,
                    source_id TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    observed_at INTEGER,
                    superseded_at INTEGER,
                    pinned INTEGER NOT NULL DEFAULT 0,
                    record_json BLOB NOT NULL
                );

                CREATE TABLE IF NOT EXISTS fact_entities (
                    entity_id TEXT NOT NULL,
                    fact_id TEXT NOT NULL,
                    PRIMARY KEY (entity_id, fact_id)
                );

                CREATE TABLE IF NOT EXISTS relations (
                    source_id TEXT NOT NULL,
                    relation_type TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    record_json BLOB NOT NULL,
                    PRIMARY KEY (source_id, relation_type, target_id)
                );

                CREATE TABLE IF NOT EXISTS search_sessions (
                    id TEXT PRIMARY KEY,
                    record_json BLOB NOT NULL
                );

                CREATE TABLE IF NOT EXISTS search_feedback (
                    key TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    fact_id TEXT NOT NULL,
                    helpful INTEGER NOT NULL,
                    timestamp INTEGER NOT NULL,
                    record_json BLOB NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_fact_entities_fact
                    ON fact_entities(fact_id);
                CREATE INDEX IF NOT EXISTS idx_relations_target
                    ON relations(target_id);
                CREATE INDEX IF NOT EXISTS idx_search_feedback_session
                    ON search_feedback(session_id);
                CREATE INDEX IF NOT EXISTS idx_search_feedback_fact
                    ON search_feedback(fact_id);
                CREATE INDEX IF NOT EXISTS idx_facts_type
                    ON facts(fact_type);
                CREATE INDEX IF NOT EXISTS idx_facts_source_id
                    ON facts(source_id);
                CREATE INDEX IF NOT EXISTS idx_facts_created_at
                    ON facts(created_at);

                CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts
                    USING fts5(fact_id UNINDEXED, content);
                "#,
        )
        .map_err(|source| sqlite_write("schema", "<batch>", source))?;
        Ok(())
    }

    fn insert_entity_record(conn: &Connection, record: &EntityRecord) -> Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|source| AideMemoError::Serialize {
            context: format!("entity {:?}", record.id),
            source,
        })?;
        conn.execute(
            "INSERT INTO entities (id, name, name_lower, entity_type, updated_at, record_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.id.to_string(),
                record.name,
                record.name_lower,
                record.entity_type.to_string(),
                record.updated_at as i64,
                bytes
            ],
        )
        .map_err(|source| sqlite_write("entities", &record.id.to_string(), source))?;

        for name in
            std::iter::once(record.name.as_str()).chain(record.aliases.iter().map(String::as_str))
        {
            conn.execute(
                "INSERT INTO entity_names (name_lower, entity_id) VALUES (?1, ?2)",
                params![name.to_lowercase(), record.id.to_string()],
            )
            .map_err(|source| sqlite_write("entity_names", name, source))?;
        }
        Ok(())
    }

    fn update_entity_record(conn: &Connection, record: &EntityRecord) -> Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|source| AideMemoError::Serialize {
            context: format!("entity update {:?}", record.id),
            source,
        })?;
        conn.execute(
            "UPDATE entities
             SET name = ?2,
                 name_lower = ?3,
                 entity_type = ?4,
                 updated_at = ?5,
                 record_json = ?6
             WHERE id = ?1",
            params![
                record.id.to_string(),
                record.name,
                record.name_lower,
                record.entity_type.to_string(),
                record.updated_at as i64,
                bytes
            ],
        )
        .map_err(|source| sqlite_write("entities", &record.id.to_string(), source))?;
        conn.execute(
            "DELETE FROM entity_names WHERE entity_id = ?1",
            params![record.id.to_string()],
        )
        .map_err(|source| sqlite_write("entity_names", &record.id.to_string(), source))?;
        for name in
            std::iter::once(record.name.as_str()).chain(record.aliases.iter().map(String::as_str))
        {
            conn.execute(
                "INSERT INTO entity_names (name_lower, entity_id) VALUES (?1, ?2)",
                params![name.to_lowercase(), record.id.to_string()],
            )
            .map_err(|source| sqlite_write("entity_names", name, source))?;
        }
        Ok(())
    }

    fn get_entity_record(conn: &Connection, id: EntityId) -> Result<EntityRecord> {
        let bytes: Vec<u8> = conn
            .query_row(
                "SELECT record_json FROM entities WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| sqlite_read("entities", &id.to_string(), source))?
            .ok_or_else(|| AideMemoError::EntityIdNotFound(id.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|source| AideMemoError::Deserialize {
            context: format!("entity {:?}", id),
            source,
        })
    }

    fn insert_fact_record(
        conn: &Connection,
        record: &FactRecord,
        content_hash: &str,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|source| AideMemoError::Serialize {
            context: format!("fact {:?}", record.id),
            source,
        })?;
        conn.execute(
            "INSERT INTO facts (
                id, content_hash, content, fact_type, source_confidence, source_id,
                created_at, updated_at, observed_at, superseded_at, pinned, record_json
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                record.id.to_string(),
                content_hash,
                record.content,
                record.fact_type.to_string(),
                record.source_confidence,
                record.source_id,
                record.created_at as i64,
                record.updated_at as i64,
                record.observed_at.map(|v| v as i64),
                record.superseded_at.map(|v| v as i64),
                if record.pinned { 1 } else { 0 },
                bytes
            ],
        )
        .map_err(|source| sqlite_write("facts", &record.id.to_string(), source))?;
        conn.execute(
            "INSERT INTO facts_fts (fact_id, content) VALUES (?1, ?2)",
            params![record.id.to_string(), record.content],
        )
        .map_err(|source| sqlite_write("facts_fts", &record.id.to_string(), source))?;
        for entity_id in &record.entity_ids {
            conn.execute(
                "INSERT OR IGNORE INTO fact_entities (entity_id, fact_id) VALUES (?1, ?2)",
                params![entity_id.to_string(), record.id.to_string()],
            )
            .map_err(|source| sqlite_write("fact_entities", &record.id.to_string(), source))?;
        }
        Ok(())
    }

    fn update_fact_record(conn: &Connection, record: &FactRecord) -> Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|source| AideMemoError::Serialize {
            context: format!("fact update {:?}", record.id),
            source,
        })?;
        conn.execute(
            "UPDATE facts
             SET content = ?2,
                 fact_type = ?3,
                 source_confidence = ?4,
                 source_id = ?5,
                 updated_at = ?6,
                 observed_at = ?7,
                 superseded_at = ?8,
                 pinned = ?9,
                 record_json = ?10,
                 content_hash = ?11
             WHERE id = ?1",
            params![
                record.id.to_string(),
                record.content,
                record.fact_type.to_string(),
                record.source_confidence,
                record.source_id,
                record.updated_at as i64,
                record.observed_at.map(|v| v as i64),
                record.superseded_at.map(|v| v as i64),
                if record.pinned { 1 } else { 0 },
                bytes,
                sha256_hex(&record.content)
            ],
        )
        .map_err(|source| sqlite_write("facts", &record.id.to_string(), source))?;
        conn.execute(
            "DELETE FROM facts_fts WHERE fact_id = ?1",
            params![record.id.to_string()],
        )
        .map_err(|source| sqlite_write("facts_fts", &record.id.to_string(), source))?;
        conn.execute(
            "INSERT INTO facts_fts (fact_id, content) VALUES (?1, ?2)",
            params![record.id.to_string(), record.content],
        )
        .map_err(|source| sqlite_write("facts_fts", &record.id.to_string(), source))?;
        Ok(())
    }

    fn replace_fact_entities(conn: &Connection, record: &FactRecord) -> Result<()> {
        conn.execute(
            "DELETE FROM fact_entities WHERE fact_id = ?1",
            params![record.id.to_string()],
        )
        .map_err(|source| sqlite_write("fact_entities", &record.id.to_string(), source))?;
        for entity_id in &record.entity_ids {
            conn.execute(
                "INSERT OR IGNORE INTO fact_entities (entity_id, fact_id) VALUES (?1, ?2)",
                params![entity_id.to_string(), record.id.to_string()],
            )
            .map_err(|source| sqlite_write("fact_entities", &record.id.to_string(), source))?;
        }
        Ok(())
    }

    fn insert_relation_record(conn: &Connection, record: &RelationRecord) -> Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|source| AideMemoError::Serialize {
            context: "relation".to_string(),
            source,
        })?;
        conn.execute(
            "INSERT OR REPLACE INTO relations (
                source_id, relation_type, target_id, created_at, record_json
             )
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.source_id.to_string(),
                record.relation_type.to_string(),
                record.target_id.to_string(),
                record.created_at as i64,
                bytes
            ],
        )
        .map_err(|source| sqlite_write("relations", &relation_key(record), source))?;
        Ok(())
    }

    fn get_fact_record(conn: &Connection, id: &FactId) -> Result<FactRecord> {
        let bytes: Vec<u8> = conn
            .query_row(
                "SELECT record_json FROM facts WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| sqlite_read("facts", &id.to_string(), source))?
            .ok_or_else(|| AideMemoError::FactNotFound(id.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|source| AideMemoError::Deserialize {
            context: format!("fact {:?}", id),
            source,
        })
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

    fn fact_id_by_hash(conn: &Connection, content_hash: &str) -> Result<Option<FactId>> {
        let raw: Option<String> = conn
            .query_row(
                "SELECT id FROM facts WHERE content_hash = ?1",
                params![content_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| sqlite_read("facts", content_hash, source))?;
        Ok(raw.and_then(|id| Ulid::from_string(&id).ok()).map(FactId))
    }

    fn merge_fact_entities(conn: &Connection, id: FactId, entity_ids: Vec<EntityId>) -> Result<()> {
        if entity_ids.is_empty() {
            return Ok(());
        }
        let mut record = Self::get_fact_record(conn, &id)?;
        let mut changed = false;
        for entity_id in entity_ids {
            conn.execute(
                "INSERT OR IGNORE INTO fact_entities (entity_id, fact_id) VALUES (?1, ?2)",
                params![entity_id.to_string(), id.to_string()],
            )
            .map_err(|source| sqlite_write("fact_entities", &id.to_string(), source))?;
            if !record.entity_ids.contains(&entity_id) {
                record.entity_ids.push(entity_id);
                changed = true;
            }
        }
        if changed {
            Self::update_fact_record(conn, &record)?;
        }
        Ok(())
    }

    #[cfg(feature = "semantic-adapt")]
    fn feedback_pairs(&self) -> Result<Vec<(String, bool)>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT record_json FROM search_feedback ORDER BY key ASC")
            .map_err(|source| sqlite_read("search_feedback", "<prepare>", source))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|source| sqlite_read("search_feedback", "<iter>", source))?;
        let mut pairs = Vec::new();
        for row in rows {
            let bytes = row.map_err(|source| sqlite_read("search_feedback", "<row>", source))?;
            let feedback: SearchFeedback =
                serde_json::from_slice(&bytes).map_err(|source| AideMemoError::Deserialize {
                    context: "search_feedback".to_string(),
                    source,
                })?;
            pairs.push((feedback.fact_id.to_string(), feedback.helpful));
        }
        Ok(pairs)
    }
}

impl StoreBackend for SqliteStore {
    fn open(path: &Path, config: Config) -> Result<Self> {
        SqliteStore::open(path, config)
    }

    fn stats(&self) -> Result<StoreStats> {
        let conn = self.conn.lock();
        let entity_count = count_table(&conn, "entities")?;
        let fact_count = count_table(&conn, "facts")?;
        let relation_count = count_table(&conn, "relations")?;
        let total_size_bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let last_ingest_at = meta_get_u64(&conn, "last_ingest_at")?;
        Ok(StoreStats {
            entity_count,
            fact_count,
            relation_count,
            total_size_bytes,
            last_ingest_at,
        })
    }

    fn config(&self) -> &Config {
        &self.config
    }

    fn set_last_ingest_at(&self) -> Result<()> {
        let now = crate::time::current_epoch_ms();
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let conn = self.conn.lock();
            conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params!["last_ingest_at", now.to_string().into_bytes()],
            )
            .map_err(|source| sqlite_write("meta", "last_ingest_at", source))?;
            Ok(())
        })
    }

    fn entity_add(&mut self, input: EntityInput) -> Result<EntityId> {
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let input = input.clone();
            let mut record =
                EntityRecord::new(input.name, input.entity_type.unwrap_or(EntityType::Unknown));
            if let Some(aliases) = input.aliases {
                record.aliases = aliases;
            }
            if let Some(tags) = input.tags {
                record.tags = tags;
            }
            if let Some(source_page) = input.source_page {
                record.source_page = Some(source_page);
            }
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("entities", "begin", source))?;
            Self::insert_entity_record(&tx, &record)?;
            tx.commit()
                .map_err(|source| sqlite_write("entities", "commit", source))?;
            Ok(record.id)
        })
    }

    fn entity_get(&self, name: &str) -> Result<EntityRecord> {
        let id = self.resolve_entity(name)?;
        self.entity_get_by_id(id)
    }

    fn resolve_entity(&self, name: &str) -> Result<EntityId> {
        let raw: Option<String> = {
            let conn = self.conn.lock();
            conn.query_row(
                "SELECT entity_id FROM entity_names WHERE name_lower = ?1",
                params![name.to_lowercase()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| sqlite_read("entity_names", name, source))?
        };
        raw.and_then(|id| Ulid::from_string(&id).ok())
            .map(EntityId)
            .ok_or_else(|| {
                let suggestions = self.suggest_similar_entities(name).unwrap_or_default();
                AideMemoError::entity_not_found(name.to_string(), suggestions)
            })
    }

    fn entity_get_by_id(&self, id: EntityId) -> Result<EntityRecord> {
        let conn = self.conn.lock();
        Self::get_entity_record(&conn, id)
    }

    fn entity_update(&mut self, name: &str, input: EntityUpdate) -> Result<()> {
        let name = name.to_string();
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let mut record = self.entity_get(&name)?;
            record.update(input.clone());
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("entities", "begin", source))?;
            Self::update_entity_record(&tx, &record)?;
            tx.commit()
                .map_err(|source| sqlite_write("entities", "commit", source))?;
            Ok(())
        })
    }

    fn entity_list(&self, opts: ListOpts) -> Result<Vec<EntitySummary>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, record_json FROM entities ORDER BY id ASC")
            .map_err(|source| sqlite_read("entities", "<prepare>", source))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|source| sqlite_read("entities", "<iter>", source))?;
        let mut out = Vec::new();
        for row in rows {
            let (raw_id, bytes) = row.map_err(|source| sqlite_read("entities", "<row>", source))?;
            let record: EntityRecord =
                serde_json::from_slice(&bytes).map_err(|source| AideMemoError::Deserialize {
                    context: format!("entity {raw_id}"),
                    source,
                })?;
            if let Some(ref entity_type) = opts.entity_type
                && &record.entity_type != entity_type
            {
                continue;
            }
            let fact_count = count_entity_facts(&conn, &record.id)?;
            if let Some(min_facts) = opts.min_facts
                && fact_count < min_facts
            {
                continue;
            }
            out.push((
                EntitySummary {
                    id: record.id,
                    name: record.name,
                    entity_type: record.entity_type,
                    fact_count,
                    tags: record.tags,
                },
                record.updated_at,
            ));
        }
        match opts.sort_by {
            EntitySort::Name => out.sort_by(|a, b| a.0.name.cmp(&b.0.name)),
            EntitySort::UpdatedAt => {
                out.sort_by_key(|(_, updated_at)| std::cmp::Reverse(*updated_at))
            }
            EntitySort::FactCount => out.sort_by_key(|(e, _)| std::cmp::Reverse(e.fact_count)),
        }
        Ok(out
            .into_iter()
            .map(|(entity, _)| entity)
            .skip(opts.offset)
            .take(opts.limit.unwrap_or(usize::MAX))
            .collect())
    }

    fn entity_delete(&mut self, name: &str) -> Result<()> {
        let name = name.to_string();
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let record = self.entity_get(&name)?;
            let id = record.id.to_string();
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("entities", "begin", source))?;
            tx.execute("DELETE FROM entity_names WHERE entity_id = ?1", params![id])
                .map_err(|source| sqlite_write("entity_names", &id, source))?;
            tx.execute(
                "DELETE FROM relations WHERE source_id = ?1 OR target_id = ?1",
                params![id],
            )
            .map_err(|source| sqlite_write("relations", &id, source))?;
            tx.execute(
                "DELETE FROM fact_entities WHERE entity_id = ?1",
                params![id],
            )
            .map_err(|source| sqlite_write("fact_entities", &id, source))?;
            tx.execute("DELETE FROM entities WHERE id = ?1", params![id])
                .map_err(|source| sqlite_write("entities", &id, source))?;
            tx.commit()
                .map_err(|source| sqlite_write("entities", "commit", source))?;
            Ok(())
        })
    }

    fn entity_upsert_record(&mut self, record: EntityRecord) -> Result<bool> {
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let record = record.clone();
            if let Ok(local) = self.entity_get_by_id(record.id) {
                if local.updated_at >= record.updated_at {
                    return Ok(false);
                }
            } else if let Ok(existing_id) = self.resolve_entity(&record.name)
                && existing_id != record.id
            {
                return Ok(false);
            }

            let exists = self.entity_get_by_id(record.id).is_ok();
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("entities", "begin", source))?;
            if exists {
                Self::update_entity_record(&tx, &record)?;
            } else {
                Self::insert_entity_record(&tx, &record)?;
            }
            tx.commit()
                .map_err(|source| sqlite_write("entities", "commit", source))?;
            Ok(true)
        })
    }

    fn suggest_similar_entities(&self, name: &str) -> Result<Vec<String>> {
        let name_lower = name.to_lowercase();
        let mut suggestions = Vec::new();
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT record_json FROM entities")
            .map_err(|source| sqlite_read("entities", "<prepare>", source))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|source| sqlite_read("entities", "<iter>", source))?;
        for row in rows {
            let bytes = row.map_err(|source| sqlite_read("entities", "<row>", source))?;
            let record: EntityRecord =
                serde_json::from_slice(&bytes).map_err(|source| AideMemoError::Deserialize {
                    context: "suggest entities".to_string(),
                    source,
                })?;
            let mut similarity = trigram::similarity(&name_lower, &record.name_lower);
            for alias in &record.aliases {
                similarity =
                    similarity.max(trigram::similarity(&name_lower, &alias.to_lowercase()));
            }
            if similarity > 0.5 {
                suggestions.push((record.name, similarity));
            }
        }
        suggestions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(suggestions
            .into_iter()
            .take(5)
            .map(|(name, _)| name)
            .collect())
    }

    fn count_entity_facts(&self, entity_id: &EntityId) -> Result<u32> {
        let conn = self.conn.lock();
        count_entity_facts(&conn, entity_id)
    }

    fn fact_add(&mut self, input: FactInput) -> Result<FactId> {
        let mut ids = self.fact_add_many(vec![input])?;
        ids.pop()
            .ok_or_else(|| AideMemoError::Internal("sqlite fact_add returned no id".to_string()))
    }

    fn fact_add_many(&mut self, inputs: Vec<FactInput>) -> Result<Vec<FactId>> {
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            sqlite_fact_add_many_once(self, inputs.clone())
        })
    }

    fn fact_get(&self, id: &FactId) -> Result<FactRecord> {
        let conn = self.conn.lock();
        Self::get_fact_record(&conn, id)
    }

    fn fact_get_many(&self, ids: &[FactId]) -> Result<Vec<FactRecord>> {
        let conn = self.conn.lock();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(record) = Self::get_fact_record(&conn, id) {
                out.push(record);
            }
        }
        Ok(out)
    }

    fn fact_list(&self, opts: FactListOpts) -> Result<Vec<FactRecord>> {
        let sql = if opts.entity_id.is_some() {
            "SELECT f.record_json
             FROM fact_entities fe
             JOIN facts f ON f.id = fe.fact_id
             WHERE fe.entity_id = ?1
             ORDER BY f.id ASC"
        } else {
            "SELECT record_json FROM facts ORDER BY id ASC"
        };
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(sql)
            .map_err(|source| sqlite_read("facts", "<prepare>", source))?;

        let mut records = Vec::new();
        if let Some(entity_id) = opts.entity_id {
            let rows = stmt
                .query_map(params![entity_id.to_string()], |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .map_err(|source| sqlite_read("facts", "<iter>", source))?;
            for row in rows {
                let bytes = row.map_err(|source| sqlite_read("facts", "<row>", source))?;
                let record: FactRecord = serde_json::from_slice(&bytes).map_err(|source| {
                    AideMemoError::Deserialize {
                        context: "sqlite fact list".to_string(),
                        source,
                    }
                })?;
                if Self::fact_matches_list_opts(&record, &opts) {
                    records.push(record);
                }
            }
        } else {
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(|source| sqlite_read("facts", "<iter>", source))?;
            for row in rows {
                let bytes = row.map_err(|source| sqlite_read("facts", "<row>", source))?;
                let record: FactRecord = serde_json::from_slice(&bytes).map_err(|source| {
                    AideMemoError::Deserialize {
                        context: "sqlite fact list".to_string(),
                        source,
                    }
                })?;
                if Self::fact_matches_list_opts(&record, &opts) {
                    records.push(record);
                }
            }
        }

        Ok(records
            .into_iter()
            .skip(opts.offset)
            .take(opts.limit.unwrap_or(usize::MAX))
            .collect())
    }

    fn fact_update(&mut self, id: &FactId, input: FactUpdate) -> Result<()> {
        let id = *id;
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("facts", "begin", source))?;
            let mut record = Self::get_fact_record(&tx, &id)?;
            record.update(input.clone());
            Self::update_fact_record(&tx, &record)?;
            tx.commit()
                .map_err(|source| sqlite_write("facts", "commit", source))?;
            Ok(())
        })
    }

    fn fact_delete(&mut self, id: &FactId) -> Result<()> {
        let id = *id;
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let raw_id = id.to_string();
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("facts", "begin", source))?;
            tx.execute("DELETE FROM facts_fts WHERE fact_id = ?1", params![raw_id])
                .map_err(|source| sqlite_write("facts_fts", &raw_id, source))?;
            tx.execute(
                "DELETE FROM fact_entities WHERE fact_id = ?1",
                params![raw_id],
            )
            .map_err(|source| sqlite_write("fact_entities", &raw_id, source))?;
            let removed = tx
                .execute("DELETE FROM facts WHERE id = ?1", params![raw_id])
                .map_err(|source| sqlite_write("facts", &raw_id, source))?;
            tx.commit()
                .map_err(|source| sqlite_write("facts", "commit", source))?;
            if removed == 0 {
                return Err(AideMemoError::FactNotFound(raw_id));
            }
            Ok(())
        })
    }

    fn fact_upsert_record(&mut self, record: FactRecord) -> Result<bool> {
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let record = record.clone();
            let content_hash = sha256_hex(&record.content);
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("facts", "begin", source))?;

            if let Ok(local) = Self::get_fact_record(&tx, &record.id)
                && local.updated_at >= record.updated_at
            {
                return Ok(false);
            }
            if let Some(existing_id) = Self::fact_id_by_hash(&tx, &content_hash)?
                && existing_id != record.id
            {
                return Ok(false);
            }

            if Self::get_fact_record(&tx, &record.id).is_ok() {
                Self::update_fact_record(&tx, &record)?;
                Self::replace_fact_entities(&tx, &record)?;
            } else {
                Self::insert_fact_record(&tx, &record, &content_hash)?;
            }
            tx.commit()
                .map_err(|source| sqlite_write("facts", "commit", source))?;
            Ok(true)
        })
    }

    fn pinned_facts(&self, limit: usize) -> Result<Vec<FactRecord>> {
        let mut facts = self.fact_list(FactListOpts {
            current_only: true,
            limit: None,
            ..Default::default()
        })?;
        facts.retain(|fact| fact.pinned);
        facts.sort_by_key(|fact| std::cmp::Reverse(fact.last_accessed_at));
        facts.truncate(limit);
        Ok(facts)
    }

    fn fact_feedback(&mut self, id: &FactId, helpful: bool) -> Result<()> {
        let id = *id;
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("facts", "begin", source))?;
            let mut record = Self::get_fact_record(&tx, &id)?;
            if helpful {
                record.relevance_score = (record.relevance_score + 0.10).min(1.0);
            } else {
                record.relevance_score = (record.relevance_score - 0.15).max(0.0);
            }
            record.updated_at =
                crate::time::current_epoch_ms().max(record.updated_at.saturating_add(1));
            Self::update_fact_record(&tx, &record)?;
            tx.commit()
                .map_err(|source| sqlite_write("facts", "commit", source))?;
            Ok(())
        })
    }

    fn search_session_add(&mut self, session: &SearchSession) -> Result<()> {
        let session = session.clone();
        let bytes = serde_json::to_vec(&session).map_err(|source| AideMemoError::Serialize {
            context: "search_session".to_string(),
            source,
        })?;
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let conn = self.conn.lock();
            conn.execute(
                "INSERT OR REPLACE INTO search_sessions (id, record_json) VALUES (?1, ?2)",
                params![session.id, bytes],
            )
            .map_err(|source| sqlite_write("search_sessions", &session.id, source))?;
            Ok(())
        })
    }

    fn search_feedback_add(&mut self, feedback: &SearchFeedback) -> Result<()> {
        let feedback = feedback.clone();
        let key = format!("{}:{}", feedback.session_id, feedback.fact_id);
        let bytes = serde_json::to_vec(&feedback).map_err(|source| AideMemoError::Serialize {
            context: "search_feedback".to_string(),
            source,
        })?;
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let conn = self.conn.lock();
            conn.execute(
                "INSERT OR REPLACE INTO search_feedback (
                        key, session_id, fact_id, helpful, timestamp, record_json
                     )
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    key,
                    feedback.session_id,
                    feedback.fact_id.to_string(),
                    if feedback.helpful { 1 } else { 0 },
                    feedback.timestamp as i64,
                    bytes
                ],
            )
            .map_err(|source| sqlite_write("search_feedback", &key, source))?;
            Ok(())
        })
    }

    fn search_feedback_count(&self) -> Result<usize> {
        let conn = self.conn.lock();
        count_table(&conn, "search_feedback").map(|n| n as usize)
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_train(&mut self) -> Result<crate::types::AdaptResult> {
        let pairs = self.feedback_pairs()?;
        let mut adapter = crate::adapt::DomainAdapter::new();
        let result = adapter.train(&pairs);
        let bytes = adapter.to_bytes()?;
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let conn = self.conn.lock();
            meta_set_bytes(&conn, "adapter_state", &bytes)?;
            Ok(())
        })?;
        Ok(result)
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_status(&self) -> Result<crate::types::AdaptStatus> {
        let feedback_count = self.search_feedback_count()?;
        let adapter = self.load_adapter()?;
        Ok(adapter.status(feedback_count))
    }

    #[cfg(feature = "semantic-adapt")]
    fn adapt_eval(&self) -> Result<crate::types::AdaptEvalReport> {
        let pairs = self.feedback_pairs()?;
        let adapter = self.load_adapter()?;
        Ok(adapter.evaluate(&pairs, 10))
    }

    #[cfg(feature = "semantic-adapt")]
    fn load_adapter(&self) -> Result<crate::adapt::DomainAdapter> {
        let conn = self.conn.lock();
        match meta_get_bytes(&conn, "adapter_state")? {
            Some(bytes) => crate::adapt::DomainAdapter::from_bytes(&bytes),
            None => Ok(crate::adapt::DomainAdapter::new()),
        }
    }

    fn relation_add(&mut self, input: RelationInput) -> Result<()> {
        let source_id = self.resolve_entity(&input.source)?;
        let target_id = self.resolve_entity(&input.target)?;
        let record = RelationRecord {
            source_id,
            target_id,
            relation_type: input.relation_type,
            weight: input.weight.unwrap_or(1.0),
            evidence: input.evidence.unwrap_or_default(),
            created_at: crate::time::current_epoch_ms(),
        };
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let conn = self.conn.lock();
            Self::insert_relation_record(&conn, &record)
        })
    }

    fn relation_upsert_record(&mut self, record: RelationRecord) -> Result<bool> {
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let record = record.clone();
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_write("relations", "begin", source))?;
            let exists: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM relations
                     WHERE source_id = ?1 AND relation_type = ?2 AND target_id = ?3",
                    params![
                        record.source_id.to_string(),
                        record.relation_type.to_string(),
                        record.target_id.to_string()
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|source| sqlite_read("relations", &relation_key(&record), source))?;
            if exists.is_some() {
                return Ok(false);
            }
            Self::insert_relation_record(&tx, &record)?;
            tx.commit()
                .map_err(|source| sqlite_write("relations", "commit", source))?;
            Ok(true)
        })
    }

    fn relation_remove(&mut self, source: &str, target: &str, rel_type: &str) -> Result<()> {
        let source_id = self.resolve_entity(source)?;
        let target_id = self.resolve_entity(target)?;
        let source_name = source.to_string();
        let target_name = target.to_string();
        let rel_type = rel_type.to_string();
        let lock_retry_ms = self.config.store.lock_retry_ms;
        sqlite_lock_retry(lock_retry_ms, || {
            let conn = self.conn.lock();
            let removed = conn
                .execute(
                    "DELETE FROM relations
                     WHERE source_id = ?1 AND relation_type = ?2 AND target_id = ?3",
                    params![source_id.to_string(), rel_type, target_id.to_string()],
                )
                .map_err(|source| sqlite_write("relations", &rel_type, source))?;
            if removed == 0 {
                return Err(AideMemoError::RelationNotFound {
                    source_name: source_name.clone(),
                    rel_type: rel_type.clone(),
                    target: target_name.clone(),
                });
            }
            Ok(())
        })
    }

    fn relations_get(
        &self,
        entity_name: &str,
        direction: TraverseDirection,
    ) -> Result<Vec<RelationRecord>> {
        let entity_id = self.resolve_entity(entity_name)?;
        let sql = match direction {
            TraverseDirection::Forward => {
                "SELECT record_json FROM relations WHERE source_id = ?1 ORDER BY created_at ASC"
            }
            TraverseDirection::Reverse => {
                "SELECT record_json FROM relations WHERE target_id = ?1 ORDER BY created_at ASC"
            }
            TraverseDirection::Both => {
                "SELECT record_json FROM relations
                 WHERE source_id = ?1 OR target_id = ?1
                 ORDER BY created_at ASC"
            }
        };
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(sql)
            .map_err(|source| sqlite_read("relations", "<prepare>", source))?;
        let rows = stmt
            .query_map(params![entity_id.to_string()], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .map_err(|source| sqlite_read("relations", "<iter>", source))?;
        let mut out = Vec::new();
        for row in rows {
            let bytes = row.map_err(|source| sqlite_read("relations", "<row>", source))?;
            out.push(serde_json::from_slice(&bytes).map_err(|source| {
                AideMemoError::Deserialize {
                    context: "relations_get".to_string(),
                    source,
                }
            })?);
        }
        Ok(out)
    }

    fn relations_list_all(&self) -> Result<Vec<RelationRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT record_json FROM relations ORDER BY created_at ASC")
            .map_err(|source| sqlite_read("relations", "<prepare>", source))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|source| sqlite_read("relations", "<iter>", source))?;
        let mut out = Vec::new();
        for row in rows {
            let bytes = row.map_err(|source| sqlite_read("relations", "<row>", source))?;
            out.push(serde_json::from_slice(&bytes).map_err(|source| {
                AideMemoError::Deserialize {
                    context: "relations_list_all".to_string(),
                    source,
                }
            })?);
        }
        Ok(out)
    }
}

fn count_table(conn: &Connection, table: &'static str) -> Result<u64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let count: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|source| sqlite_read(table, "<count>", source))?;
    Ok(count as u64)
}

fn meta_get_u64(conn: &Connection, key: &str) -> Result<Option<u64>> {
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| sqlite_read("meta", key, source))?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let raw = String::from_utf8(bytes).map_err(|source| AideMemoError::StoreRead {
        table: "meta",
        key: key.to_string(),
        source: Box::new(source),
    })?;
    Ok(raw.parse::<u64>().ok())
}

#[cfg(feature = "semantic-adapt")]
fn meta_get_bytes(conn: &Connection, key: &str) -> Result<Option<Vec<u8>>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|source| sqlite_read("meta", key, source))
}

#[cfg(feature = "semantic-adapt")]
fn meta_set_bytes(conn: &Connection, key: &str, value: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(|source| sqlite_write("meta", key, source))?;
    Ok(())
}

fn count_entity_facts(conn: &Connection, entity_id: &EntityId) -> Result<u32> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM fact_entities WHERE entity_id = ?1",
            params![entity_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|source| sqlite_read("fact_entities", &entity_id.to_string(), source))?;
    Ok(count as u32)
}

fn relation_key(record: &RelationRecord) -> String {
    format!(
        "{}\0{}\0{}",
        record.source_id, record.relation_type, record.target_id
    )
}

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

fn sqlite_read(table: &'static str, key: &str, source: rusqlite::Error) -> AideMemoError {
    AideMemoError::StoreRead {
        table,
        key: key.to_string(),
        source: Box::new(source),
    }
}

fn sqlite_write(table: &'static str, key: &str, source: rusqlite::Error) -> AideMemoError {
    AideMemoError::StoreWrite {
        table,
        key: key.to_string(),
        source: Box::new(source),
    }
}

fn sqlite_fact_add_many_once(
    store: &mut SqliteStore,
    inputs: Vec<FactInput>,
) -> Result<Vec<FactId>> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }

    let mut conn = store.conn.lock();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| sqlite_write("facts", "begin", source))?;
    let mut ids = Vec::with_capacity(inputs.len());

    for input in inputs {
        let content_hash = sha256_hex(&input.content);
        if let Some(existing_id) = SqliteStore::fact_id_by_hash(&tx, &content_hash)? {
            if let Some(entity_ids) = input.entity_ids {
                SqliteStore::merge_fact_entities(&tx, existing_id, entity_ids)?;
            }
            ids.push(existing_id);
            continue;
        }

        let mut record = FactRecord::new(
            input.content,
            input.fact_type.unwrap_or(FactType::Unknown),
            input.entity_ids.unwrap_or_default(),
        );
        if let Some(tags) = input.tags {
            record.tags = tags;
        }
        if let Some(source) = input.source {
            record.source = Some(source);
        }
        if let Some(source_id) = input.source_id {
            record.source_id = Some(source_id);
        }
        if let Some(actor_id) = input.actor_id {
            record.actor_id = Some(actor_id);
        }
        if let Some(confidence) = input.source_confidence {
            record.source_confidence = confidence;
        }
        if let Some(observed_at) = input.observed_at {
            record.observed_at = Some(observed_at);
        }
        SqliteStore::insert_fact_record(&tx, &record, &content_hash)?;
        ids.push(record.id);
    }

    tx.commit()
        .map_err(|source| sqlite_write("facts", "commit", source))?;
    Ok(ids)
}

fn sqlite_lock_retry<T>(lock_retry_ms: u64, mut op: impl FnMut() -> Result<T>) -> Result<T> {
    if lock_retry_ms == 0 {
        return op();
    }
    let budget = Duration::from_millis(lock_retry_ms);
    let started = Instant::now();
    let mut attempt = 0_u64;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err) if is_sqlite_lock_contention(&err) && started.elapsed() < budget => {
                attempt = attempt.saturating_add(1);
                let remaining = budget.saturating_sub(started.elapsed());
                // 20-150ms jitter mirrors the shared-session pattern used by
                // Hermes Agent. Distinct wake-up intervals prevent multiple
                // Codex/Hermes profiles from retrying in lockstep.
                let jitter_ms = 20
                    + crate::time::current_epoch_ms().wrapping_add(attempt.saturating_mul(73))
                        % 131;
                let sleep_for = remaining.min(Duration::from_millis(jitter_ms));
                if !sleep_for.is_zero() {
                    std::thread::sleep(sleep_for);
                }
            }
            Err(err) => return Err(err),
        }
    }
}

fn is_sqlite_lock_contention(err: &AideMemoError) -> bool {
    let source = match err {
        AideMemoError::StoreRead { source, .. }
        | AideMemoError::StoreWrite { source, .. }
        | AideMemoError::StoreOpen { source, .. }
        | AideMemoError::TransactionBegin { source } => source,
        _ => return false,
    };
    let Some(sqlite) = source.downcast_ref::<rusqlite::Error>() else {
        return false;
    };
    matches!(
        sqlite.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;
    use crate::index::Bm25IndexState;
    use crate::ingest::ingest_wiki;
    use crate::lint::LintEngine;
    use crate::search::SearchEngine;
    use crate::types::{RelationType, SearchOpts, TraverseOpts};
    use parking_lot::RwLock;

    fn open_store() -> (tempfile::TempDir, SqliteStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("wiki.sqlite");
        let store = SqliteStore::open(&path, Config::default()).expect("open sqlite");
        (dir, store)
    }

    #[test]
    fn sqlite_store_applies_lock_retry_as_busy_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("wiki.sqlite");
        let mut config = Config::default();
        config.store.lock_retry_ms = 2500;

        let store = SqliteStore::open(&path, config).expect("open sqlite");
        let conn = store.conn.lock();
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy timeout");
        assert_eq!(busy_timeout, 1000);
    }

    #[test]
    fn sqlite_store_entity_fact_roundtrip() {
        let (_dir, mut store) = open_store();
        let entity_id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                aliases: Some(vec!["Redis DB".to_string()]),
                ..Default::default()
            })
            .expect("entity_add");
        assert_eq!(store.resolve_entity("redis db").expect("alias"), entity_id);

        let fact_id = store
            .fact_add(FactInput {
                content: "Redis caches hot keys".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![entity_id]),
                source_id: Some("alpha".to_string()),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .expect("fact_add");

        let fact = store.fact_get(&fact_id).expect("fact_get");
        assert_eq!(fact.content, "Redis caches hot keys");
        assert_eq!(fact.entity_ids, vec![entity_id]);

        let stats = store.stats().expect("stats");
        assert_eq!(stats.entity_count, 1);
        assert_eq!(stats.fact_count, 1);
    }

    #[test]
    fn sqlite_store_fact_list_filters_match_redb_slice() {
        let (_dir, mut store) = open_store();
        let redis_id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .expect("redis");
        let postgres_id = store
            .entity_add(EntityInput {
                name: "Postgres".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .expect("postgres");

        let keep = store
            .fact_add(FactInput {
                content: "redis alpha note".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis_id]),
                source_id: Some("alpha".to_string()),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .expect("keep");
        let superseded = store
            .fact_add(FactInput {
                content: "redis beta note".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis_id]),
                source_id: Some("beta".to_string()),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .expect("superseded");
        store
            .fact_add(FactInput {
                content: "postgres alpha note".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![postgres_id]),
                source_id: Some("alpha".to_string()),
                source_confidence: Some(0.9),
                ..Default::default()
            })
            .expect("other entity");

        let filtered = store
            .fact_list(FactListOpts {
                entity_id: Some(redis_id),
                source_id: Some("alpha".to_string()),
                ..Default::default()
            })
            .expect("filtered");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, keep);

        store
            .fact_update(
                &superseded,
                FactUpdate {
                    superseded_at: Some(crate::time::current_epoch_ms()),
                    superseded_by: Some(keep),
                    ..Default::default()
                },
            )
            .expect("supersede");
        let current = store
            .fact_list(FactListOpts {
                entity_id: Some(redis_id),
                current_only: true,
                ..Default::default()
            })
            .expect("current");
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].id, keep);
    }

    #[test]
    fn sqlite_store_runs_graph_lint_and_bm25_search() {
        let (_dir, mut store) = open_store();
        let redis_id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                aliases: Some(vec!["redis-server".to_string()]),
                ..Default::default()
            })
            .expect("redis");
        let sentinel_id = store
            .entity_add(EntityInput {
                name: "Sentinel".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .expect("sentinel");
        store
            .relation_add(RelationInput {
                source: "Sentinel".to_string(),
                target: "Redis".to_string(),
                relation_type: RelationType::new("monitors"),
                weight: Some(1.0),
                evidence: Some(vec!["fixture".to_string()]),
            })
            .expect("relation");
        store
            .fact_add(FactInput {
                content: "Redis keeps hot cache keys".to_string(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![redis_id]),
                source_confidence: Some(1.0),
                ..Default::default()
            })
            .expect("redis fact");
        store
            .fact_add(FactInput {
                content: "Sentinel monitors Redis availability".to_string(),
                fact_type: Some(FactType::Claim),
                entity_ids: Some(vec![sentinel_id]),
                source_confidence: Some(1.0),
                ..Default::default()
            })
            .expect("sentinel fact");

        let graph = Graph::new(&store);
        let traversed = graph
            .traverse(
                "Sentinel",
                TraverseOpts {
                    depth: 1,
                    direction: TraverseDirection::Forward,
                    relation_types: None,
                },
            )
            .expect("traverse");
        assert_eq!(traversed.relations.len(), 1);
        assert!(
            traversed
                .entities
                .iter()
                .any(|entity| entity.name == "Redis")
        );

        let lint = LintEngine::new(&store).lint().expect("lint");
        assert_eq!(lint.relation_count, 1);
        assert_eq!(lint.fact_count, 2);

        let config = store.config().clone();
        let bm25_state = RwLock::new(Bm25IndexState::new());
        let engine = SearchEngine::new(&store, &config, &bm25_state);
        let hits = engine
            .search(
                "hot cache",
                SearchOpts {
                    limit: Some(5),
                    ..Default::default()
                },
            )
            .expect("search");
        assert_eq!(hits[0].fact_type, FactType::Note);
        assert!(hits[0].entity_names.contains(&"Redis".to_string()));

        store
            .relation_remove("Sentinel", "Redis", "monitors")
            .expect("relation_remove");
        assert_eq!(store.relations_list_all().expect("relations").len(), 0);
    }

    #[test]
    fn sqlite_store_ingests_markdown_wiki() {
        let (dir, mut store) = open_store();
        let wiki_root = dir.path().join("wiki");
        std::fs::create_dir_all(&wiki_root).expect("wiki dir");
        std::fs::write(
            wiki_root.join("Redis.md"),
            r#"---
type: technology
aliases:
  - redis-server
tags:
  - cache
---

Redis references [[Sentinel]].

## Decision: Cache

Use Redis for hot key caching.
"#,
        )
        .expect("write markdown");

        let stats = ingest_wiki(&wiki_root, &mut store, false).expect("ingest");
        assert_eq!(stats.files_scanned, 1);
        assert_eq!(stats.entities_added, 1);
        assert_eq!(stats.facts_added, 1);

        let redis = store.entity_get("Redis").expect("entity");
        let facts = store
            .fact_list(FactListOpts {
                entity_id: Some(redis.id),
                ..Default::default()
            })
            .expect("facts");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact_type, FactType::Decision);
        assert!(store.stats().expect("stats").last_ingest_at.is_some());
    }
}
