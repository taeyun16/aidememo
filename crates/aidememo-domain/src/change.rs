//! Ordered per-project change-feed types and validation.

use crate::{
    ActorId, CanonicalResource, DomainError, ProjectEpoch, ProjectId, ProjectScope,
    ProjectSequence, ResourceRef, ResourceState, Revision, TenantId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Canonical mutation represented in the replica change feed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOperation {
    /// Create or replace the canonical representation at this revision.
    Upsert,
    /// Durable tombstone. Replicas must remove the resource at this revision.
    Delete,
}

/// One ordered project mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChangeEntry {
    /// Tenant scope.
    pub tenant_id: TenantId,
    /// Project scope.
    pub project_id: ProjectId,
    /// History generation containing this entry.
    pub project_epoch: ProjectEpoch,
    /// Positive, monotonically increasing project sequence.
    pub seq: ProjectSequence,
    /// Mutated or deleted resource.
    pub resource: ResourceRef,
    /// Upsert or tombstone.
    pub operation: ChangeOperation,
    /// Resource revision after this mutation.
    pub revision: Revision,
    /// Authenticated writer provenance.
    pub actor_id: ActorId,
    /// UTC Unix timestamp in milliseconds.
    pub committed_at_ms: i64,
}

impl ChangeEntry {
    /// Validate invariants that every adapter must enforce before publication.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidChangeBatch`] when the entry uses the
    /// reserved zero sequence.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.seq == ProjectSequence::ZERO {
            return Err(DomainError::InvalidChangeBatch(
                "change entry sequence must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Replica cursor for one canonical project history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChangeCursor {
    /// History generation. A mismatch requires snapshot refresh.
    pub project_epoch: ProjectEpoch,
    /// Last sequence durably applied by the replica.
    pub after_seq: ProjectSequence,
}

/// Validated incremental feed response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChangeBatch {
    /// Tenant-project scope selected by the authenticated route.
    pub scope: ProjectScope,
    /// Input cursor.
    pub cursor: ChangeCursor,
    /// Strictly increasing entries after the input cursor.
    pub entries: Vec<ChangeEntry>,
    /// Cursor to persist only after all entries commit locally.
    pub next_cursor: ChangeCursor,
    /// Whether the server has additional entries available.
    pub has_more: bool,
}

impl ChangeBatch {
    /// Build and validate an ordered batch.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidChangeBatch`] when an entry has the wrong
    /// project or epoch, does not advance the cursor, or an empty batch claims
    /// that more entries are available.
    pub fn new(
        scope: ProjectScope,
        cursor: ChangeCursor,
        entries: Vec<ChangeEntry>,
        has_more: bool,
    ) -> Result<Self, DomainError> {
        let mut previous = cursor.after_seq;
        for entry in &entries {
            entry.validate()?;
            if entry.tenant_id != scope.tenant_id || entry.project_id != scope.project_id {
                return Err(DomainError::InvalidChangeBatch(
                    "entry project does not match requested project".to_owned(),
                ));
            }
            if entry.project_epoch != cursor.project_epoch {
                return Err(DomainError::InvalidChangeBatch(
                    "entry epoch does not match cursor epoch".to_owned(),
                ));
            }
            if entry.seq <= previous {
                return Err(DomainError::InvalidChangeBatch(
                    "entry sequences must be strictly increasing after the cursor".to_owned(),
                ));
            }
            previous = entry.seq;
        }
        if has_more && entries.is_empty() {
            return Err(DomainError::InvalidChangeBatch(
                "an empty batch cannot claim additional entries".to_owned(),
            ));
        }
        let next_cursor = ChangeCursor {
            project_epoch: cursor.project_epoch.clone(),
            after_seq: previous,
        };
        Ok(Self {
            scope,
            cursor,
            entries,
            next_cursor,
            has_more,
        })
    }

