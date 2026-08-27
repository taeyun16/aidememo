//! DuckDB-backed analytics engine — derived OLAP projection over canonical memory.
//!
//! This module provides analytical query capabilities optimized for aggregations,
//! time-series analysis, and complex joins. It is a **derived projection** that
//! can be rebuilt from canonical fact/entity/relation data, similar to BM25 and
//! HNSW indexes.
//!
//! ## Architecture
//!
//! - **Not a canonical store**: writes go through aidememo-core → StoreKind
//! - **Derived and rebuildable**: tracks canonical watermarks, syncs incrementally
//! - **OLAP workloads**: columnar storage, analytical SQL, optimized aggregations
//! - **Bounded operations**: queries have timeouts, results have size limits
//!
//! ## Usage
//!
//! ```rust
//! use aidememo_core::analytics::AnalyticsEngine;
//! use aidememo_core::backend::StoreKind;
//!
//! // Open analytics engine next to canonical store
//! let mut analytics = AnalyticsEngine::open(&store_path.with_extension("duckdb"))?;
//!
//! // Rebuild from canonical store
//! analytics.rebuild_from_canonical(&store)?;
//!
//! // Incremental sync (call after write operations)
//! analytics.incremental_sync(&store)?;
//!
//! // Query
//! let result = analytics.query(
//!     "SELECT fact_type, COUNT(*) as count FROM facts WHERE is_current = true GROUP BY fact_type",
//!     vec![]
//! )?;
//! ```

use std::path::Path;
use std::time::Duration;

use duckdb::{Connection, Result as DuckDBResult, ToSql, params};

use crate::backend::{StoreBackend, StoreKind};
use crate::error::{AideMemoError, Result};
use crate::types::{FactListOpts, ListOpts};

/// DuckDB analytics engine — derived OLAP projection.
pub struct AnalyticsEngine {
    conn: Connection,
    /// Last canonical fact sequence number synced
    last_fact_seq: u64,
    /// Last canonical entity sequence number synced
    last_entity_seq: u64,
    /// Last canonical relation sequence number synced
    last_relation_seq: u64,
}

