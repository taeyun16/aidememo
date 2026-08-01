//! Single-node SQLite implementation of the portable SSOT command ledger.
//!
//! This database is intentionally separate from the current embedded
//! `aidememo-core` store. It proves atomic receipt, resource revision, audit,
//! and change-feed semantics without changing existing local file formats.

use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, AuthenticatedActor, CanonicalResource, ChangeBatch,
    ChangeCursor, ChangeEntry, ChangeOperation, CommandFingerprint, CommandId, CommandReceipt,
    CommandStore, DomainError, MembershipRole, MembershipStatus, MutationCommand, OperationName,
    ProjectEpoch, ProjectId, ProjectMembership, ProjectRecord, ProjectScope, ProjectSequence,
    RecordStatus, ResourceId, ResourceKind, ResourceRef, ResourceState, Revision, TenantId,
    TenantRecord,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{path::Path, time::Duration};

const SCHEMA_VERSION: i64 = 2;
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

    /// Return the SQLite schema version used by health endpoints.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StorageFailure`] when SQLite cannot read the
    /// schema pragma.
    pub fn schema_version(&self) -> Result<u32, DomainError> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| storage("schema_version_read", error))?;
        u32::try_from(version).map_err(|error| storage("schema_version_decode", error))
    }

    /// Return the current epoch for an existing project scope.
    ///
    /// This is primarily used by retry-safe bootstrap tooling so omitting an
    /// explicit epoch does not accidentally attempt to reset an existing
    /// project.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StorageFailure`] when SQLite cannot read the
    /// project row.
    pub fn project_epoch(&self, scope: &ProjectScope) -> Result<Option<ProjectEpoch>, DomainError> {
        Ok(load_project(&self.connection, scope)?.map(|(epoch, _)| epoch))
    }

    /// Atomically bootstrap an idempotent tenant and project pair.
    ///
    /// Existing immutable identity, epoch, status, and revision fields must
    /// match. Labels and timestamps from the first successful bootstrap are
    /// retained so a later retry remains idempotent. Administrative changes
    /// belong to revisioned commands rather than this bootstrap path.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidCommand`] for inconsistent records or
    /// [`DomainError::StorageFailure`] for conflicts and SQLite failures.
    pub fn bootstrap_project(
        &mut self,
        tenant: &TenantRecord,
        project: &ProjectRecord,
    ) -> Result<(), DomainError> {
        validate_tenant_project(tenant, project)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("bootstrap_project_begin", error))?;
        tx.execute(
            "INSERT INTO ssot_tenants
                (tenant_id, display_name, status, revision, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (tenant_id) DO NOTHING",
            params![
                tenant.tenant_id.as_str(),
                tenant.display_name,
                record_status_text(tenant.status),
                to_i64(tenant.revision.get(), "tenant_revision")?,
                tenant.created_at_ms,
                tenant.updated_at_ms,
            ],
        )
        .map_err(|error| storage("tenant_bootstrap_write", error))?;
        ensure_tenant_matches(&tx, tenant)?;

        tx.execute(
            "INSERT INTO ssot_projects
                (tenant_id, project_id, project_epoch, next_seq, display_name,
                 status, revision, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (tenant_id, project_id) DO NOTHING",
            params![
                project.tenant_id.as_str(),
                project.project_id.as_str(),
                project.project_epoch.as_str(),
                project.display_name,
                record_status_text(project.status),
                to_i64(project.revision.get(), "project_revision")?,
                project.created_at_ms,
                project.updated_at_ms,
            ],
        )
        .map_err(|error| storage("project_bootstrap_write", error))?;
        tx.execute(
            "UPDATE ssot_projects
             SET display_name = ?4, status = ?5, revision = ?6,
                 created_at_ms = ?7, updated_at_ms = ?8
             WHERE tenant_id = ?1 AND project_id = ?2 AND project_epoch = ?3
               AND display_name = '' AND status = 'active' AND revision = 1
               AND created_at_ms = 0 AND updated_at_ms = 0",
            params![
                project.tenant_id.as_str(),
                project.project_id.as_str(),
                project.project_epoch.as_str(),
                project.display_name,
                record_status_text(project.status),
                to_i64(project.revision.get(), "project_revision")?,
                project.created_at_ms,
                project.updated_at_ms,
            ],
        )
        .map_err(|error| storage("project_bootstrap_adopt", error))?;
        ensure_project_matches(&tx, project)?;
        tx.commit()
            .map_err(|error| storage("bootstrap_project_commit", error))
    }

    /// Provision an actor, project membership, and SHA-256 bearer-token digest.
    ///
    /// Token plaintext must be hashed by the caller and is never accepted by
    /// this storage API. Retry-time labels and timestamps are ignored after
    /// first creation; conflicting actor kind/status/revision, membership, or
    /// token binding fails closed.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidCommand`] for inconsistent scope or token
    /// length and [`DomainError::StorageFailure`] for conflicts or SQLite
    /// failures.
    pub fn provision_actor(
        &mut self,
        actor: &ActorRecord,
        membership: &ProjectMembership,
        token_sha256: &[u8],
        created_at_ms: i64,
    ) -> Result<(), DomainError> {
        validate_actor_membership(actor, membership, token_sha256)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("provision_actor_begin", error))?;
        ensure_active_tenant_project(&tx, &membership.tenant_id, &membership.project_id)?;
        tx.execute(
            "INSERT INTO ssot_actors
                (tenant_id, actor_id, display_name, kind, status, revision,
                 created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (tenant_id, actor_id) DO NOTHING",
            params![
                actor.tenant_id.as_str(),
                actor.actor_id.as_str(),
                actor.display_name,
                actor_kind_text(actor.kind),
                record_status_text(actor.status),
                to_i64(actor.revision.get(), "actor_revision")?,
                actor.created_at_ms,
                actor.updated_at_ms,
            ],
        )
        .map_err(|error| storage("actor_provision_write", error))?;
        ensure_actor_matches(&tx, actor)?;
        tx.execute(
            "INSERT INTO ssot_memberships
                (tenant_id, project_id, actor_id, role, status)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (tenant_id, project_id, actor_id) DO NOTHING",
            params![
                membership.tenant_id.as_str(),
                membership.project_id.as_str(),
                membership.actor_id.as_str(),
                membership_role_text(membership.role),
                membership_status_text(membership.status),
            ],
        )
        .map_err(|error| storage("membership_provision_write", error))?;
        ensure_membership_matches(&tx, membership)?;
        tx.execute(
            "INSERT INTO ssot_token_bindings
                (token_sha256, tenant_id, actor_id, active, created_at_ms)
             VALUES (?1, ?2, ?3, 1, ?4)
             ON CONFLICT (token_sha256) DO NOTHING",
            params![
                token_sha256,
                actor.tenant_id.as_str(),
                actor.actor_id.as_str(),
                created_at_ms,
            ],
        )
        .map_err(|error| storage("token_provision_write", error))?;
        ensure_token_matches(&tx, token_sha256, actor)?;
        tx.commit()
            .map_err(|error| storage("provision_actor_commit", error))
    }

    /// Resolve an active bearer-token digest to authenticated server identity.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidCommand`] when the digest is not SHA-256
    /// length or [`DomainError::StorageFailure`] when SQLite cannot read it.
    pub fn authenticate_token(
        &self,
        token_sha256: &[u8],
    ) -> Result<Option<AuthenticatedActor>, DomainError> {
        if token_sha256.len() != 32 {
            return Err(DomainError::InvalidCommand(
                "bearer token digest must contain 32 bytes".to_owned(),
            ));
        }
        let row: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT binding.tenant_id, binding.actor_id
                 FROM ssot_token_bindings AS binding
                 JOIN ssot_tenants AS tenant
                   ON tenant.tenant_id = binding.tenant_id
                 JOIN ssot_actors AS actor
                   ON actor.tenant_id = binding.tenant_id
                  AND actor.actor_id = binding.actor_id
                 WHERE binding.token_sha256 = ?1 AND binding.active = 1
                   AND tenant.status = 'active' AND actor.status = 'active'",
                params![token_sha256],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage("token_authenticate", error))?;
        row.map(|(tenant_id, actor_id)| {
            Ok(AuthenticatedActor::new(
                TenantId::try_from(tenant_id)?,
                ActorId::try_from(actor_id)?,
            ))
        })
        .transpose()
    }

    /// Load active project membership for authenticated identity.
    ///
    /// Suspended tenant, project, actor, or membership records are not returned.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StorageFailure`] when SQLite cannot read or decode
    /// the membership.
    pub fn membership(
        &self,
        authenticated: &AuthenticatedActor,
        project_id: &ProjectId,
    ) -> Result<Option<ProjectMembership>, DomainError> {
        let row: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT membership.role, membership.status
                 FROM ssot_memberships AS membership
                 JOIN ssot_tenants AS tenant
                   ON tenant.tenant_id = membership.tenant_id
                 JOIN ssot_projects AS project
                   ON project.tenant_id = membership.tenant_id
                  AND project.project_id = membership.project_id
                 JOIN ssot_actors AS actor
                   ON actor.tenant_id = membership.tenant_id
                  AND actor.actor_id = membership.actor_id
                 WHERE membership.tenant_id = ?1 AND membership.project_id = ?2
                   AND membership.actor_id = ?3 AND membership.status = 'active'
                   AND tenant.status = 'active' AND project.status = 'active'
                   AND actor.status = 'active'",
                params![
                    authenticated.tenant_id().as_str(),
                    project_id.as_str(),
                    authenticated.actor_id().as_str(),
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage("membership_read", error))?;
        row.map(|(role, status)| {
            Ok(ProjectMembership {
                tenant_id: authenticated.tenant_id().clone(),
                project_id: project_id.clone(),
                actor_id: authenticated.actor_id().clone(),
                role: parse_membership_role(&role)?,
                status: parse_membership_status(&status)?,
            })
        })
        .transpose()
    }

    fn execute_transaction(
        tx: &Transaction<'_>,
        command: &MutationCommand,
    ) -> Result<CommandReceipt, DomainError> {
        let authorization = command.command.authorization();
        let envelope = command.command.envelope();
        let scope = authorization.scope();

        match (command.change, command.resource_body.as_ref()) {
            (ChangeOperation::Upsert, Some(_)) | (ChangeOperation::Delete, None) => {}
            (ChangeOperation::Upsert, None) => {
                return Err(DomainError::InvalidCommand(
                    "upsert requires a canonical resource body".to_owned(),
                ));
            }
            (ChangeOperation::Delete, Some(_)) => {
                return Err(DomainError::InvalidCommand(
                    "delete must not carry a resource body".to_owned(),
                ));
            }
        }

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
                (tenant_id, project_id, resource_kind, resource_id, revision, deleted, body_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (tenant_id, project_id, resource_kind, resource_id)
             DO UPDATE SET revision = excluded.revision, deleted = excluded.deleted,
                           body_json = excluded.body_json",
            params![
                scope.tenant_id.as_str(),
                scope.project_id.as_str(),
                command.resource.kind.as_str(),
                command.resource.id.as_str(),
                to_i64(revision.get(), "resource_revision")?,
                i64::from(command.change == ChangeOperation::Delete),
                command.resource_body.as_deref(),
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

    fn resource(
        &self,
        scope: &ProjectScope,
        resource: &ResourceRef,
    ) -> Result<Option<CanonicalResource>, DomainError> {
        let row: Option<(i64, i64, Option<Vec<u8>>)> = self
            .connection
            .query_row(
                "SELECT revision, deleted, body_json FROM ssot_resources
                 WHERE tenant_id = ?1 AND project_id = ?2
                   AND resource_kind = ?3 AND resource_id = ?4",
                params![
                    scope.tenant_id.as_str(),
                    scope.project_id.as_str(),
                    resource.kind.as_str(),
                    resource.id.as_str(),
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| storage("resource_state_read", error))?;
        row.map(|(revision, deleted, body)| {
            let state = match (deleted, body) {
                (0, Some(body)) => ResourceState::Present { body },
                (1, None) => ResourceState::Deleted,
                _ => {
                    return Err(DomainError::StorageFailure {
                        operation: "resource_state_decode",
                        detail: "resource body and deletion marker are inconsistent".to_owned(),
                    });
                }
            };
            Ok(CanonicalResource {
                scope: scope.clone(),
                resource: resource.clone(),
                revision: Revision::new(from_i64(revision, "resource_revision")?)?,
                state,
            })
        })
        .transpose()
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
    match version {
        0 => create_schema_v2(connection),
        1 => migrate_v1_to_v2(connection),
        SCHEMA_VERSION => Ok(()),
        _ => Err(DomainError::StorageFailure {
            operation: "schema_version",
            detail: format!("unsupported database schema {version}"),
        }),
    }
}

fn create_schema_v2(connection: &Connection) -> Result<(), DomainError> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE ssot_tenants (
                tenant_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'archived')),
                revision INTEGER NOT NULL CHECK (revision > 0),
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
             ) STRICT;
             CREATE TABLE ssot_projects (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_epoch TEXT NOT NULL,
                next_seq INTEGER NOT NULL CHECK (next_seq >= 0),
                display_name TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active', 'suspended', 'archived')),
                revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
                created_at_ms INTEGER NOT NULL DEFAULT 0,
                updated_at_ms INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (tenant_id, project_id)
             ) STRICT;
             CREATE TABLE ssot_actors (
                tenant_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('human', 'agent', 'service')),
                status TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'archived')),
                revision INTEGER NOT NULL CHECK (revision > 0),
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, actor_id),
                FOREIGN KEY (tenant_id) REFERENCES ssot_tenants (tenant_id)
             ) STRICT;
             CREATE TABLE ssot_memberships (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'writer', 'reader')),
                status TEXT NOT NULL CHECK (status IN ('active', 'suspended')),
                PRIMARY KEY (tenant_id, project_id, actor_id),
                FOREIGN KEY (tenant_id, project_id)
                    REFERENCES ssot_projects (tenant_id, project_id),
                FOREIGN KEY (tenant_id, actor_id)
                    REFERENCES ssot_actors (tenant_id, actor_id)
             ) STRICT;
             CREATE TABLE ssot_token_bindings (
                token_sha256 BLOB PRIMARY KEY CHECK (length(token_sha256) = 32),
                tenant_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                active INTEGER NOT NULL CHECK (active IN (0, 1)),
                created_at_ms INTEGER NOT NULL,
                FOREIGN KEY (tenant_id, actor_id)
                    REFERENCES ssot_actors (tenant_id, actor_id)
             ) STRICT;
             CREATE TABLE ssot_resources (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
                body_json BLOB,
                CHECK ((deleted = 0 AND body_json IS NOT NULL)
                    OR (deleted = 1 AND body_json IS NULL)),
                PRIMARY KEY (tenant_id, project_id, resource_kind, resource_id),
                FOREIGN KEY (tenant_id, project_id)
                    REFERENCES ssot_projects (tenant_id, project_id)
             ) STRICT;
             CREATE TABLE ssot_receipts (
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
             CREATE TABLE ssot_changes (
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
             CREATE TABLE ssot_audit (
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
             PRAGMA user_version = 2;
             COMMIT;",
        )
        .map_err(|error| storage("schema_create", error))
}

fn migrate_v1_to_v2(connection: &Connection) -> Result<(), DomainError> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE ssot_projects ADD COLUMN display_name TEXT NOT NULL DEFAULT '';
             ALTER TABLE ssot_projects ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
                CHECK (status IN ('active', 'suspended', 'archived'));
             ALTER TABLE ssot_projects ADD COLUMN revision INTEGER NOT NULL DEFAULT 1
                CHECK (revision > 0);
             ALTER TABLE ssot_projects ADD COLUMN created_at_ms INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE ssot_projects ADD COLUMN updated_at_ms INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE ssot_resources ADD COLUMN body_json BLOB;
             UPDATE ssot_resources SET body_json = X'7B7D' WHERE deleted = 0;
             CREATE TABLE ssot_tenants (
                tenant_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'archived')),
                revision INTEGER NOT NULL CHECK (revision > 0),
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
             ) STRICT;
             CREATE TABLE ssot_actors (
                tenant_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('human', 'agent', 'service')),
                status TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'archived')),
                revision INTEGER NOT NULL CHECK (revision > 0),
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, actor_id),
                FOREIGN KEY (tenant_id) REFERENCES ssot_tenants (tenant_id)
             ) STRICT;
             CREATE TABLE ssot_memberships (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'writer', 'reader')),
                status TEXT NOT NULL CHECK (status IN ('active', 'suspended')),
                PRIMARY KEY (tenant_id, project_id, actor_id),
                FOREIGN KEY (tenant_id, project_id)
                    REFERENCES ssot_projects (tenant_id, project_id),
                FOREIGN KEY (tenant_id, actor_id)
                    REFERENCES ssot_actors (tenant_id, actor_id)
             ) STRICT;
             CREATE TABLE ssot_token_bindings (
                token_sha256 BLOB PRIMARY KEY CHECK (length(token_sha256) = 32),
                tenant_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                active INTEGER NOT NULL CHECK (active IN (0, 1)),
                created_at_ms INTEGER NOT NULL,
                FOREIGN KEY (tenant_id, actor_id)
                    REFERENCES ssot_actors (tenant_id, actor_id)
             ) STRICT;
             PRAGMA user_version = 2;
             COMMIT;",
        )
        .map_err(|error| storage("schema_migrate_v1_v2", error))
}

