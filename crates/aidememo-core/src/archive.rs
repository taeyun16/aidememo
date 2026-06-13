//! Cold-tier archive support.
//!
//! Old / decayed facts can be moved out of the hot store file into a
//! sibling cold-tier file so the hot store stays bounded. Search
//! reads hot only by default; an opt-in `include_archive` flag (added
//! in stage 2) opens the cold file on demand and merges results.
//!
//! ### Invariants (stage 1)
//!
//! * Cold stores **facts only**. Entity records stay in hot — cold
//!   facts' `entity_ids` cross-reference hot's `entities` table.
//!   Search hydration must always look up entities from hot.
//! * Cold preserves the original `FactId`. No re-keying. Back-
//!   references (e.g. `aidememo_fact_get` after a search hit) keep working
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
//! Cold path is derived from the hot store path. redb stores use
//! `<hot>.cold.redb`; SQLite stores use `<hot>.cold.sqlite`.

use std::path::PathBuf;

#[cfg(feature = "redb")]
use redb::ReadableTable;

#[cfg(feature = "redb")]
use crate::backend::StoreBackend;
#[cfg(feature = "redb")]
use crate::error::{AideMemoError, Result};
#[cfg(feature = "redb")]
use crate::store::{FACTS_TABLE, Store};
#[cfg(feature = "redb")]
use crate::types::FactId;

/// Compute the cold-tier db path for a hot db file. Both files sit
/// side by side so user backups copy them together. This follows the compiled
/// default backend: SQLite in normal builds, redb in redb-only builds.
pub fn cold_path_for(hot_path: &std::path::Path) -> PathBuf {
    let backend = if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
        "redb"
    } else {
        "sqlite"
    };
    cold_path_for_backend(hot_path, backend)
}

/// Compute the cold-tier db path for a hot db file and selected backend.
pub fn cold_path_for_backend(hot_path: &std::path::Path, backend: &str) -> PathBuf {
    let mut s = hot_path.as_os_str().to_os_string();
    let suffix = match backend.trim().to_lowercase().as_str() {
        "sqlite" | "libsqlite" => ".cold.sqlite",
        _ => ".cold.redb",
    };
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(feature = "redb")]
impl Store {
    /// Where this store's cold-tier file lives.
    pub fn cold_path(&self) -> PathBuf {
        cold_path_for_backend(
            std::path::Path::new(&self.config().store.path),
            &self.config().store.backend,
        )
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
    /// This legacy redb-specific entry point delegates to the shared
    /// `StoreBackend::fact_archive_to` contract so direct `Store` callers and
    /// the public `AideMemo` API preserve the same archive semantics.
    pub fn archive_facts(&mut self, fact_ids: &[FactId]) -> Result<usize> {
        if fact_ids.is_empty() {
            return Ok(0);
        }
        let hot_ids = <Store as StoreBackend>::existing_fact_ids(self, fact_ids)?;
        if hot_ids.is_empty() {
            return Ok(0);
        }
        let mut cold = self.open_cold()?;
        <Store as StoreBackend>::fact_archive_to(self, &mut cold, &hot_ids)
    }

    /// Count facts in this store (used by archive tests / `aidememo stats
    /// --include-archive` later). Tiny single-table iter; not a hot
    /// path.
    pub fn fact_count(&self) -> Result<u64> {
        let read_txn = self
            .db_arc()
            .begin_read()
            .map_err(|e| AideMemoError::TransactionBegin {
                source: Box::new(e),
            })?;
        let facts = read_txn
            .open_table(FACTS_TABLE)
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<count>".to_string(),
                source: Box::new(e),
            })?;
        Ok(facts
            .iter()
            .map_err(|e| AideMemoError::StoreRead {
                table: "facts",
                key: "<iter>".to_string(),
                source: Box::new(e),
            })?
            .count() as u64)
    }
}

