//! Pull-only delta sync between two aidememo stores (Phase 2).
//!
//! Wire format is JSONL — one record per line, line-prefixed with a
//! `kind` discriminator so the receiver can dispatch without a full
//! envelope schema. Each line is one of:
//!
//! ```jsonl
//! {"kind":"header","schema":2,"upstream_clock_ms":1741540123456}
//! {"kind":"entity","data":<EntityRecord>}
//! {"kind":"fact","data":<FactRecord>}
//! {"kind":"relation","data":<RelationRecord>}
//! {"kind":"cursor","entity":"01J...","fact":"01J..."}
//! ```
//!
//! The trailing `cursor` line tells the puller "no more records past
//! this point in this batch — start your next pull from these ULIDs".
//! Both the entity and fact cursors are emitted independently because
//! their ULID streams advance at very different rates.
//!
//! Sync is one-way (upstream → downstream). The shared `aidememo mcp-serve`
//! is the canonical writer; downstream agents pull periodically into a
//! local read cache. Multi-master writes with conflict resolution are
//! Phase 3 — not in this module.

use crate::backend::StoreBackend;
use crate::error::{AideMemoError, Result};
use crate::types::{EntityId, EntityRecord, FactId, FactListOpts, FactRecord, ListOpts};
use crate::{AideMemo, TraverseDirection};
use serde::{Deserialize, Serialize};
use std::io::Write;
use ulid::Ulid;

/// Wire schema version. Receivers should reject blobs with a higher
/// schema than they understand to keep operators from silently
/// importing newer formats with missing fields.
pub const SYNC_SCHEMA: u32 = 2;

/// Cursor pair — where a downstream agent left off on its last pull
/// from a given upstream. Two dimensions per record kind:
///
/// * **ULID cursor** (`entity` / `fact`) — high-water for new
///   inserts. ULID is time-sortable, so "anything strictly above
///   this ULID is new".
/// * **updated_at cursor** (`entity_updated_at` / `fact_updated_at`)
///   — high-water for in-place mutations on already-pulled records.
///   `supersede`, `pin`, `entity_describe`, etc. bump `updated_at`
///   without changing the ULID, so we need a second cursor to
///   notice them.
///
/// Both fields are inclusive lower bounds. Phase 2 stored only the
/// ULID pair; the new updated_at fields use `#[serde(default)]` so
/// existing `<store>.sync.json` cursor files keep loading.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncCursor {
    /// Last entity ULID the downstream successfully applied.
    pub entity: Option<EntityId>,
    /// Last fact ULID the downstream successfully applied.
    pub fact: Option<FactId>,
    /// Highest `updated_at` (epoch ms) the downstream has seen for
    /// any entity. Drives the "in-place updates" pass.
    #[serde(default)]
    pub entity_updated_at: Option<u64>,
    /// Highest `updated_at` (epoch ms) the downstream has seen for
    /// any fact. Drives the "in-place updates" pass.
    #[serde(default)]
    pub fact_updated_at: Option<u64>,
}

/// Options for `AideMemo::sync_export`.
#[derive(Debug, Clone, Default)]
pub struct SyncExportOpts {
    /// Cursor from the downstream's previous pull. None = full export.
    pub since: SyncCursor,
    /// Maximum number of records (entities + facts + relations) to
    /// emit in this batch. The downstream can drive multiple pulls
    /// in a loop until it sees fewer than `limit` records returned.
    /// 0 = unbounded.
    pub limit: usize,
    /// Include relations in the export. Relations have no ULID, so
    /// they're emitted in entity-source order — bumping `since.entity`
    /// past their source naturally drops them from the next batch.
    pub include_relations: bool,
}

/// Stats for `AideMemo::sync_import`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncImportStats {
    pub entities_inserted: usize,
    pub entities_skipped: usize,
    pub facts_inserted: usize,
    pub facts_skipped: usize,
    pub relations_inserted: usize,
    pub relations_skipped: usize,
    pub errors: usize,
    /// Cursor advance to record locally after this batch — copy of
    /// the trailing `cursor` line emitted by the upstream.
    pub new_cursor: SyncCursor,
}

