//! Lint engine for graph health checks.

use crate::error::{Result, WgError};
use crate::store::Store;
use crate::types::*;

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
        let mut issues = Vec::new();

        // Get stats for report
        let stats = self.store.stats()?;

        // Check for orphan entities
        issues.extend(self.check_orphans()?);

        // Check for duplicate entities
        issues.extend(self.check_duplicates()?);

        // Check for stale entities/facts
        issues.extend(self.check_stale()?);

        // Check for one-way relations
        issues.extend(self.check_one_way_relations()?);

        // Check for unresolved conflicts (multiple current decisions
        // / conventions for the same entity).
        issues.extend(self.check_conflicts()?);

        Ok(LintReport {
            issues,
            entity_count: stats.entity_count,
            fact_count: stats.fact_count,
            relation_count: stats.relation_count,
        })
    }

    fn check_orphans(&self) -> Result<Vec<LintIssue>> {
        let mut issues = Vec::new();

        let entities = self.store.entity_list(ListOpts {
            limit: Some(10000),
            ..Default::default()
        })?;

        for entity in entities {
            let relations = self
                .store
                .relations_get(&entity.name, TraverseDirection::Both)?;
            if relations.is_empty() {
                issues.push(LintIssue {
                    severity: LintSeverity::Warning,
                    code: "orphan".to_string(),
                    message: format!("Entity '{}' has no relations", entity.name),
                    entity_id: Some(entity.id),
                    fact_id: None,
                });
            }
        }

        Ok(issues)
    }

    fn check_duplicates(&self) -> Result<Vec<LintIssue>> {
        let mut issues = Vec::new();

        let entities = self.store.entity_list(ListOpts {
            limit: Some(10000),
            ..Default::default()
        })?;

        // Check for entities with very similar names
        for i in 0..entities.len() {
            for j in (i + 1)..entities.len() {
                let sim = trigram::similarity(
                    &entities[i].name.to_lowercase(),
                    &entities[j].name.to_lowercase(),
                );
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

        Ok(issues)
    }

    fn check_stale(&self) -> Result<Vec<LintIssue>> {
        let mut issues = Vec::new();

        let stale_threshold = 90 * 24 * 60 * 60 * 1000; // 90 days in ms
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let entities = self.store.entity_list(ListOpts {
            limit: Some(10000),
            ..Default::default()
        })?;

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

    /// Surface entities that have ≥2 *current* facts of an "atomic"
    /// type (decision / convention / pattern). The user almost never
    /// has two simultaneously-true decisions about the same subject;
    /// either one supersedes the other, or the entity needs to be
    /// split. We flag the situation and let the user resolve it via
    /// `wg fact supersede <old> <new>`.
    ///
    /// Notes / claims / questions are NOT atomic — many of those can
    /// legitimately coexist (multiple notes about the same entity
    /// describe different aspects).
    fn check_conflicts(&self) -> Result<Vec<LintIssue>> {
        use std::collections::HashMap;

        let entities = self.store.entity_list(ListOpts {
            limit: Some(10000),
            ..Default::default()
        })?;

        let mut issues = Vec::new();

        for entity in entities {
            let facts = self.store.fact_list(FactListOpts {
                entity_id: Some(entity.id),
                current_only: true,
                limit: Some(10000),
                ..Default::default()
            })?;

            let mut by_type: HashMap<FactType, Vec<&FactRecord>> = HashMap::new();
            for fact in &facts {
                let ft = fact.fact_type;
                if matches!(
                    ft,
                    FactType::Decision | FactType::Convention | FactType::Pattern
                ) {
                    by_type.entry(ft).or_default().push(fact);
                }
            }

            for (ft, group) in by_type {
                if group.len() < 2 {
                    continue;
                }
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
                    entity_id: Some(entity.id),
                    fact_id: None,
                });
            }
        }

        Ok(issues)
    }

    fn check_one_way_relations(&self) -> Result<Vec<LintIssue>> {
        let issues = Vec::new();

        let entities = self.store.entity_list(ListOpts {
            limit: Some(10000),
            ..Default::default()
        })?;

        for entity in entities {
            let fwd = self
                .store
                .relations_get(&entity.name, TraverseDirection::Forward)?;
            let rev = self
                .store
                .relations_get(&entity.name, TraverseDirection::Reverse)?;

            // If entity has outgoing but no incoming relations, it might be a one-way link
            if !fwd.is_empty() && rev.is_empty() {
                // This is informational only - one-way relations are sometimes valid
            }
        }

        Ok(issues)
    }
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
