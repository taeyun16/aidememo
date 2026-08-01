//! Backend-neutral command and change-feed conformance fixture.
//!
//! Adapter crates implement [`CommandStore`] and
//! invoke [`run`]. The fixture is synchronous because it describes outcomes,
//! not I/O: async adapters can execute their runtime inside the wrapper.

use crate::{
    ActorId, AuthenticatedActor, AuthorizedCommand, ChangeCursor, ChangeOperation, CommandEnvelope,
    CommandFingerprint, CommandId, CommandStore, DomainError, ErrorCode, MembershipRole,
    MembershipStatus, MutationCommand, OperationName, ProjectAuthorization, ProjectEpoch,
    ProjectId, ProjectMembership, ProjectScope, ProjectSequence, ResourceId, ResourceKind,
    ResourceRef, Revision, TenantId,
};

/// Successful fixture report suitable for test output or CI artifacts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConformanceReport {
    /// Stable checks completed by the adapter.
    pub checks: Vec<&'static str>,
    /// Sequence after create, update, and delete.
    pub final_sequence: ProjectSequence,
    /// Revision of the durable deletion tombstone.
    pub tombstone_revision: Revision,
}

/// Run the Phase 0 idempotency, CAS, epoch, and tombstone contract.
///
/// The adapter must be empty for `tenant_fixture/project_fixture` and use the
/// supplied epoch as its current canonical project epoch.
///
/// # Errors
///
/// Returns the adapter's stable domain error, or
/// [`DomainError::ConformanceViolation`] when an observed result breaks a
/// Phase 0 invariant.
pub fn run<A: CommandStore>(
    adapter: &mut A,
    project_epoch: ProjectEpoch,
) -> Result<ConformanceReport, DomainError> {
    let authorization = fixture_authorization()?;
    let resource = ResourceRef {
        kind: ResourceKind::try_from("fact")?,
        id: ResourceId::try_from("fact_fixture")?,
    };
    let create = fixture_command(
        authorization.clone(),
        "command_create",
        None,
        "fact.add",
        'a',
        resource.clone(),
        ChangeOperation::Upsert,
    )?;

    let created = adapter.execute(&create)?;
    check(
        created.project_seq == ProjectSequence::new(1) && created.revision.get() == 1,
        "initial_commit",
        "first mutation must commit sequence 1 and revision 1",
    )?;

    let replayed = adapter.execute(&create)?;
    check(
        replayed == created,
        "idempotent_replay",
        "identical command retry must return the exact stored receipt",
    )?;

    let mut conflicting = create.clone();
    conflicting.fingerprint = fingerprint('b')?;
    check_error_code(
        adapter.execute(&conflicting),
        ErrorCode::CommandConflict,
        "command_id_conflict",
    )?;

    let stale = fixture_command(
        authorization.clone(),
        "command_stale",
        Some(Revision::new(2)?),
        "fact.edit",
        'c',
        resource.clone(),
        ChangeOperation::Upsert,
    )?;
    check_error_code(
        adapter.execute(&stale),
        ErrorCode::StaleRevision,
        "stale_revision",
    )?;

    let update = fixture_command(
        authorization.clone(),
        "command_update",
        Some(created.revision),
        "fact.edit",
        'd',
        resource.clone(),
        ChangeOperation::Upsert,
    )?;
    let updated = adapter.execute(&update)?;
    check(
        updated.project_seq == ProjectSequence::new(2) && updated.revision.get() == 2,
        "compare_and_swap",
        "matching revision must advance project sequence and resource revision once",
    )?;

    let delete = fixture_command(
        authorization,
        "command_delete",
        Some(updated.revision),
        "fact.delete",
        'e',
        resource,
        ChangeOperation::Delete,
    )?;
    let deleted = adapter.execute(&delete)?;

    let cursor = ChangeCursor {
        project_epoch: project_epoch.clone(),
        after_seq: ProjectSequence::ZERO,
    };
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_fixture")?,
        ProjectId::try_from("project_fixture")?,
    );
    let changes = adapter.changes(&scope, &cursor, 100)?;
    check(
        changes.entries.len() == 3,
        "single_mutation_per_command",
        "create retry, command conflict, and stale CAS must not append changes",
    )?;
    let last = changes.entries.last().ok_or_else(|| {
        violation(
            "delete_tombstone",
            "change feed did not contain the deletion entry",
        )
    })?;
    check(
        last.operation == ChangeOperation::Delete
            && last.seq == deleted.project_seq
            && last.revision == deleted.revision,
        "delete_tombstone",
        "last change must be a deletion tombstone with the committed receipt coordinates",
    )?;

    let wrong_epoch_cursor = ChangeCursor {
        project_epoch: ProjectEpoch::try_from("epoch_replaced")?,
        after_seq: ProjectSequence::ZERO,
    };
    check_error_code(
        adapter.changes(&scope, &wrong_epoch_cursor, 100),
        ErrorCode::CursorEpochMismatch,
        "cursor_epoch_fail_closed",
    )?;
    let future_cursor = ChangeCursor {
        project_epoch,
        after_seq: ProjectSequence::new(deleted.project_seq.get() + 1),
    };
    check_error_code(
        adapter.changes(&scope, &future_cursor, 100),
        ErrorCode::CursorOutOfRange,
        "cursor_sequence_fail_closed",
    )?;

    Ok(ConformanceReport {
        checks: vec![
            "initial_commit",
            "idempotent_replay",
            "command_id_conflict",
            "stale_revision",
            "compare_and_swap",
            "single_mutation_per_command",
            "delete_tombstone",
            "cursor_epoch_fail_closed",
            "cursor_sequence_fail_closed",
        ],
        final_sequence: deleted.project_seq,
        tombstone_revision: deleted.revision,
    })
}

