//! Single-node SQLite implementation of the portable SSOT command ledger.
//!
//! This database is intentionally separate from the current embedded
//! `aidememo-core` store. It proves atomic receipt, resource revision, audit,
//! and change-feed semantics without changing existing local file formats.

use aidememo_domain::{
    ActorId, ChangeBatch, ChangeCursor, ChangeEntry, ChangeOperation, CommandFingerprint,
    CommandId, CommandReceipt, CommandStore, DomainError, MutationCommand, OperationName,
    ProjectEpoch, ProjectId, ProjectScope, ProjectSequence, ResourceId, ResourceKind, ResourceRef,
    Revision, TenantId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{path::Path, time::Duration};

const SCHEMA_VERSION: i64 = 1;
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CHANGE_LIMIT: usize = 10_000;

/// Durable single-process SQLite command ledger.
pub struct SqliteCommandStore {
    connection: Connection,
}

impl SqliteCommandStore {
    /// Open or create a ledger at `path` and apply the current schema.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StorageFailure`] when SQLite cannot be opened,
    /// configured, or migrated.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DomainError> {
        let connection = Connection::open(path).map_err(|error| storage("open", error))?;
        Self::from_connection(connection)
    }

    /// Open an isolated in-memory ledger, primarily for service tests.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StorageFailure`] when SQLite cannot be configured
    /// or migrated.
    pub fn open_in_memory() -> Result<Self, DomainError> {
        let connection = Connection::open_in_memory().map_err(|error| storage("open", error))?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, DomainError> {
        connection
            .busy_timeout(DEFAULT_BUSY_TIMEOUT)
            .map_err(|error| storage("busy_timeout", error))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|error| storage("foreign_keys", error))?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| storage("journal_mode", error))?;
        migrate(&connection)?;
        Ok(Self { connection })
    }

    /// Register an empty canonical project history.
    ///
    /// Repeating the same scope and epoch is idempotent. Reusing the scope with
    /// another epoch fails so restore/reset remains an explicit administration
    /// operation.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StorageFailure`] for a conflicting epoch or an
    /// SQLite failure.
    pub fn initialize_project(
        &mut self,
        scope: &ProjectScope,
        epoch: &ProjectEpoch,
    ) -> Result<(), DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("initialize_begin", error))?;
        tx.execute(
            "INSERT INTO ssot_projects (tenant_id, project_id, project_epoch, next_seq)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT (tenant_id, project_id) DO NOTHING",
            params![
                scope.tenant_id.as_str(),
                scope.project_id.as_str(),
                epoch.as_str()
            ],
        )
        .map_err(|error| storage("initialize_insert", error))?;
        let current: String = tx
            .query_row(
                "SELECT project_epoch FROM ssot_projects
                 WHERE tenant_id = ?1 AND project_id = ?2",
                params![scope.tenant_id.as_str(), scope.project_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage("initialize_read", error))?;
        if current != epoch.as_str() {
            return Err(DomainError::StorageFailure {
                operation: "initialize_project",
                detail: "project already exists with a different epoch".to_owned(),
            });
        }
        tx.commit()
            .map_err(|error| storage("initialize_commit", error))
    }

    /// Count immutable audit records for one tenant-project scope.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StorageFailure`] when SQLite cannot read the
    /// audit table or the count is invalid.
    pub fn audit_count(&self, scope: &ProjectScope) -> Result<u64, DomainError> {
        let count: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM ssot_audit WHERE tenant_id = ?1 AND project_id = ?2",
                params![scope.tenant_id.as_str(), scope.project_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage("audit_count", error))?;
        u64::try_from(count).map_err(|error| storage("audit_count_decode", error))
    }

    fn execute_transaction(
        tx: &Transaction<'_>,
        command: &MutationCommand,
    ) -> Result<CommandReceipt, DomainError> {
        let authorization = command.command.authorization();
        let envelope = command.command.envelope();
        let scope = authorization.scope();

        let project = load_project(tx, &scope)?;
        let Some((_epoch, current_sequence)) = project else {
            return Err(DomainError::ProjectUnauthorized {
                project_id: scope.project_id,
            });
        };

        if let Some(row) = load_receipt(tx, &scope, &envelope.command_id)? {
            let receipt = row.decode()?;
            if receipt.fingerprint != command.fingerprint {
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

        tx.execute(
            "INSERT INTO ssot_resources
                (tenant_id, project_id, resource_kind, resource_id, revision, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (tenant_id, project_id, resource_kind, resource_id)
             DO UPDATE SET revision = excluded.revision, deleted = excluded.deleted",
            params![
                scope.tenant_id.as_str(),
                scope.project_id.as_str(),
                command.resource.kind.as_str(),
                command.resource.id.as_str(),
                to_i64(revision.get(), "resource_revision")?,
                i64::from(command.change == ChangeOperation::Delete),
            ],
        )
        .map_err(|error| storage("resource_write", error))?;

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
        insert_change(tx, &receipt, command.change)?;
        insert_audit(tx, &receipt, &envelope.operation)?;

        let updated = tx
            .execute(
                "UPDATE ssot_projects SET next_seq = ?3
                 WHERE tenant_id = ?1 AND project_id = ?2 AND next_seq = ?4",
                params![
                    scope.tenant_id.as_str(),
                    scope.project_id.as_str(),
                    to_i64(sequence.get(), "project_sequence")?,
                    to_i64(current_sequence.get(), "project_sequence")?,
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

impl CommandStore for SqliteCommandStore {
    fn execute(&mut self, command: &MutationCommand) -> Result<CommandReceipt, DomainError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("command_begin", error))?;
        let receipt = Self::execute_transaction(&tx, command)?;
        tx.commit()
            .map_err(|error| storage("command_commit", error))?;
        Ok(receipt)
    }

    fn changes(
        &self,
        scope: &ProjectScope,
        cursor: &ChangeCursor,
        limit: usize,
    ) -> Result<ChangeBatch, DomainError> {
        if limit == 0 || limit > MAX_CHANGE_LIMIT {
            return Err(DomainError::InvalidChangeBatch(format!(
                "limit must be between 1 and {MAX_CHANGE_LIMIT}"
            )));
        }
        let Some((epoch, current_sequence)) = load_project(&self.connection, scope)? else {
            return Err(DomainError::ProjectUnauthorized {
                project_id: scope.project_id.clone(),
            });
        };
        if cursor.project_epoch != epoch {
            return Err(DomainError::CursorEpochMismatch {
                cursor: cursor.project_epoch.clone(),
                current: epoch,
            });
        }
        if cursor.after_seq > current_sequence {
            return Err(DomainError::CursorOutOfRange {
                after_seq: cursor.after_seq,
                current: current_sequence,
            });
        }

        let fetch_limit = limit
            .checked_add(1)
            .ok_or_else(|| DomainError::InvalidChangeBatch("limit overflow".to_owned()))?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT project_epoch, seq, resource_kind, resource_id, operation,
                        revision, actor_id, committed_at_ms
                 FROM ssot_changes
                 WHERE tenant_id = ?1 AND project_id = ?2 AND seq > ?3
                 ORDER BY seq ASC LIMIT ?4",
            )
            .map_err(|error| storage("changes_prepare", error))?;
        let rows = statement
            .query_map(
                params![
                    scope.tenant_id.as_str(),
                    scope.project_id.as_str(),
                    to_i64(cursor.after_seq.get(), "cursor_sequence")?,
                    to_i64(fetch_limit as u64, "change_limit")?,
                ],
                |row| {
                    Ok(ChangeRow {
                        project_epoch: row.get(0)?,
                        seq: row.get(1)?,
                        resource_kind: row.get(2)?,
                        resource_id: row.get(3)?,
                        operation: row.get(4)?,
                        revision: row.get(5)?,
                        actor_id: row.get(6)?,
                        committed_at_ms: row.get(7)?,
                    })
                },
            )
            .map_err(|error| storage("changes_query", error))?;
        let mut decoded = Vec::with_capacity(fetch_limit);
        for row in rows {
            decoded.push(
                row.map_err(|error| storage("changes_row", error))?
                    .decode(scope)?,
            );
        }
        let has_more = decoded.len() > limit;
        if has_more {
            decoded.truncate(limit);
        }
        ChangeBatch::new(scope.clone(), cursor.clone(), decoded, has_more)
    }
}

fn migrate(connection: &Connection) -> Result<(), DomainError> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| storage("schema_version_read", error))?;
    if version > SCHEMA_VERSION {
        return Err(DomainError::StorageFailure {
            operation: "schema_version",
            detail: format!("database schema {version} is newer than supported {SCHEMA_VERSION}"),
        });
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS ssot_projects (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_epoch TEXT NOT NULL,
                next_seq INTEGER NOT NULL CHECK (next_seq >= 0),
                PRIMARY KEY (tenant_id, project_id)
             ) STRICT;
             CREATE TABLE IF NOT EXISTS ssot_resources (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
                PRIMARY KEY (tenant_id, project_id, resource_kind, resource_id),
                FOREIGN KEY (tenant_id, project_id)
                    REFERENCES ssot_projects (tenant_id, project_id)
             ) STRICT;
             CREATE TABLE IF NOT EXISTS ssot_receipts (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                command_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                project_seq INTEGER NOT NULL CHECK (project_seq > 0),
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                committed_at_ms INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, project_id, command_id),
                UNIQUE (tenant_id, project_id, project_seq),
                FOREIGN KEY (tenant_id, project_id)
                    REFERENCES ssot_projects (tenant_id, project_id)
             ) STRICT;
             CREATE TABLE IF NOT EXISTS ssot_changes (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_epoch TEXT NOT NULL,
                seq INTEGER NOT NULL CHECK (seq > 0),
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
                revision INTEGER NOT NULL CHECK (revision > 0),
                actor_id TEXT NOT NULL,
                committed_at_ms INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, project_id, seq),
                FOREIGN KEY (tenant_id, project_id)
                    REFERENCES ssot_projects (tenant_id, project_id)
             ) STRICT;
             CREATE TABLE IF NOT EXISTS ssot_audit (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_seq INTEGER NOT NULL CHECK (project_seq > 0),
                command_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                committed_at_ms INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, project_id, project_seq),
                UNIQUE (tenant_id, project_id, command_id),
                FOREIGN KEY (tenant_id, project_id)
                    REFERENCES ssot_projects (tenant_id, project_id)
             ) STRICT;
             PRAGMA user_version = 1;",
        )
        .map_err(|error| storage("schema_migrate", error))
}

