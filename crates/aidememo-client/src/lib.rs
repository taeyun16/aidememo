//! Authenticated SSOT transport and a separate local exact-read replica.
//!
//! The replica is intentionally not an `aidememo-core` store. It caches
//! canonical server resources and one ordered project cursor without opening,
//! migrating, or reinterpreting the embedded search database.

use aidememo_domain::{
    ActorId, CanonicalResource, ChangeCursor, ChangeEntry, DomainError, MaterializedChange,
    MaterializedChangeBatch, MembershipRole, ProjectEpoch, ProjectId, ProjectScope,
    ProjectSequence, ProjectSnapshot, ResourceRef, ResourceState, Revision, TenantId,
};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

const REPLICA_SCHEMA_VERSION: i64 = 1;
const MAX_CHANGE_LIMIT: usize = 1_000;
const MAX_PULL_BATCHES: usize = 10_000;
const PATH_SEGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'/').add(b'?').add(b'#').add(b'%');

/// Errors from authenticated transport or the isolated replica cache.
#[derive(Debug, Error)]
pub enum ClientError {
    /// Portable domain validation failed.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// A local replica database operation failed.
    #[error("replica storage operation '{operation}' failed: {detail}")]
    Storage {
        /// Stable operation label.
        operation: &'static str,
        /// Adapter diagnostic.
        detail: String,
    },
    /// A local filesystem operation failed.
    #[error("replica filesystem operation '{operation}' failed for {path}: {detail}")]
    Filesystem {
        /// Stable operation label.
        operation: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Adapter diagnostic.
        detail: String,
    },
    /// HTTP transport could not complete.
    #[error("remote transport failed: {0}")]
    Transport(String),
    /// The server returned a non-success response.
    #[error("remote server returned HTTP {status}: {message}")]
    Remote {
        /// HTTP status code.
        status: u16,
        /// Stable server error message when available.
        message: String,
    },
    /// A response could not be decoded or violated the replica protocol.
    #[error("invalid replica protocol response: {0}")]
    Protocol(String),
    /// This file already belongs to another tenant or project.
    #[error(
        "replica scope mismatch: cached {cached:?}, remote {remote:?}; reset explicitly before reusing this file"
    )]
    ScopeMismatch {
        /// Existing local scope.
        cached: ProjectScope,
        /// Authenticated remote scope.
        remote: ProjectScope,
    },
    /// The canonical project history was replaced or restored.
    #[error(
        "replica epoch mismatch: cached {cached}, remote {remote}; reset explicitly before pulling"
    )]
    EpochMismatch {
        /// Existing local epoch.
        cached: ProjectEpoch,
        /// Authenticated remote epoch.
        remote: ProjectEpoch,
    },
    /// A batch did not start at the durable local cursor.
    #[error("replica cursor mismatch: cached {cached}, batch starts at {batch}")]
    CursorMismatch {
        /// Durable local cursor.
        cached: ProjectSequence,
        /// Input batch cursor.
        batch: ProjectSequence,
    },
}

/// Named authenticated server route without exposing credentials in debug logs.
#[derive(Clone)]
pub struct RemoteProfile {
    /// Canonical server base URL without a trailing slash.
    pub url: String,
    /// Project selected by the profile.
    pub project_id: ProjectId,
    token: String,
}

impl fmt::Debug for RemoteProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteProfile")
            .field("url", &self.url)
            .field("project_id", &self.project_id)
            .field("token", &"[redacted]")
            .finish()
    }
}

impl RemoteProfile {
    /// Validate and construct an authenticated remote route.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Protocol`] for an unsupported URL or empty token.
    pub fn new(
        url: impl Into<String>,
        project_id: ProjectId,
        token: impl Into<String>,
    ) -> Result<Self, ClientError> {
        let url = url.into().trim().trim_end_matches('/').to_owned();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ClientError::Protocol(
                "remote URL must use http:// or https://".to_owned(),
            ));
        }
        let token = token.into();
        if token.trim().is_empty() || token.chars().any(char::is_whitespace) {
            return Err(ClientError::Protocol(
                "bearer token must be non-empty and contain no whitespace".to_owned(),
            ));
        }
        Ok(Self {
            url,
            project_id,
            token,
        })
    }
}