/// Extract the trailing cursor from a sync JSONL blob.
///
/// Branch logs and backup manifests use this to remember the high-water mark
/// produced by `sync_export` without applying the export to another store.
pub fn cursor_from_jsonl(jsonl: &str) -> Result<SyncCursor> {
    let mut cursor = None;
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|source| AideMemoError::Deserialize {
                context: "sync cursor line".to_string(),
                source,
            })?;
        if value.get("kind").and_then(|kind| kind.as_str()) != Some("cursor") {
            continue;
        }
        let parse_ulid = |key: &str| -> Result<Option<Ulid>> {
            value
                .get(key)
                .and_then(|raw| raw.as_str())
                .map(|raw| {
                    Ulid::from_string(raw).map_err(|source| {
                        AideMemoError::InvalidInput(format!(
                            "invalid sync cursor {key} ULID `{raw}`: {source}"
                        ))
                    })
                })
                .transpose()
        };
        cursor = Some(SyncCursor {
            entity: parse_ulid("entity")?.map(EntityId),
            fact: parse_ulid("fact")?.map(FactId),
            entity_updated_at: value.get("entity_updated_at").and_then(|raw| raw.as_u64()),
            fact_updated_at: value.get("fact_updated_at").and_then(|raw| raw.as_u64()),
        });
    }
    cursor.ok_or_else(|| AideMemoError::InvalidInput("sync JSONL missing cursor line".to_string()))
}