#[cfg(all(test, any(feature = "sqlite", feature = "redb")))]
mod tests {
    use super::*;
    use crate::types::{EntityInput, EntityType, FactId, FactInput, FactListOpts, FactType};
    use crate::{AideMemo, Config};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn test_store_path(dir: &TempDir, stem: &str, mut config: Config) -> (PathBuf, Config) {
        if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
            config.store.backend = "redb".to_string();
        }
        let suffix = if config.store.backend == "redb" {
            "redb"
        } else {
            "sqlite"
        };
        let path = dir.path().join(format!("{stem}.{suffix}"));
        config.store.path = path.to_string_lossy().into_owned();
        (path, config)
    }

    fn make_wiki() -> (TempDir, AideMemo, PathBuf, String) {
        let dir = TempDir::new().unwrap();
        let (path, config) = test_store_path(&dir, "hot", Config::default());
        let backend = config.store.backend.clone();
        let wiki = AideMemo::open(&path, config).unwrap();
        (dir, wiki, path, backend)
    }

    #[test]
    fn cold_path_uses_compiled_default_backend() {
        let p = std::path::Path::new("/tmp/x.redb");
        let expected = if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
            std::path::PathBuf::from("/tmp/x.redb.cold.redb")
        } else {
            std::path::PathBuf::from("/tmp/x.redb.cold.sqlite")
        };
        assert_eq!(cold_path_for(p), expected);
    }

    #[test]
    fn cold_path_backend_specific_redb_sits_next_to_hot() {
        let p = std::path::Path::new("/tmp/x.redb");
        assert_eq!(
            cold_path_for_backend(p, "redb"),
            std::path::PathBuf::from("/tmp/x.redb.cold.redb")
        );
    }

    #[test]
    fn cold_path_uses_backend_specific_suffix() {
        assert_eq!(
            cold_path_for_backend(std::path::Path::new("/tmp/x.redb"), "redb"),
            std::path::PathBuf::from("/tmp/x.redb.cold.redb")
        );
        assert_eq!(
            cold_path_for_backend(std::path::Path::new("/tmp/x.sqlite"), "sqlite"),
            std::path::PathBuf::from("/tmp/x.sqlite.cold.sqlite")
        );
        assert_eq!(
            cold_path_for_backend(std::path::Path::new("/tmp/x.db"), "libsqlite"),
            std::path::PathBuf::from("/tmp/x.db.cold.sqlite")
        );
    }

    #[test]
    fn archive_facts_moves_records_hot_to_cold() {
        let (_dir, wiki, _path, _backend) = make_wiki();
        let entity_id = wiki
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
        let ids = wiki.fact_add_many(inputs).unwrap();
        assert_eq!(wiki.stats().unwrap().fact_count, 5);

        // Archive the first three.
        let to_archive = &ids[..3];
        let moved = wiki.archive_facts(to_archive).unwrap();
        assert_eq!(moved, 3);

        // Hot now has 2.
        assert_eq!(wiki.stats().unwrap().fact_count, 2);

        // Cold has 3, with content + ids preserved.
        let cold = wiki.cold().unwrap().expect("cold sibling");
        assert_eq!(cold.stats().unwrap().fact_count, 3);
        for id in to_archive {
            let parsed = cold.fact_get(id).unwrap();
            assert_eq!(parsed.id, *id);
            assert!(parsed.content.starts_with("fact "));
            let via_hot = wiki.fact_get(id).unwrap();
            assert_eq!(via_hot.id, *id);
        }

        // Hot's per-entity list should now report only the 2 remaining.
        let hot_entity_facts = wiki
            .fact_list(FactListOpts {
                entity_id: Some(entity_id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hot_entity_facts.len(), 2);
    }

    #[test]
    fn archive_facts_is_idempotent() {
        let (_dir, wiki, _path, _backend) = make_wiki();
        let entity_id = wiki
            .entity_add(EntityInput {
                name: "T".into(),
                entity_type: Some(EntityType::Custom("topic".into())),
                ..Default::default()
            })
            .unwrap();
        let id = wiki
            .fact_add(FactInput {
                content: "lonely fact".into(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![entity_id]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(wiki.archive_facts(&[id]).unwrap(), 1);
        // Second call: id no longer in hot, so 0 moved (not an error).
        assert_eq!(wiki.archive_facts(&[id]).unwrap(), 0);
        // And cold still has exactly one fact.
        let cold = wiki.cold().unwrap().expect("cold sibling");
        assert_eq!(cold.stats().unwrap().fact_count, 1);
    }

    #[cfg(feature = "semantic")]
    #[test]
    fn search_merges_cold_when_include_archive_set() {
        // aidememo-core archive_facts test goes through Store directly. For
        // include_archive search we need the AideMemo wrapper because
        // cold sibling lifecycle lives there. Build a small wiki, ingest
        // 6 facts, archive 3, then search hot-only vs include_archive
        // and assert the cold facts only show up in the latter.
        let (_dir, wiki, _path, _backend) = make_wiki();
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
        assert_eq!(
            hot_only.len(),
            3,
            "hot-only search returns 3 surviving facts"
        );
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
        let (_dir, wiki, path, backend) = make_wiki();
        let unknown = FactId::new();
        assert_eq!(wiki.archive_facts(&[unknown]).unwrap(), 0);
        // Cold file isn't even created in this case (no records to write).
        assert_eq!(wiki.stats().unwrap().fact_count, 0);
        assert!(!cold_path_for_backend(&path, &backend).exists());
    }
}
