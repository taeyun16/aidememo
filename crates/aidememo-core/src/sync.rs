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
//! {"kind":"cursor","entity":"01J...","fact":"01J...","entity_updated_at":1741540123000,"entity_updated_id":"01J...","relation_generation":"ab12...","relation_scan_key":null}
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
//! Phase 3 — not in this module. Relations replicate as append/upsert records;
//! deletions are not represented on this wire and therefore do not propagate.

use crate::AideMemo;
use crate::backend::StoreBackend;
use crate::error::{AideMemoError, Result};
use crate::types::{EntityId, EntityRecord, FactId, FactListOpts, FactRecord, ListOpts};
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
/// * **update cursor** (`entity_updated_at` + `entity_updated_id`, and the
///   corresponding fact pair) — stable high-water for in-place mutations on
///   already-pulled records.
///   `supersede`, `pin`, `entity_describe`, etc. bump `updated_at`
///   without changing the ULID, so we need a second cursor to
///   notice them.
///
/// Phase 2 stored only the ULID pair. Every later field uses
/// `#[serde(default)]` so existing `<store>.sync.json` cursor files keep
/// loading and safely replay the newly tracked record class once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCursor {
    /// Last entity ULID the downstream successfully applied.
    pub entity: Option<EntityId>,
    /// Last fact ULID the downstream successfully applied.
    pub fact: Option<FactId>,
    /// Highest `updated_at` (epoch ms) the downstream has seen for
    /// any entity. Drives the "in-place updates" pass.
    #[serde(default)]
    pub entity_updated_at: Option<u64>,
    /// Entity-ID tie-breaker for records sharing `entity_updated_at`.
    /// A missing ID is a legacy timestamp-only cursor and deliberately
    /// replays every record at the boundary timestamp once.
    #[serde(default)]
    pub entity_updated_id: Option<EntityId>,
    /// Highest `updated_at` (epoch ms) the downstream has seen for
    /// any fact. Drives the "in-place updates" pass.
    #[serde(default)]
    pub fact_updated_at: Option<u64>,
    /// Fact-ID tie-breaker for records sharing `fact_updated_at`.
    #[serde(default)]
    pub fact_updated_id: Option<FactId>,
    /// Creation-time high-water for relations. Relations have no ULID, so
    /// pagination uses `(created_at, relation_key)` as a stable cursor.
    /// Missing on pre-relation-cursor files, which intentionally triggers one
    /// idempotent full relation pass on the next pull.
    #[serde(default)]
    pub relation_created_at: Option<u64>,
    /// Deterministic tie-breaker for relations created in the same
    /// millisecond. The value is URL-safe so CLI pull can send it directly as
    /// a query parameter.
    #[serde(default)]
    pub relation_key: Option<String>,
    /// Digest of the complete, stably sorted relation snapshot. A changed
    /// generation restarts an idempotent full relation scan, which catches
    /// late historical inserts and in-place relation content changes.
    #[serde(default)]
    pub relation_generation: Option<String>,
    /// Stable pagination key inside `relation_generation`. `None` together
    /// with a matching generation means that snapshot is fully consumed.
    #[serde(default)]
    pub relation_scan_key: Option<String>,
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
    /// Include relations in the export. Relations have no ULID, so current
    /// clients use snapshot generation + scan-key pagination; legacy clients
    /// retain `(created_at, relation_key)` fallback semantics.
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
    Ok(preflight_sync_jsonl(jsonl)?.cursor)
}

impl AideMemo {
    /// Emit a JSONL delta in two passes:
    ///   1. **Inserts** — entities / facts whose ULID is strictly
    ///      above `since.entity` / `since.fact`.
    ///   2. **Updates** — entities / facts whose ULID is *at or below*
    ///      that cursor (i.e. already pulled before) but whose
    ///      `(updated_at, id)` is strictly above the corresponding update
    ///      cursor. Catches `supersede`, `pin`, `entity_describe`, and other
    ///      in-place mutations without dropping same-millisecond ties.
    ///
    /// Both passes share the same `limit` budget and the same output
    /// stream. Insert-pass records do not advance update watermarks; after the
    /// ULID streams drain, those records replay once through the update pass.
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
        let mut last_entity_updated_id: Option<EntityId> = opts.since.entity_updated_id;
        let mut last_fact_updated_at: Option<u64> = opts.since.fact_updated_at;
        let mut last_fact_updated_id: Option<FactId> = opts.since.fact_updated_id;
        let mut last_relation_created_at: Option<u64> = opts.since.relation_created_at;
        let mut last_relation_key: Option<String> = opts.since.relation_key.clone();
        let mut last_relation_generation = opts.since.relation_generation.clone();
        let mut last_relation_scan_key = opts.since.relation_scan_key.clone();

        let store = self.store_handle();