    /// Build a visibility-projected batch while preserving the scanned
    /// canonical cursor.
    ///
    /// Actor-scoped feeds may omit entries that the authenticated actor cannot
    /// observe. `next_cursor` therefore may advance beyond the last returned
    /// entry, or advance with no returned entries at all.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidChangeBatch`] when the projected entries
    /// are invalid, the cursor epoch changes, the scan cursor moves backwards,
    /// or a non-terminal batch makes no progress.
    pub fn projected(
        scope: ProjectScope,
        cursor: ChangeCursor,
        entries: Vec<ChangeEntry>,
        next_cursor: ChangeCursor,
        has_more: bool,
    ) -> Result<Self, DomainError> {
        let validated = Self::new(scope.clone(), cursor.clone(), entries, false)?;
        if next_cursor.project_epoch != cursor.project_epoch {
            return Err(DomainError::InvalidChangeBatch(
                "projected change cursor changed history epoch".to_owned(),
            ));
        }
        if next_cursor.after_seq < validated.next_cursor.after_seq {
            return Err(DomainError::InvalidChangeBatch(
                "projected change cursor precedes a returned entry".to_owned(),
            ));
        }
        if has_more && next_cursor.after_seq <= cursor.after_seq {
            return Err(DomainError::InvalidChangeBatch(
                "a projected batch with more entries must advance its cursor".to_owned(),
            ));
        }
        Ok(Self {
            scope,
            cursor,
            entries: validated.entries,
            next_cursor,
            has_more,
        })
    }
}

/// One ordered change together with the canonical state at exactly its revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterializedChange {
    /// Ordered mutation metadata.
    pub change: ChangeEntry,
    /// Resource body or tombstone at `change.revision`.
    pub resource: CanonicalResource,
}