/// Authenticated project identity and replica history generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectIdentity {
    /// Server-derived tenant.
    pub tenant_id: TenantId,
    /// Profile-selected project.
    pub project_id: ProjectId,
    /// Current canonical history generation.
    pub project_epoch: ProjectEpoch,
    /// Server-derived actor.
    pub actor_id: ActorId,
    /// Current project membership role.
    pub role: MembershipRole,
}

impl ProjectIdentity {
    /// Tenant-project scope selected by this authenticated profile.
    #[must_use]
    pub fn scope(&self) -> ProjectScope {
        ProjectScope::new(self.tenant_id.clone(), self.project_id.clone())
    }
}

/// Authenticated blocking HTTP client for the replica protocol.
pub struct HttpReplicaClient {
    profile: RemoteProfile,
    agent: ureq::Agent,
}

impl HttpReplicaClient {
    /// Construct a client around a validated profile.
    #[must_use]
    pub fn new(profile: RemoteProfile) -> Self {
        Self {
            profile,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(5))
                .timeout_read(Duration::from_secs(30))
                .timeout_write(Duration::from_secs(30))
                .build(),
        }
    }

    /// Resolve bearer-derived identity and the current project epoch.
    ///
    /// # Errors
    ///
    /// Returns a transport, server, or decoding error.
    pub fn identity(&self) -> Result<ProjectIdentity, ClientError> {
        self.get_json(&format!(
            "/v1/projects/{}/identity",
            segment(self.profile.project_id.as_str())
        ))
    }

    /// Pull one validated revision-pinned change batch.
    ///
    /// # Errors
    ///
    /// Returns a transport, server, decoding, or change-validation error.
    pub fn materialized_changes(
        &self,
        cursor: &ChangeCursor,
        limit: usize,
    ) -> Result<MaterializedChangeBatch, ClientError> {
        if limit == 0 || limit > MAX_CHANGE_LIMIT {
            return Err(ClientError::Protocol(format!(
                "change limit must be between 1 and {MAX_CHANGE_LIMIT}"
            )));
        }
        let wire: MaterializedChangeResponseBatch = self.get_json(&format!(
            "/v1/projects/{}/changes/materialized?project_epoch={}&after_seq={}&limit={limit}",
            segment(self.profile.project_id.as_str()),
            segment(cursor.project_epoch.as_str()),
            cursor.after_seq.get(),
        ))?;
        let batch = MaterializedChangeBatch::try_from(wire)?;
        validate_materialized_batch(&batch)?;
        Ok(batch)
    }

    /// Fetch a complete current-state project snapshot and represented head.
    ///
    /// # Errors
    ///
    /// Returns a transport, server, decoding, or snapshot-validation error.
    pub fn snapshot(&self) -> Result<ProjectSnapshot, ClientError> {
        let wire: SnapshotResponse = self.get_json(&format!(
            "/v1/projects/{}/snapshot",
            segment(self.profile.project_id.as_str())
        ))?;
        let snapshot = ProjectSnapshot::try_from(wire)?;
        validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    /// Fetch current canonical state for one changed resource.
    ///
    /// # Errors
    ///
    /// Returns a transport, server, decoding, or resource-validation error.
    pub fn resource(&self, resource: &ResourceRef) -> Result<CanonicalResource, ClientError> {
        let wire: ResourceResponse = self.get_json(&format!(
            "/v1/projects/{}/resources/{}/{}",
            segment(self.profile.project_id.as_str()),
            segment(resource.kind.as_str()),
            segment(resource.id.as_str()),
        ))?;
        wire.try_into()
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, ClientError> {
        let response = self
            .agent
            .get(&format!("{}{path}", self.profile.url))
            .set("Authorization", &format!("Bearer {}", self.profile.token))
            .call();
        match response {
            Ok(response) => response
                .into_json::<T>()
                .map_err(|error| ClientError::Protocol(error.to_string())),
            Err(ureq::Error::Status(status, response)) => {
                let message = response
                    .into_json::<RemoteErrorResponse>()
                    .map(|body| body.error.message)
                    .unwrap_or_else(|_| "remote request failed".to_owned());
                Err(ClientError::Remote { status, message })
            }
            Err(ureq::Error::Transport(error)) => Err(ClientError::Transport(error.to_string())),
        }
    }
}

#[derive(Deserialize)]
struct RemoteErrorResponse {
    error: RemoteErrorDetail,
}

#[derive(Deserialize)]
struct RemoteErrorDetail {
    message: String,
}

#[derive(Deserialize)]
struct ResourceResponse {
    scope: ProjectScope,
    resource: ResourceRef,
    revision: Revision,
    state: ResourceResponseState,
}

#[derive(Deserialize)]
struct MaterializedChangeResponse {
    change: ChangeEntry,
    resource: ResourceResponse,
}

#[derive(Deserialize)]
struct MaterializedChangeResponseBatch {
    scope: ProjectScope,
    cursor: ChangeCursor,
    entries: Vec<MaterializedChangeResponse>,
    next_cursor: ChangeCursor,
    has_more: bool,
}

impl TryFrom<MaterializedChangeResponseBatch> for MaterializedChangeBatch {
    type Error = ClientError;

    fn try_from(batch: MaterializedChangeResponseBatch) -> Result<Self, Self::Error> {
        let entries = batch
            .entries
            .into_iter()
            .map(|entry| {
                Ok(MaterializedChange {
                    change: entry.change,
                    resource: CanonicalResource::try_from(entry.resource)?,
                })
            })
            .collect::<Result<Vec<_>, ClientError>>()?;
        let validated =
            MaterializedChangeBatch::new(batch.scope, batch.cursor, entries, batch.has_more)?;
        if validated.next_cursor != batch.next_cursor {
            return Err(ClientError::Protocol(
                "server next_cursor does not match the ordered entries".to_owned(),
            ));
        }
        Ok(validated)
    }
}

#[derive(Deserialize)]
struct SnapshotResponse {
    scope: ProjectScope,
    project_epoch: ProjectEpoch,
    at_seq: ProjectSequence,
    resources: Vec<ResourceResponse>,
}

impl TryFrom<SnapshotResponse> for ProjectSnapshot {
    type Error = ClientError;

    fn try_from(snapshot: SnapshotResponse) -> Result<Self, Self::Error> {
        Ok(ProjectSnapshot::new(
            snapshot.scope,
            snapshot.project_epoch,
            snapshot.at_seq,
            snapshot
                .resources
                .into_iter()
                .map(CanonicalResource::try_from)
                .collect::<Result<Vec<_>, ClientError>>()?,
        )?)
    }
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ResourceResponseState {
    Present { body: Value },
    Deleted,
}

impl TryFrom<ResourceResponse> for CanonicalResource {
    type Error = ClientError;

    fn try_from(response: ResourceResponse) -> Result<Self, Self::Error> {
        let state = match response.state {
            ResourceResponseState::Present { body } => ResourceState::Present {
                body: serde_json::to_vec(&body)
                    .map_err(|error| ClientError::Protocol(error.to_string()))?,
            },
            ResourceResponseState::Deleted => ResourceState::Deleted,
        };
        Ok(Self {
            scope: response.scope,
            resource: response.resource,
            revision: response.revision,
            state,
        })
    }
}

/// Durable state of one isolated project replica file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplicaStatus {
    /// Whether the file has authenticated project metadata.
    pub initialized: bool,
    /// Cached tenant-project scope.
    pub scope: Option<ProjectScope>,
    /// Cached history generation.
    pub project_epoch: Option<ProjectEpoch>,
    /// Last fully committed change sequence.
    pub after_seq: ProjectSequence,
    /// Present canonical resources.
    pub resource_count: u64,
    /// Durable deletion markers retained locally.
    pub tombstone_count: u64,
    /// Last successful metadata update.
    pub updated_at_ms: Option<i64>,
}

/// Summary of one bootstrap or incremental pull.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PullReport {
    /// True when this pull initialized an empty replica file.
    pub bootstrapped: bool,
    /// Number of committed server batches.
    pub batches: usize,
    /// Number of ordered change entries consumed.
    pub changes: usize,
    /// Final durable cursor.
    pub after_seq: ProjectSequence,
    /// Authenticated remote identity.
    pub identity: ProjectIdentity,
    /// Current local resource counts.
    pub resource_count: u64,
    /// Current local tombstone counts.
    pub tombstone_count: u64,
}

