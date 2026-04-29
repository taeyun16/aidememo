//! Lint engine for graph health checks.
//!
//! Performance contract: a single `lint()` call loads every entity,
//! every current fact, and every relation **once**, then runs each
//! check against the in-memory copy. Earlier versions did per-entity
//! `fact_list` and `relations_get` calls — at 10K facts that meant
//! ~500 full scans of the facts table per lint.

use crate::error::{Result, WgError};
use crate::store::Store;
use crate::types::*;
use std::collections::{HashMap, HashSet};

/// Lint engine for checking graph health.
pub struct LintEngine<'a> {
    store: &'a Store,
}

impl<'a> LintEngine<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Run all lint checks.
    pub fn lint(&self) -> Result<LintReport> {
        // Same dual-output pattern as search: DEBUG tracing event for
        // anyone running with `RUST_LOG=wg_core=debug`, plus the
        // legacy WG_LINT_PROFILE eprintln for self-contained dumps.
        let profile = std::env::var("WG_LINT_PROFILE").is_ok();
        let phase = |label: &str, t0: std::time::Instant| {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            tracing::debug!(scope = "lint", phase = label, ms, "phase");
            if profile {
                eprintln!("[lint] {label}: {ms:.2}ms");
            }
        };
        let t0 = std::time::Instant::now();

        // Get stats for report
        let stats = self.store.stats()?;
        phase("stats", t0);

        // Single load: entities, all facts, all relations. Each
        // check then walks these slices in memory.
        let t = std::time::Instant::now();
        let entities = self.store.entity_list(ListOpts {
            limit: Some(10_000),
            ..Default::default()
        })?;
        phase("entity_list", t);

        let t = std::time::Instant::now();
        let facts = self.store.fact_list(FactListOpts {
            limit: Some(usize::MAX),
            ..Default::default()
        })?;
        phase("fact_list", t);

        let t = std::time::Instant::now();
        let relations = self.store.relations_list_all()?;
        phase("relations_list_all", t);

        let mut issues = Vec::new();

        let t = std::time::Instant::now();
        issues.extend(check_orphans(&entities, &relations));
        phase("check_orphans", t);

        let t = std::time::Instant::now();
        issues.extend(check_duplicates(&entities));
        phase("check_duplicates", t);

        let t = std::time::Instant::now();
        issues.extend(self.check_stale(&entities)?);
        phase("check_stale", t);

        let t = std::time::Instant::now();
        issues.extend(check_conflicts(&entities, &facts));
        phase("check_conflicts", t);

        Ok(LintReport {
            issues,
            entity_count: stats.entity_count,
            fact_count: stats.fact_count,
            relation_count: stats.relation_count,
        })
    }

    fn check_stale(&self, entities: &[EntitySummary]) -> Result<Vec<LintIssue>> {
        let mut issues = Vec::new();

        let stale_threshold = 90 * 24 * 60 * 60 * 1000; // 90 days in ms
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        for entity in entities {
            let record = match self.store.entity_get_by_id(entity.id) {
                Ok(r) => r,
                Err(WgError::EntityIdNotFound(_)) => {
                    // Corrupt or migrated entity ID — skip gracefully
                    issues.push(LintIssue {
                        severity: LintSeverity::Warning,
                        code: "malformed_entity".to_string(),
                        message: format!(
                            "Entity '{}' has an unreadable ID '{}'",
                            entity.name, entity.id
                        ),
                        entity_id: Some(entity.id),
                        fact_id: None,
                    });
                    continue;
                }
                Err(e) => return Err(e),
            };
            let age = now.saturating_sub(record.updated_at);

            if age > stale_threshold {
                issues.push(LintIssue {
                    severity: LintSeverity::Info,
                    code: "stale".to_string(),
                    message: format!(
                        "Entity '{}' has not been updated in {} days",
                        entity.name,
                        age / (24 * 60 * 60 * 1000)
                    ),
                    entity_id: Some(entity.id),
                    fact_id: None,
                });
            }
        }

        Ok(issues)
    }
}

