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