        // PASS A — entities: new ULIDs above the cursor.
        let entity_summaries = store.read().entity_list(ListOpts {
            limit: Some(usize::MAX),
            ..Default::default()
        })?;
        let mut new_entities: Vec<EntityRecord> = Vec::new();
        for s in &entity_summaries {
            if let Some(cut) = opts.since.entity
                && s.id.0 <= cut.0
            {
                continue;
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
                if let Some(cut) = opts.since.entity
                    && s.id.0 > cut.0
                {
                    continue; // already in pass A
                }
                if let Ok(rec) = store.read().entity_get_by_id(s.id)
                    && update_is_after_cursor(
                        rec.updated_at,
                        rec.id.0,
                        opts.since.entity_updated_at,
                        opts.since.entity_updated_id.map(|id| id.0),
                    )
                {
                    updates.push(rec);
                }
            }
            updates.sort_by_key(|e| (e.updated_at, e.id.0));
            for e in &updates {
                if emitted >= limit {
                    break;
                }
                let line = serde_json::json!({"kind": "entity", "data": e});
                writeln!(writer, "{}", line).map_err(io_serialize)?;
                last_entity_updated_at = Some(e.updated_at);
                last_entity_updated_id = Some(e.id);
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
                if let Some(cut) = opts.since.fact
                    && f.id.0 <= cut.0
                {
                    continue;
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
                emitted += 1;
            }

            // PASS B — fact updates.
            if emitted < limit && opts.since.fact.is_some() {
                let mut updates: Vec<FactRecord> = Vec::new();
                for f in &all_facts {
                    if let Some(cut) = opts.since.fact
                        && f.id.0 > cut.0
                    {
                        continue; // already in pass A
                    }
                    if update_is_after_cursor(
                        f.updated_at,
                        f.id.0,
                        opts.since.fact_updated_at,
                        opts.since.fact_updated_id.map(|id| id.0),
                    ) {
                        updates.push(f.clone());
                    }
                }
                updates.sort_by_key(|f| (f.updated_at, f.id.0));
                for f in &updates {
                    if emitted >= limit {
                        break;
                    }
                    let line = serde_json::json!({"kind": "fact", "data": f});
                    writeln!(writer, "{}", line).map_err(io_serialize)?;
                    last_fact_updated_at = Some(f.updated_at);
                    last_fact_updated_id = Some(f.id);
                    emitted += 1;
                }
            }
        }

        // Relations are append/upsert-only and have no monotonic record ID.
        // The new cursor therefore fingerprints the complete sorted relation
        // snapshot. Any addition or content change produces a new generation
        // and restarts an idempotent full scan. The scan key is ordered by
        // `(created_at, identity_hash)`, matching the legacy cursor order so
        // timestamp/key-only clients can continue a page stream safely.
        if opts.include_relations && emitted < limit {
            let relations = store.read().relations_list_all()?;
            let snapshot = relation_snapshot(relations)?;
            let is_fresh_or_snapshot_cursor = opts.since.relation_generation.is_some()
                || opts.since.relation_scan_key.is_some()
                || (opts.since.relation_created_at.is_none() && opts.since.relation_key.is_none());

            let pending: Vec<&RelationSnapshotEntry> = if is_fresh_or_snapshot_cursor {
                if opts.since.relation_generation.as_deref() == Some(&snapshot.generation) {
                    match opts.since.relation_scan_key.as_deref() {
                        Some(scan_key) => snapshot
                            .entries
                            .iter()
                            .filter(|entry| entry.scan_key.as_str() > scan_key)
                            .collect(),
                        // Matching generation + no scan key means complete.
                        None => Vec::new(),
                    }
                } else {
                    // Fresh client or changed snapshot: replay from the start.
                    snapshot.entries.iter().collect()
                }
            } else {
                // Legacy created_at/key clients keep their historical resume
                // semantics. A missing tie-breaker replays the boundary time.
                snapshot
                    .entries
                    .iter()
                    .filter(|entry| {
                        relation_is_after_cursor(
                            entry.relation.created_at,
                            &entry.identity_key,
                            opts.since.relation_created_at,
                            opts.since.relation_key.as_deref(),
                        )
                    })
                    .collect()
            };

            let remaining_budget = limit.saturating_sub(emitted);
            let page_len = pending.len().min(remaining_budget);
            for entry in pending.iter().take(page_len) {
                let relation = &entry.relation;
                let line = serde_json::json!({"kind": "relation", "data": relation});
                writeln!(writer, "{}", line).map_err(io_serialize)?;
                last_relation_created_at = Some(relation.created_at);
                last_relation_key = Some(entry.identity_key.clone());
                emitted += 1;
            }

            if page_len == pending.len() {
                last_relation_generation = Some(snapshot.generation);
                last_relation_scan_key = None;
            } else if is_fresh_or_snapshot_cursor && page_len > 0 {
                last_relation_generation = Some(snapshot.generation);
                last_relation_scan_key = pending
                    .get(page_len - 1)
                    .map(|entry| entry.scan_key.clone());
            }
        }

        // Trailing cursor — tells downstream where to resume.
        let cursor_line = serde_json::json!({
            "kind": "cursor",
            "entity": last_entity.map(|e| e.0.to_string()),
            "fact": last_fact.map(|f| f.0.to_string()),
            "entity_updated_at": last_entity_updated_at,
            "entity_updated_id": last_entity_updated_id.map(|e| e.0.to_string()),
            "fact_updated_at": last_fact_updated_at,
            "fact_updated_id": last_fact_updated_id.map(|f| f.0.to_string()),
            "relation_created_at": last_relation_created_at,
            "relation_key": last_relation_key,
            "relation_generation": last_relation_generation,
            "relation_scan_key": last_relation_scan_key,
        });
        writeln!(writer, "{}", cursor_line).map_err(io_serialize)?;

        Ok(emitted)
    }