impl AideMemo {
    /// Emit a JSONL delta in two passes:
    ///   1. **Inserts** — entities / facts whose ULID is strictly
    ///      above `since.entity` / `since.fact`.
    ///   2. **Updates** — entities / facts whose ULID is *at or below*
    ///      that cursor (i.e. already pulled before) but whose
    ///      `updated_at` is strictly above `since.entity_updated_at` /
    ///      `since.fact_updated_at`. Catches `supersede`, `pin`,
    ///      `entity_describe`, and other in-place mutations.
    ///
    /// Both passes share the same `limit` budget and the same output
    /// stream. The trailing `cursor` line carries all four high-water
    /// values so the downstream knows where to resume.
    pub fn sync_export(&self, opts: SyncExportOpts, writer: &mut dyn Write) -> Result<usize> {
        // Header — schema + emit time. The clock helps the downstream
        // sanity-check skew.
        let header = serde_json::json!({
            "kind": "header",
            "schema": SYNC_SCHEMA,
            "upstream_clock_ms": crate::time::current_epoch_ms(),
        });
        writeln!(writer, "{}", header).map_err(io_serialize)?;

        let limit = if opts.limit == 0 {
            usize::MAX
        } else {
            opts.limit
        };
        let mut emitted: usize = 0;
        let mut last_entity: Option<EntityId> = opts.since.entity;
        let mut last_fact: Option<FactId> = opts.since.fact;
        let mut last_entity_updated_at: Option<u64> = opts.since.entity_updated_at;
        let mut last_fact_updated_at: Option<u64> = opts.since.fact_updated_at;

        let store = self.store_handle();

        // PASS A — entities: new ULIDs above the cursor.
        let entity_summaries = store.read().entity_list(ListOpts {
            limit: Some(usize::MAX),
            ..Default::default()
        })?;
        let mut new_entities: Vec<EntityRecord> = Vec::new();
        for s in &entity_summaries {
            if let Some(cut) = opts.since.entity {
                if s.id.0 <= cut.0 {
                    continue;
                }
            }
            if let Ok(rec) = store.read().entity_get_by_id(s.id) {
                new_entities.push(rec);
            }
        }
        new_entities.sort_by_key(|e| e.id.0);
        for e in &new_entities {
            if emitted >= limit {
                break;
            }
            let line = serde_json::json!({"kind": "entity", "data": e});
            writeln!(writer, "{}", line).map_err(io_serialize)?;
            last_entity = Some(e.id);
            if e.updated_at > last_entity_updated_at.unwrap_or(0) {
                last_entity_updated_at = Some(e.updated_at);
            }
            emitted += 1;
        }

        // PASS B — entity updates: already-known ULIDs whose
        // updated_at moved past the watermark. Only kicks in when the
        // downstream provided an entity_updated_at cursor (full first
        // pull skips this naturally because cursor.entity = None means
        // pass A already covers everything).
        if emitted < limit && opts.since.entity.is_some() {
            let mut updates: Vec<EntityRecord> = Vec::new();
            for s in &entity_summaries {
                if let Some(cut) = opts.since.entity {
                    if s.id.0 > cut.0 {
                        continue; // already in pass A
                    }
                }
                if let Ok(rec) = store.read().entity_get_by_id(s.id) {
                    if rec.updated_at > opts.since.entity_updated_at.unwrap_or(0) {
                        updates.push(rec);
                    }
                }
            }
            updates.sort_by_key(|e| e.updated_at);
            for e in &updates {
                if emitted >= limit {
                    break;
                }
                let line = serde_json::json!({"kind": "entity", "data": e});
                writeln!(writer, "{}", line).map_err(io_serialize)?;
                if e.updated_at > last_entity_updated_at.unwrap_or(0) {
                    last_entity_updated_at = Some(e.updated_at);
                }
                emitted += 1;
            }
        }

        // PASS A — facts: new ULIDs above the cursor.
        if emitted < limit {
            let all_facts = store.read().fact_list(FactListOpts {
                limit: None,
                ..Default::default()
            })?;
            let mut new_facts: Vec<FactRecord> = Vec::new();
            for f in &all_facts {
                if let Some(cut) = opts.since.fact {
                    if f.id.0 <= cut.0 {
                        continue;
                    }
                }
                new_facts.push(f.clone());
            }
            new_facts.sort_by_key(|f| f.id.0);
            for f in &new_facts {
                if emitted >= limit {
                    break;
                }
                let line = serde_json::json!({"kind": "fact", "data": f});
                writeln!(writer, "{}", line).map_err(io_serialize)?;
                last_fact = Some(f.id);
                if f.updated_at > last_fact_updated_at.unwrap_or(0) {
                    last_fact_updated_at = Some(f.updated_at);
                }
                emitted += 1;
            }

            // PASS B — fact updates.
            if emitted < limit && opts.since.fact.is_some() {
                let mut updates: Vec<FactRecord> = Vec::new();
                for f in &all_facts {
                    if let Some(cut) = opts.since.fact {
                        if f.id.0 > cut.0 {
                            continue; // already in pass A
                        }
                    }
                    if f.updated_at > opts.since.fact_updated_at.unwrap_or(0) {
                        updates.push(f.clone());
                    }
                }
                updates.sort_by_key(|f| f.updated_at);
                for f in &updates {
                    if emitted >= limit {
                        break;
                    }
                    let line = serde_json::json!({"kind": "fact", "data": f});
                    writeln!(writer, "{}", line).map_err(io_serialize)?;
                    if f.updated_at > last_fact_updated_at.unwrap_or(0) {
                        last_fact_updated_at = Some(f.updated_at);
                    }
                    emitted += 1;
                }
            }
        }

        // Relations — keyed by (src, type, tgt), no ULID. We emit them
        // alongside their source entity advance: a relation is in this
        // batch iff its source_id is in the (since.entity, last_entity]
        // half-open range advanced this pull. This makes relations
        // resync deterministically with entity advancement.
        if opts.include_relations && emitted < limit {
            let entities = store.read().entity_list(ListOpts {
                limit: Some(usize::MAX),
                ..Default::default()
            })?;
            for s in entities {
                if let Some(cut) = opts.since.entity {
                    if s.id.0 <= cut.0 {
                        continue;
                    }
                }
                if let Some(latest) = last_entity {
                    if s.id.0 > latest.0 {
                        continue;
                    }
                }
                let rels = store
                    .read()
                    .relations_get(&s.name, TraverseDirection::Forward)
                    .unwrap_or_default();
                for r in rels {
                    if emitted >= limit {
                        break;
                    }
                    let line = serde_json::json!({"kind": "relation", "data": r});
                    writeln!(writer, "{}", line).map_err(io_serialize)?;
                    emitted += 1;
                }
            }
        }

        // Trailing cursor — tells downstream where to resume. Carries
        // both ULID watermarks and updated_at watermarks (Phase 2.5).
        let cursor_line = serde_json::json!({
            "kind": "cursor",
            "entity": last_entity.map(|e| e.0.to_string()),
            "fact": last_fact.map(|f| f.0.to_string()),
            "entity_updated_at": last_entity_updated_at,
            "fact_updated_at": last_fact_updated_at,
        });
        writeln!(writer, "{}", cursor_line).map_err(io_serialize)?;

        Ok(emitted)
    }