/// Separate SQLite cache for exact canonical resource reads.
pub struct ReplicaStore {
    connection: Connection,
}

impl ReplicaStore {
    /// Open or create a replica database without touching the embedded store.
    ///
    /// # Errors
    ///
    /// Returns a filesystem or SQLite migration error.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ClientError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| ClientError::Filesystem {
                operation: "create_replica_directory",
                path: parent.to_path_buf(),
                detail: error.to_string(),
            })?;
        }
        let connection = Connection::open(path).map_err(|error| storage("open", error))?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = WAL;
                 PRAGMA busy_timeout = 5000;",
            )
            .map_err(|error| storage("configure", error))?;
        migrate(&connection)?;
        Ok(Self { connection })
    }

    fn verify_identity(&self, identity: &ProjectIdentity) -> Result<bool, ClientError> {
        let remote_scope = identity.scope();
        if let Some(meta) = load_meta(&self.connection)? {
            if meta.scope != remote_scope {
                return Err(ClientError::ScopeMismatch {
                    cached: meta.scope,
                    remote: remote_scope,
                });
            }
            if meta.project_epoch != identity.project_epoch {
                return Err(ClientError::EpochMismatch {
                    cached: meta.project_epoch,
                    remote: identity.project_epoch.clone(),
                });
            }
            return Ok(false);
        }
        Ok(true)
    }

    /// Return the durable local cursor, if initialized.
    ///
    /// # Errors
    ///
    /// Returns a storage or record-decoding error.
    pub fn cursor(&self) -> Result<Option<ChangeCursor>, ClientError> {
        Ok(load_meta(&self.connection)?.map(|meta| ChangeCursor {
            project_epoch: meta.project_epoch,
            after_seq: meta.after_seq,
        }))
    }

    /// Atomically initialize an empty replica from one complete project snapshot.
    ///
    /// # Errors
    ///
    /// Fails without mutation when the snapshot is invalid or the replica is
    /// already initialized.
    pub fn apply_snapshot(&mut self, snapshot: &ProjectSnapshot) -> Result<(), ClientError> {
        validate_snapshot(snapshot)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| storage("snapshot_apply_begin", error))?;
        if load_meta_tx(&transaction)?.is_some() {
            return Err(ClientError::Protocol(
                "replica is already initialized; reset explicitly before applying a snapshot"
                    .to_owned(),
            ));
        }
        let updated_at_ms = now_ms()?;
        transaction
            .execute(
                "INSERT INTO replica_meta
                    (singleton, tenant_id, project_id, project_epoch, after_seq, updated_at_ms)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5)",
                params![
                    snapshot.scope.tenant_id.as_str(),
                    snapshot.scope.project_id.as_str(),
                    snapshot.project_epoch.as_str(),
                    to_i64(snapshot.at_seq.get(), "snapshot_cursor")?,
                    updated_at_ms,
                ],
            )
            .map_err(|error| storage("snapshot_meta_apply", error))?;
        for resource in &snapshot.resources {
            apply_resource(&transaction, resource, snapshot.at_seq)?;
        }
        transaction
            .commit()
            .map_err(|error| storage("snapshot_apply_commit", error))
    }

    /// Atomically apply one revision-pinned change batch and advance cursor.
    ///
    /// # Errors
    ///
    /// Fails without cursor advancement on invalid scope, epoch, sequence, or
    /// resource materialization.
    pub fn apply_materialized_batch(
        &mut self,
        batch: &MaterializedChangeBatch,
    ) -> Result<(), ClientError> {
        validate_materialized_batch(batch)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| storage("apply_begin", error))?;
        let meta = load_meta_tx(&transaction)?.ok_or_else(|| {
            ClientError::Protocol("replica must be prepared before applying changes".to_owned())
        })?;
        if meta.scope != batch.scope {
            return Err(ClientError::ScopeMismatch {
                cached: meta.scope,
                remote: batch.scope.clone(),
            });
        }
        if meta.project_epoch != batch.cursor.project_epoch {
            return Err(ClientError::EpochMismatch {
                cached: meta.project_epoch,
                remote: batch.cursor.project_epoch.clone(),
            });
        }
        if meta.after_seq != batch.cursor.after_seq {
            return Err(ClientError::CursorMismatch {
                cached: meta.after_seq,
                batch: batch.cursor.after_seq,
            });
        }

        for entry in &batch.entries {
            apply_resource(&transaction, &entry.resource, entry.change.seq)?;
        }
        transaction
            .execute(
                "UPDATE replica_meta SET after_seq = ?1, updated_at_ms = ?2 WHERE singleton = 1",
                params![
                    to_i64(batch.next_cursor.after_seq.get(), "cursor")?,
                    now_ms()?
                ],
            )
            .map_err(|error| storage("cursor_advance", error))?;
        transaction
            .commit()
            .map_err(|error| storage("apply_commit", error))
    }

    /// Read one cached canonical resource while the server is unavailable.
    ///
    /// # Errors
    ///
    /// Returns a storage or record-decoding error.
    pub fn resource(
        &self,
        resource: &ResourceRef,
    ) -> Result<Option<CanonicalResource>, ClientError> {
        let Some(meta) = load_meta(&self.connection)? else {
            return Ok(None);
        };
        let row: Option<(i64, i64, Option<Vec<u8>>)> = self
            .connection
            .query_row(
                "SELECT revision, deleted, body_json FROM replica_resources
                 WHERE resource_kind = ?1 AND resource_id = ?2",
                params![resource.kind.as_str(), resource.id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| storage("resource_read", error))?;
        row.map(|(revision, deleted, body)| {
            decode_resource(meta.scope, resource.clone(), revision, deleted, body)
        })
        .transpose()
    }

    /// Inspect local cache state without contacting the server.
    ///
    /// # Errors
    ///
    /// Returns a storage or record-decoding error.
    pub fn status(&self) -> Result<ReplicaStatus, ClientError> {
        let meta = load_meta(&self.connection)?;
        let (resource_count, tombstone_count): (i64, i64) = self
            .connection
            .query_row(
                "SELECT
                    COALESCE(SUM(CASE WHEN deleted = 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN deleted = 1 THEN 1 ELSE 0 END), 0)
                 FROM replica_resources",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| storage("status_counts", error))?;
        Ok(ReplicaStatus {
            initialized: meta.is_some(),
            scope: meta.as_ref().map(|meta| meta.scope.clone()),
            project_epoch: meta.as_ref().map(|meta| meta.project_epoch.clone()),
            after_seq: meta
                .as_ref()
                .map_or(ProjectSequence::ZERO, |meta| meta.after_seq),
            resource_count: from_i64(resource_count, "resource_count")?,
            tombstone_count: from_i64(tombstone_count, "tombstone_count")?,
            updated_at_ms: meta.map(|meta| meta.updated_at_ms),
        })
    }

    /// Explicitly clear project identity, cursor, and cached resources.
    ///
    /// # Errors
    ///
    /// Returns a storage error. The reset is one transaction.
    pub fn reset(&mut self) -> Result<(), ClientError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| storage("reset_begin", error))?;
        transaction
            .execute("DELETE FROM replica_resources", [])
            .map_err(|error| storage("reset_resources", error))?;
        transaction
            .execute("DELETE FROM replica_meta", [])
            .map_err(|error| storage("reset_meta", error))?;
        transaction
            .commit()
            .map_err(|error| storage("reset_commit", error))
    }
}