impl MaterializedChange {
    /// Validate the exact revision-pinned resource materialization.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidChangeBatch`] when scope, coordinate,
    /// revision, or operation state differs from the change entry.
    pub fn validate(&self, scope: &ProjectScope) -> Result<(), DomainError> {
        self.change.validate()?;
        if self.resource.scope != *scope
            || self.change.tenant_id != scope.tenant_id
            || self.change.project_id != scope.project_id
        {
            return Err(DomainError::InvalidChangeBatch(
                "materialized change has the wrong project scope".to_owned(),
            ));
        }
        if self.resource.resource != self.change.resource {
            return Err(DomainError::InvalidChangeBatch(
                "materialized resource coordinate does not match the change".to_owned(),
            ));
        }
        if self.resource.revision != self.change.revision {
            return Err(DomainError::InvalidChangeBatch(
                "materialized resource revision does not match the change".to_owned(),
            ));
        }
        if !matches!(
            (self.change.operation, &self.resource.state),
            (ChangeOperation::Upsert, ResourceState::Present { .. })
                | (ChangeOperation::Delete, ResourceState::Deleted)
        ) {
            return Err(DomainError::InvalidChangeBatch(
                "materialized resource state does not match the change operation".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Validated incremental feed whose bodies are pinned to each change revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterializedChangeBatch {
    /// Tenant-project scope selected by the authenticated route.
    pub scope: ProjectScope,
    /// Input cursor.
    pub cursor: ChangeCursor,
    /// Strictly increasing revision-pinned changes after the input cursor.
    pub entries: Vec<MaterializedChange>,
    /// Cursor to persist only after all entries commit locally.
    pub next_cursor: ChangeCursor,
    /// Whether the server has additional entries available.
    pub has_more: bool,
}

impl MaterializedChangeBatch {
    /// Build and validate one revision-pinned incremental batch.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidChangeBatch`] for any metadata or exact
    /// resource mismatch.
    pub fn new(
        scope: ProjectScope,
        cursor: ChangeCursor,
        entries: Vec<MaterializedChange>,
        has_more: bool,
    ) -> Result<Self, DomainError> {
        for entry in &entries {
            entry.validate(&scope)?;
        }
        let metadata = ChangeBatch::new(
            scope.clone(),
            cursor.clone(),
            entries.iter().map(|entry| entry.change.clone()).collect(),
            has_more,
        )?;
        Ok(Self {
            scope,
            cursor,
            entries,
            next_cursor: metadata.next_cursor,
            has_more,
        })
    }

    /// Build an actor-visible materialized batch while preserving the scanned
    /// canonical cursor across hidden entries.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidChangeBatch`] when any visible entry or
    /// projected cursor is invalid.
    pub fn projected(
        scope: ProjectScope,
        cursor: ChangeCursor,
        entries: Vec<MaterializedChange>,
        next_cursor: ChangeCursor,
        has_more: bool,
    ) -> Result<Self, DomainError> {
        for entry in &entries {
            entry.validate(&scope)?;
        }
        let metadata = ChangeBatch::projected(
            scope.clone(),
            cursor.clone(),
            entries.iter().map(|entry| entry.change.clone()).collect(),
            next_cursor.clone(),
            has_more,
        )?;
        Ok(Self {
            scope,
            cursor,
            entries,
            next_cursor: metadata.next_cursor,
            has_more,
        })
    }
}

/// Atomic current-state bootstrap for one project history generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    /// Tenant-project scope selected by the authenticated route.
    pub scope: ProjectScope,
    /// History generation containing the snapshot.
    pub project_epoch: ProjectEpoch,
    /// Project head represented by every resource in this snapshot.
    pub at_seq: ProjectSequence,
    /// Complete current resource set, including durable tombstones.
    pub resources: Vec<CanonicalResource>,
}

impl ProjectSnapshot {
    /// Build and validate one complete project snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidChangeBatch`] when a resource has the
    /// wrong scope or a coordinate appears more than once.
    pub fn new(
        scope: ProjectScope,
        project_epoch: ProjectEpoch,
        at_seq: ProjectSequence,
        resources: Vec<CanonicalResource>,
    ) -> Result<Self, DomainError> {
        let mut seen = HashSet::with_capacity(resources.len());
        for resource in &resources {
            if resource.scope != scope {
                return Err(DomainError::InvalidChangeBatch(
                    "snapshot resource has the wrong project scope".to_owned(),
                ));
            }
            if !seen.insert(resource.resource.clone()) {
                return Err(DomainError::InvalidChangeBatch(
                    "snapshot contains a duplicate resource coordinate".to_owned(),
                ));
            }
        }
        Ok(Self {
            scope,
            project_epoch,
            at_seq,
            resources,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ResourceId, ResourceKind};

    fn entry(seq: u64, operation: ChangeOperation) -> Result<ChangeEntry, DomainError> {
        Ok(ChangeEntry {
            tenant_id: TenantId::try_from("tenant_a")?,
            project_id: crate::ProjectId::try_from("project_a")?,
            project_epoch: ProjectEpoch::try_from("epoch_a")?,
            seq: ProjectSequence::new(seq),
            resource: ResourceRef {
                kind: ResourceKind::try_from("fact")?,
                id: ResourceId::try_from("fact_01")?,
            },
            operation,
            revision: Revision::new(seq)?,
            actor_id: ActorId::try_from("codex-p1")?,
            committed_at_ms: 1_700_000_000_000,
        })
    }

    #[test]
    fn deletion_is_a_durable_ordered_tombstone() -> Result<(), Box<dyn std::error::Error>> {
        let cursor = ChangeCursor {
            project_epoch: ProjectEpoch::try_from("epoch_a")?,
            after_seq: ProjectSequence::ZERO,
        };
        let batch = ChangeBatch::new(
            ProjectScope::new(
                TenantId::try_from("tenant_a")?,
                crate::ProjectId::try_from("project_a")?,
            ),
            cursor,
            vec![
                entry(1, ChangeOperation::Upsert)?,
                entry(2, ChangeOperation::Delete)?,
            ],
            false,
        )?;
        assert_eq!(batch.next_cursor.after_seq, ProjectSequence::new(2));
        assert_eq!(batch.entries[1].operation, ChangeOperation::Delete);
        Ok(())
    }

    #[test]
    fn batch_rejects_reordered_entries() -> Result<(), Box<dyn std::error::Error>> {
        let cursor = ChangeCursor {
            project_epoch: ProjectEpoch::try_from("epoch_a")?,
            after_seq: ProjectSequence::ZERO,
        };
        assert!(
            ChangeBatch::new(
                ProjectScope::new(
                    TenantId::try_from("tenant_a")?,
                    crate::ProjectId::try_from("project_a")?,
                ),
                cursor,
                vec![
                    entry(2, ChangeOperation::Upsert)?,
                    entry(1, ChangeOperation::Delete)?,
                ],
                false,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn projected_batch_advances_across_hidden_entries() -> Result<(), Box<dyn std::error::Error>> {
        let scope = ProjectScope::new(
            TenantId::try_from("tenant_a")?,
            crate::ProjectId::try_from("project_a")?,
        );
        let cursor = ChangeCursor {
            project_epoch: ProjectEpoch::try_from("epoch_a")?,
            after_seq: ProjectSequence::new(4),
        };
        let next_cursor = ChangeCursor {
            project_epoch: cursor.project_epoch.clone(),
            after_seq: ProjectSequence::new(8),
        };
        let batch = ChangeBatch::projected(scope, cursor, Vec::new(), next_cursor.clone(), true)?;
        assert!(batch.entries.is_empty());
        assert_eq!(batch.next_cursor, next_cursor);
        assert!(batch.has_more);
        Ok(())
    }
}