fn validate_tenant_project(
    tenant: &TenantRecord,
    project: &ProjectRecord,
) -> Result<(), DomainError> {
    if tenant.tenant_id != project.tenant_id {
        return Err(DomainError::InvalidCommand(
            "project tenant does not match tenant record".to_owned(),
        ));
    }
    validate_record_text("tenant display_name", &tenant.display_name)?;
    validate_record_text("project display_name", &project.display_name)?;
    validate_record_times("tenant", tenant.created_at_ms, tenant.updated_at_ms)?;
    validate_record_times("project", project.created_at_ms, project.updated_at_ms)
}

fn validate_actor_membership(
    actor: &ActorRecord,
    membership: &ProjectMembership,
    token_sha256: &[u8],
) -> Result<(), DomainError> {
    if actor.tenant_id != membership.tenant_id || actor.actor_id != membership.actor_id {
        return Err(DomainError::InvalidCommand(
            "actor identity does not match membership".to_owned(),
        ));
    }
    if token_sha256.len() != 32 {
        return Err(DomainError::InvalidCommand(
            "bearer token digest must contain 32 bytes".to_owned(),
        ));
    }
    validate_record_text("actor display_name", &actor.display_name)?;
    validate_record_times("actor", actor.created_at_ms, actor.updated_at_ms)
}