    /// Apply a JSONL delta produced by `sync_export`. Idempotent — a
    /// re-applied blob skips records the local store already has and
    /// returns them in the `*_skipped` counters.
    ///
    /// The complete envelope is parsed and validated before the first store
    /// write. A malformed record, missing/late header, unsupported schema, or
    /// invalid/non-trailing cursor therefore applies zero records.
    pub fn sync_import(&self, jsonl: &str) -> Result<SyncImportStats> {
        let envelope = preflight_sync_jsonl(jsonl)?;
        let mut stats = SyncImportStats::default();
        let store = self.store_handle();

        for record in envelope.records {
            match record {
                SyncWireRecord::Entity(rec) => match store.write().entity_upsert_record(rec) {
                    Ok(true) => stats.entities_inserted += 1,
                    Ok(false) => stats.entities_skipped += 1,
                    Err(_) => stats.errors += 1,
                },
                SyncWireRecord::Fact(rec) => match store.write().fact_upsert_record(rec) {
                    Ok(true) => stats.facts_inserted += 1,
                    Ok(false) => stats.facts_skipped += 1,
                    Err(_) => stats.errors += 1,
                },
                SyncWireRecord::Relation(rec) => match store.write().relation_upsert_record(rec) {
                    Ok(true) => stats.relations_inserted += 1,
                    Ok(false) => stats.relations_skipped += 1,
                    Err(_) => stats.errors += 1,
                },
            }
        }

        // Applying individual records is intentionally idempotent, so a
        // partially successful import can be retried. Never expose the
        // upstream cursor when any record failed; doing so would make the
        // failed record unreachable on the next pull.
        if stats.errors == 0 {
            stats.new_cursor = envelope.cursor;
        }

        // Mark BM25 dirty since fact / entity rows advanced. Cheap.
        self.bm25_mark_dirty_pub();
        Ok(stats)
    }
}

enum SyncWireRecord {
    Entity(EntityRecord),
    Fact(FactRecord),
    Relation(crate::types::RelationRecord),
}

struct SyncEnvelope {
    records: Vec<SyncWireRecord>,
    cursor: SyncCursor,
}

fn preflight_sync_jsonl(jsonl: &str) -> Result<SyncEnvelope> {
    let lines: Vec<&str> = jsonl
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() < 2 {
        return Err(AideMemoError::InvalidInput(
            "sync envelope requires a header first and one trailing cursor".to_string(),
        ));
    }

    let mut records = Vec::with_capacity(lines.len().saturating_sub(2));
    let mut cursor = None;
    let last = lines.len() - 1;
    for (index, line) in lines.into_iter().enumerate() {
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|source| AideMemoError::Deserialize {
                context: format!("sync envelope line {}", index + 1),
                source,
            })?;
        let kind = value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                AideMemoError::InvalidInput(format!(
                    "sync envelope line {} has no string kind",
                    index + 1
                ))
            })?;

        if index == 0 {
            if kind != "header" {
                return Err(AideMemoError::InvalidInput(
                    "sync envelope first record must be a header".to_string(),
                ));
            }
            let schema = value
                .get("schema")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| {
                    AideMemoError::InvalidInput(
                        "sync header schema must be a positive integer".to_string(),
                    )
                })?;
            if schema == 0 || schema > u64::from(SYNC_SCHEMA) {
                return Err(AideMemoError::InvalidInput(format!(
                    "sync schema {schema} is not supported (maximum {SYNC_SCHEMA})"
                )));
            }
            continue;
        }

        if index == last {
            if kind != "cursor" {
                return Err(AideMemoError::InvalidInput(
                    "sync envelope must end with exactly one cursor".to_string(),
                ));
            }
            cursor = Some(parse_cursor_value(&value)?);
            continue;
        }

        let data = value.get("data").cloned().ok_or_else(|| {
            AideMemoError::InvalidInput(format!(
                "sync {kind} record on line {} is missing data",
                index + 1
            ))
        })?;
        let record = match kind {
            "entity" => SyncWireRecord::Entity(serde_json::from_value(data).map_err(|source| {
                AideMemoError::Deserialize {
                    context: format!("sync entity record on line {}", index + 1),
                    source,
                }
            })?),
            "fact" => SyncWireRecord::Fact(serde_json::from_value(data).map_err(|source| {
                AideMemoError::Deserialize {
                    context: format!("sync fact record on line {}", index + 1),
                    source,
                }
            })?),
            "relation" => {
                SyncWireRecord::Relation(serde_json::from_value(data).map_err(|source| {
                    AideMemoError::Deserialize {
                        context: format!("sync relation record on line {}", index + 1),
                        source,
                    }
                })?)
            }
            "header" => {
                return Err(AideMemoError::InvalidInput(
                    "sync envelope may contain only one leading header".to_string(),
                ));
            }
            "cursor" => {
                return Err(AideMemoError::InvalidInput(
                    "sync cursor must be the trailing record".to_string(),
                ));
            }
            other => {
                return Err(AideMemoError::InvalidInput(format!(
                    "unknown sync record kind `{other}`"
                )));
            }
        };
        records.push(record);
    }

    Ok(SyncEnvelope {
        records,
        cursor: cursor.ok_or_else(|| {
            AideMemoError::InvalidInput("sync envelope missing trailing cursor".to_string())
        })?,
    })
}

