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
        // Get stats for report
        let stats = self.store.stats()?;

        // Single load: entities, all facts, all relations. Each
        // check then walks these slices in memory.
        let entities = self.store.entity_list(ListOpts {
            limit: Some(10_000),
            ..Default::default()
        })?;
        let facts = self.store.fact_list(FactListOpts {
            limit: Some(usize::MAX),
            ..Default::default()
        })?;
        let relations = self.store.relations_list_all()?;

        let mut issues = Vec::new();
        issues.extend(check_orphans(&entities, &relations));
        issues.extend(check_duplicates(&entities));
        issues.extend(self.check_stale(&entities)?);
        issues.extend(check_conflicts(&entities, &facts));

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
/// the user forgot to merge). Cheap length pre-filter prunes pairs
/// whose lengths can't reach the 0.9 threshold; on real wikis with
/// diverse names this drops most comparisons. Synthetic stress tests
/// where every name shares a prefix won't benefit, but the trigram
/// similarity itself is still O(min(|a|,|b|)) which keeps the inner
/// cost tight.
fn check_duplicates(entities: &[EntitySummary]) -> Vec<LintIssue> {
    let mut issues = Vec::new();
    let lowered: Vec<String> = entities.iter().map(|e| e.name.to_lowercase()).collect();
    let lens: Vec<usize> = lowered.iter().map(|s| s.len()).collect();

    for i in 0..entities.len() {
        for j in (i + 1)..entities.len() {
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
}