fn validate_record_text(name: &str, value: &str) -> Result<(), DomainError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(DomainError::InvalidCommand(format!(
            "{name} must contain 1 to 256 bytes and no control characters"
        )));
    }
    Ok(())
}

fn validate_record_times(name: &str, created: i64, updated: i64) -> Result<(), DomainError> {
    if created < 0 || updated < created {
        return Err(DomainError::InvalidCommand(format!(
            "{name} timestamps must be non-negative and updated_at must not precede created_at"
        )));
    }
    Ok(())
}

fn ensure_tenant_matches(tx: &Transaction<'_>, tenant: &TenantRecord) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM ssot_tenants
             WHERE tenant_id = ?1 AND status = ?2 AND revision = ?3",
            params![
                tenant.tenant_id.as_str(),
                record_status_text(tenant.status),
                to_i64(tenant.revision.get(), "tenant_revision")?,
            ],
            |row| row.get(0),
        )
        .map_err(|error| storage("tenant_bootstrap_read", error))?;
    ensure_exact_match(matched, "tenant_bootstrap", "tenant record conflicts")
}

fn ensure_project_matches(
    tx: &Transaction<'_>,
    project: &ProjectRecord,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM ssot_projects
             WHERE tenant_id = ?1 AND project_id = ?2 AND project_epoch = ?3
               AND status = ?4 AND revision = ?5",
            params![
                project.tenant_id.as_str(),
                project.project_id.as_str(),
                project.project_epoch.as_str(),
                record_status_text(project.status),
                to_i64(project.revision.get(), "project_revision")?,
            ],
            |row| row.get(0),
        )
        .map_err(|error| storage("project_bootstrap_read", error))?;
    ensure_exact_match(matched, "project_bootstrap", "project record conflicts")
}