fn parse_cursor_value(value: &serde_json::Value) -> Result<SyncCursor> {
    let parse_ulid = |key: &str| -> Result<Option<Ulid>> {
        parse_optional_string(value, key)?
            .map(|raw| {
                Ulid::from_string(&raw).map_err(|source| {
                    AideMemoError::InvalidInput(format!(
                        "invalid sync cursor {key} ULID `{raw}`: {source}"
                    ))
                })
            })
            .transpose()
    };
    Ok(SyncCursor {
        entity: parse_ulid("entity")?.map(EntityId),
        fact: parse_ulid("fact")?.map(FactId),
        entity_updated_at: parse_optional_u64(value, "entity_updated_at")?,
        entity_updated_id: parse_ulid("entity_updated_id")?.map(EntityId),
        fact_updated_at: parse_optional_u64(value, "fact_updated_at")?,
        fact_updated_id: parse_ulid("fact_updated_id")?.map(FactId),
        relation_created_at: parse_optional_u64(value, "relation_created_at")?,
        relation_key: parse_optional_string(value, "relation_key")?,
        relation_generation: parse_optional_string(value, "relation_generation")?,
        relation_scan_key: parse_optional_string(value, "relation_scan_key")?,
    })
}

fn parse_optional_u64(value: &serde_json::Value, key: &str) -> Result<Option<u64>> {
    match value.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(raw) => raw.as_u64().map(Some).ok_or_else(|| {
            AideMemoError::InvalidInput(format!(
                "sync cursor {key} must be an unsigned integer or null"
            ))
        }),
    }
}

fn parse_optional_string(value: &serde_json::Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(raw) => raw
            .as_str()
            .map(|text| Some(text.to_string()))
            .ok_or_else(|| {
                AideMemoError::InvalidInput(format!("sync cursor {key} must be a string or null"))
            }),
    }
}

fn update_is_after_cursor(
    updated_at: u64,
    id: Ulid,
    cursor_updated_at: Option<u64>,
    cursor_id: Option<Ulid>,
) -> bool {
    match cursor_updated_at {
        None => true,
        Some(cursor_updated_at) if updated_at > cursor_updated_at => true,
        Some(cursor_updated_at) if updated_at < cursor_updated_at => false,
        Some(_) => cursor_id.is_none_or(|cursor_id| id > cursor_id),
    }
}

struct RelationSnapshotEntry {
    scan_key: String,
    identity_key: String,
    relation: crate::types::RelationRecord,
}

struct RelationSnapshot {
    generation: String,
    entries: Vec<RelationSnapshotEntry>,
}

fn relation_snapshot(relations: Vec<crate::types::RelationRecord>) -> Result<RelationSnapshot> {
    use sha2::{Digest, Sha256};

    let mut entries = Vec::with_capacity(relations.len());
    for relation in relations {
        let identity_key = relation_cursor_key(&relation);
        // Fixed-width decimal timestamp keeps lexical and numeric order equal.
        let scan_key = format!("{:020}-{identity_key}", relation.created_at);
        entries.push(RelationSnapshotEntry {
            scan_key,
            identity_key,
            relation,
        });
    }
    entries.sort_by(|left, right| left.scan_key.cmp(&right.scan_key));

    let mut hasher = Sha256::new();
    for entry in &entries {
        // Serialize the full record, not only identity. Weight/evidence and
        // scope changes must invalidate an in-progress/completed generation.
        let record =
            serde_json::to_vec(&entry.relation).map_err(|source| AideMemoError::Serialize {
                context: "relation sync generation".to_string(),
                source,
            })?;
        hasher.update((record.len() as u64).to_be_bytes());
        hasher.update(record);
    }

    Ok(RelationSnapshot {
        generation: hex_digest(hasher.finalize()),
        entries,
    })
}