/// Flag entities that don't appear as either the source or target of
/// any relation. One pass over the relations slice builds a "has any
/// edge" set; the entity loop then just probes it.
fn check_orphans(entities: &[EntitySummary], relations: &[RelationRecord]) -> Vec<LintIssue> {
    let mut connected: HashSet<EntityId> = HashSet::with_capacity(relations.len() * 2);
    for r in relations {
        connected.insert(r.source_id);
        connected.insert(r.target_id);
    }

    entities
        .iter()
        .filter(|e| !connected.contains(&e.id))
        .map(|e| LintIssue {
            severity: LintSeverity::Warning,
            code: "orphan".to_string(),
            message: format!("Entity '{}' has no relations", e.name),
            entity_id: Some(e.id),
            fact_id: None,
        })
        .collect()
}

/// Flag pairs of entities with very similar names (possible aliases
/// the user forgot to merge).
///
/// Optimization layers, applied in order:
/// 1. Build a `trigram → entity-indices` inverted map (one pass per
///    entity). For each pair to even be a candidate they must share
///    at least 4 trigrams — a name can share fewer than four trigrams
///    with another name only if the name pair is too short or too
///    different to ever clear the 0.9 similarity threshold.
/// 2. Prune candidate pairs whose length ratio can't reach 0.9 (cheap
///    div + cmp, drops most pairs on real wikis with diverse names).
/// 3. Run `trigram::similarity` only on the surviving pairs.
///
/// The trigram-blocking step turns the O(N²) all-pairs scan into
/// roughly O(N × avg_postings_per_trigram) for diverse-name corpora.
/// It still degrades to O(N²) when every name shares a long prefix
/// (e.g. synthetic `Entity_0..N` benchmarks), but that's an
/// adversarial input — real wikis don't have it.
fn check_duplicates(entities: &[EntitySummary]) -> Vec<LintIssue> {
    if entities.len() < 2 {
        return Vec::new();
    }

    let lowered: Vec<String> = entities.iter().map(|e| e.name.to_lowercase()).collect();
    let lens: Vec<usize> = lowered.iter().map(|s| s.len()).collect();

    // Per-entity trigram set (deduped). Pair is a candidate only if
    // they share ≥ MIN_SHARED trigrams.
    const MIN_SHARED: usize = 4;
    let trigrams_per: Vec<HashSet<[u8; 3]>> = lowered.iter().map(|s| trigrams_of(s)).collect();

    // Inverted index: trigram -> indices that contain it.
    let mut postings: HashMap<[u8; 3], Vec<usize>> = HashMap::new();
    for (i, trigs) in trigrams_per.iter().enumerate() {
        for t in trigs {
            postings.entry(*t).or_default().push(i);
        }
    }

    // Drop trigrams that aren't discriminative — anything appearing
    // in more than `common_cutoff` names. These match every pair they
    // touch and inflate candidate sets without buying recall: two
    // names that overlap only on common trigrams (e.g. "the lion" and
    // "the tiger" share " th"/"the"/"he " but nothing else) can't
    // reach the 0.9 similarity bar anyway. The ~50× speedup on
    // shared-prefix synthetic corpora comes from this step.
    let common_cutoff = (entities.len() / 4).max(20);
    postings.retain(|_, idxs| idxs.len() <= common_cutoff);

    // For each entity i, count trigram overlap with later entities j.
    // Avoids the (j > i) duplicate-pair problem and lets us use a
    // simple `Vec<u32>` as a counting buffer.
    let mut shared_count = vec![0u32; entities.len()];
    let mut touched: Vec<usize> = Vec::new();
    let mut issues = Vec::new();

    for i in 0..entities.len() {
        // Reset the shared-count buffer for this row.
        for &j in &touched {
            shared_count[j] = 0;
        }
        touched.clear();

        for t in &trigrams_per[i] {
            if let Some(idxs) = postings.get(t) {
                for &j in idxs {
                    if j <= i {
                        continue;
                    }
                    if shared_count[j] == 0 {
                        touched.push(j);
                    }
                    shared_count[j] += 1;
                }
            }
        }

        for &j in &touched {
            if (shared_count[j] as usize) < MIN_SHARED {
                continue;
            }
            // Length-ratio prune (cheap, also caught by trigram count
            // for very short names but useful for medium-length pairs).
            let len_i = lens[i] as f64;
            let len_j = lens[j] as f64;
            let (lo, hi) = if len_i < len_j {
                (len_i, len_j)
            } else {
                (len_j, len_i)
            };
            if hi == 0.0 || lo / hi < 0.9 {
                continue;
            }

            let sim = trigram::similarity(&lowered[i], &lowered[j]);
            if sim > 0.9 {
                issues.push(LintIssue {
                    severity: LintSeverity::Warning,
                    code: "duplicate".to_string(),
                    message: format!(
                        "Entities '{}' and '{}' are very similar (similarity: {:.2})",
                        entities[i].name, entities[j].name, sim
                    ),
                    entity_id: Some(entities[i].id),
                    fact_id: None,
                });
            }
        }
    }

    issues
}