/// Bootstrap or incrementally catch up one replica to the remote change head.
///
/// An empty replica starts from one atomic snapshot. Incremental pulls apply
/// revision-pinned change bodies directly, so the local state never observes a
/// resource revision newer than its durable feed cursor. If the server is
/// unavailable or a response is invalid, the previous durable cursor remains
/// usable for offline reads.
///
/// # Errors
///
/// Returns a transport, server, protocol, or replica storage error.
pub fn pull_to_current(
    client: &HttpReplicaClient,
    store: &mut ReplicaStore,
    limit: usize,
) -> Result<PullReport, ClientError> {
    if limit == 0 || limit > MAX_CHANGE_LIMIT {
        return Err(ClientError::Protocol(format!(
            "change limit must be between 1 and {MAX_CHANGE_LIMIT}"
        )));
    }
    let identity = client.identity()?;
    let bootstrapped = store.verify_identity(&identity)?;
    if bootstrapped {
        let snapshot = client.snapshot()?;
        if snapshot.scope != identity.scope() {
            return Err(ClientError::ScopeMismatch {
                cached: identity.scope(),
                remote: snapshot.scope,
            });
        }
        if snapshot.project_epoch != identity.project_epoch {
            return Err(ClientError::EpochMismatch {
                cached: identity.project_epoch.clone(),
                remote: snapshot.project_epoch,
            });
        }
        store.apply_snapshot(&snapshot)?;
    }
    let mut batches = 0_usize;
    let mut changes = 0_usize;
    loop {
        if batches >= MAX_PULL_BATCHES {
            return Err(ClientError::Protocol(format!(
                "pull exceeded {MAX_PULL_BATCHES} batches without reaching the current head"
            )));
        }
        let cursor = store.cursor()?.ok_or_else(|| {
            ClientError::Protocol("prepared replica omitted its cursor".to_owned())
        })?;
        let batch = client.materialized_changes(&cursor, limit)?;
        if batch.scope != identity.scope() {
            return Err(ClientError::ScopeMismatch {
                cached: identity.scope(),
                remote: batch.scope,
            });
        }
        store.apply_materialized_batch(&batch)?;
        batches += 1;
        changes += batch.entries.len();
        if !batch.has_more {
            break;
        }
    }
    let status = store.status()?;
    Ok(PullReport {
        bootstrapped,
        batches,
        changes,
        after_seq: status.after_seq,
        identity,
        resource_count: status.resource_count,
        tombstone_count: status.tombstone_count,
    })
}