fn ensure_actor_matches(tx: &Transaction<'_>, actor: &ActorRecord) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM ssot_actors
             WHERE tenant_id = ?1 AND actor_id = ?2
               AND kind = ?3 AND status = ?4 AND revision = ?5",
            params![
                actor.tenant_id.as_str(),
                actor.actor_id.as_str(),
                actor_kind_text(actor.kind),
                record_status_text(actor.status),
                to_i64(actor.revision.get(), "actor_revision")?,
            ],
            |row| row.get(0),
        )
        .map_err(|error| storage("actor_provision_read", error))?;
    ensure_exact_match(matched, "actor_provision", "actor record conflicts")
}

fn ensure_membership_matches(
    tx: &Transaction<'_>,
    membership: &ProjectMembership,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM ssot_memberships
             WHERE tenant_id = ?1 AND project_id = ?2 AND actor_id = ?3
               AND role = ?4 AND status = ?5",
            params![
                membership.tenant_id.as_str(),
                membership.project_id.as_str(),
                membership.actor_id.as_str(),
                membership_role_text(membership.role),
                membership_status_text(membership.status),
            ],
            |row| row.get(0),
        )
        .map_err(|error| storage("membership_provision_read", error))?;
    ensure_exact_match(
        matched,
        "membership_provision",
        "membership record conflicts",
    )
}