/// Padded byte-trigram set for similarity blocking. Pads with two
/// space bytes on each side so prefix/suffix trigrams participate.
/// Operates on raw bytes, so non-ASCII names produce trigrams that
/// straddle UTF-8 boundaries — that's fine for *blocking* (the
/// authoritative similarity check still uses `trigram::similarity`).
fn trigrams_of(s: &str) -> HashSet<[u8; 3]> {
    let mut out = HashSet::new();
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return out;
    }
    let mut padded = Vec::with_capacity(bytes.len() + 4);
    padded.extend_from_slice(b"  ");
    padded.extend_from_slice(bytes);
    padded.extend_from_slice(b"  ");
    for w in padded.windows(3) {
        out.insert([w[0], w[1], w[2]]);
    }
    out
}

/// Surface entities that have ≥2 *current* facts of an "atomic"
/// type (decision / convention / pattern). One pass over the facts
/// slice builds a `(entity_id, fact_type) → count` map; entities with
/// any group of size ≥2 are reported.
///
/// Notes / claims / questions are NOT atomic — many of those can
/// legitimately coexist describing different aspects of one entity.
fn check_conflicts(entities: &[EntitySummary], facts: &[FactRecord]) -> Vec<LintIssue> {
    // Group atomic-type, current facts by (entity_id, fact_type).
    let mut groups: HashMap<(EntityId, FactType), Vec<&FactRecord>> = HashMap::new();
    for fact in facts {
        if fact.superseded_at.is_some() {
            continue;
        }
        let ft = fact.fact_type;
        if !matches!(
            ft,
            FactType::Decision | FactType::Convention | FactType::Pattern
        ) {
            continue;
        }
        for eid in &fact.entity_ids {
            groups.entry((*eid, ft)).or_default().push(fact);
        }
    }

    // Index entities by id so messages can name them.
    let entity_by_id: HashMap<EntityId, &EntitySummary> =
        entities.iter().map(|e| (e.id, e)).collect();

    let mut issues = Vec::new();
    for ((eid, ft), group) in groups {
        if group.len() < 2 {
            continue;
        }
        let Some(entity) = entity_by_id.get(&eid) else {
            continue;
        };
        let preview: Vec<String> = group
            .iter()
            .take(3)
            .map(|f| {
                let snippet: String = f.content.chars().take(60).collect();
                format!("{} (id={})", snippet, f.id)
            })
            .collect();
        let extra = if group.len() > 3 {
            format!(" (+{} more)", group.len() - 3)
        } else {
            String::new()
        };
        issues.push(LintIssue {
            severity: LintSeverity::Warning,
            code: "conflict".to_string(),
            message: format!(
                "Entity '{}' has {} current {:?} facts that may conflict — supersede the stale one with `wg fact supersede <old> <new>`. Examples: {}{}",
                entity.name,
                group.len(),
                ft,
                preview.join("; "),
                extra,
            ),
            entity_id: Some(eid),
            fact_id: None,
        });
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::tempdir;

    fn fresh_store() -> (Store, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lint-test.redb");
        let store = Store::open(&path, Config::default()).unwrap();
        (store, dir)
    }

    #[test]
    fn conflict_detected_for_two_current_decisions_on_same_entity() {
        let (mut store, _dir) = fresh_store();
        store
            .entity_add(EntityInput {
                name: "Cache".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let cache = store.resolve_entity("Cache").unwrap();

        store
            .fact_add(FactInput {
                content: "use Redis as the cache layer".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![cache]),
                ..Default::default()
            })
            .unwrap();
        store
            .fact_add(FactInput {
                content: "use Postgres as the cache layer".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![cache]),
                ..Default::default()
            })
            .unwrap();

        let report = LintEngine::new(&store).lint().unwrap();
        let conflicts: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.code == "conflict")
            .collect();
        assert_eq!(
            conflicts.len(),
            1,
            "expected one conflict, got {:?}",
            report.issues
        );
        assert!(conflicts[0].message.contains("Cache"));
    }

    #[test]
    fn conflict_clears_after_supersede() {
        let (mut store, _dir) = fresh_store();
        store
            .entity_add(EntityInput {
                name: "Cache".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let cache = store.resolve_entity("Cache").unwrap();

        let old = store
            .fact_add(FactInput {
                content: "use Redis as the cache layer".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![cache]),
                ..Default::default()
            })
            .unwrap();
        let new = store
            .fact_add(FactInput {
                content: "use Postgres as the cache layer".to_string(),
                fact_type: Some(FactType::Decision),
                entity_ids: Some(vec![cache]),
                ..Default::default()
            })
            .unwrap();

        // Mark `old` as superseded directly via fact_update — the
        // higher-level fact_supersede helper lives on WikiGraph,
        // not Store, and we're operating at the Store layer here.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store
            .fact_update(
                &old,
                FactUpdate {
                    superseded_at: Some(now),
                    superseded_by: Some(new),
                    ..Default::default()
                },
            )
            .unwrap();
        let _ = new; // silence unused variable on test paths

        let report = LintEngine::new(&store).lint().unwrap();
        let conflicts: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.code == "conflict")
            .collect();
        assert!(
            conflicts.is_empty(),
            "supersede should clear conflict, got {:?}",
            conflicts
        );
    }

    #[test]
    fn multiple_notes_do_not_conflict() {
        // Notes / claims / questions are NOT atomic — many can
        // legitimately coexist describing different aspects.
        let (mut store, _dir) = fresh_store();
        store
            .entity_add(EntityInput {
                name: "Cache".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();
        let cache = store.resolve_entity("Cache").unwrap();

        for i in 0..3 {
            store
                .fact_add(FactInput {
                    content: format!("note number {i}"),
                    fact_type: Some(FactType::Note),
                    entity_ids: Some(vec![cache]),
                    ..Default::default()
                })
                .unwrap();
        }

        let report = LintEngine::new(&store).lint().unwrap();
        let conflicts: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.code == "conflict")
            .collect();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn duplicate_detector_catches_near_identical_names() {
        // Trigram blocking shouldn't drop true near-duplicates. We
        // bypass `entity_add` (which already runs a fuzzy check) by
        // calling lint's free function directly with two manually
        // constructed `EntitySummary`s.
        let entities = vec![
            EntitySummary {
                id: EntityId::new(),
                name: "Customer Order Pipeline Service".to_string(),
                entity_type: EntityType::Custom("service".into()),
                fact_count: 0,
                tags: vec![],
            },
            EntitySummary {
                id: EntityId::new(),
                name: "Customer Order Pipeline Services".to_string(),
                entity_type: EntityType::Custom("service".into()),
                fact_count: 0,
                tags: vec![],
            },
        ];
        let issues = check_duplicates(&entities);
        assert_eq!(issues.len(), 1, "expected one duplicate, got {:?}", issues);
        assert_eq!(issues[0].code, "duplicate");
    }

    #[test]
    fn duplicate_detector_does_not_false_positive_on_diverse_names() {
        // Trigram blocking should prune unrelated names; the
        // similarity check should also reject them.
        let entities: Vec<EntitySummary> =
            ["Redis", "Postgres", "Kafka", "MongoDB", "Elasticsearch"]
                .iter()
                .map(|name| EntitySummary {
                    id: EntityId::new(),
                    name: (*name).to_string(),
                    entity_type: EntityType::Technology,
                    fact_count: 0,
                    tags: vec![],
                })
                .collect();
        let issues = check_duplicates(&entities);
        assert!(
            issues.is_empty(),
            "no duplicates expected, got {:?}",
            issues
        );
    }
}