#[derive(Debug)]
struct ReplicaMeta {
    scope: ProjectScope,
    project_epoch: ProjectEpoch,
    after_seq: ProjectSequence,
    updated_at_ms: i64,
}

fn migrate(connection: &Connection) -> Result<(), ClientError> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| storage("schema_version", error))?;
    match version {
        0 => connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE replica_meta (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_epoch TEXT NOT NULL,
                    after_seq INTEGER NOT NULL CHECK (after_seq >= 0),
                    updated_at_ms INTEGER NOT NULL
                 ) STRICT;
                 CREATE TABLE replica_resources (
                    resource_kind TEXT NOT NULL,
                    resource_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0),
                    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
                    body_json BLOB,
                    observed_after_seq INTEGER NOT NULL CHECK (observed_after_seq >= 0),
                    CHECK ((deleted = 0 AND body_json IS NOT NULL)
                        OR (deleted = 1 AND body_json IS NULL)),
                    PRIMARY KEY (resource_kind, resource_id)
                 ) STRICT;
                 PRAGMA user_version = 1;
                 COMMIT;",
            )
            .map_err(|error| storage("schema_create", error)),
        REPLICA_SCHEMA_VERSION => Ok(()),
        other => Err(ClientError::Storage {
            operation: "schema_version",
            detail: format!(
                "replica schema {other} is not supported by version {REPLICA_SCHEMA_VERSION}"
            ),
        }),
    }
}

