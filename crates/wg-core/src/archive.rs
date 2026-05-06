//! Cold-tier archive support.
//!
//! Old / decayed facts can be moved out of the hot redb file into a
//! sibling `.cold.redb` file so the hot store stays bounded. Search
//! reads hot only by default; an opt-in `include_archive` flag (added
//! in stage 2) opens the cold file on demand and merges results.
//!
//! ### Invariants (stage 1)
//!
//! * Cold stores **facts only**. Entity records stay in hot — cold
//!   facts' `entity_ids` cross-reference hot's `entities` table.
//!   Search hydration must always look up entities from hot.
//! * Cold preserves the original `FactId`. No re-keying. Back-
//!   references (e.g. `wg_fact_get` after a search hit) keep working
//!   from any caller that knows the original id.
//! * Cold maintains its own `fact_content_hash` index so re-archiving
//!   the same fact is idempotent (the dedup path on hot also keeps
//!   the cold side consistent).
//! * Hot deletion is committed in a separate write transaction *after*
//!   the cold write commits. If the process crashes between the two
//!   commits, the fact lives in both stores; the next archive call
//!   re-runs the cold write (idempotent via content hash) and
//!   completes the hot delete.
//!
//! Cold path is derived from the hot store path: `<hot>.cold.redb`.

use std::path::PathBuf;

use redb::ReadableTable;

use crate::error::{Result, WgError};
use crate::store::{
    FACTS_TABLE, FACT_BY_ENTITY_TABLE, FACT_CONTENT_HASH_TABLE, Store,
};
use crate::types::{FactId, FactRecord};

/// Compute the cold-tier db path for a hot db file. Both files sit
/// side by side so user backups copy them together.
pub fn cold_path_for(hot_path: &std::path::Path) -> PathBuf {
    let mut s = hot_path.as_os_str().to_os_string();
    s.push(".cold.redb");
    PathBuf::from(s)
}

/// SHA-256 hex of the fact content. Mirrors `store::sha256_hex` so
/// archive entries land in the same hash bucket cold-side dedup
/// expects. Reimplemented here so the archive module doesn't need to
/// expose store internals.
fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let bytes = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

impl Store {
    /// Where this store's cold-tier file lives.
    pub fn cold_path(&self) -> PathBuf {
        cold_path_for(std::path::Path::new(&self.config().store.path))
    }

    /// Open (creating if missing) the cold-tier `Store` sibling. The
    /// cold store inherits the hot store's `Config` so durability /
    /// retry settings stay consistent. Caller is responsible for not
    /// holding the hot write lock when opening cold to avoid lock
    /// nesting on the same redb file (different files here, but the
    /// pattern is worth preserving).
    pub fn open_cold(&self) -> Result<Store> {
        let path = self.cold_path();
        let mut cfg = (**self.config_arc()).clone();
        cfg.store.path = path.to_string_lossy().into_owned();
        Store::open(&path, cfg)
    }