fn relation_cursor_key(relation: &crate::types::RelationRecord) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(relation.source_id.to_string());
    hasher.update([0]);
    hasher.update(relation.scope_source_id.as_deref().unwrap_or("").as_bytes());
    hasher.update([0]);
    hasher.update(relation.relation_type.0.as_bytes());
    hasher.update([0]);
    hasher.update(relation.target_id.to_string());
    hex_digest(hasher.finalize())
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut key = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut key, "{byte:02x}");
    }
    key
}

fn relation_is_after_cursor(
    created_at: u64,
    key: &str,
    cursor_created_at: Option<u64>,
    cursor_key: Option<&str>,
) -> bool {
    match (cursor_created_at, cursor_key) {
        (Some(cursor_created_at), Some(cursor_key)) => {
            (created_at, key) > (cursor_created_at, cursor_key)
        }
        // A partial/legacy cursor cannot safely disambiguate equal
        // timestamps. Replay that timestamp; relation upsert is idempotent.
        (Some(cursor_created_at), None) => created_at >= cursor_created_at,
        _ => true,
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
    use crate::types::{FactInput, RelationInput, RelationRecord, RelationType};
    use crate::{Config, EntityInput, EntityType, TraverseDirection};

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
                scope_source_id: None,
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
    fn legacy_cursor_deserializes_without_relation_watermark() {
        let cursor: SyncCursor = serde_json::from_str(
            r#"{"entity":null,"fact":null,"entity_updated_at":12,"fact_updated_at":34}"#,
        )
        .unwrap();
        assert_eq!(cursor.entity_updated_at, Some(12));
        assert_eq!(cursor.fact_updated_at, Some(34));
        assert!(cursor.relation_created_at.is_none());
        assert!(cursor.relation_key.is_none());
        assert!(cursor.entity_updated_id.is_none());
        assert!(cursor.fact_updated_id.is_none());
        assert!(cursor.relation_generation.is_none());
        assert!(cursor.relation_scan_key.is_none());
    }

    #[test]
    fn update_cursor_replays_legacy_boundary_then_paginates_by_id() {
        let (_dir, upstream) = open_temp();
        let first = upstream
            .fact_add(FactInput {
                content: "same timestamp update one".into(),
                ..Default::default()
            })
            .unwrap();
        let second = upstream
            .fact_add(FactInput {
                content: "same timestamp update two".into(),
                ..Default::default()
            })
            .unwrap();
        let (lower, higher) = if first.0 < second.0 {
            (first, second)
        } else {
            (second, first)
        };
        let shared_updated_at = crate::time::current_epoch_ms() + 10_000;
        for id in [lower, higher] {
            let mut record = upstream.fact_get(&id).unwrap();
            record.updated_at = shared_updated_at;
            assert!(
                upstream
                    .store_handle()
                    .write()
                    .fact_upsert_record(record)
                    .unwrap()
            );
        }

        let legacy = SyncCursor {
            fact: Some(higher),
            fact_updated_at: Some(shared_updated_at),
            // Legacy timestamp-only cursor: boundary ties must replay.
            fact_updated_id: None,
            ..Default::default()
        };
        let mut first_page = Vec::new();
        assert_eq!(
            upstream
                .sync_export(
                    SyncExportOpts {
                        since: legacy,
                        limit: 1,
                        include_relations: false,
                    },
                    &mut first_page,
                )
                .unwrap(),
            1
        );
        let first_cursor = cursor_from_jsonl(std::str::from_utf8(&first_page).unwrap()).unwrap();
        assert_eq!(first_cursor.fact_updated_at, Some(shared_updated_at));
        assert_eq!(first_cursor.fact_updated_id, Some(lower));

        let mut second_page = Vec::new();
        assert_eq!(
            upstream
                .sync_export(
                    SyncExportOpts {
                        since: first_cursor,
                        limit: 1,
                        include_relations: false,
                    },
                    &mut second_page,
                )
                .unwrap(),
            1
        );
        let second_cursor = cursor_from_jsonl(std::str::from_utf8(&second_page).unwrap()).unwrap();
        assert_eq!(second_cursor.fact_updated_id, Some(higher));
    }

    #[test]
    fn malformed_envelopes_are_rejected_before_any_write() {
        let (_dir, downstream) = open_temp();
        let missing_header = concat!(r#"{"kind":"fact","data":{}}"#, "\n", r#"{"kind":"cursor"}"#);
        assert!(downstream.sync_import(missing_header).is_err());

        let unsupported = concat!(
            r#"{"kind":"header","schema":999}"#,
            "\n",
            r#"{"kind":"cursor"}"#
        );
        assert!(downstream.sync_import(unsupported).is_err());

        let wrong_cursor_type = concat!(
            r#"{"kind":"header","schema":2}"#,
            "\n",
            r#"{"kind":"cursor","entity_updated_at":"yesterday"}"#
        );
        assert!(downstream.sync_import(wrong_cursor_type).is_err());
        assert_eq!(downstream.stats().unwrap().fact_count, 0);
    }

    #[test]
    fn relation_added_after_entities_propagates_incrementally() {
        let (_dir_a, upstream) = open_temp();
        let redis = upstream
            .entity_add(EntityInput {
                name: "Redis".into(),
                ..Default::default()
            })
            .unwrap();
        upstream
            .entity_add(EntityInput {
                name: "Sentinel".into(),
                ..Default::default()
            })
            .unwrap();

        let mut initial = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut initial,
            )
            .unwrap();
        let (_dir_b, downstream) = open_temp();
        let initial_stats = downstream
            .sync_import(std::str::from_utf8(&initial).unwrap())
            .unwrap();

        upstream
            .relation_add(RelationInput {
                source: "Sentinel".into(),
                target: "Redis".into(),
                scope_source_id: None,
                relation_type: RelationType::new("monitors"),
                weight: None,
                evidence: None,
            })
            .unwrap();

        let mut delta = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    since: initial_stats.new_cursor,
                    include_relations: true,
                    ..Default::default()
                },
                &mut delta,
            )
            .unwrap();
        let stats = downstream
            .sync_import(std::str::from_utf8(&delta).unwrap())
            .unwrap();
        assert_eq!(stats.relations_inserted, 1);
        let relations = downstream
            .relations_get("Sentinel", TraverseDirection::Forward)
            .unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].target_id, redis);
    }

    #[test]
    fn relation_generation_catches_late_historical_insert() {
        let (_dir_a, upstream) = open_temp();
        let mut ids = std::collections::HashMap::new();
        for name in ["A", "B", "C"] {
            ids.insert(
                name,
                upstream
                    .entity_add(EntityInput {
                        name: name.to_string(),
                        ..Default::default()
                    })
                    .unwrap(),
            );
        }
        upstream
            .store_handle()
            .write()
            .relation_upsert_record(RelationRecord {
                source_id: ids["A"],
                target_id: ids["B"],
                scope_source_id: None,
                relation_type: RelationType::new("links"),
                weight: 1.0,
                evidence: vec!["initial".to_string()],
                created_at: 100,
            })
            .unwrap();

        let mut initial = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut initial,
            )
            .unwrap();
        let (_dir_b, downstream) = open_temp();
        let initial_stats = downstream
            .sync_import(std::str::from_utf8(&initial).unwrap())
            .unwrap();
        assert!(initial_stats.new_cursor.relation_generation.is_some());

        // This record sorts before the old relation. A monotonic
        // created_at/key cursor would miss it; generation restart must not.
        upstream
            .store_handle()
            .write()
            .relation_upsert_record(RelationRecord {
                source_id: ids["A"],
                target_id: ids["C"],
                scope_source_id: None,
                relation_type: RelationType::new("links"),
                weight: 1.0,
                evidence: vec!["late historical".to_string()],
                created_at: 1,
            })
            .unwrap();

        let mut delta = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    since: initial_stats.new_cursor,
                    include_relations: true,
                    ..Default::default()
                },
                &mut delta,
            )
            .unwrap();
        let stats = downstream
            .sync_import(std::str::from_utf8(&delta).unwrap())
            .unwrap();
        assert_eq!(stats.relations_inserted, 1);
        assert_eq!(
            downstream
                .relations_get("A", TraverseDirection::Forward)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn relation_generation_catches_same_timestamp_lower_key() {
        let (_dir_a, upstream) = open_temp();
        let source = upstream
            .entity_add(EntityInput {
                name: "Source".into(),
                ..Default::default()
            })
            .unwrap();
        let mut candidates = Vec::new();
        for index in 0..8 {
            let target = upstream
                .entity_add(EntityInput {
                    name: format!("Target {index}"),
                    ..Default::default()
                })
                .unwrap();
            let relation = RelationRecord {
                source_id: source,
                target_id: target,
                scope_source_id: None,
                relation_type: RelationType::new("links"),
                weight: 1.0,
                evidence: Vec::new(),
                created_at: 42,
            };
            candidates.push((relation_cursor_key(&relation), relation));
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        let lower = candidates.first().unwrap().1.clone();
        let higher = candidates.last().unwrap().1.clone();
        upstream
            .store_handle()
            .write()
            .relation_upsert_record(higher)
            .unwrap();

        let mut initial = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut initial,
            )
            .unwrap();
        let (_dir_b, downstream) = open_temp();
        let initial_stats = downstream
            .sync_import(std::str::from_utf8(&initial).unwrap())
            .unwrap();

        upstream
            .store_handle()
            .write()
            .relation_upsert_record(lower.clone())
            .unwrap();
        let mut delta = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    since: initial_stats.new_cursor,
                    include_relations: true,
                    ..Default::default()
                },
                &mut delta,
            )
            .unwrap();
        downstream
            .sync_import(std::str::from_utf8(&delta).unwrap())
            .unwrap();
        let relations = downstream
            .relations_get("Source", TraverseDirection::Forward)
            .unwrap();
        assert!(
            relations
                .iter()
                .any(|relation| relation.target_id == lower.target_id)
        );
    }

    #[test]
    fn relation_content_change_invalidates_generation_and_upserts() {
        let (_dir_a, upstream) = open_temp();
        let source = upstream
            .entity_add(EntityInput {
                name: "Source".into(),
                ..Default::default()
            })
            .unwrap();
        let target = upstream
            .entity_add(EntityInput {
                name: "Target".into(),
                ..Default::default()
            })
            .unwrap();
        let original = RelationRecord {
            source_id: source,
            target_id: target,
            scope_source_id: Some("alpha".to_string()),
            relation_type: RelationType::new("links"),
            weight: 1.0,
            evidence: vec!["old evidence".to_string()],
            created_at: 42,
        };
        upstream
            .store_handle()
            .write()
            .relation_upsert_record(original.clone())
            .unwrap();

        let original_generation = relation_snapshot(vec![original.clone()])
            .unwrap()
            .generation;
        let mut changed = original;
        changed.weight = 0.25;
        changed.evidence = vec!["canonical update".to_string()];
        assert_ne!(
            original_generation,
            relation_snapshot(vec![changed.clone()]).unwrap().generation
        );
        let mut changed_scope = changed.clone();
        changed_scope.scope_source_id = Some("beta".to_string());
        assert_ne!(
            relation_snapshot(vec![changed.clone()]).unwrap().generation,
            relation_snapshot(vec![changed_scope]).unwrap().generation
        );

        let mut initial = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut initial,
            )
            .unwrap();
        let (_dir_b, downstream) = open_temp();
        let initial_stats = downstream
            .sync_import(std::str::from_utf8(&initial).unwrap())
            .unwrap();

        assert!(
            upstream
                .store_handle()
                .write()
                .relation_upsert_record(changed)
                .unwrap()
        );
        let mut delta = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    since: initial_stats.new_cursor,
                    include_relations: true,
                    ..Default::default()
                },
                &mut delta,
            )
            .unwrap();
        let stats = downstream
            .sync_import(std::str::from_utf8(&delta).unwrap())
            .unwrap();
        assert_eq!(stats.relations_inserted, 1);
        let synced = downstream
            .relations_get_scoped("Source", TraverseDirection::Forward, Some("alpha"))
            .unwrap();
        assert_eq!(synced.len(), 1);
        assert_eq!(synced[0].weight, 0.25);
        assert_eq!(synced[0].evidence, ["canonical update"]);
    }

    #[test]
    fn relation_cursor_paginates_same_timestamp_without_omission() {
        let (_dir_a, upstream) = open_temp();
        let mut ids = std::collections::HashMap::new();
        for name in ["A", "B", "C", "D"] {
            let id = upstream
                .entity_add(EntityInput {
                    name: name.into(),
                    ..Default::default()
                })
                .unwrap();
            ids.insert(name, id);
        }
        let mut initial = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut initial,
            )
            .unwrap();
        let (_dir_b, downstream) = open_temp();
        let mut cursor = downstream
            .sync_import(std::str::from_utf8(&initial).unwrap())
            .unwrap()
            .new_cursor;

        for target in ["B", "C", "D"] {
            upstream
                .store_handle()
                .write()
                .relation_upsert_record(RelationRecord {
                    source_id: ids["A"],
                    target_id: ids[target],
                    scope_source_id: None,
                    relation_type: RelationType::new("links"),
                    weight: 1.0,
                    evidence: Vec::new(),
                    created_at: 42,
                })
                .unwrap();
        }

        let mut imported = 0;
        for _ in 0..12 {
            let mut page = Vec::new();
            let emitted = upstream
                .sync_export(
                    SyncExportOpts {
                        since: cursor.clone(),
                        limit: 1,
                        include_relations: true,
                    },
                    &mut page,
                )
                .unwrap();
            let stats = downstream
                .sync_import(std::str::from_utf8(&page).unwrap())
                .unwrap();
            cursor = stats.new_cursor;
            imported += stats.relations_inserted;
            if emitted == 0 {
                break;
            }
        }
        assert_eq!(imported, 3);
        assert!(cursor.relation_generation.is_some());
        assert!(cursor.relation_scan_key.is_none());
        assert_eq!(
            downstream
                .relations_get("A", TraverseDirection::Forward)
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn sync_preserves_same_relation_in_distinct_source_namespaces() {
        let (_dir_a, upstream) = open_temp();
        for name in ["SharedSource", "SharedTarget"] {
            upstream
                .entity_add(EntityInput {
                    name: name.to_string(),
                    ..Default::default()
                })
                .unwrap();
        }
        for (namespace, evidence) in [("alpha", "alpha proof"), ("beta", "beta proof")] {
            upstream
                .relation_add(RelationInput {
                    source: "SharedSource".to_string(),
                    target: "SharedTarget".to_string(),
                    scope_source_id: Some(namespace.to_string()),
                    relation_type: RelationType::new("links"),
                    weight: Some(1.0),
                    evidence: Some(vec![evidence.to_string()]),
                })
                .unwrap();
        }

        let mut blob = Vec::new();
        upstream
            .sync_export(
                SyncExportOpts {
                    include_relations: true,
                    ..Default::default()
                },
                &mut blob,
            )
            .unwrap();
        let (_dir_b, downstream) = open_temp();
        let stats = downstream
            .sync_import(std::str::from_utf8(&blob).unwrap())
            .unwrap();
        assert_eq!(stats.relations_inserted, 2);

        let alpha = downstream
            .relations_get_scoped("SharedSource", TraverseDirection::Forward, Some("alpha"))
            .unwrap();
        assert_eq!(alpha.len(), 1);
        assert_eq!(alpha[0].evidence, ["alpha proof"]);
        let beta = downstream
            .relations_get_scoped("SharedSource", TraverseDirection::Forward, Some("beta"))
            .unwrap();
        assert_eq!(beta.len(), 1);
        assert_eq!(beta[0].evidence, ["beta proof"]);
    }

    #[test]
    fn failed_import_withholds_cursor_and_full_retry_is_idempotent() {
        let (_dir_a, upstream) = open_temp();
        upstream
            .fact_add(FactInput {
                content: "first valid fact".into(),
                ..Default::default()
            })
            .unwrap();
        upstream
            .fact_add(FactInput {
                content: "second valid fact".into(),
                ..Default::default()
            })
            .unwrap();
        let mut export = Vec::new();
        upstream
            .sync_export(SyncExportOpts::default(), &mut export)
            .unwrap();
        let original = std::str::from_utf8(&export).unwrap();
        let mut corrupted_lines = Vec::new();
        let mut corrupted_one = false;
        for line in original.lines() {
            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
            if !corrupted_one && value.get("kind").and_then(|kind| kind.as_str()) == Some("fact") {
                value["data"] = serde_json::json!({"invalid": true});
                corrupted_one = true;
            }
            corrupted_lines.push(value.to_string());
        }
        let corrupted = corrupted_lines.join("\n");

        let (_dir_b, downstream) = open_temp();
        let error = downstream.sync_import(&corrupted).unwrap_err();
        assert!(error.to_string().contains("sync fact record"));
        assert_eq!(downstream.stats().unwrap().fact_count, 0);

        let retry = downstream.sync_import(original).unwrap();
        assert_eq!(retry.errors, 0);
        assert_eq!(retry.facts_inserted, 2);
        assert_eq!(retry.facts_skipped, 0);
        assert!(retry.new_cursor.fact.is_some());
    }

    #[test]
    fn cursor_must_be_the_trailing_non_empty_record() {
        let (_dir_a, upstream) = open_temp();
        upstream
            .fact_add(FactInput {
                content: "cursor ordering fact".into(),
                ..Default::default()
            })
            .unwrap();
        let mut export = Vec::new();
        upstream
            .sync_export(SyncExportOpts::default(), &mut export)
            .unwrap();
        let original = std::str::from_utf8(&export).unwrap();
        let mut lines: Vec<&str> = original.lines().collect();
        let cursor = lines.pop().unwrap();
        lines.insert(1, cursor);
        let reordered = lines.join("\n");

        let (_dir_b, downstream) = open_temp();
        let error = downstream.sync_import(&reordered).unwrap_err();
        assert!(error.to_string().contains("cursor"));
        assert_eq!(downstream.stats().unwrap().fact_count, 0);
    }

    #[test]
    fn insert_pass_does_not_advance_update_watermark() {
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
        assert!(stats1.new_cursor.fact_updated_at.is_none());
        assert!(stats1.new_cursor.fact_updated_id.is_none());

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

        // f2 is the new insert. f1 deliberately replays through PASS B once
        // because PASS A did not claim it had processed the update stream.
        let blob = std::str::from_utf8(&buf2).unwrap();
        assert!(blob.contains(&f2.0.to_string()), "delta should include f2");
        assert!(
            blob.contains(&f1.0.to_string()),
            "initial insert should replay once through the update stream"
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
