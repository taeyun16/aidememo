//! PostgreSQL implementation of the portable AideMemo SSOT command ledger.
//!
//! This crate implements the synchronous outcome contract from `aidememo-domain`.
//! It is intentionally not wired directly into Axum request handling yet: the
//! production server boundary must place blocking database work behind a pool /
//! blocking-executor boundary before enabling this adapter for HTTP traffic.

use aidememo_domain::{
    ActorId, CanonicalResource, ChangeBatch, ChangeCursor, ChangeEntry, ChangeOperation, CommandId,
    CommandReceipt, CommandStore, DomainError, HandoffListEntry, HandoffMailbox, HandoffPage,
    HandoffQuery, HandoffRecord, HandoffStatus, HandoffStore, MaterializedChange,
    MaterializedChangeBatch, MutationCommand, ProjectEpoch, ProjectScope, ProjectSequence,
    ProjectSnapshot, ResourceId, ResourceKind, ResourceRef, ResourceState, Revision, SourceId,
};
use postgres::{Client, GenericClient, IsolationLevel, NoTls, Row, Transaction};
use std::{
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA_COMPONENT: &str = "canonical_store";
const SCHEMA_VERSION: i32 = 1;
const MAX_CHANGE_LIMIT: usize = 10_000;
const MAX_SNAPSHOT_RESOURCES: usize = 10_000;

/// Synchronous PostgreSQL canonical ledger adapter.
///
/// The connection mutex exists because the portable storage contract exposes
/// read operations through `&self`, while the synchronous `postgres::Client`
/// API requires mutable access even for queries. Production HTTP wiring must
/// not hold this blocking connection on an Axum runtime worker thread.
pub struct PostgresCommandStore {
    client: Mutex<Client>,
}

impl PostgresCommandStore {
    /// Connect without TLS and initialize the adapter schema.
    ///
    /// This constructor is intended for local conformance and development
    /// environments. Production transport and pooling are a separate server
    /// integration boundary.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StorageFailure`] when PostgreSQL cannot connect,
    /// initialize, or validate the schema version.
    pub fn connect_no_tls(url: &str) -> Result<Self, DomainError> {
        let client = Client::connect(url, NoTls).map_err(|error| storage("connect", error))?;
        let store = Self {
            client: Mutex::new(client),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Initialize one empty canonical tenant-project history.
    ///
    /// Repeating the same scope and epoch is idempotent. Reusing a scope with a
    /// different epoch fails closed so restore remains an explicit operation.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error for PostgreSQL failures or an epoch
    /// conflict.
    pub fn initialize_project(
        &self,
        scope: &ProjectScope,
        epoch: &ProjectEpoch,
    ) -> Result<(), DomainError> {
        let mut client = self.lock_client()?;
        let mut tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::Serializable)
            .start()
            .map_err(|error| storage("initialize_begin", error))?;
        tx.execute(
            "INSERT INTO ssot_projects (tenant_id, project_id, project_epoch, next_seq)
             VALUES ($1, $2, $3, 0)
             ON CONFLICT (tenant_id, project_id) DO NOTHING",
            &[
                &scope.tenant_id.as_str(),
                &scope.project_id.as_str(),
                &epoch.as_str(),
            ],
        )
        .map_err(|error| storage("initialize_insert", error))?;
        let current = tx
            .query_one(
                "SELECT project_epoch FROM ssot_projects
                 WHERE tenant_id = $1 AND project_id = $2 FOR UPDATE",
                &[&scope.tenant_id.as_str(), &scope.project_id.as_str()],
            )
            .map_err(|error| storage("initialize_read", error))?
            .get::<_, String>(0);
        if current != epoch.as_str() {
            return Err(DomainError::StorageFailure {
                operation: "initialize_project",
                detail: "project already exists with a different epoch".to_owned(),
            });
        }
        tx.commit()
            .map_err(|error| storage("initialize_commit", error))
    }

    /// Return the adapter schema version stored in PostgreSQL.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when schema metadata cannot be read.
    pub fn schema_version(&self) -> Result<u32, DomainError> {
        let mut client = self.lock_client()?;
        let row = client
            .query_one(
                "SELECT version FROM aidememo_schema WHERE component = $1",
                &[&SCHEMA_COMPONENT],
            )
            .map_err(|error| storage("schema_version_read", error))?;
        let version: i32 = row.get(0);
        u32::try_from(version).map_err(|error| storage("schema_version_decode", error))
    }

    fn migrate(&self) -> Result<(), DomainError> {
        let mut client = self.lock_client()?;
        client
            .batch_execute(include_str!("schema.sql"))
            .map_err(|error| storage("schema_create", error))?;
        client
            .execute(
                "INSERT INTO aidememo_schema (component, version)
                 VALUES ($1, $2)
                 ON CONFLICT (component) DO NOTHING",
                &[&SCHEMA_COMPONENT, &SCHEMA_VERSION],
            )
            .map_err(|error| storage("schema_version_write", error))?;
        let version: i32 = client
            .query_one(
                "SELECT version FROM aidememo_schema WHERE component = $1",
                &[&SCHEMA_COMPONENT],
            )
            .map_err(|error| storage("schema_version_read", error))?
            .get(0);
        if version != SCHEMA_VERSION {
            return Err(DomainError::StorageFailure {
                operation: "schema_version",
                detail: format!(
                    "unsupported PostgreSQL canonical schema {version}; expected {SCHEMA_VERSION}"
                ),
            });
        }
        Ok(())
    }

    fn lock_client(&self) -> Result<MutexGuard<'_, Client>, DomainError> {
        self.client.lock().map_err(|_| DomainError::StorageFailure {
            operation: "postgres_connection_lock",
            detail: "PostgreSQL connection mutex is poisoned".to_owned(),
        })
    }

    fn execute_transaction(
        tx: &mut Transaction<'_>,
        command: &MutationCommand,
    ) -> Result<CommandReceipt, DomainError> {
        validate_command_shape(command)?;
        let authorization = command.command.authorization();
        let envelope = command.command.envelope();
        let scope = authorization.scope();

        let project = tx
            .query_opt(
                "SELECT project_epoch, next_seq FROM ssot_projects
                 WHERE tenant_id = $1 AND project_id = $2 FOR UPDATE",
                &[&scope.tenant_id.as_str(), &scope.project_id.as_str()],
            )
            .map_err(|error| storage("project_lock", error))?;
        let Some(project) = project else {
            return Err(DomainError::ProjectUnauthorized {
                project_id: scope.project_id,
            });
        };
        let project_epoch: String = project.get(0);
        let current_sequence = project_sequence(project.get(1), "project_sequence")?;

        if let Some(receipt) = load_receipt(tx, &scope, &envelope.command_id)? {
            if receipt.actor_id != *authorization.actor_id()
                || receipt.fingerprint != command.fingerprint
            {
                return Err(DomainError::CommandConflict);
            }
            return Ok(receipt);
        }

        let current_revision = load_resource_revision(tx, &scope, &command.resource)?;
        if let Some(expected) = envelope.expected_revision {
            match current_revision {
                Some(current) if current == expected => {}
                Some(current) => return Err(DomainError::StaleRevision { expected, current }),
                None => return Err(DomainError::ResourceNotFound),
            }
        }
        let revision = current_revision.map_or_else(|| Revision::new(1), Revision::next)?;
        let sequence = next_sequence(current_sequence)?;
        let committed_at_ms = unix_time_ms()?;
        let deleted = command.change == ChangeOperation::Delete;

        tx.execute(
            "INSERT INTO ssot_resources
                (tenant_id, project_id, resource_kind, resource_id, revision, deleted, body_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (tenant_id, project_id, resource_kind, resource_id)
             DO UPDATE SET revision = EXCLUDED.revision,
                           deleted = EXCLUDED.deleted,
                           body_json = EXCLUDED.body_json",
            &[
                &scope.tenant_id.as_str(),
                &scope.project_id.as_str(),
                &command.resource.kind.as_str(),
                &command.resource.id.as_str(),
                &to_i64(revision.get(), "resource_revision")?,
                &deleted,
                &command.resource_body.as_deref(),
            ],
        )
        .map_err(|error| storage("resource_write", error))?;
        update_handoff_index(
            tx,
            &scope,
            &command.resource,
            command.change,
            command.resource_body.as_deref(),
            revision,
            sequence,
        )?;

        let receipt = CommandReceipt {
            command_id: envelope.command_id.clone(),
            fingerprint: command.fingerprint.clone(),
            tenant_id: authorization.tenant_id().clone(),
            project_id: authorization.project_id().clone(),
            actor_id: authorization.actor_id().clone(),
            project_seq: sequence,
            resource: command.resource.clone(),
            revision,
            committed_at_ms,
        };
        insert_receipt(tx, &receipt)?;
        insert_change(
            tx,
            &receipt,
            &project_epoch,
            command.change,
            command.resource_body.as_deref(),
        )?;
        insert_audit(tx, &receipt, envelope.operation.as_str())?;

        let updated = tx
            .execute(
                "UPDATE ssot_projects SET next_seq = $3
                 WHERE tenant_id = $1 AND project_id = $2 AND next_seq = $4",
                &[
                    &scope.tenant_id.as_str(),
                    &scope.project_id.as_str(),
                    &to_i64(sequence.get(), "project_sequence")?,
                    &to_i64(current_sequence.get(), "project_sequence")?,
                ],
            )
            .map_err(|error| storage("project_sequence_write", error))?;
        if updated != 1 {
            return Err(DomainError::StorageFailure {
                operation: "project_sequence_write",
                detail: "project sequence compare-and-swap did not update one row".to_owned(),
            });
        }
        Ok(receipt)
    }
}

impl CommandStore for PostgresCommandStore {
    fn execute(&mut self, command: &MutationCommand) -> Result<CommandReceipt, DomainError> {
        let mut client = self.lock_client()?;
        let mut tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::Serializable)
            .start()
            .map_err(|error| storage("command_begin", error))?;
        let receipt = Self::execute_transaction(&mut tx, command)?;
        tx.commit()
            .map_err(|error| storage("command_commit", error))?;
        Ok(receipt)
    }

    fn receipt(
        &self,
        scope: &ProjectScope,
        command_id: &CommandId,
    ) -> Result<Option<CommandReceipt>, DomainError> {
        let mut client = self.lock_client()?;
        load_receipt(&mut *client, scope, command_id)
    }

    fn changes(
        &self,
        scope: &ProjectScope,
        cursor: &ChangeCursor,
        limit: usize,
    ) -> Result<ChangeBatch, DomainError> {
        validate_limit(limit)?;
        let mut client = self.lock_client()?;
        let mut tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .map_err(|error| storage("changes_begin", error))?;
        let (_epoch, current_sequence) = validate_cursor(&mut tx, scope, cursor)?;
        let fetch_limit = limit
            .checked_add(1)
            .ok_or_else(|| DomainError::InvalidChangeBatch("limit overflow".to_owned()))?;
        let rows = tx
            .query(
                "SELECT project_epoch, seq, resource_kind, resource_id, operation,
                        revision, actor_id, committed_at_ms
                 FROM ssot_changes
                 WHERE tenant_id = $1 AND project_id = $2 AND seq > $3
                 ORDER BY seq ASC LIMIT $4",
                &[
                    &scope.tenant_id.as_str(),
                    &scope.project_id.as_str(),
                    &to_i64(cursor.after_seq.get(), "cursor_sequence")?,
                    &to_i64(fetch_limit as u64, "change_limit")?,
                ],
            )
            .map_err(|error| storage("changes_query", error))?;
        let mut entries = rows
            .iter()
            .map(|row| decode_change(scope, row))
            .collect::<Result<Vec<_>, DomainError>>()?;
        let has_more = entries.len() > limit;
        if has_more {
            entries.truncate(limit);
        }
        // Reading the head and page in one repeatable-read transaction prevents
        // a response from pairing a newer cursor validation with an older page.
        let _ = current_sequence;
        tx.commit()
            .map_err(|error| storage("changes_commit", error))?;
        ChangeBatch::new(scope.clone(), cursor.clone(), entries, has_more)
    }

    fn materialized_changes(
        &self,
        scope: &ProjectScope,
        cursor: &ChangeCursor,
        limit: usize,
    ) -> Result<MaterializedChangeBatch, DomainError> {
        validate_limit(limit)?;
        let mut client = self.lock_client()?;
        let mut tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .map_err(|error| storage("materialized_changes_begin", error))?;
        validate_cursor(&mut tx, scope, cursor)?;
        let fetch_limit = limit
            .checked_add(1)
            .ok_or_else(|| DomainError::InvalidChangeBatch("limit overflow".to_owned()))?;
        let rows = tx
            .query(
                "SELECT project_epoch, seq, resource_kind, resource_id, operation,
                        revision, actor_id, committed_at_ms, body_json
                 FROM ssot_changes
                 WHERE tenant_id = $1 AND project_id = $2 AND seq > $3
                 ORDER BY seq ASC LIMIT $4",
                &[
                    &scope.tenant_id.as_str(),
                    &scope.project_id.as_str(),
                    &to_i64(cursor.after_seq.get(), "cursor_sequence")?,
                    &to_i64(fetch_limit as u64, "change_limit")?,
                ],
            )
            .map_err(|error| storage("materialized_changes_query", error))?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in &rows {
            let change = decode_change(scope, row)?;
            let body: Option<Vec<u8>> = row.get(8);
            let state = match (change.operation, body) {
                (ChangeOperation::Upsert, Some(body)) => ResourceState::Present { body },
                (ChangeOperation::Delete, None) => ResourceState::Deleted,
                _ => {
                    return Err(DomainError::StorageFailure {
                        operation: "materialized_change_decode",
                        detail: "change operation and stored state are inconsistent".to_owned(),
                    });
                }
            };
            entries.push(MaterializedChange {
                resource: CanonicalResource {
                    scope: scope.clone(),
                    resource: change.resource.clone(),
                    revision: change.revision,
                    state,
                },
                change,
            });
        }
        let has_more = entries.len() > limit;
        if has_more {
            entries.truncate(limit);
        }
        tx.commit()
            .map_err(|error| storage("materialized_changes_commit", error))?;
        MaterializedChangeBatch::new(scope.clone(), cursor.clone(), entries, has_more)
    }

    fn snapshot(&self, scope: &ProjectScope) -> Result<ProjectSnapshot, DomainError> {
        let mut client = self.lock_client()?;
        let mut tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .map_err(|error| storage("snapshot_begin", error))?;
        let Some(project) = tx
            .query_opt(
                "SELECT project_epoch, next_seq FROM ssot_projects
                 WHERE tenant_id = $1 AND project_id = $2",
                &[&scope.tenant_id.as_str(), &scope.project_id.as_str()],
            )
            .map_err(|error| storage("snapshot_project", error))?
        else {
            return Err(DomainError::ProjectUnauthorized {
                project_id: scope.project_id.clone(),
            });
        };
        let project_epoch = ProjectEpoch::try_from(project.get::<_, String>(0))?;
        let at_seq = project_sequence(project.get(1), "snapshot_sequence")?;
        let fetch_limit = MAX_SNAPSHOT_RESOURCES
            .checked_add(1)
            .ok_or_else(|| DomainError::InvalidChangeBatch("snapshot limit overflow".to_owned()))?;
        let rows = tx
            .query(
                "SELECT resource_kind, resource_id, revision, deleted, body_json
                 FROM ssot_resources
                 WHERE tenant_id = $1 AND project_id = $2
                 ORDER BY resource_kind ASC, resource_id ASC LIMIT $3",
                &[
                    &scope.tenant_id.as_str(),
                    &scope.project_id.as_str(),
                    &to_i64(fetch_limit as u64, "snapshot_limit")?,
                ],
            )
            .map_err(|error| storage("snapshot_query", error))?;
        if rows.len() > MAX_SNAPSHOT_RESOURCES {
            return Err(DomainError::SnapshotTooLarge {
                limit: MAX_SNAPSHOT_RESOURCES,
            });
        }
        let resources = rows
            .iter()
            .map(|row| decode_resource(scope, row))
            .collect::<Result<Vec<_>, DomainError>>()?;
        tx.commit()
            .map_err(|error| storage("snapshot_commit", error))?;
        ProjectSnapshot::new(scope.clone(), project_epoch, at_seq, resources)
    }

    fn resource(
        &self,
        scope: &ProjectScope,
        resource: &ResourceRef,
    ) -> Result<Option<CanonicalResource>, DomainError> {
        let mut client = self.lock_client()?;
        let row = client
            .query_opt(
                "SELECT resource_kind, resource_id, revision, deleted, body_json
                 FROM ssot_resources
                 WHERE tenant_id = $1 AND project_id = $2
                   AND resource_kind = $3 AND resource_id = $4",
                &[
                    &scope.tenant_id.as_str(),
                    &scope.project_id.as_str(),
                    &resource.kind.as_str(),
                    &resource.id.as_str(),
                ],
            )
            .map_err(|error| storage("resource_state_read", error))?;
        row.as_ref()
            .map(|row| decode_resource(scope, row))
            .transpose()
    }
}

impl HandoffStore for PostgresCommandStore {
    fn handoffs(
        &self,
        scope: &ProjectScope,
        actor_id: &ActorId,
        query: &HandoffQuery,
    ) -> Result<HandoffPage, DomainError> {
        let fetch_limit = query.limit().checked_add(1).ok_or_else(|| {
            DomainError::InvalidCommand("handoff mailbox limit overflow".to_owned())
        })?;
        let actor_column = match query.mailbox() {
            HandoffMailbox::Inbox => "to_actor",
            HandoffMailbox::Outbox => "from_actor",
        };
        // `actor_column` is selected only from the closed enum above. All user
        // values remain PostgreSQL bind parameters.
        let sql = format!(
            "SELECT idx.updated_seq, idx.revision, idx.handoff_id,
                    idx.from_actor, idx.to_actor, idx.source_id, idx.status,
                    resource.revision, resource.body_json
             FROM ssot_handoff_index AS idx
             JOIN ssot_resources AS resource
               ON resource.tenant_id = idx.tenant_id
              AND resource.project_id = idx.project_id
              AND resource.resource_kind = 'handoff'
              AND resource.resource_id = idx.handoff_id
             WHERE idx.tenant_id = $1 AND idx.project_id = $2
               AND idx.{actor_column} = $3
               AND ($4::TEXT IS NULL OR idx.source_id = $4)
               AND ($5 OR idx.status != 'completed')
               AND ($6::BIGINT IS NULL OR idx.updated_seq < $6)
               AND NOT resource.deleted
             ORDER BY idx.updated_seq DESC LIMIT $7"
        );
        let source_id = query.source_id().map(SourceId::as_str);
        let before_seq = query
            .before_seq()
            .map(|sequence| to_i64(sequence.get(), "handoff_before_seq"))
            .transpose()?;
        let mut client = self.lock_client()?;
        let rows = client
            .query(
                &sql,
                &[
                    &scope.tenant_id.as_str(),
                    &scope.project_id.as_str(),
                    &actor_id.as_str(),
                    &source_id,
                    &query.include_completed(),
                    &before_seq,
                    &to_i64(fetch_limit as u64, "handoff_limit")?,
                ],
            )
            .map_err(|error| storage("handoff_mailbox_query", error))?;
        let mut assignments = Vec::with_capacity(rows.len());
        for row in &rows {
            let body: Vec<u8> = row.get(8);
            let record: HandoffRecord =
                serde_json::from_slice(&body).map_err(|error| DomainError::StorageFailure {
                    operation: "handoff_mailbox_decode",
                    detail: error.to_string(),
                })?;
            validate_handoff_index(row, &record)?;
            assignments.push(HandoffListEntry {
                project_seq: project_sequence(row.get(0), "handoff_project_sequence")?,
                revision: revision(row.get(7), "handoff_revision")?,
                record,
            });
        }
        let has_more = assignments.len() > query.limit();
        if has_more {
            assignments.truncate(query.limit());
        }
        let next_before_seq = has_more
            .then(|| assignments.last().map(|entry| entry.project_seq))
            .flatten();
        Ok(HandoffPage {
            assignments,
            next_before_seq,
        })
    }
}

fn validate_command_shape(command: &MutationCommand) -> Result<(), DomainError> {
    match (command.change, command.resource_body.as_ref()) {
        (ChangeOperation::Upsert, Some(_)) | (ChangeOperation::Delete, None) => Ok(()),
        (ChangeOperation::Upsert, None) => Err(DomainError::InvalidCommand(
            "upsert requires a canonical resource body".to_owned(),
        )),
        (ChangeOperation::Delete, Some(_)) => Err(DomainError::InvalidCommand(
            "delete must not carry a resource body".to_owned(),
        )),
    }
}

fn validate_limit(limit: usize) -> Result<(), DomainError> {
    if limit == 0 || limit > MAX_CHANGE_LIMIT {
        Err(DomainError::InvalidChangeBatch(format!(
            "limit must be between 1 and {MAX_CHANGE_LIMIT}"
        )))
    } else {
        Ok(())
    }
}

fn validate_cursor<C: GenericClient>(
    client: &mut C,
    scope: &ProjectScope,
    cursor: &ChangeCursor,
) -> Result<(ProjectEpoch, ProjectSequence), DomainError> {
    let row = client
        .query_opt(
            "SELECT project_epoch, next_seq FROM ssot_projects
             WHERE tenant_id = $1 AND project_id = $2",
            &[&scope.tenant_id.as_str(), &scope.project_id.as_str()],
        )
        .map_err(|error| storage("cursor_project_read", error))?;
    let Some(row) = row else {
        return Err(DomainError::ProjectUnauthorized {
            project_id: scope.project_id.clone(),
        });
    };
    let epoch = ProjectEpoch::try_from(row.get::<_, String>(0))?;
    let current = project_sequence(row.get(1), "project_sequence")?;
    if cursor.project_epoch != epoch {
        return Err(DomainError::CursorEpochMismatch {
            cursor: cursor.project_epoch.clone(),
            current: epoch,
        });
    }
    if cursor.after_seq > current {
        return Err(DomainError::CursorOutOfRange {
            after_seq: cursor.after_seq,
            current,
        });
    }
    Ok((cursor.project_epoch.clone(), current))
}

fn load_resource_revision<C: GenericClient>(
    client: &mut C,
    scope: &ProjectScope,
    resource: &ResourceRef,
) -> Result<Option<Revision>, DomainError> {
    client
        .query_opt(
            "SELECT revision FROM ssot_resources
             WHERE tenant_id = $1 AND project_id = $2
               AND resource_kind = $3 AND resource_id = $4",
            &[
                &scope.tenant_id.as_str(),
                &scope.project_id.as_str(),
                &resource.kind.as_str(),
                &resource.id.as_str(),
            ],
        )
        .map_err(|error| storage("resource_revision_read", error))?
        .map(|row| revision(row.get(0), "resource_revision"))
        .transpose()
}

fn load_receipt<C: GenericClient>(
    client: &mut C,
    scope: &ProjectScope,
    command_id: &CommandId,
) -> Result<Option<CommandReceipt>, DomainError> {
    client
        .query_opt(
            "SELECT command_id, fingerprint, actor_id, project_seq,
                    resource_kind, resource_id, revision, committed_at_ms
             FROM ssot_receipts
             WHERE tenant_id = $1 AND project_id = $2 AND command_id = $3",
            &[
                &scope.tenant_id.as_str(),
                &scope.project_id.as_str(),
                &command_id.as_str(),
            ],
        )
        .map_err(|error| storage("receipt_read", error))?
        .as_ref()
        .map(|row| decode_receipt(scope, row))
        .transpose()
}

fn decode_receipt(scope: &ProjectScope, row: &Row) -> Result<CommandReceipt, DomainError> {
    Ok(CommandReceipt {
        command_id: CommandId::try_from(row.get::<_, String>(0))?,
        fingerprint: aidememo_domain::CommandFingerprint::try_from(row.get::<_, String>(1))?,
        tenant_id: scope.tenant_id.clone(),
        project_id: scope.project_id.clone(),
        actor_id: ActorId::try_from(row.get::<_, String>(2))?,
        project_seq: project_sequence(row.get(3), "receipt_project_sequence")?,
        resource: ResourceRef {
            kind: ResourceKind::try_from(row.get::<_, String>(4))?,
            id: ResourceId::try_from(row.get::<_, String>(5))?,
        },
        revision: revision(row.get(6), "receipt_revision")?,
        committed_at_ms: row.get(7),
    })
}

fn decode_change(scope: &ProjectScope, row: &Row) -> Result<ChangeEntry, DomainError> {
    Ok(ChangeEntry {
        tenant_id: scope.tenant_id.clone(),
        project_id: scope.project_id.clone(),
        project_epoch: ProjectEpoch::try_from(row.get::<_, String>(0))?,
        seq: project_sequence(row.get(1), "change_sequence")?,
        resource: ResourceRef {
            kind: ResourceKind::try_from(row.get::<_, String>(2))?,
            id: ResourceId::try_from(row.get::<_, String>(3))?,
        },
        operation: parse_change_operation(&row.get::<_, String>(4))?,
        revision: revision(row.get(5), "change_revision")?,
        actor_id: ActorId::try_from(row.get::<_, String>(6))?,
        committed_at_ms: row.get(7),
    })
}

fn decode_resource(scope: &ProjectScope, row: &Row) -> Result<CanonicalResource, DomainError> {
    let resource = ResourceRef {
        kind: ResourceKind::try_from(row.get::<_, String>(0))?,
        id: ResourceId::try_from(row.get::<_, String>(1))?,
    };
    let revision = revision(row.get(2), "resource_revision")?;
    let deleted: bool = row.get(3);
    let body: Option<Vec<u8>> = row.get(4);
    let state = match (deleted, body) {
        (false, Some(body)) => ResourceState::Present { body },
        (true, None) => ResourceState::Deleted,
        _ => {
            return Err(DomainError::StorageFailure {
                operation: "resource_state_decode",
                detail: "resource deletion flag and body are inconsistent".to_owned(),
            });
        }
    };
    Ok(CanonicalResource {
        scope: scope.clone(),
        resource,
        revision,
        state,
    })
}

fn insert_receipt<C: GenericClient>(
    client: &mut C,
    receipt: &CommandReceipt,
) -> Result<(), DomainError> {
    client
        .execute(
            "INSERT INTO ssot_receipts
                (tenant_id, project_id, command_id, fingerprint, actor_id,
                 project_seq, resource_kind, resource_id, revision, committed_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            &[
                &receipt.tenant_id.as_str(),
                &receipt.project_id.as_str(),
                &receipt.command_id.as_str(),
                &receipt.fingerprint.as_str(),
                &receipt.actor_id.as_str(),
                &to_i64(receipt.project_seq.get(), "receipt_project_sequence")?,
                &receipt.resource.kind.as_str(),
                &receipt.resource.id.as_str(),
                &to_i64(receipt.revision.get(), "receipt_revision")?,
                &receipt.committed_at_ms,
            ],
        )
        .map_err(|error| storage("receipt_write", error))?;
    Ok(())
}

fn insert_change<C: GenericClient>(
    client: &mut C,
    receipt: &CommandReceipt,
    project_epoch: &str,
    operation: ChangeOperation,
    body: Option<&[u8]>,
) -> Result<(), DomainError> {
    client
        .execute(
            "INSERT INTO ssot_changes
                (tenant_id, project_id, project_epoch, seq, resource_kind,
                 resource_id, operation, revision, actor_id, committed_at_ms, body_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            &[
                &receipt.tenant_id.as_str(),
                &receipt.project_id.as_str(),
                &project_epoch,
                &to_i64(receipt.project_seq.get(), "change_sequence")?,
                &receipt.resource.kind.as_str(),
                &receipt.resource.id.as_str(),
                &change_operation_text(operation),
                &to_i64(receipt.revision.get(), "change_revision")?,
                &receipt.actor_id.as_str(),
                &receipt.committed_at_ms,
                &body,
            ],
        )
        .map_err(|error| storage("change_write", error))?;
    Ok(())
}

fn insert_audit<C: GenericClient>(
    client: &mut C,
    receipt: &CommandReceipt,
    operation: &str,
) -> Result<(), DomainError> {
    client
        .execute(
            "INSERT INTO ssot_audit
                (tenant_id, project_id, project_seq, command_id, operation,
                 resource_kind, resource_id, actor_id, committed_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                &receipt.tenant_id.as_str(),
                &receipt.project_id.as_str(),
                &to_i64(receipt.project_seq.get(), "audit_sequence")?,
                &receipt.command_id.as_str(),
                &operation,
                &receipt.resource.kind.as_str(),
                &receipt.resource.id.as_str(),
                &receipt.actor_id.as_str(),
                &receipt.committed_at_ms,
            ],
        )
        .map_err(|error| storage("audit_write", error))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn update_handoff_index(
    tx: &mut Transaction<'_>,
    scope: &ProjectScope,
    resource: &ResourceRef,
    change: ChangeOperation,
    body: Option<&[u8]>,
    revision: Revision,
    sequence: ProjectSequence,
) -> Result<(), DomainError> {
    if resource.kind.as_str() != "handoff" {
        return Ok(());
    }
    if change == ChangeOperation::Delete {
        tx.execute(
            "DELETE FROM ssot_handoff_index
             WHERE tenant_id = $1 AND project_id = $2 AND handoff_id = $3",
            &[
                &scope.tenant_id.as_str(),
                &scope.project_id.as_str(),
                &resource.id.as_str(),
            ],
        )
        .map_err(|error| storage("handoff_index_delete", error))?;
        return Ok(());
    }
    let body = body.ok_or_else(|| DomainError::StorageFailure {
        operation: "handoff_index_decode",
        detail: "handoff upsert is missing its canonical body".to_owned(),
    })?;
    let record: HandoffRecord =
        serde_json::from_slice(body).map_err(|error| DomainError::StorageFailure {
            operation: "handoff_index_decode",
            detail: error.to_string(),
        })?;
    if record.handoff_id.as_str() != resource.id.as_str() {
        return Err(DomainError::StorageFailure {
            operation: "handoff_index_decode",
            detail: "handoff body identity does not match its resource coordinate".to_owned(),
        });
    }
    tx.execute(
        "INSERT INTO ssot_handoff_index
            (tenant_id, project_id, handoff_id, resource_kind, from_actor,
             to_actor, source_id, status, revision, updated_seq)
         VALUES ($1, $2, $3, 'handoff', $4, $5, $6, $7, $8, $9)
         ON CONFLICT (tenant_id, project_id, handoff_id)
         DO UPDATE SET from_actor = EXCLUDED.from_actor,
                       to_actor = EXCLUDED.to_actor,
                       source_id = EXCLUDED.source_id,
                       status = EXCLUDED.status,
                       revision = EXCLUDED.revision,
                       updated_seq = EXCLUDED.updated_seq",
        &[
            &scope.tenant_id.as_str(),
            &scope.project_id.as_str(),
            &record.handoff_id.as_str(),
            &record.from_actor.as_str(),
            &record.to_actor.as_str(),
            &record.source_id.as_ref().map(SourceId::as_str),
            &handoff_status_text(record.status),
            &to_i64(revision.get(), "handoff_revision")?,
            &to_i64(sequence.get(), "handoff_project_sequence")?,
        ],
    )
    .map_err(|error| storage("handoff_index_write", error))?;
    Ok(())
}

fn validate_handoff_index(row: &Row, record: &HandoffRecord) -> Result<(), DomainError> {
    let index_revision: i64 = row.get(1);
    let handoff_id: String = row.get(2);
    let from_actor: String = row.get(3);
    let to_actor: String = row.get(4);
    let source_id: Option<String> = row.get(5);
    let status: String = row.get(6);
    let resource_revision: i64 = row.get(7);
    let consistent = index_revision == resource_revision
        && handoff_id == record.handoff_id.as_str()
        && from_actor == record.from_actor.as_str()
        && to_actor == record.to_actor.as_str()
        && source_id.as_deref() == record.source_id.as_ref().map(SourceId::as_str)
        && status == handoff_status_text(record.status);
    if consistent {
        Ok(())
    } else {
        Err(DomainError::StorageFailure {
            operation: "handoff_mailbox_decode",
            detail: "handoff index does not match canonical resource state".to_owned(),
        })
    }
}

fn parse_change_operation(value: &str) -> Result<ChangeOperation, DomainError> {
    match value {
        "upsert" => Ok(ChangeOperation::Upsert),
        "delete" => Ok(ChangeOperation::Delete),
        _ => Err(DomainError::StorageFailure {
            operation: "change_operation_decode",
            detail: format!("invalid change operation {value}"),
        }),
    }
}

const fn change_operation_text(operation: ChangeOperation) -> &'static str {
    match operation {
        ChangeOperation::Upsert => "upsert",
        ChangeOperation::Delete => "delete",
    }
}

const fn handoff_status_text(status: HandoffStatus) -> &'static str {
    match status {
        HandoffStatus::Pending => "pending",
        HandoffStatus::Accepted => "accepted",
        HandoffStatus::Completed => "completed",
    }
}

fn revision(value: i64, operation: &'static str) -> Result<Revision, DomainError> {
    Revision::new(from_i64(value, operation)?)
}

fn project_sequence(value: i64, operation: &'static str) -> Result<ProjectSequence, DomainError> {
    Ok(ProjectSequence::new(from_i64(value, operation)?))
}

fn next_sequence(current: ProjectSequence) -> Result<ProjectSequence, DomainError> {
    current
        .get()
        .checked_add(1)
        .map(ProjectSequence::new)
        .ok_or_else(|| DomainError::StorageFailure {
            operation: "project_sequence_overflow",
            detail: "project sequence overflow".to_owned(),
        })
}

fn to_i64(value: u64, operation: &'static str) -> Result<i64, DomainError> {
    i64::try_from(value).map_err(|error| storage(operation, error))
}

fn from_i64(value: i64, operation: &'static str) -> Result<u64, DomainError> {
    u64::try_from(value).map_err(|error| storage(operation, error))
}

fn unix_time_ms() -> Result<i64, DomainError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| storage("system_time", error))?;
    i64::try_from(duration.as_millis()).map_err(|error| storage("system_time", error))
}

fn storage(operation: &'static str, error: impl std::fmt::Display) -> DomainError {
    DomainError::StorageFailure {
        operation,
        detail: error.to_string(),
    }
}