fn fixture_authorization() -> Result<ProjectAuthorization, DomainError> {
    let authenticated = AuthenticatedActor::new(
        TenantId::try_from("tenant_fixture")?,
        ActorId::try_from("actor_fixture")?,
    );
    ProjectAuthorization::authorize(
        &authenticated,
        &ProjectMembership {
            tenant_id: TenantId::try_from("tenant_fixture")?,
            project_id: ProjectId::try_from("project_fixture")?,
            actor_id: ActorId::try_from("actor_fixture")?,
            role: MembershipRole::Writer,
            status: MembershipStatus::Active,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn fixture_command(
    authorization: ProjectAuthorization,
    command_id: &str,
    expected_revision: Option<Revision>,
    operation: &str,
    fingerprint_byte: char,
    resource: ResourceRef,
    change: ChangeOperation,
) -> Result<MutationCommand, DomainError> {
    let envelope = CommandEnvelope {
        command_id: CommandId::try_from(command_id)?,
        project_id: authorization.project_id().clone(),
        expected_revision,
        operation: OperationName::try_from(operation)?,
        payload: (),
    };
    Ok(MutationCommand {
        command: AuthorizedCommand::authorize(authorization, envelope)?,
        fingerprint: fingerprint(fingerprint_byte)?,
        resource,
        change,
    })
}

fn fingerprint(byte: char) -> Result<CommandFingerprint, DomainError> {
    CommandFingerprint::try_from(byte.to_string().repeat(64))
}

fn check(condition: bool, name: &'static str, detail: &str) -> Result<(), DomainError> {
    if condition {
        Ok(())
    } else {
        Err(violation(name, detail))
    }
}

fn check_error_code<T>(
    result: Result<T, DomainError>,
    expected: ErrorCode,
    name: &'static str,
) -> Result<(), DomainError> {
    match result {
        Err(error) if error.code() == expected => Ok(()),
        Err(error) => Err(violation(
            name,
            &format!("expected {expected:?}, observed {:?}", error.code()),
        )),
        Ok(_) => Err(violation(
            name,
            &format!("expected {expected:?}, observed success"),
        )),
    }
}

fn violation(check: &'static str, detail: &str) -> DomainError {
    DomainError::ConformanceViolation {
        check,
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChangeBatch, ChangeEntry, CommandReceipt};
    use std::collections::HashMap;

    struct ReferenceAdapter {
        epoch: ProjectEpoch,
        sequence: ProjectSequence,
        revisions: HashMap<ResourceRef, Revision>,
        receipts: HashMap<CommandId, CommandReceipt>,
        changes: Vec<ChangeEntry>,
    }

    impl ReferenceAdapter {
        fn new(epoch: ProjectEpoch) -> Self {
            Self {
                epoch,
                sequence: ProjectSequence::ZERO,
                revisions: HashMap::new(),
                receipts: HashMap::new(),
                changes: Vec::new(),
            }
        }
    }

    impl CommandStore for ReferenceAdapter {
        fn execute(&mut self, command: &MutationCommand) -> Result<CommandReceipt, DomainError> {
            let envelope = command.command.envelope();
            if let Some(receipt) = self.receipts.get(&envelope.command_id) {
                if receipt.fingerprint != command.fingerprint {
                    return Err(DomainError::CommandConflict);
                }
                return Ok(receipt.clone());
            }

            let current = self.revisions.get(&command.resource).copied();
            if let Some(expected) = envelope.expected_revision {
                match current {
                    Some(revision) if revision == expected => {}
                    Some(revision) => {
                        return Err(DomainError::StaleRevision {
                            expected,
                            current: revision,
                        });
                    }
                    None => return Err(DomainError::ResourceNotFound),
                }
            }
            let revision = current.map_or_else(|| Revision::new(1), Revision::next)?;
            let sequence = ProjectSequence::new(self.sequence.get() + 1);
            let authorization = command.command.authorization();
            let receipt = CommandReceipt {
                command_id: envelope.command_id.clone(),
                fingerprint: command.fingerprint.clone(),
                tenant_id: authorization.tenant_id().clone(),
                project_id: authorization.project_id().clone(),
                actor_id: authorization.actor_id().clone(),
                project_seq: sequence,
                resource: command.resource.clone(),
                revision,
                committed_at_ms: 1_700_000_000_000 + sequence.get() as i64,
            };
            self.revisions.insert(command.resource.clone(), revision);
            self.receipts
                .insert(envelope.command_id.clone(), receipt.clone());
            self.changes.push(ChangeEntry {
                tenant_id: authorization.tenant_id().clone(),
                project_id: authorization.project_id().clone(),
                project_epoch: self.epoch.clone(),
                seq: sequence,
                resource: command.resource.clone(),
                operation: command.change,
                revision,
                actor_id: authorization.actor_id().clone(),
                committed_at_ms: receipt.committed_at_ms,
            });
            self.sequence = sequence;
            Ok(receipt)
        }

        fn changes(
            &self,
            scope: &ProjectScope,
            cursor: &ChangeCursor,
            limit: usize,
        ) -> Result<ChangeBatch, DomainError> {
            if cursor.project_epoch != self.epoch {
                return Err(DomainError::CursorEpochMismatch {
                    cursor: cursor.project_epoch.clone(),
                    current: self.epoch.clone(),
                });
            }
            if cursor.after_seq > self.sequence {
                return Err(DomainError::CursorOutOfRange {
                    after_seq: cursor.after_seq,
                    current: self.sequence,
                });
            }
            let entries: Vec<_> = self
                .changes
                .iter()
                .filter(|entry| entry.seq > cursor.after_seq)
                .take(limit)
                .cloned()
                .collect();
            let has_more = self
                .changes
                .iter()
                .filter(|entry| entry.seq > cursor.after_seq)
                .count()
                > entries.len();
            ChangeBatch::new(scope.clone(), cursor.clone(), entries, has_more)
        }
    }

    #[test]
    fn reference_adapter_passes_portable_phase_zero_contract()
    -> Result<(), Box<dyn std::error::Error>> {
        let epoch = ProjectEpoch::try_from("epoch_fixture")?;
        let mut adapter = ReferenceAdapter::new(epoch.clone());
        let report = run(&mut adapter, epoch)?;
        assert_eq!(report.final_sequence, ProjectSequence::new(3));
        assert_eq!(report.tombstone_revision.get(), 3);
        assert_eq!(report.checks.len(), 9);
        Ok(())
    }
}