fn ensure_token_matches(
    tx: &Transaction<'_>,
    token_sha256: &[u8],
    actor: &ActorRecord,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM ssot_token_bindings
             WHERE token_sha256 = ?1 AND tenant_id = ?2 AND actor_id = ?3 AND active = 1",
            params![
                token_sha256,
                actor.tenant_id.as_str(),
                actor.actor_id.as_str()
            ],
            |row| row.get(0),
        )
        .map_err(|error| storage("token_provision_read", error))?;
    ensure_exact_match(matched, "token_provision", "token binding conflicts")
}

fn ensure_active_tenant_project(
    tx: &Transaction<'_>,
    tenant_id: &TenantId,
    project_id: &ProjectId,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM ssot_tenants AS tenant
             JOIN ssot_projects AS project ON project.tenant_id = tenant.tenant_id
             WHERE tenant.tenant_id = ?1 AND project.project_id = ?2
               AND tenant.status = 'active' AND project.status = 'active'",
            params![tenant_id.as_str(), project_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage("active_project_read", error))?;
    ensure_exact_match(
        matched,
        "active_project",
        "active tenant and project were not found",
    )
}

fn ensure_exact_match(
    matched: i64,
    operation: &'static str,
    detail: &'static str,
) -> Result<(), DomainError> {
    if matched == 1 {
        Ok(())
    } else {
        Err(DomainError::StorageFailure {
            operation,
            detail: detail.to_owned(),
        })
    }
}