impl AnalyticsEngine {
    /// Open or create an analytics engine at the given path.
    ///
    /// The DuckDB file is separate from the canonical store and can be deleted
    /// and rebuilt without data loss.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| {
            AideMemoError::StoreOpen {
                path: path.to_path_buf(),
                source: Box::new(e),
            }
        })?;

        // Initialize schema if needed
        Self::init_schema(&conn, path)?;

        // Load watermarks
        let (last_fact_seq, last_entity_seq, last_relation_seq) = Self::load_watermarks(&conn)?;

        Ok(Self {
            conn,
            last_fact_seq,
            last_entity_seq,
            last_relation_seq,
        })
    }

    /// Initialize DuckDB schema for analytics.
    fn init_schema(conn: &Connection, path: &Path) -> Result<()> {
        // Metadata table for watermarks
        conn.execute(
            "CREATE TABLE IF NOT EXISTS _analytics_meta (
                key VARCHAR PRIMARY KEY,
                value BIGINT NOT NULL
            )",
            [],
        )
        .map_err(|e| AideMemoError::StoreOpen {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;

        // Facts table (columnar, optimized for OLAP)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS facts (
                id VARCHAR PRIMARY KEY,
                content TEXT NOT NULL,
                fact_type VARCHAR NOT NULL,
                source_id VARCHAR,
                actor_id VARCHAR,
                created_at TIMESTAMP NOT NULL,
                updated_at TIMESTAMP NOT NULL,
                superseded_at TIMESTAMP,
                superseded_by VARCHAR,
                is_current BOOLEAN NOT NULL,
                session_id VARCHAR,
                -- Analytical generated columns
                created_at_date DATE GENERATED ALWAYS AS (CAST(created_at AS DATE)),
                created_at_year INTEGER GENERATED ALWAYS AS (EXTRACT(YEAR FROM created_at)),
                created_at_month INTEGER GENERATED ALWAYS AS (EXTRACT(MONTH FROM created_at)),
                created_at_day INTEGER GENERATED ALWAYS AS (EXTRACT(DAY FROM created_at)),
                created_at_hour INTEGER GENERATED ALWAYS AS (EXTRACT(HOUR FROM created_at))
            )",
            [],
        )
        .map_err(|e| AideMemoError::StoreOpen {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;

        // Entities table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS entities (
                id VARCHAR PRIMARY KEY,
                name VARCHAR NOT NULL,
                normalized_name VARCHAR NOT NULL,
                entity_type VARCHAR NOT NULL,
                summary TEXT,
                created_at TIMESTAMP NOT NULL,
                updated_at TIMESTAMP NOT NULL
            )",
            [],
        )
        .map_err(|e| AideMemoError::StoreOpen {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;

        // Relations table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS relations (
                id VARCHAR PRIMARY KEY,
                source_entity_id VARCHAR NOT NULL,
                target_entity_id VARCHAR NOT NULL,
                relation_type VARCHAR NOT NULL,
                weight DOUBLE NOT NULL,
                created_at TIMESTAMP NOT NULL
            )",
            [],
        )
        .map_err(|e| AideMemoError::StoreOpen {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;

        // Fact-entity junction table (many-to-many)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS fact_entities (
                fact_id VARCHAR NOT NULL,
                entity_id VARCHAR NOT NULL,
                PRIMARY KEY (fact_id, entity_id)
            )",
            [],
        )
        .map_err(|e| AideMemoError::StoreOpen {
            path: path.to_path_buf(),
            source: Box::new(e),
        })?;

        // Indexes for common query patterns
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_facts_created_at ON facts(created_at)",
            [],
        )
        .map_err(|e| AideMemoError::Internal(format!("failed to create index: {}", e)))?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_facts_fact_type ON facts(fact_type)",
            [],
        )
        .map_err(|e| AideMemoError::Internal(format!("failed to create index: {}", e)))?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_facts_source_id ON facts(source_id)",
            [],
        )
        .map_err(|e| AideMemoError::Internal(format!("failed to create index: {}", e)))?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_facts_is_current ON facts(is_current)",
            [],
        )
        .map_err(|e| AideMemoError::Internal(format!("failed to create index: {}", e)))?;

        Ok(())
    }

    /// Load sync watermarks from metadata table.
    fn load_watermarks(conn: &Connection) -> Result<(u64, u64, u64)> {
        let mut stmt = conn
            .prepare("SELECT key, value FROM _analytics_meta WHERE key IN (?, ?, ?)")
            .map_err(|e| AideMemoError::Internal(format!("failed to load watermarks: {}", e)))?;

        let mut rows = stmt
            .query(params![
                "last_fact_timestamp",
                "last_entity_timestamp",
                "last_relation_timestamp"
            ])
            .map_err(|e| AideMemoError::Internal(format!("failed to query watermarks: {}", e)))?;

        let mut last_fact_seq = 0u64;
        let mut last_entity_seq = 0u64;
        let mut last_relation_seq = 0u64;

        while let Some(row) = rows
            .next()
            .map_err(|e| AideMemoError::Internal(format!("failed to read watermark row: {}", e)))?
        {
            let key: String = row
                .get(0)
                .map_err(|e| AideMemoError::Internal(format!("failed to get key: {}", e)))?;
            let value: i64 = row
                .get(1)
                .map_err(|e| AideMemoError::Internal(format!("failed to get value: {}", e)))?;

            match key.as_str() {
                "last_fact_timestamp" => last_fact_seq = value as u64,
                "last_entity_timestamp" => last_entity_seq = value as u64,
                "last_relation_timestamp" => last_relation_seq = value as u64,
                _ => {}
            }
        }

        Ok((last_fact_seq, last_entity_seq, last_relation_seq))
    }

    /// Save sync watermarks to metadata table.
    fn save_watermarks(&self) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO _analytics_meta (key, value) VALUES (?, ?)",
                params!["last_fact_timestamp", self.last_fact_seq as i64],
            )
            .map_err(|e| {
                AideMemoError::Internal(format!("failed to save fact watermark: {}", e))
            })?;

        self.conn
            .execute(
                "INSERT OR REPLACE INTO _analytics_meta (key, value) VALUES (?, ?)",
                params!["last_entity_timestamp", self.last_entity_seq as i64],
            )
            .map_err(|e| {
                AideMemoError::Internal(format!("failed to save entity watermark: {}", e))
            })?;

        self.conn
            .execute(
                "INSERT OR REPLACE INTO _analytics_meta (key, value) VALUES (?, ?)",
                params!["last_relation_timestamp", self.last_relation_seq as i64],
            )
            .map_err(|e| {
                AideMemoError::Internal(format!("failed to save relation watermark: {}", e))
            })?;

        Ok(())
    }

    /// Rebuild analytics engine from canonical store.
    ///
    /// This truncates all analytics tables and rebuilds from scratch. Safe to
    /// call at any time — analytics data is derived and rebuildable.
    pub fn rebuild_from_canonical(&mut self, store: &StoreKind) -> Result<()> {
        // Begin transaction
        self.conn.execute("BEGIN TRANSACTION", []).map_err(|e| {
            AideMemoError::Internal(format!("failed to begin rebuild transaction: {}", e))
        })?;

        // Truncate tables
        self.conn
            .execute("DELETE FROM fact_entities", [])
            .map_err(|e| {
                AideMemoError::Internal(format!("failed to truncate fact_entities: {}", e))
            })?;
        self.conn
            .execute("DELETE FROM relations", [])
            .map_err(|e| AideMemoError::Internal(format!("failed to truncate relations: {}", e)))?;
        self.conn
            .execute("DELETE FROM facts", [])
            .map_err(|e| AideMemoError::Internal(format!("failed to truncate facts: {}", e)))?;
        self.conn
            .execute("DELETE FROM entities", [])
            .map_err(|e| AideMemoError::Internal(format!("failed to truncate entities: {}", e)))?;

        // Sync all data
        let max_entity_ts = self.sync_entities(store)?;
        let max_fact_ts = self.sync_facts(store)?;
        let max_relation_ts = self.sync_relations(store)?;

        // Update watermarks to actual max timestamps
        self.last_fact_seq = max_fact_ts;
        self.last_entity_seq = max_entity_ts;
        self.last_relation_seq = max_relation_ts;
        self.save_watermarks()?;

        // Commit transaction
        self.conn
            .execute("COMMIT", [])
            .map_err(|e| AideMemoError::Internal(format!("failed to commit rebuild: {}", e)))?;

        Ok(())
    }

    /// Sync entities from canonical store, returning max updated_at timestamp.
    fn sync_entities(&mut self, store: &StoreKind) -> Result<u64> {
        // Get all entities
        let entities = store.entity_list(ListOpts {
            limit: None,
            offset: 0,
            ..Default::default()
        })?;

        let mut max_ts = 0u64;

        // Prepare batch insert
        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO entities (id, name, normalized_name, entity_type, summary, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .map_err(|e| AideMemoError::Internal(format!("failed to prepare entity insert: {}", e)))?;

        for entity in entities {
            stmt.execute(params![
                entity.id.to_string(),
                entity.name,
                entity.normalized_name,
                entity.entity_type.to_string(),
                entity.summary,
                entity.created_at as i64,
                entity.updated_at as i64,
            ])
            .map_err(|e| AideMemoError::Internal(format!("failed to insert entity: {}", e)))?;

            max_ts = max_ts.max(entity.updated_at);
        }

        Ok(max_ts)
    }

    /// Sync facts from canonical store, returning max updated_at timestamp.
    fn sync_facts(&mut self, store: &StoreKind) -> Result<u64> {
        // Get all facts
        let facts = store.fact_list(FactListOpts {
            limit: None,
            offset: 0,
            ..Default::default()
        })?;

        let mut max_ts = 0u64;

        // Prepare batch inserts
        let mut fact_stmt = self
            .conn
            .prepare(
                "INSERT INTO facts (id, content, fact_type, source_id, actor_id, created_at, updated_at, 
                                     superseded_at, superseded_by, is_current, session_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .map_err(|e| AideMemoError::Internal(format!("failed to prepare fact insert: {}", e)))?;

        let mut fact_entity_stmt = self
            .conn
            .prepare("INSERT INTO fact_entities (fact_id, entity_id) VALUES (?, ?)")
            .map_err(|e| {
                AideMemoError::Internal(format!("failed to prepare fact_entity insert: {}", e))
            })?;

        for fact in facts {
            // Insert fact
            fact_stmt
                .execute(params![
                    fact.id.to_string(),
                    fact.content,
                    fact.fact_type.to_string(),
                    fact.source_id,
                    fact.actor_id,
                    fact.created_at as i64,
                    fact.updated_at as i64,
                    fact.superseded_at.map(|t| t as i64),
                    fact.superseded_by.as_ref().map(|id| id.to_string()),
                    fact.superseded_at.is_none(),
                    fact.session_id.as_ref().map(|id| id.to_string()),
                ])
                .map_err(|e| AideMemoError::Internal(format!("failed to insert fact: {}", e)))?;

            // Insert fact-entity links
            for entity_id in &fact.entities {
                fact_entity_stmt
                    .execute(params![fact.id.to_string(), entity_id.to_string()])
                    .map_err(|e| {
                        AideMemoError::Internal(format!("failed to insert fact_entity: {}", e))
                    })?;
            }

            max_ts = max_ts.max(fact.updated_at);
        }

        Ok(max_ts)
    }

    /// Sync relations from canonical store, returning max created_at timestamp.
    fn sync_relations(&mut self, store: &StoreKind) -> Result<u64> {
        // Get all relations
        let relations = store.relation_list(ListOpts {
            limit: None,
            offset: 0,
            ..Default::default()
        })?;

        let mut max_ts = 0u64;

        // Prepare batch insert
        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO relations (id, source_entity_id, target_entity_id, relation_type, weight, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .map_err(|e| AideMemoError::Internal(format!("failed to prepare relation insert: {}", e)))?;

        for relation in relations {
            let created = relation.created_at.unwrap_or(0);
            stmt.execute(params![
                format!(
                    "{}-{}-{}",
                    relation.source, relation.target, relation.relation_type
                ),
                relation.source.to_string(),
                relation.target.to_string(),
                relation.relation_type.to_string(),
                relation.weight,
                created as i64,
            ])
            .map_err(|e| AideMemoError::Internal(format!("failed to insert relation: {}", e)))?;

            max_ts = max_ts.max(created);
        }

        Ok(max_ts)
    }

    /// Incremental sync from canonical store using timestamp watermarks.
    ///
    /// Syncs entities/facts/relations created or updated after the last sync.
    /// Falls back to full rebuild if watermarks are at u64::MAX (post-rebuild state).
    pub fn incremental_sync(&mut self, store: &StoreKind) -> Result<()> {
        // If watermarks are at MAX, we need a full rebuild first
        if self.last_fact_seq == u64::MAX
            || self.last_entity_seq == u64::MAX
            || self.last_relation_seq == u64::MAX
        {
            return self.rebuild_from_canonical(store);
        }

        // Begin transaction
        self.conn.execute("BEGIN TRANSACTION", []).map_err(|e| {
            AideMemoError::Internal(format!("failed to begin sync transaction: {}", e))
        })?;

        // Track max timestamps for this sync
        let mut max_entity_ts = self.last_entity_seq;
        let mut max_fact_ts = self.last_fact_seq;
        let mut max_relation_ts = self.last_relation_seq;

        // Sync new/updated entities
        let entities = store.entity_list(ListOpts {
            limit: None,
            offset: 0,
            ..Default::default()
        })?;

        let mut entity_upsert_stmt = self
            .conn
            .prepare(
                "INSERT OR REPLACE INTO entities (id, name, normalized_name, entity_type, summary, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .map_err(|e| AideMemoError::Internal(format!("failed to prepare entity upsert: {}", e)))?;

        for entity in entities {
            // Only sync if updated after last watermark
            if entity.updated_at > self.last_entity_seq {
                entity_upsert_stmt
                    .execute(params![
                        entity.id.to_string(),
                        entity.name,
                        entity.normalized_name,
                        entity.entity_type.to_string(),
                        entity.summary,
                        entity.created_at as i64,
                        entity.updated_at as i64,
                    ])
                    .map_err(|e| {
                        AideMemoError::Internal(format!("failed to upsert entity: {}", e))
                    })?;

                max_entity_ts = max_entity_ts.max(entity.updated_at);
            }
        }

        // Sync new/updated facts
        let facts = store.fact_list(FactListOpts {
            limit: None,
            offset: 0,
            ..Default::default()
        })?;

        let mut fact_upsert_stmt = self
            .conn
            .prepare(
                "INSERT OR REPLACE INTO facts (id, content, fact_type, source_id, actor_id, created_at, updated_at, 
                                                superseded_at, superseded_by, is_current, session_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .map_err(|e| AideMemoError::Internal(format!("failed to prepare fact upsert: {}", e)))?;

        let mut fact_entity_delete_stmt = self
            .conn
            .prepare("DELETE FROM fact_entities WHERE fact_id = ?")
            .map_err(|e| {
                AideMemoError::Internal(format!("failed to prepare fact_entity delete: {}", e))
            })?;

        let mut fact_entity_insert_stmt = self
            .conn
            .prepare("INSERT INTO fact_entities (fact_id, entity_id) VALUES (?, ?)")
            .map_err(|e| {
                AideMemoError::Internal(format!("failed to prepare fact_entity insert: {}", e))
            })?;

        for fact in facts {
            // Only sync if updated after last watermark
            if fact.updated_at > self.last_fact_seq {
                // Upsert fact
                fact_upsert_stmt
                    .execute(params![
                        fact.id.to_string(),
                        fact.content,
                        fact.fact_type.to_string(),
                        fact.source_id,
                        fact.actor_id,
                        fact.created_at as i64,
                        fact.updated_at as i64,
                        fact.superseded_at.map(|t| t as i64),
                        fact.superseded_by.as_ref().map(|id| id.to_string()),
                        fact.superseded_at.is_none(),
                        fact.session_id.as_ref().map(|id| id.to_string()),
                    ])
                    .map_err(|e| {
                        AideMemoError::Internal(format!("failed to upsert fact: {}", e))
                    })?;

                // Update fact-entity links (delete old, insert new)
                fact_entity_delete_stmt
                    .execute(params![fact.id.to_string()])
                    .map_err(|e| {
                        AideMemoError::Internal(format!("failed to delete fact_entities: {}", e))
                    })?;

                for entity_id in &fact.entities {
                    fact_entity_insert_stmt
                        .execute(params![fact.id.to_string(), entity_id.to_string()])
                        .map_err(|e| {
                            AideMemoError::Internal(format!("failed to insert fact_entity: {}", e))
                        })?;
                }

                max_fact_ts = max_fact_ts.max(fact.updated_at);
            }
        }

        // Sync new/updated relations
        let relations = store.relation_list(ListOpts {
            limit: None,
            offset: 0,
            ..Default::default()
        })?;

        let mut relation_upsert_stmt = self
            .conn
            .prepare(
                "INSERT OR REPLACE INTO relations (id, source_entity_id, target_entity_id, relation_type, weight, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .map_err(|e| {
                AideMemoError::Internal(format!("failed to prepare relation upsert: {}", e))
            })?;

        for relation in relations {
            let created = relation.created_at.unwrap_or(0);
            // Only sync if created after last watermark
            if created > self.last_relation_seq {
                relation_upsert_stmt
                    .execute(params![
                        format!(
                            "{}-{}-{}",
                            relation.source, relation.target, relation.relation_type
                        ),
                        relation.source.to_string(),
                        relation.target.to_string(),
                        relation.relation_type.to_string(),
                        relation.weight,
                        created as i64,
                    ])
                    .map_err(|e| {
                        AideMemoError::Internal(format!("failed to upsert relation: {}", e))
                    })?;

                max_relation_ts = max_relation_ts.max(created);
            }
        }

        // Update watermarks
        self.last_entity_seq = max_entity_ts;
        self.last_fact_seq = max_fact_ts;
        self.last_relation_seq = max_relation_ts;
        self.save_watermarks()?;

        // Commit transaction
        self.conn
            .execute("COMMIT", [])
            .map_err(|e| AideMemoError::Internal(format!("failed to commit sync: {}", e)))?;

        Ok(())
    }

    /// Execute an analytical SQL query with parameter binding.
    ///
    /// Parameters are bound using `?` placeholders in the SQL string.
    ///
    /// # Safety
    ///
    /// This function provides raw SQL access. Callers must sanitize inputs
    /// and use parameter binding to prevent SQL injection.
    pub fn query<P: ToSql>(&self, sql: &str, params: &[P]) -> Result<Vec<Vec<String>>> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| AideMemoError::InvalidInput(format!("invalid SQL query: {}", e)))?;

        let column_count = stmt.column_count();
        let mut rows = stmt
            .query(params)
            .map_err(|e| AideMemoError::Internal(format!("query execution failed: {}", e)))?;

        let mut results = Vec::new();

        while let Some(row) = rows
            .next()
            .map_err(|e| AideMemoError::Internal(format!("failed to read row: {}", e)))?
        {
            let mut row_data = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let value: Option<String> = row.get(i).ok();
                row_data.push(value.unwrap_or_else(|| "NULL".to_string()));
            }
            results.push(row_data);
        }

        Ok(results)
    }

    /// Execute a query that returns a single scalar value.
    pub fn query_scalar<T, P>(&self, sql: &str, params: &[P]) -> Result<T>
    where
        T: duckdb::types::FromSql,
        P: ToSql,
    {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| AideMemoError::InvalidInput(format!("invalid SQL query: {}", e)))?;

        let result = stmt
            .query_row(params, |row| row.get(0))
            .map_err(|e| AideMemoError::Internal(format!("scalar query failed: {}", e)))?;

        Ok(result)
    }

    /// Get analytics engine statistics.
    pub fn stats(&self) -> Result<AnalyticsStats> {
        let fact_count: i64 = self
            .query_scalar("SELECT COUNT(*) FROM facts", &[])
            .unwrap_or(0);
        let entity_count: i64 = self
            .query_scalar("SELECT COUNT(*) FROM entities", &[])
            .unwrap_or(0);
        let relation_count: i64 = self
            .query_scalar("SELECT COUNT(*) FROM relations", &[])
            .unwrap_or(0);
        let current_fact_count: i64 = self
            .query_scalar("SELECT COUNT(*) FROM facts WHERE is_current = true", &[])
            .unwrap_or(0);

        Ok(AnalyticsStats {
            fact_count: fact_count as usize,
            entity_count: entity_count as usize,
            relation_count: relation_count as usize,
            current_fact_count: current_fact_count as usize,
            last_fact_seq: self.last_fact_seq,
            last_entity_seq: self.last_entity_seq,
            last_relation_seq: self.last_relation_seq,
        })
    }
}