    /// Apply a JSONL delta produced by `sync_export`. Idempotent — a
    /// re-applied blob skips records the local store already has and
    /// returns them in the `*_skipped` counters.
    pub fn sync_import(&self, jsonl: &str) -> Result<SyncImportStats> {
        let mut stats = SyncImportStats::default();
        let store = self.store_handle();

        for line in jsonl.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => {
                    stats.errors += 1;
                    continue;
                }
            };
            let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            match kind {
                "header" => {
                    let schema = v.get("schema").and_then(|s| s.as_u64()).unwrap_or(0) as u32;
                    if schema > SYNC_SCHEMA {
                        return Err(AideMemoError::InvalidInput(format!(
                            "sync schema {} is newer than this aidememo understands ({})",
                            schema, SYNC_SCHEMA
                        )));
                    }
                }
                "entity" => match serde_json::from_value::<EntityRecord>(
                    v.get("data").cloned().unwrap_or(serde_json::Value::Null),
                ) {
                    Ok(rec) => match store.write().entity_upsert_record(rec) {
                        Ok(true) => stats.entities_inserted += 1,
                        Ok(false) => stats.entities_skipped += 1,
                        Err(_) => stats.errors += 1,
                    },
                    Err(_) => stats.errors += 1,
                },
                "fact" => match serde_json::from_value::<FactRecord>(
                    v.get("data").cloned().unwrap_or(serde_json::Value::Null),
                ) {
                    Ok(rec) => match store.write().fact_upsert_record(rec) {
                        Ok(true) => stats.facts_inserted += 1,
                        Ok(false) => stats.facts_skipped += 1,
                        Err(_) => stats.errors += 1,
                    },
                    Err(_) => stats.errors += 1,
                },
                "relation" => match serde_json::from_value::<crate::types::RelationRecord>(
                    v.get("data").cloned().unwrap_or(serde_json::Value::Null),
                ) {
                    Ok(rec) => match store.write().relation_upsert_record(rec) {
                        Ok(true) => stats.relations_inserted += 1,
                        Ok(false) => stats.relations_skipped += 1,
                        Err(_) => stats.errors += 1,
                    },
                    Err(_) => stats.errors += 1,
                },
                "cursor" => {
                    let parse_ulid = |k: &str| -> Option<Ulid> {
                        v.get(k)
                            .and_then(|x| x.as_str())
                            .and_then(|s| Ulid::from_string(s).ok())
                    };
                    stats.new_cursor.entity = parse_ulid("entity").map(EntityId);
                    stats.new_cursor.fact = parse_ulid("fact").map(FactId);
                    stats.new_cursor.entity_updated_at =
                        v.get("entity_updated_at").and_then(|x| x.as_u64());
                    stats.new_cursor.fact_updated_at =
                        v.get("fact_updated_at").and_then(|x| x.as_u64());
                }
                _ => stats.errors += 1,
            }
        }

        // Mark BM25 dirty since fact / entity rows advanced. Cheap.
        self.bm25_mark_dirty_pub();
        Ok(stats)
    }
}