fn record_status_text(status: RecordStatus) -> &'static str {
    match status {
        RecordStatus::Active => "active",
        RecordStatus::Suspended => "suspended",
        RecordStatus::Archived => "archived",
    }
}

fn actor_kind_text(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Human => "human",
        ActorKind::Agent => "agent",
        ActorKind::Service => "service",
    }
}

fn membership_role_text(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "owner",
        MembershipRole::Admin => "admin",
        MembershipRole::Writer => "writer",
        MembershipRole::Reader => "reader",
    }
}

fn parse_membership_role(value: &str) -> Result<MembershipRole, DomainError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "writer" => Ok(MembershipRole::Writer),
        "reader" => Ok(MembershipRole::Reader),
        other => Err(decode_error("membership role", other)),
    }
}

fn membership_status_text(status: MembershipStatus) -> &'static str {
    match status {
        MembershipStatus::Active => "active",
        MembershipStatus::Suspended => "suspended",
    }
}

fn parse_membership_status(value: &str) -> Result<MembershipStatus, DomainError> {
    match value {
        "active" => Ok(MembershipStatus::Active),
        "suspended" => Ok(MembershipStatus::Suspended),
        other => Err(decode_error("membership status", other)),
    }
}

fn decode_error(kind: &'static str, value: &str) -> DomainError {
    DomainError::StorageFailure {
        operation: "record_decode",
        detail: format!("unknown {kind} '{value}'"),
    }
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