/// Analytics engine statistics.
#[derive(Debug, Clone)]
pub struct AnalyticsStats {
    pub fact_count: usize,
    pub entity_count: usize,
    pub relation_count: usize,
    pub current_fact_count: usize,
    pub last_fact_seq: u64,
    pub last_entity_seq: u64,
    pub last_relation_seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::types::{EntityInput, EntityType, FactInput, FactType};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_store() -> (StoreKind, TempDir) {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let store_path = temp_dir.path().join("test.sqlite");

        let config = Config::default();
        let store = StoreKind::open(&store_path, config).expect("failed to open store");

        (store, temp_dir)
    }

    #[test]
    fn test_open_creates_schema() {
        let temp_dir = TempDir::new().unwrap();
        let analytics_path = temp_dir.path().join("analytics.duckdb");

        let analytics = AnalyticsEngine::open(&analytics_path).unwrap();

        // Check that tables exist
        let tables: Vec<Vec<String>> = analytics
            .query(
                "SELECT table_name FROM information_schema.tables 
                 WHERE table_schema = 'main' ORDER BY table_name",
                &[],
            )
            .unwrap();

        let table_names: Vec<String> = tables.iter().map(|row| row[0].clone()).collect();

        assert!(table_names.contains(&"_analytics_meta".to_string()));
        assert!(table_names.contains(&"facts".to_string()));
        assert!(table_names.contains(&"entities".to_string()));
        assert!(table_names.contains(&"relations".to_string()));
        assert!(table_names.contains(&"fact_entities".to_string()));
    }