fn load_meta(connection: &Connection) -> Result<Option<ReplicaMeta>, ClientError> {
    let row: Option<(String, String, String, i64, i64)> = connection
        .query_row(
            "SELECT tenant_id, project_id, project_epoch, after_seq, updated_at_ms
             FROM replica_meta WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage("meta_read", error))?;
    row.map(decode_meta).transpose()
}

fn load_meta_tx(transaction: &Transaction<'_>) -> Result<Option<ReplicaMeta>, ClientError> {
    let row: Option<(String, String, String, i64, i64)> = transaction
        .query_row(
            "SELECT tenant_id, project_id, project_epoch, after_seq, updated_at_ms
             FROM replica_meta WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage("meta_read", error))?;
    row.map(decode_meta).transpose()
}

fn decode_meta(row: (String, String, String, i64, i64)) -> Result<ReplicaMeta, ClientError> {
    Ok(ReplicaMeta {
        scope: ProjectScope::new(TenantId::try_from(row.0)?, ProjectId::try_from(row.1)?),
        project_epoch: ProjectEpoch::try_from(row.2)?,
        after_seq: ProjectSequence::new(from_i64(row.3, "cursor")?),
        updated_at_ms: row.4,
    })
}

fn validate_materialized_batch(batch: &MaterializedChangeBatch) -> Result<(), ClientError> {
    let validated = MaterializedChangeBatch::new(
        batch.scope.clone(),
        batch.cursor.clone(),
        batch.entries.clone(),
        batch.has_more,
    )?;
    if validated.next_cursor != batch.next_cursor {
        return Err(ClientError::Protocol(
            "server next_cursor does not match the ordered entries".to_owned(),
        ));
    }
    Ok(())
}

fn validate_snapshot(snapshot: &ProjectSnapshot) -> Result<(), ClientError> {
    let validated = ProjectSnapshot::new(
        snapshot.scope.clone(),
        snapshot.project_epoch.clone(),
        snapshot.at_seq,
        snapshot.resources.clone(),
    )?;
    if validated != *snapshot {
        return Err(ClientError::Protocol(
            "server snapshot does not match its validated representation".to_owned(),
        ));
    }
    Ok(())
}

