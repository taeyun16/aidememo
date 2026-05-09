//! Pull-only delta sync between two wg stores (Phase 2).
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
//! Sync is one-way (upstream → downstream). The shared `wg mcp-serve`
//! is the canonical writer; downstream agents pull periodically into a
//! local read cache. Multi-master writes with conflict resolution are
//! Phase 3 — not in this module.

use crate::error::{Result, WgError};
use crate::types::{EntityId, EntityRecord, FactId, FactListOpts, FactRecord, ListOpts};
use crate::{TraverseDirection, WikiGraph};
use serde::{Deserialize, Serialize};
use std::io::Write;
use ulid::Ulid;

/// Wire schema version. Receivers should reject blobs with a higher
/// schema than they understand to keep operators from silently
/// importing newer formats with missing fields.
pub const SYNC_SCHEMA: u32 = 2;

/// Cursor pair — where a downstream agent left off on its last pull
/// from a given upstream. Both fields are inclusive lower bounds:
/// "give me everything *strictly after* this ULID".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncCursor {
    /// Last entity ULID the downstream successfully applied.
    pub entity: Option<EntityId>,
    /// Last fact ULID the downstream successfully applied.
    pub fact: Option<FactId>,
}

/// Options for `WikiGraph::sync_export`.
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

/// Stats for `WikiGraph::sync_import`.
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

impl WikiGraph {
    /// Emit a JSONL delta of entities + facts (+ optional relations)
    /// created strictly after the cursor. Caller writes the bytes to
    /// any sink (HTTP body, file, etc).
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

        // Entities — fetch all then filter by ULID > since.entity.
        // entity_list returns EntitySummary; we need the full record.
        let store = self.store_handle();
        let entity_summaries = store.read().entity_list(ListOpts {
            limit: Some(usize::MAX),
            ..Default::default()
        })?;
        let mut new_entities: Vec<EntityRecord> = Vec::new();
        for s in entity_summaries {
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
        for e in new_entities {
            if emitted >= limit {
                break;
            }
            let line = serde_json::json!({"kind": "entity", "data": e});
            writeln!(writer, "{}", line).map_err(io_serialize)?;
            last_entity = Some(e.id);
            emitted += 1;
        }

        // Facts — fact_list already returns Vec<FactRecord>.
        if emitted < limit {
            let mut facts = store.read().fact_list(FactListOpts {
                limit: None,
                ..Default::default()
            })?;
            facts.retain(|f| match opts.since.fact {
                Some(cut) => f.id.0 > cut.0,
                None => true,
            });
            facts.sort_by_key(|f| f.id.0);
            for f in facts {
                if emitted >= limit {
                    break;
                }
                let line = serde_json::json!({"kind": "fact", "data": f});
                writeln!(writer, "{}", line).map_err(io_serialize)?;
                last_fact = Some(f.id);
                emitted += 1;
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

        // Trailing cursor — tells downstream where to resume.
        let cursor_line = serde_json::json!({
            "kind": "cursor",
            "entity": last_entity.map(|e| e.0.to_string()),
            "fact": last_fact.map(|f| f.0.to_string()),
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
                        return Err(WgError::InvalidInput(format!(
                            "sync schema {} is newer than this wg understands ({})",
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
                }
                _ => stats.errors += 1,
            }
        }

        // Mark BM25 dirty since fact / entity rows advanced. Cheap.
        self.bm25_mark_dirty_pub();
        Ok(stats)
    }
}

fn io_serialize(e: std::io::Error) -> WgError {
    WgError::Serialize {
        context: "sync".to_string(),
        source: serde_json::Error::io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FactInput;
    use crate::{Config, EntityInput, EntityType};

    fn open_temp() -> (tempfile::TempDir, WikiGraph) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let wiki = WikiGraph::open(&path, Config::default()).unwrap();
        (dir, wiki)
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
}