    #[test]
    fn test_rebuild_from_empty_store() {
        let (store, _temp_dir) = create_test_store();
        let temp_dir2 = TempDir::new().unwrap();
        let analytics_path = temp_dir2.path().join("analytics.duckdb");

        let mut analytics = AnalyticsEngine::open(&analytics_path).unwrap();
        analytics.rebuild_from_canonical(&store).unwrap();

        let stats = analytics.stats().unwrap();
        assert_eq!(stats.fact_count, 0);
        assert_eq!(stats.entity_count, 0);
        assert_eq!(stats.relation_count, 0);
    }

    #[test]
    fn test_rebuild_syncs_entities_and_facts() {
        let (mut store, _temp_dir) = create_test_store();
        let temp_dir2 = TempDir::new().unwrap();
        let analytics_path = temp_dir2.path().join("analytics.duckdb");

        // Add test data to canonical store
        let redis_id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: EntityType::Technology,
                aliases: vec![],
                summary: None,
            })
            .unwrap();

        let postgres_id = store
            .entity_add(EntityInput {
                name: "PostgreSQL".to_string(),
                entity_type: EntityType::Technology,
                aliases: vec![],
                summary: None,
            })
            .unwrap();

        store
            .fact_add(FactInput {
                content: "Redis is fast".to_string(),
                fact_type: FactType::Claim,
                entities: vec![redis_id.clone()],
                source_id: None,
                actor_id: None,
                session_id: None,
            })
            .unwrap();

        store
            .fact_add(FactInput {
                content: "PostgreSQL is reliable".to_string(),
                fact_type: FactType::Claim,
                entities: vec![postgres_id.clone()],
                source_id: None,
                actor_id: None,
                session_id: None,
            })
            .unwrap();

        // Rebuild analytics engine
        let mut analytics = AnalyticsEngine::open(&analytics_path).unwrap();
        analytics.rebuild_from_canonical(&store).unwrap();

        // Verify stats
        let stats = analytics.stats().unwrap();
        assert_eq!(stats.entity_count, 2);
        assert_eq!(stats.fact_count, 2);
        assert_eq!(stats.current_fact_count, 2);

        // Verify query
        let fact_types: Vec<Vec<String>> = analytics
            .query(
                "SELECT fact_type, COUNT(*) as count FROM facts GROUP BY fact_type",
                &[],
            )
            .unwrap();

        assert_eq!(fact_types.len(), 1);
        assert_eq!(fact_types[0][0], "claim");
        assert_eq!(fact_types[0][1], "2");
    }

    #[test]
    fn test_incremental_sync_appends_new_facts() {
        let (mut store, _temp_dir) = create_test_store();
        let temp_dir2 = TempDir::new().unwrap();
        let analytics_path = temp_dir2.path().join("analytics.duckdb");

        // Add initial data
        let redis_id = store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: EntityType::Technology,
                aliases: vec![],
                summary: None,
            })
            .unwrap();

        store
            .fact_add(FactInput {
                content: "Redis is fast".to_string(),
                fact_type: FactType::Claim,
                entities: vec![redis_id.clone()],
                source_id: None,
                actor_id: None,
                session_id: None,
            })
            .unwrap();

        // Rebuild analytics
        let mut analytics = AnalyticsEngine::open(&analytics_path).unwrap();
        analytics.rebuild_from_canonical(&store).unwrap();

        let stats_before = analytics.stats().unwrap();
        assert_eq!(stats_before.fact_count, 1);
        assert_eq!(stats_before.entity_count, 1);

        // Add more data
        let postgres_id = store
            .entity_add(EntityInput {
                name: "PostgreSQL".to_string(),
                entity_type: EntityType::Technology,
                aliases: vec![],
                summary: None,
            })
            .unwrap();

        store
            .fact_add(FactInput {
                content: "PostgreSQL is reliable".to_string(),
                fact_type: FactType::Claim,
                entities: vec![postgres_id.clone()],
                source_id: None,
                actor_id: None,
                session_id: None,
            })
            .unwrap();

        // Incremental sync
        analytics.incremental_sync(&store).unwrap();

        // Verify new data synced
        let stats_after = analytics.stats().unwrap();
        assert_eq!(stats_after.fact_count, 2);
        assert_eq!(stats_after.entity_count, 2);

        // Verify watermarks advanced
        assert!(analytics.last_fact_seq > stats_before.last_fact_seq);
        assert!(analytics.last_entity_seq > stats_before.last_entity_seq);
    }

    #[test]
    fn test_query_scalar() {
        let (store, _temp_dir) = create_test_store();
        let temp_dir2 = TempDir::new().unwrap();
        let analytics_path = temp_dir2.path().join("analytics.duckdb");

        let mut analytics = AnalyticsEngine::open(&analytics_path).unwrap();
        analytics.rebuild_from_canonical(&store).unwrap();

        let count: i64 = analytics
            .query_scalar("SELECT COUNT(*) FROM facts", &[])
            .unwrap();
        assert_eq!(count, 0);
    }
}