fn apply_resource(
    transaction: &Transaction<'_>,
    resource: &CanonicalResource,
    observed_after_seq: ProjectSequence,
) -> Result<(), ClientError> {
    let current: Option<(i64, i64, Option<Vec<u8>>)> = transaction
        .query_row(
            "SELECT revision, deleted, body_json FROM replica_resources
             WHERE resource_kind = ?1 AND resource_id = ?2",
            params![
                resource.resource.kind.as_str(),
                resource.resource.id.as_str()
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage("resource_current", error))?;
    let revision = to_i64(resource.revision.get(), "revision")?;
    let (deleted, body): (i64, Option<&[u8]>) = match &resource.state {
        ResourceState::Present { body } => (0, Some(body.as_slice())),
        ResourceState::Deleted => (1, None),
    };
    if let Some((current_revision, current_deleted, current_body)) = current {
        if current_revision > revision {
            return Ok(());
        }
        if current_revision == revision {
            if current_deleted != deleted || current_body.as_deref() != body {
                return Err(ClientError::Protocol(format!(
                    "resource {:?} changed without a revision advance",
                    resource.resource
                )));
            }
            return Ok(());
        }
    }
    transaction
        .execute(
            "INSERT INTO replica_resources
                (resource_kind, resource_id, revision, deleted, body_json, observed_after_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(resource_kind, resource_id) DO UPDATE SET
                revision = excluded.revision,
                deleted = excluded.deleted,
                body_json = excluded.body_json,
                observed_after_seq = excluded.observed_after_seq",
            params![
                resource.resource.kind.as_str(),
                resource.resource.id.as_str(),
                revision,
                deleted,
                body,
                to_i64(observed_after_seq.get(), "cursor")?,
            ],
        )
        .map_err(|error| storage("resource_apply", error))?;
    Ok(())
}

fn decode_resource(
    scope: ProjectScope,
    resource: ResourceRef,
    revision: i64,
    deleted: i64,
    body: Option<Vec<u8>>,
) -> Result<CanonicalResource, ClientError> {
    let state = match (deleted, body) {
        (0, Some(body)) => ResourceState::Present { body },
        (1, None) => ResourceState::Deleted,
        _ => {
            return Err(ClientError::Storage {
                operation: "resource_decode",
                detail: "body and deletion marker are inconsistent".to_owned(),
            });
        }
    };
    Ok(CanonicalResource {
        scope,
        resource,
        revision: Revision::new(from_i64(revision, "revision")?)?,
        state,
    })
}

fn storage(operation: &'static str, error: rusqlite::Error) -> ClientError {
    ClientError::Storage {
        operation,
        detail: error.to_string(),
    }
}

fn to_i64(value: u64, field: &'static str) -> Result<i64, ClientError> {
    i64::try_from(value).map_err(|_| ClientError::Protocol(format!("{field} exceeds i64")))
}

fn from_i64(value: i64, field: &'static str) -> Result<u64, ClientError> {
    u64::try_from(value).map_err(|_| ClientError::Storage {
        operation: "integer_decode",
        detail: format!("{field} is negative"),
    })
}

fn now_ms() -> Result<i64, ClientError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ClientError::Protocol(error.to_string()))?;
    i64::try_from(duration.as_millis())
        .map_err(|_| ClientError::Protocol("current timestamp exceeds i64".to_owned()))
}