fn io_serialize(e: std::io::Error) -> AideMemoError {
    AideMemoError::Serialize {
        context: "sync".to_string(),
        source: serde_json::Error::io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FactInput;
    #[cfg(all(feature = "redb", feature = "sqlite"))]
    use crate::types::{RelationInput, RelationType};
    use crate::{Config, EntityInput, EntityType};

    fn open_temp() -> (tempfile::TempDir, AideMemo) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
            config.store.backend = "redb".to_string();
        }
        let path = dir.path().join(if config.store.backend == "redb" {
            "test.redb"
        } else {
            "test.sqlite"
        });
        let wiki = AideMemo::open(&path, config).unwrap();
        (dir, wiki)
    }

    #[cfg(all(feature = "redb", feature = "sqlite"))]
    fn open_backend(dir: &tempfile::TempDir, name: &str, backend: &str) -> AideMemo {
        let path = dir.path().join(format!("{name}.{backend}"));
        let mut config = Config::default();
        config.store.backend = backend.to_string();
        config.store.path = path.to_string_lossy().into_owned();
        AideMemo::open(&path, config).unwrap()
    }

    #[cfg(all(feature = "redb", feature = "sqlite"))]
    fn assert_sync_compatible_between_backends(source_backend: &str, target_backend: &str) {
        let dir = tempfile::tempdir().unwrap();
        let source = open_backend(&dir, "source", source_backend);
        let target = open_backend(&dir, "target", target_backend);

        let redis = source
            .entity_add(EntityInput {
                name: "Redis".into(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let sentinel = source
            .entity_add(EntityInput {
                name: "Sentinel".into(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        source
            .relation_add(RelationInput {
                source: "Sentinel".into(),
                target: "Redis".into(),
                relation_type: RelationType::new("monitors"),
                weight: Some(1.0),
                evidence: Some(vec![format!("{source_backend} to {target_backend}")]),
            })
            .unwrap();
        let original = source
            .fact_add(FactInput {
                content: format!("Sync compatibility fact from {source_backend}"),
                entity_ids: Some(vec![redis]),
                fact_type: Some(crate::types::FactType::Claim),
                ..Default::default()
            })
            .unwrap();

        let mut full = Vec::new();
        source
            .sync_export(
                SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut full,
            )
            .unwrap();
        let first = target
            .sync_import(std::str::from_utf8(&full).unwrap())
            .unwrap();
        assert_eq!(first.entities_inserted, 2);
        assert_eq!(first.facts_inserted, 1);
        assert_eq!(first.relations_inserted, 1);
        assert_eq!(first.errors, 0);
        assert!(matches!(first.new_cursor.entity, Some(id) if id == redis || id == sentinel));
        assert_eq!(first.new_cursor.fact, Some(original));

        assert_eq!(target.entity_get_by_id(redis).unwrap().name, "Redis");
        assert_eq!(
            target.fact_get(&original).unwrap().content,
            format!("Sync compatibility fact from {source_backend}")
        );
        let rels = target
            .relations_get("Sentinel", TraverseDirection::Forward)
            .unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].target_id, redis);

        std::thread::sleep(std::time::Duration::from_millis(2));
        source
            .entity_describe("Redis", "Cross-backend sync summary")
            .unwrap();
        let replacement = source
            .fact_add(FactInput {
                content: format!("Replacement fact from {source_backend}"),
                entity_ids: Some(vec![redis]),
                fact_type: Some(crate::types::FactType::Claim),
                ..Default::default()
            })
            .unwrap();
        source.fact_supersede(&original, &replacement).unwrap();

        let mut delta = Vec::new();
        source
            .sync_export(
                SyncExportOpts {
                    since: first.new_cursor.clone(),
                    include_relations: true,
                    ..Default::default()
                },
                &mut delta,
            )
            .unwrap();
        let second = target
            .sync_import(std::str::from_utf8(&delta).unwrap())
            .unwrap();
        assert_eq!(second.errors, 0);
        assert!(
            second.entities_inserted + second.entities_skipped >= 1,
            "entity update should cross from {source_backend} to {target_backend}"
        );
        assert!(
            second.facts_inserted + second.facts_skipped >= 2,
            "replacement and supersede update should cross from {source_backend} to {target_backend}"
        );

        assert_eq!(
            target.entity_get_by_id(redis).unwrap().summary.as_deref(),
            Some("Cross-backend sync summary")
        );
        let synced_original = target.fact_get(&original).unwrap();
        assert_eq!(synced_original.superseded_by, Some(replacement));
        assert!(synced_original.superseded_at.is_some());
        assert_eq!(
            target.fact_get(&replacement).unwrap().content,
            format!("Replacement fact from {source_backend}")
        );
    }

    #[cfg(all(feature = "redb", feature = "sqlite"))]
    #[test]
    fn sync_export_import_is_backend_compatible() {
        assert_sync_compatible_between_backends("redb", "sqlite");
        assert_sync_compatible_between_backends("redb", "libsqlite");
        assert_sync_compatible_between_backends("sqlite", "redb");
        assert_sync_compatible_between_backends("libsqlite", "redb");
    }

    #[test]
    fn export_then_import_roundtrip_preserves_ids() {
        let (_dir_a, upstream) = open_temp();
        let eid = upstream
            .entity_add(EntityInput {
                name: "Redis".into(),
                entity_type: Some(EntityType::Custom("service".into())),
                ..Default::default()
            })
            .unwrap();
        let fid = upstream
            .fact_add(FactInput {
                content: "Redis is a cache".into(),
                entity_ids: Some(vec![eid]),
                ..Default::default()
            })
            .unwrap();

        let mut buf = Vec::new();
        let n = upstream
            .sync_export(
                SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut buf,
            )
            .unwrap();
        assert_eq!(n, 2, "1 entity + 1 fact");

        let (_dir_b, downstream) = open_temp();
        let stats = downstream
            .sync_import(std::str::from_utf8(&buf).unwrap())
            .unwrap();
        assert_eq!(stats.entities_inserted, 1);
        assert_eq!(stats.facts_inserted, 1);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.new_cursor.entity, Some(eid));
        assert_eq!(stats.new_cursor.fact, Some(fid));

        // ID preserved end-to-end.
        let downstream_e = downstream.entity_get_by_id(eid).unwrap();
        assert_eq!(downstream_e.name, "Redis");
        let downstream_f = downstream.fact_get(&fid).unwrap();
        assert_eq!(downstream_f.content, "Redis is a cache");
    }

    #[test]
    fn import_is_idempotent() {
        let (_dir_a, upstream) = open_temp();
        upstream
            .fact_add(FactInput {
                content: "Stable claim".into(),
                ..Default::default()
            })
            .unwrap();

        let mut buf = Vec::new();
        upstream
            .sync_export(SyncExportOpts::default(), &mut buf)
            .unwrap();
        let blob = std::str::from_utf8(&buf).unwrap().to_string();

        let (_dir_b, downstream) = open_temp();
        let first = downstream.sync_import(&blob).unwrap();
        let second = downstream.sync_import(&blob).unwrap();
        assert_eq!(first.facts_inserted, 1);
        assert_eq!(second.facts_inserted, 0);
        assert_eq!(second.facts_skipped, 1);
    }

    #[test]
    fn cursor_advance_only_emits_new_records() {
        let (_dir_a, upstream) = open_temp();
        let f1 = upstream
            .fact_add(FactInput {
                content: "first".into(),
                ..Default::default()
            })
            .unwrap();

        let mut buf1 = Vec::new();
        upstream
            .sync_export(SyncExportOpts::default(), &mut buf1)
            .unwrap();
        let stats1 = {
            let (_dir_b, downstream) = open_temp();
            downstream
                .sync_import(std::str::from_utf8(&buf1).unwrap())
                .unwrap()
        };
        assert_eq!(stats1.new_cursor.fact, Some(f1));

        // Add another fact upstream, pull only the delta past stats1.cursor.
        let f2 = upstream
            .fact_add(FactInput {
                content: "second".into(),
                ..Default::default()
            })
            .unwrap();
        let mut buf2 = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    since: stats1.new_cursor.clone(),
                    ..Default::default()
                },
                &mut buf2,
            )
            .unwrap();

        // Should contain f2 only.
        let blob = std::str::from_utf8(&buf2).unwrap();
        assert!(blob.contains(&f2.0.to_string()), "delta should include f2");
        assert!(
            !blob.contains(&f1.0.to_string()),
            "delta must NOT re-include f1"
        );
    }

    /// Pull a fact, supersede it upstream, pull again. The downstream
    /// must observe the supersede flag without the fact's ULID
    /// changing — that's the whole point of Phase 2.5.
    #[test]
    fn supersede_propagates_through_pull() {
        let (_dir_a, upstream) = open_temp();
        let original = upstream
            .fact_add(FactInput {
                content: "old claim".into(),
                ..Default::default()
            })
            .unwrap();

        // First pull — downstream gets the original, no supersede.
        let mut buf1 = Vec::new();
        upstream
            .sync_export(SyncExportOpts::default(), &mut buf1)
            .unwrap();
        let (_dir_b, downstream) = open_temp();
        let stats1 = downstream
            .sync_import(std::str::from_utf8(&buf1).unwrap())
            .unwrap();
        assert_eq!(stats1.facts_inserted, 1);
        let downstream_before = downstream.fact_get(&original).unwrap();
        assert!(downstream_before.superseded_at.is_none());

        // Upstream supersedes via a new fact.
        // Sleep a millisecond so updated_at strictly advances even on
        // fast machines where two writes can land in the same ms.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let replacement = upstream
            .fact_add(FactInput {
                content: "new claim".into(),
                ..Default::default()
            })
            .unwrap();
        upstream.fact_supersede(&original, &replacement).unwrap();

        // Second pull — incremental from cursor1. Must include the
        // replacement (new ULID) AND the original's supersede update.
        let mut buf2 = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    since: stats1.new_cursor.clone(),
                    ..Default::default()
                },
                &mut buf2,
            )
            .unwrap();
        let stats2 = downstream
            .sync_import(std::str::from_utf8(&buf2).unwrap())
            .unwrap();
        assert!(
            stats2.facts_inserted >= 1,
            "expected the replacement fact in the delta, got {} inserts",
            stats2.facts_inserted
        );

        let downstream_after = downstream.fact_get(&original).unwrap();
        assert!(
            downstream_after.superseded_at.is_some(),
            "supersede flag must propagate through the second pull \
             (Phase 2.5 update pass)"
        );
        assert_eq!(downstream_after.superseded_by, Some(replacement));
    }

    /// `entity_describe` mutates `summary` and bumps `updated_at`.
    /// Phase 2.5 must propagate it to the downstream on incremental
    /// pull.
    #[test]
    fn entity_describe_propagates_through_pull() {
        let (_dir_a, upstream) = open_temp();
        let eid = upstream
            .entity_add(EntityInput {
                name: "Redis".into(),
                ..Default::default()
            })
            .unwrap();

        let mut buf1 = Vec::new();
        upstream
            .sync_export(SyncExportOpts::default(), &mut buf1)
            .unwrap();
        let (_dir_b, downstream) = open_temp();
        let stats1 = downstream
            .sync_import(std::str::from_utf8(&buf1).unwrap())
            .unwrap();
        assert_eq!(stats1.entities_inserted, 1);
        assert!(downstream.entity_get_by_id(eid).unwrap().summary.is_none());

        std::thread::sleep(std::time::Duration::from_millis(2));
        upstream
            .entity_describe("Redis", "In-memory cache; cluster mode for HA")
            .unwrap();

        let mut buf2 = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    since: stats1.new_cursor.clone(),
                    ..Default::default()
                },
                &mut buf2,
            )
            .unwrap();
        let stats2 = downstream
            .sync_import(std::str::from_utf8(&buf2).unwrap())
            .unwrap();
        // The entity is in the cursor's known set, so it lands in the
        // updates pass — entities_inserted counter goes up because
        // upsert returns Ok(true) for the LWW overwrite path too.
        assert!(stats2.entities_inserted + stats2.entities_skipped >= 1);

        let after = downstream.entity_get_by_id(eid).unwrap();
        assert_eq!(
            after.summary.as_deref(),
            Some("In-memory cache; cluster mode for HA"),
            "summary update must reach the downstream"
        );
    }
}
