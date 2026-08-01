//! Ordered per-project change-feed types and validation.

use crate::{
    ActorId, DomainError, ProjectEpoch, ProjectId, ProjectSequence, ResourceRef, Revision,
};
use serde::{Deserialize, Serialize};

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
    /// Project selected by the authenticated route.
    pub project_id: ProjectId,
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
        project_id: ProjectId,
        cursor: ChangeCursor,
        entries: Vec<ChangeEntry>,
        has_more: bool,
    ) -> Result<Self, DomainError> {
        let mut previous = cursor.after_seq;
        for entry in &entries {
            entry.validate()?;
            if entry.project_id != project_id {
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
            project_id,
            cursor,
            entries,
            next_cursor,
            has_more,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ResourceId, ResourceKind};

    fn entry(seq: u64, operation: ChangeOperation) -> Result<ChangeEntry, DomainError> {
        Ok(ChangeEntry {
            project_id: ProjectId::try_from("project_a")?,
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
            ProjectId::try_from("project_a")?,
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
                ProjectId::try_from("project_a")?,
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
}