    /// Move `fact_ids` from hot to cold. Returns the number of facts
    /// actually transferred (skips ids that don't exist in hot — they
    /// may already have been archived in an earlier call).
    ///
    /// Two write transactions: one on cold (insert facts + content
    /// hash), one on hot (delete facts + content hash + per-entity
    /// index rows). Cold commits first; hot commits second. A crash
    /// between them leaves a duplicate that the next archive call
    /// resolves via cold's content-hash dedup.
    pub fn archive_facts(&mut self, fact_ids: &[FactId]) -> Result<usize> {
        if fact_ids.is_empty() {
            return Ok(0);
        }
        // Phase 1: read fact records from hot (one read txn).
        let read_txn = self
            .db_arc()
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;
        let hot_facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: "<archive-read>".to_string(),
                source: Box::new(e),
            })?;
        let mut to_move: Vec<(FactId, FactRecord)> = Vec::with_capacity(fact_ids.len());
        for id in fact_ids {
            let raw = match hot_facts.get(id.as_bytes().as_slice()) {
                Ok(Some(v)) => v.value().to_vec(),
                _ => continue,
            };
            let record: FactRecord = match serde_json::from_slice(&raw) {
                Ok(r) => r,
                Err(_) => continue,
            };
            to_move.push((*id, record));
        }
        drop(hot_facts);
        drop(read_txn);

        if to_move.is_empty() {
            return Ok(0);
        }

        // Phase 2: write to cold (separate Store + separate redb file).
        let mut cold = self.open_cold()?;
        cold.cold_insert_archived(&to_move)?;

        // Phase 3: delete from hot.
        let entity_keys: Vec<(String, FactId)> = to_move
            .iter()
            .flat_map(|(id, rec)| {
                rec.entity_ids
                    .iter()
                    .map(move |eid| (format!("{}\0{}", eid, id), *id))
            })
            .collect();
        let content_hashes: Vec<String> = to_move
            .iter()
            .map(|(_, rec)| sha256_hex(&rec.content))
            .collect();
        let ids_to_delete: Vec<FactId> = to_move.iter().map(|(id, _)| *id).collect();

        let write_txn = self.begin_archive_write()?;
        {
            let mut facts =
                write_txn
                    .open_table(FACTS_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "facts",
                        key: "<archive-delete>".to_string(),
                        source: Box::new(e),
                    })?;
            let mut fact_by_entity =
                write_txn
                    .open_table(FACT_BY_ENTITY_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_by_entity",
                        key: "<archive-delete>".to_string(),
                        source: Box::new(e),
                    })?;
            let mut content_hash =
                write_txn
                    .open_table(FACT_CONTENT_HASH_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_content_hash",
                        key: "<archive-delete>".to_string(),
                        source: Box::new(e),
                    })?;
            for id in &ids_to_delete {
                let _ = facts.remove(id.as_bytes().as_slice());
            }
            for (key, _) in &entity_keys {
                let _ = fact_by_entity.remove(key.as_str());
            }
            for h in &content_hashes {
                let _ = content_hash.remove(h.as_str());
            }
        }
        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "facts",
            key: "archive-commit".to_string(),
            source: Box::new(e),
        })?;
        Ok(to_move.len())
    }

    /// Count facts in this store (used by archive tests / `wg stats
    /// --include-archive` later). Tiny single-table iter; not a hot
    /// path.
    pub fn fact_count(&self) -> Result<u64> {
        let read_txn = self
            .db_arc()
            .begin_read()
            .map_err(|e| WgError::TransactionBegin {
                source: Box::new(e),
            })?;
        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: "<count>".to_string(),
                source: Box::new(e),
            })?;
        Ok(facts
            .iter()
            .map_err(|e| WgError::StoreRead {
                table: "facts",
                key: "<iter>".to_string(),
                source: Box::new(e),
            })?
            .count() as u64)
    }

    /// Cold-side raw insert. Preserves IDs and rebuilds the
    /// content-hash index so cold-tier dedup keeps working. Skips ids
    /// that already exist in cold (idempotent re-archive).
    fn cold_insert_archived(&mut self, items: &[(FactId, FactRecord)]) -> Result<()> {
        let write_txn = self.begin_archive_write()?;
        {
            let mut facts = write_txn
                .open_table(FACTS_TABLE)
                .map_err(|e| WgError::StoreWrite {
                    table: "facts",
                    key: "<cold-insert>".to_string(),
                    source: Box::new(e),
                })?;
            let mut content_hash =
                write_txn
                    .open_table(FACT_CONTENT_HASH_TABLE)
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_content_hash",
                        key: "<cold-insert>".to_string(),
                        source: Box::new(e),
                    })?;
            for (id, record) in items {
                if facts
                    .get(id.as_bytes().as_slice())
                    .ok()
                    .flatten()
                    .is_some()
                {
                    continue; // already archived
                }
                let bytes = serde_json::to_vec(record).map_err(|e| WgError::Serialize {
                    context: format!("cold fact {:?}", id),
                    source: e,
                })?;
                facts
                    .insert(id.as_bytes().as_slice(), bytes.as_slice())
                    .map_err(|e| WgError::StoreWrite {
                        table: "facts",
                        key: id.to_string(),
                        source: Box::new(e),
                    })?;
                let h = sha256_hex(&record.content);
                content_hash
                    .insert(h.as_str(), id.as_bytes().as_slice())
                    .map_err(|e| WgError::StoreWrite {
                        table: "fact_content_hash",
                        key: h,
                        source: Box::new(e),
                    })?;
            }
        }
        write_txn.commit().map_err(|e| WgError::StoreWrite {
            table: "facts",
            key: "cold-commit".to_string(),
            source: Box::new(e),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::types::{EntityInput, EntityType, FactInput, FactType};
    use tempfile::TempDir;

    fn make_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hot.redb");
        let mut config = Config::default();
        config.store.path = path.to_string_lossy().into_owned();
        let store = Store::open(&path, config).unwrap();
        (dir, store)
    }

    #[test]
    fn cold_path_sits_next_to_hot() {
        let p = std::path::Path::new("/tmp/x.redb");
        assert_eq!(cold_path_for(p), std::path::PathBuf::from("/tmp/x.redb.cold.redb"));
    }

    #[test]
    fn archive_facts_moves_records_hot_to_cold() {
        let (_dir, mut store) = make_store();
        let entity_id = store
            .entity_add(EntityInput {
                name: "Topic".into(),
                entity_type: Some(EntityType::Custom("topic".into())),
                ..Default::default()
            })
            .unwrap();

        let inputs: Vec<FactInput> = (0..5)
            .map(|i| FactInput {
                content: format!("fact {i}"),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![entity_id]),
                ..Default::default()
            })
            .collect();
        let ids = store.fact_add_many(inputs).unwrap();
        assert_eq!(store.fact_count().unwrap(), 5);

        // Archive the first three.
        let to_archive = &ids[..3];
        let moved = store.archive_facts(to_archive).unwrap();
        assert_eq!(moved, 3);

        // Hot now has 2.
        assert_eq!(store.fact_count().unwrap(), 2);

        // Cold has 3, with content + ids preserved.
        let cold = store.open_cold().unwrap();
        assert_eq!(cold.fact_count().unwrap(), 3);
        for id in to_archive {
            let rec = cold
                .db_arc()
                .begin_read()
                .unwrap()
                .open_table(FACTS_TABLE)
                .unwrap()
                .get(id.as_bytes().as_slice())
                .unwrap()
                .unwrap()
                .value()
                .to_vec();
            let parsed: FactRecord = serde_json::from_slice(&rec).unwrap();
            assert_eq!(parsed.id, *id);
            assert!(parsed.content.starts_with("fact "));
        }

        // Hot's per-entity index should now report only the 2 remaining.
        assert_eq!(store.count_entity_facts(&entity_id).unwrap(), 2);
    }

    #[test]
    fn archive_facts_is_idempotent() {
        let (_dir, mut store) = make_store();
        let entity_id = store
            .entity_add(EntityInput {
                name: "T".into(),
                entity_type: Some(EntityType::Custom("topic".into())),
                ..Default::default()
            })
            .unwrap();
        let id = store
            .fact_add(FactInput {
                content: "lonely fact".into(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![entity_id]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.archive_facts(&[id]).unwrap(), 1);
        // Second call: id no longer in hot, so 0 moved (not an error).
        assert_eq!(store.archive_facts(&[id]).unwrap(), 0);
        // And cold still has exactly one fact.
        assert_eq!(store.open_cold().unwrap().fact_count().unwrap(), 1);
    }

    #[cfg(feature = "semantic")]
    #[test]
    fn search_merges_cold_when_include_archive_set() {
        // wg-core archive_facts test goes through Store directly. For
        // include_archive search we need the WikiGraph wrapper because
        // cold sibling lifecycle lives there. Build a small wiki, ingest
        // 6 facts, archive 3, then search hot-only vs include_archive
        // and assert the cold facts only show up in the latter.
        use crate::WikiGraph;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hot.redb");
        let mut config = crate::Config::default();
        config.store.path = path.to_string_lossy().into_owned();
        let wiki = WikiGraph::open(&path, config).unwrap();
        let entity_id = wiki
            .entity_add(EntityInput {
                name: "T".into(),
                entity_type: Some(EntityType::Custom("topic".into())),
                ..Default::default()
            })
            .unwrap();
        let mut ids = Vec::new();
        for i in 0..6 {
            ids.push(
                wiki.fact_add(FactInput {
                    content: format!("redis tip number {i}"),
                    fact_type: Some(FactType::Note),
                    entity_ids: Some(vec![entity_id]),
                    ..Default::default()
                })
                .unwrap(),
            );
        }
        // Archive the first three.
        let moved = wiki.archive_facts(&ids[..3]).unwrap();
        assert_eq!(moved, 3);

        let opts = crate::SearchOpts {
            limit: Some(10),
            bm25_only: true,
            ..Default::default()
        };
        let hot_only = wiki.search("redis", opts.clone()).unwrap();
        assert_eq!(hot_only.len(), 3, "hot-only search returns 3 surviving facts");
        let archived: std::collections::HashSet<FactId> = ids[..3].iter().copied().collect();
        for r in &hot_only {
            assert!(!archived.contains(&r.fact_id));
        }

        let merged_opts = crate::SearchOpts {
            include_archive: true,
            ..opts
        };
        let merged = wiki.search("redis", merged_opts).unwrap();
        assert_eq!(merged.len(), 6, "include_archive search merges hot + cold");
        // Hot results must keep their leading positions; cold fills tail.
        for r in &merged[..3] {
            assert!(!archived.contains(&r.fact_id), "hot keeps top slots");
        }
        let mut found_archived = 0;
        for r in &merged[3..] {
            if archived.contains(&r.fact_id) {
                found_archived += 1;
            }
        }
        assert_eq!(found_archived, 3, "all 3 archived facts surface in tail");
    }

    #[test]
    fn archive_facts_skips_unknown_ids() {
        let (_dir, mut store) = make_store();
        let unknown = FactId::new();
        assert_eq!(store.archive_facts(&[unknown]).unwrap(), 0);
        // Cold file isn't even created in this case (no records to write).
        assert_eq!(store.fact_count().unwrap(), 0);
    }
}