fn segment(value: &str) -> String {
    utf8_percent_encode(value, PATH_SEGMENT).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidememo_domain::{ChangeEntry, ChangeOperation, ResourceId, ResourceKind};

    fn identity(epoch: &str) -> Result<ProjectIdentity, ClientError> {
        Ok(ProjectIdentity {
            tenant_id: TenantId::try_from("tenant-a")?,
            project_id: ProjectId::try_from("project-a")?,
            project_epoch: ProjectEpoch::try_from(epoch)?,
            actor_id: ActorId::try_from("codex-p1")?,
            role: MembershipRole::Writer,
        })
    }

    fn resource_ref(id: &str) -> Result<ResourceRef, ClientError> {
        Ok(ResourceRef {
            kind: ResourceKind::try_from("fact")?,
            id: ResourceId::try_from(id)?,
        })
    }

    fn batch(
        identity: &ProjectIdentity,
        after_seq: u64,
        seq: u64,
        resource: ResourceRef,
        operation: ChangeOperation,
        revision: u64,
    ) -> Result<MaterializedChangeBatch, ClientError> {
        let change = ChangeEntry {
            tenant_id: identity.tenant_id.clone(),
            project_id: identity.project_id.clone(),
            project_epoch: identity.project_epoch.clone(),
            seq: ProjectSequence::new(seq),
            resource,
            operation,
            revision: Revision::new(revision)?,
            actor_id: identity.actor_id.clone(),
            committed_at_ms: 1_700_000_000_000,
        };
        let state = match operation {
            ChangeOperation::Upsert => ResourceState::Present {
                body: br#"{"content":"cached"}"#.to_vec(),
            },
            ChangeOperation::Delete => ResourceState::Deleted,
        };
        Ok(MaterializedChangeBatch::new(
            identity.scope(),
            ChangeCursor {
                project_epoch: identity.project_epoch.clone(),
                after_seq: ProjectSequence::new(after_seq),
            },
            vec![MaterializedChange {
                resource: CanonicalResource {
                    scope: identity.scope(),
                    resource: change.resource.clone(),
                    revision: change.revision,
                    state,
                },
                change,
            }],
            false,
        )?)
    }

    #[test]
    fn replica_bootstrap_incremental_pull_and_offline_read_are_atomic()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("replica.sqlite");
        let mut store = ReplicaStore::open(&path)?;
        let identity = identity("epoch-a")?;

        let fact = resource_ref("fact-1")?;
        let current = CanonicalResource {
            scope: identity.scope(),
            resource: fact.clone(),
            revision: Revision::new(1)?,
            state: ResourceState::Present {
                body: br#"{"content":"cached"}"#.to_vec(),
            },
        };
        store.apply_snapshot(&ProjectSnapshot::new(
            identity.scope(),
            identity.project_epoch.clone(),
            ProjectSequence::new(1),
            vec![current.clone()],
        )?)?;

        drop(store);
        let mut reopened = ReplicaStore::open(&path)?;
        let cached = reopened.resource(&fact)?.ok_or("cached fact missing")?;
        assert_eq!(cached, current);
        assert_eq!(reopened.status()?.after_seq, ProjectSequence::new(1));

        let second = batch(&identity, 1, 2, fact.clone(), ChangeOperation::Delete, 2)?;
        reopened.apply_materialized_batch(&second)?;
        assert!(matches!(
            reopened.resource(&fact)?.map(|resource| resource.state),
            Some(ResourceState::Deleted)
        ));
        let status = reopened.status()?;
        assert_eq!(status.after_seq, ProjectSequence::new(2));
        assert_eq!(status.resource_count, 0);
        assert_eq!(status.tombstone_count, 1);
        Ok(())
    }

    #[test]
    fn invalid_materialization_does_not_advance_cursor() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut store = ReplicaStore::open(dir.path().join("replica.sqlite"))?;
        let identity = identity("epoch-a")?;
        let fact = resource_ref("fact-1")?;
        store.apply_snapshot(&ProjectSnapshot::new(
            identity.scope(),
            identity.project_epoch.clone(),
            ProjectSequence::ZERO,
            vec![],
        )?)?;
        let mut batch = batch(&identity, 0, 1, fact, ChangeOperation::Upsert, 2)?;
        batch.next_cursor.after_seq = ProjectSequence::new(2);
        assert!(store.apply_materialized_batch(&batch).is_err());
        assert_eq!(
            store.cursor()?.map(|cursor| cursor.after_seq),
            Some(ProjectSequence::ZERO)
        );
        Ok(())
    }

    #[test]
    fn scope_and_epoch_changes_require_explicit_reset() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut store = ReplicaStore::open(dir.path().join("replica.sqlite"))?;
        let first = identity("epoch-a")?;
        store.apply_snapshot(&ProjectSnapshot::new(
            first.scope(),
            first.project_epoch.clone(),
            ProjectSequence::ZERO,
            vec![],
        )?)?;
        assert!(matches!(
            store.verify_identity(&identity("epoch-b")?),
            Err(ClientError::EpochMismatch { .. })
        ));

        let mut other = identity("epoch-a")?;
        other.project_id = ProjectId::try_from("project-b")?;
        assert!(matches!(
            store.verify_identity(&other),
            Err(ClientError::ScopeMismatch { .. })
        ));

        store.reset()?;
        store.apply_snapshot(&ProjectSnapshot::new(
            other.scope(),
            other.project_epoch.clone(),
            ProjectSequence::ZERO,
            vec![],
        )?)?;
        assert_eq!(store.status()?.scope, Some(other.scope()));
        Ok(())
    }
}