fn load_project(
    connection: &Connection,
    scope: &ProjectScope,
) -> Result<Option<(ProjectEpoch, ProjectSequence)>, DomainError> {
    let row: Option<(String, i64)> = connection
        .query_row(
            "SELECT project_epoch, next_seq FROM ssot_projects
             WHERE tenant_id = ?1 AND project_id = ?2",
            params![scope.tenant_id.as_str(), scope.project_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| storage("project_read", error))?;
    row.map(|(epoch, sequence)| {
        Ok((
            ProjectEpoch::try_from(epoch)?,
            ProjectSequence::new(from_i64(sequence, "project_sequence")?),
        ))
    })
    .transpose()
}

fn load_resource_revision(
    tx: &Transaction<'_>,
    scope: &ProjectScope,
    resource: &ResourceRef,
) -> Result<Option<Revision>, DomainError> {
    let value: Option<i64> = tx
        .query_row(
            "SELECT revision FROM ssot_resources
             WHERE tenant_id = ?1 AND project_id = ?2
               AND resource_kind = ?3 AND resource_id = ?4",
            params![
                scope.tenant_id.as_str(),
                scope.project_id.as_str(),
                resource.kind.as_str(),
                resource.id.as_str(),
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage("resource_read", error))?;
    value
        .map(|revision| Revision::new(from_i64(revision, "resource_revision")?))
        .transpose()
}

fn insert_receipt(tx: &Transaction<'_>, receipt: &CommandReceipt) -> Result<(), DomainError> {
    tx.execute(
        "INSERT INTO ssot_receipts
            (tenant_id, project_id, command_id, fingerprint, actor_id, project_seq,
             resource_kind, resource_id, revision, committed_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            receipt.tenant_id.as_str(),
            receipt.project_id.as_str(),
            receipt.command_id.as_str(),
            receipt.fingerprint.as_str(),
            receipt.actor_id.as_str(),
            to_i64(receipt.project_seq.get(), "project_sequence")?,
            receipt.resource.kind.as_str(),
            receipt.resource.id.as_str(),
            to_i64(receipt.revision.get(), "resource_revision")?,
            receipt.committed_at_ms,
        ],
    )
    .map_err(|error| storage("receipt_write", error))?;
    Ok(())
}

fn insert_change(
    tx: &Transaction<'_>,
    receipt: &CommandReceipt,
    change: ChangeOperation,
) -> Result<(), DomainError> {
    let epoch: String = tx
        .query_row(
            "SELECT project_epoch FROM ssot_projects
             WHERE tenant_id = ?1 AND project_id = ?2",
            params![receipt.tenant_id.as_str(), receipt.project_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage("change_epoch_read", error))?;
    tx.execute(
        "INSERT INTO ssot_changes
            (tenant_id, project_id, project_epoch, seq, resource_kind, resource_id,
             operation, revision, actor_id, committed_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            receipt.tenant_id.as_str(),
            receipt.project_id.as_str(),
            epoch,
            to_i64(receipt.project_seq.get(), "project_sequence")?,
            receipt.resource.kind.as_str(),
            receipt.resource.id.as_str(),
            change_text(change),
            to_i64(receipt.revision.get(), "resource_revision")?,
            receipt.actor_id.as_str(),
            receipt.committed_at_ms,
        ],
    )
    .map_err(|error| storage("change_write", error))?;
    Ok(())
}

fn insert_audit(
    tx: &Transaction<'_>,
    receipt: &CommandReceipt,
    operation: &OperationName,
) -> Result<(), DomainError> {
    tx.execute(
        "INSERT INTO ssot_audit
            (tenant_id, project_id, project_seq, command_id, operation,
             resource_kind, resource_id, actor_id, committed_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            receipt.tenant_id.as_str(),
            receipt.project_id.as_str(),
            to_i64(receipt.project_seq.get(), "project_sequence")?,
            receipt.command_id.as_str(),
            operation.as_str(),
            receipt.resource.kind.as_str(),
            receipt.resource.id.as_str(),
            receipt.actor_id.as_str(),
            receipt.committed_at_ms,
        ],
    )
    .map_err(|error| storage("audit_write", error))?;
    Ok(())
}

fn load_receipt(
    tx: &Transaction<'_>,
    scope: &ProjectScope,
    command_id: &CommandId,
) -> Result<Option<ReceiptRow>, DomainError> {
    tx.query_row(
        "SELECT command_id, fingerprint, tenant_id, project_id, actor_id, project_seq,
                resource_kind, resource_id, revision, committed_at_ms
         FROM ssot_receipts
         WHERE tenant_id = ?1 AND project_id = ?2 AND command_id = ?3",
        params![
            scope.tenant_id.as_str(),
            scope.project_id.as_str(),
            command_id.as_str()
        ],
        |row| {
            Ok(ReceiptRow {
                command_id: row.get(0)?,
                fingerprint: row.get(1)?,
                tenant_id: row.get(2)?,
                project_id: row.get(3)?,
                actor_id: row.get(4)?,
                project_seq: row.get(5)?,
                resource_kind: row.get(6)?,
                resource_id: row.get(7)?,
                revision: row.get(8)?,
                committed_at_ms: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(|error| storage("receipt_read", error))
}

struct ReceiptRow {
    command_id: String,
    fingerprint: String,
    tenant_id: String,
    project_id: String,
    actor_id: String,
    project_seq: i64,
    resource_kind: String,
    resource_id: String,
    revision: i64,
    committed_at_ms: i64,
}

impl ReceiptRow {
    fn decode(self) -> Result<CommandReceipt, DomainError> {
        Ok(CommandReceipt {
            command_id: CommandId::try_from(self.command_id)?,
            fingerprint: CommandFingerprint::try_from(self.fingerprint)?,
            tenant_id: TenantId::try_from(self.tenant_id)?,
            project_id: ProjectId::try_from(self.project_id)?,
            actor_id: ActorId::try_from(self.actor_id)?,
            project_seq: ProjectSequence::new(from_i64(self.project_seq, "project_sequence")?),
            resource: ResourceRef {
                kind: ResourceKind::try_from(self.resource_kind)?,
                id: ResourceId::try_from(self.resource_id)?,
            },
            revision: Revision::new(from_i64(self.revision, "resource_revision")?)?,
            committed_at_ms: self.committed_at_ms,
        })
    }
}

struct ChangeRow {
    project_epoch: String,
    seq: i64,
    resource_kind: String,
    resource_id: String,
    operation: String,
    revision: i64,
    actor_id: String,
    committed_at_ms: i64,
}

impl ChangeRow {
    fn decode(self, scope: &ProjectScope) -> Result<ChangeEntry, DomainError> {
        Ok(ChangeEntry {
            tenant_id: scope.tenant_id.clone(),
            project_id: scope.project_id.clone(),
            project_epoch: ProjectEpoch::try_from(self.project_epoch)?,
            seq: ProjectSequence::new(from_i64(self.seq, "change_sequence")?),
            resource: ResourceRef {
                kind: ResourceKind::try_from(self.resource_kind)?,
                id: ResourceId::try_from(self.resource_id)?,
            },
            operation: parse_change(&self.operation)?,
            revision: Revision::new(from_i64(self.revision, "resource_revision")?)?,
            actor_id: ActorId::try_from(self.actor_id)?,
            committed_at_ms: self.committed_at_ms,
        })
    }
}

fn change_text(change: ChangeOperation) -> &'static str {
    match change {
        ChangeOperation::Upsert => "upsert",
        ChangeOperation::Delete => "delete",
    }
}

fn parse_change(value: &str) -> Result<ChangeOperation, DomainError> {
    match value {
        "upsert" => Ok(ChangeOperation::Upsert),
        "delete" => Ok(ChangeOperation::Delete),
        other => Err(DomainError::StorageFailure {
            operation: "change_decode",
            detail: format!("unknown change operation '{other}'"),
        }),
    }
}

fn next_sequence(current: ProjectSequence) -> Result<ProjectSequence, DomainError> {
    current
        .get()
        .checked_add(1)
        .map(ProjectSequence::new)
        .ok_or_else(|| DomainError::StorageFailure {
            operation: "project_sequence",
            detail: "overflow".to_owned(),
        })
}

fn unix_time_ms() -> Result<i64, DomainError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| storage("system_time", error))?;
    i64::try_from(duration.as_millis()).map_err(|error| storage("system_time", error))
}

fn to_i64(value: u64, operation: &'static str) -> Result<i64, DomainError> {
    i64::try_from(value).map_err(|error| storage(operation, error))
}

fn from_i64(value: i64, operation: &'static str) -> Result<u64, DomainError> {
    u64::try_from(value).map_err(|error| storage(operation, error))
}

fn storage(operation: &'static str, error: impl std::fmt::Display) -> DomainError {
    DomainError::StorageFailure {
        operation,
        detail: error.to_string(),
    }
}
