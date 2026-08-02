//! Local Phase-1 artifact repository with immutable body generations.
//!
//! Canonical logical paths and publication metadata live in a private SQLite
//! catalog. Body bytes are written once under adapter-owned generation keys.
//! A caller must reserve a path, upload the selected generation, and publish
//! the server-observed metadata before readers can discover it.

#[cfg(feature = "s3")]
mod s3;

#[cfg(feature = "s3")]
pub use s3::{DirectBodyGrant, S3BodyStore, S3BodyStoreConfig};

use aidememo_domain::{
    ArtifactBodyRef, ArtifactId, ArtifactObservation, ArtifactPath, ArtifactReference,
    ArtifactReservation, ContentDigest, DomainError, ProjectId, ProjectScope, Revision, TenantId,
};
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};
use sha2::{Digest, Sha256};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;
use thiserror::Error;
use ulid::Ulid;

const SCHEMA_VERSION: i64 = 4;
/// Stable identity for the repository-owned local immutable-body layout.
pub const LOCAL_BODY_STORE_IDENTITY: &str = "local:v1";
const MAX_TTL_MS: i64 = 24 * 60 * 60 * 1_000;
/// Maximum body accepted by the bounded local direct-upload path.
pub const MAX_DIRECT_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const GC_SETTLEMENT_GRACE_MS: i64 = 60_000;
const GC_LEASE_MS: i64 = 30_000;
const MAX_READ_RETENTION_MS: i64 = 8 * 24 * 60 * 60 * 1_000;
const MAX_GC_BATCH: usize = 100;
const MAX_REQUEST_ID_BYTES: usize = 1_024;
const RECEIPT_RETENTION_MS: i64 = 24 * 60 * 60 * 1_000;

/// Errors from the isolated local artifact repository.
#[derive(Debug, Error)]
pub enum ArtifactStoreError {
    /// Portable identity, path, revision, or reference validation failed.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// SQLite catalog operation failed.
    #[error("artifact catalog operation '{operation}' failed: {detail}")]
    Storage {
        /// Stable operation label.
        operation: &'static str,
        /// Adapter diagnostic.
        detail: String,
    },
    /// Local immutable-body operation failed.
    #[error("artifact filesystem operation '{operation}' failed for {path}: {detail}")]
    Filesystem {
        /// Stable operation label.
        operation: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Adapter diagnostic.
        detail: String,
    },
    /// A reservation or publication compare-and-swap precondition failed.
    #[error("artifact path reservation conflicts with current state")]
    Conflict,
    /// The reservation has passed its server-issued expiry.
    #[error("artifact reservation has expired")]
    ReservationExpired,
    /// The requested published artifact or reservation does not exist.
    #[error("artifact was not found")]
    NotFound,
    /// Uploaded bytes differ from the observation supplied for publication.
    #[error("artifact body does not match the observed immutable metadata")]
    BodyMismatch,
    /// The metadata catalog is already bound to another immutable-body store.
    #[error("artifact catalog is bound to a different immutable body store")]
    BodyStoreMismatch,
    /// A populated catalog predates body-store identity and cannot be bound safely.
    #[error("populated artifact catalog has no immutable body-store identity")]
    BodyStoreIdentityMissing,
    /// The bounded direct-upload API rejected an oversized body.
    #[error("artifact upload exceeds the local limit of {limit_bytes} bytes")]
    UploadTooLarge {
        /// Maximum accepted body size.
        limit_bytes: usize,
    },
    /// S3-compatible provider request or response failed.
    #[error("artifact body-store operation '{operation}' failed: {detail}")]
    Provider {
        /// Stable operation label without bucket, key, or credentials.
        operation: &'static str,
        /// Sanitized provider diagnostic.
        detail: String,
    },
}

/// Input to an exclusive path reservation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReserveRequest {
    /// Client-generated idempotency key for exact reservation replay.
    pub request_id: String,
    /// Tenant-project scope owning the logical path.
    pub scope: ProjectScope,
    /// Canonical logical namespace path.
    pub path: ArtifactPath,
    /// Current published mutation token, or `None` only for a new path.
    pub expected_mutation_token: Option<String>,
    /// Server-observed current UTC Unix time in milliseconds.
    pub now_ms: i64,
    /// Positive reservation lifetime, bounded to 24 hours.
    pub ttl_ms: i64,
}

/// Outcome of one bounded durable garbage-collection pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GarbageCollectionReport {
    /// Expired reservations moved to durable GC intent.
    pub expired_reservations: usize,
    /// Due object generations claimed by this pass.
    pub claimed: usize,
    /// Object generations deleted or already absent.
    pub deleted: usize,
    /// Object generations retained for a later retry.
    pub failed: usize,
    /// Expired reservation/publication replay receipts removed from the catalog.
    pub pruned_receipts: usize,
    /// Expired direct-download retention records removed from the catalog.
    pub pruned_read_retentions: usize,
}

/// One durably leased, unreachable immutable generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GarbageLease {
    /// Tenant-project scope that owns the generation.
    pub scope: ProjectScope,
    /// Adapter-selected immutable generation.
    pub generation: String,
    /// Catalog key used for local body deletion and exact lease acknowledgement.
    pub object_key: String,
    lease_token: String,
    attempts: u32,
}

/// Catalog work claimed before a body adapter performs external deletion.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GarbageClaim {
    /// Reaping, pruning, and lease counts committed with this claim.
    pub report: GarbageCollectionReport,
    /// Exact generations leased to the caller.
    pub leases: Vec<GarbageLease>,
}

/// Separate SQLite catalog plus immutable local object directory.
pub struct LocalArtifactStore {
    root: PathBuf,
    connection: Connection,
}

impl LocalArtifactStore {
    /// Open or create an isolated artifact repository below `root`.
    ///
    /// # Errors
    ///
    /// Returns an error when the root is a symlink, directories cannot be
    /// created, or the private SQLite catalog cannot be configured or migrated.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ArtifactStoreError> {
        let root = root.as_ref();
        if fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(filesystem(
                "open_root",
                root,
                "artifact root must not be a symbolic link",
            ));
        }
        fs::create_dir_all(root).map_err(|error| filesystem_error("create_root", root, error))?;
        let objects = root.join("objects");
        fs::create_dir_all(&objects)
            .map_err(|error| filesystem_error("create_objects", &objects, error))?;
        let connection = Connection::open(root.join("catalog.sqlite"))
            .map_err(|error| storage("open_catalog", error))?;
        connection
            .busy_timeout(DEFAULT_BUSY_TIMEOUT)
            .map_err(|error| storage("busy_timeout", error))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|error| storage("foreign_keys", error))?;
        configure_wal(&connection)?;
        migrate(&connection)?;
        Ok(Self {
            root: root.to_path_buf(),
            connection,
        })
    }

    /// Bind this catalog to one immutable-body adapter identity.
    ///
    /// An empty catalog is bound exactly once. Existing catalogs from the
    /// local-only schema are migrated as `local:v1`; a populated catalog with
    /// no trustworthy legacy identity fails closed instead of guessing.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactStoreError::BodyStoreMismatch`] when the catalog is
    /// already bound to another adapter, or
    /// [`ArtifactStoreError::BodyStoreIdentityMissing`] when populated legacy
    /// state cannot be attributed safely.
    pub fn bind_body_store(&mut self, identity: &str) -> Result<(), ArtifactStoreError> {
        validate_body_store_identity(identity)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("body_store_bind_begin", error))?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT value FROM artifact_repository_meta WHERE key = 'body_store_identity'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage("body_store_identity_read", error))?;
        if let Some(existing) = existing {
            if existing != identity {
                return Err(ArtifactStoreError::BodyStoreMismatch);
            }
            transaction
                .commit()
                .map_err(|error| storage("body_store_bind_commit", error))?;
            return Ok(());
        }
        let populated: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM artifact_paths LIMIT 1)
                     OR EXISTS(SELECT 1 FROM artifact_reservations LIMIT 1)
                     OR EXISTS(SELECT 1 FROM artifact_gc_queue LIMIT 1)
                     OR EXISTS(SELECT 1 FROM artifact_read_retentions LIMIT 1)",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage("body_store_identity_probe", error))?;
        if populated {
            return Err(ArtifactStoreError::BodyStoreIdentityMissing);
        }
        transaction
            .execute(
                "INSERT INTO artifact_repository_meta (key, value)
                 VALUES ('body_store_identity', ?1)",
                params![identity],
            )
            .map_err(|error| storage("body_store_identity_write", error))?;
        transaction
            .commit()
            .map_err(|error| storage("body_store_bind_commit", error))?;
        Ok(())
    }

    /// Reserve one logical path for a new immutable generation.
    ///
    /// Existing published paths require their current mutation token. One live
    /// reservation per path is allowed; an expired reservation is replaced.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactStoreError::Conflict`] for a stale token or live
    /// reservation, and a validation/storage error for invalid input.
    pub fn reserve(
        &mut self,
        request: ReserveRequest,
    ) -> Result<ArtifactReservation, ArtifactStoreError> {
        validate_request_id(&request.request_id)?;
        if request.now_ms <= 0 || request.ttl_ms <= 0 || request.ttl_ms > MAX_TTL_MS {
            return Err(DomainError::InvalidArtifactReference(format!(
                "reservation time must be positive and ttl_ms must be between 1 and {MAX_TTL_MS}"
            ))
            .into());
        }
        let expires_at_ms = request.now_ms.checked_add(request.ttl_ms).ok_or_else(|| {
            DomainError::InvalidArtifactReference("reservation expiry overflow".to_owned())
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("reserve_begin", error))?;
        if let Some(reservation) = load_reservation_receipt(&transaction, &request)? {
            return Ok(reservation);
        }
        let published = load_published_row(&transaction, &request.scope, &request.path)?;
        match (&published, request.expected_mutation_token.as_deref()) {
            (None, None) => {}
            (Some(current), Some(expected)) if current.mutation_token == expected => {}
            _ => return Err(ArtifactStoreError::Conflict),
        }
        let active: Option<(String, i64)> = transaction
            .query_row(
                "SELECT generation, expires_at_ms FROM artifact_reservations
                 WHERE tenant_id = ?1 AND project_id = ?2 AND logical_path = ?3",
                params![
                    request.scope.tenant_id.as_str(),
                    request.scope.project_id.as_str(),
                    request.path.as_str(),
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage("reservation_current", error))?;
        if active
            .as_ref()
            .is_some_and(|(_, expiry)| *expiry > request.now_ms)
        {
            return Err(ArtifactStoreError::Conflict);
        }
        if let Some((generation, expiry)) = &active {
            queue_gc_candidate(
                &transaction,
                &request.scope,
                generation,
                expiry.saturating_add(GC_SETTLEMENT_GRACE_MS),
            )?;
        }
        transaction
            .execute(
                "DELETE FROM artifact_reservations
                 WHERE tenant_id = ?1 AND project_id = ?2 AND logical_path = ?3",
                params![
                    request.scope.tenant_id.as_str(),
                    request.scope.project_id.as_str(),
                    request.path.as_str(),
                ],
            )
            .map_err(|error| storage("reservation_expired_delete", error))?;

        let artifact_id = published.as_ref().map_or_else(
            || ArtifactId::try_from(format!("artifact_{}", new_id())),
            |current| Ok(current.artifact_id.clone()),
        )?;
        let revision = published
            .as_ref()
            .map_or_else(|| Revision::new(1), |current| current.revision.next())?;
        let reservation = ArtifactReservation {
            artifact_id,
            scope: request.scope.clone(),
            path: request.path.clone(),
            revision,
            mutation_token: format!("mut_{}", new_id()),
            generation: format!("gen_{}", new_id()),
            expires_at_ms,
        };
        reservation.validate()?;
        transaction
            .execute(
                "INSERT INTO artifact_reservations
                    (tenant_id, project_id, logical_path, artifact_id, revision,
                     mutation_token, generation, expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    reservation.scope.tenant_id.as_str(),
                    reservation.scope.project_id.as_str(),
                    reservation.path.as_str(),
                    reservation.artifact_id.as_str(),
                    to_i64(reservation.revision.get(), "revision")?,
                    &reservation.mutation_token,
                    &reservation.generation,
                    reservation.expires_at_ms,
                ],
            )
            .map_err(|error| storage("reservation_insert", error))?;
        insert_reservation_receipt(&transaction, &request, &reservation)?;
        transaction
            .commit()
            .map_err(|error| storage("reserve_commit", error))?;
        Ok(reservation)
    }

    /// Write one reservation generation as an immutable local object.
    ///
    /// Exact retries return the same observation. A generation that already
    /// exists with different bytes fails closed.
    ///
    /// # Errors
    ///
    /// Returns an expiry, conflict, size, filesystem, or body-mismatch error.
    pub fn upload(
        &self,
        reservation: &ArtifactReservation,
        bytes: &[u8],
        now_ms: i64,
    ) -> Result<ArtifactObservation, ArtifactStoreError> {
        reservation.validate()?;
        self.ensure_live_reservation(reservation, now_ms)?;
        if bytes.len() > MAX_DIRECT_UPLOAD_BYTES {
            return Err(ArtifactStoreError::UploadTooLarge {
                limit_bytes: MAX_DIRECT_UPLOAD_BYTES,
            });
        }
        let digest = digest_bytes(bytes)?;
        let object_key = object_key(&reservation.scope, &reservation.generation);
        let object_path = self.object_path(&object_key)?;
        let parent = ensure_object_parent(&self.root, &reservation.scope)?;
        if object_path.parent() != Some(parent.as_path()) {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        if object_path.exists() {
            verify_file(&object_path, bytes.len() as u64, &digest)?;
        } else {
            let mut temporary = NamedTempFile::new_in(&parent)
                .map_err(|error| filesystem_error("object_temp_create", &parent, error))?;
            temporary
                .write_all(bytes)
                .map_err(|error| filesystem_error("object_temp_write", temporary.path(), error))?;
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| filesystem_error("object_temp_sync", temporary.path(), error))?;
            match temporary.persist_noclobber(&object_path) {
                Ok(_) => {}
                Err(error) if object_path.exists() => {
                    verify_file(&object_path, bytes.len() as u64, &digest)?;
                    drop(error);
                }
                Err(error) => {
                    return Err(filesystem_error(
                        "object_publish",
                        &object_path,
                        error.error,
                    ));
                }
            }
        }
        Ok(ArtifactObservation {
            object_key,
            generation: reservation.generation.clone(),
            size_bytes: bytes.len() as u64,
            etag: digest.as_str().to_owned(),
            version: None,
            digest: Some(digest),
        })
    }

    /// Observe a previously uploaded immutable generation using trusted local I/O.
    ///
    /// # Errors
    ///
    /// Returns a missing-body or filesystem error. Publication separately
    /// verifies that the reservation is live or is an exact committed retry.
    pub fn observe(
        &self,
        reservation: &ArtifactReservation,
        _now_ms: i64,
    ) -> Result<ArtifactObservation, ArtifactStoreError> {
        reservation.validate()?;
        let object_key = object_key(&reservation.scope, &reservation.generation);
        let path = self.object_path(&object_key)?;
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ArtifactStoreError::NotFound
            } else {
                filesystem_error("object_observe", &path, error)
            }
        })?;
        let digest = digest_bytes(&bytes)?;
        Ok(ArtifactObservation {
            object_key,
            generation: reservation.generation.clone(),
            size_bytes: bytes.len() as u64,
            etag: digest.as_str().to_owned(),
            version: None,
            digest: Some(digest),
        })
    }

    /// Publish observed immutable body metadata under the reserved logical path.
    ///
    /// Publication re-reads and hashes the local object, then updates canonical
    /// path metadata and consumes the reservation in one SQLite transaction.
    ///
    /// # Errors
    ///
    /// Returns an expiry, conflict, body-mismatch, filesystem, or storage error.
    pub fn publish(
        &mut self,
        reservation: &ArtifactReservation,
        observation: &ArtifactObservation,
        now_ms: i64,
    ) -> Result<ArtifactReference, ArtifactStoreError> {
        reservation.validate()?;
        if observation.generation != reservation.generation {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        let expected_key = object_key(&reservation.scope, &reservation.generation);
        if observation.object_key != expected_key {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        let digest = observation
            .digest
            .as_ref()
            .ok_or(ArtifactStoreError::BodyMismatch)?;
        if observation.etag != digest.as_str() {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        if let Some(reference) = load_publication_receipt(
            &self.connection,
            &reservation.scope,
            &reservation.mutation_token,
        )? {
            if publication_matches(&reference, reservation, observation) {
                return Ok(reference);
            }
            return Err(ArtifactStoreError::BodyMismatch);
        }
        verify_file(
            &self.object_path(&expected_key)?,
            observation.size_bytes,
            digest,
        )?;
        self.commit_publication(reservation, observation, now_ms)
    }

    /// Publish metadata obtained from a trusted external body-store observation.
    ///
    /// The caller must obtain `observation` directly from the configured body
    /// adapter after it validates the reservation-owned immutable key. Unlike
    /// [`Self::publish`], this method does not read a local file and permits a
    /// provider that exposes no portable full-object digest.
    ///
    /// # Errors
    ///
    /// Returns an expiry, conflict, body-mismatch, or storage error.
    pub fn publish_trusted_observation(
        &mut self,
        reservation: &ArtifactReservation,
        observation: &ArtifactObservation,
        now_ms: i64,
    ) -> Result<ArtifactReference, ArtifactStoreError> {
        reservation.validate()?;
        observation.body_ref()?;
        if observation.generation != reservation.generation {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        if let Some(reference) = load_publication_receipt(
            &self.connection,
            &reservation.scope,
            &reservation.mutation_token,
        )? {
            if publication_matches(&reference, reservation, observation) {
                return Ok(reference);
            }
            return Err(ArtifactStoreError::BodyMismatch);
        }
        self.commit_publication(reservation, observation, now_ms)
    }

    fn commit_publication(
        &mut self,
        reservation: &ArtifactReservation,
        observation: &ArtifactObservation,
        now_ms: i64,
    ) -> Result<ArtifactReference, ArtifactStoreError> {
        let body = observation.body_ref()?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("publish_begin", error))?;
        match ensure_live_reservation_tx(&transaction, reservation, now_ms) {
            Ok(()) => {}
            Err(ArtifactStoreError::NotFound) => {
                let published =
                    load_published_row(&transaction, &reservation.scope, &reservation.path)?
                        .map(|row| {
                            row.reference(reservation.scope.clone(), reservation.path.clone())
                        })
                        .transpose()?;
                if published.as_ref().is_some_and(|reference| {
                    publication_matches(reference, reservation, observation)
                }) {
                    return published.ok_or(ArtifactStoreError::NotFound);
                }
                return Err(ArtifactStoreError::NotFound);
            }
            Err(error) => return Err(error),
        }
        if let Some(previous) =
            load_published_row(&transaction, &reservation.scope, &reservation.path)?
            && previous.object_key != observation.object_key
        {
            queue_gc_candidate(
                &transaction,
                &reservation.scope,
                &previous.generation,
                now_ms,
            )?;
        }
        transaction
            .execute(
                "INSERT INTO artifact_paths
                    (tenant_id, project_id, logical_path, artifact_id, revision,
                     mutation_token, generation, object_key, size_bytes, etag,
                     version, digest, published_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT (tenant_id, project_id, logical_path) DO UPDATE SET
                    artifact_id = excluded.artifact_id,
                    revision = excluded.revision,
                    mutation_token = excluded.mutation_token,
                    generation = excluded.generation,
                    object_key = excluded.object_key,
                    size_bytes = excluded.size_bytes,
                    etag = excluded.etag,
                    version = excluded.version,
                    digest = excluded.digest,
                    published_at_ms = excluded.published_at_ms",
                params![
                    reservation.scope.tenant_id.as_str(),
                    reservation.scope.project_id.as_str(),
                    reservation.path.as_str(),
                    reservation.artifact_id.as_str(),
                    to_i64(reservation.revision.get(), "revision")?,
                    &reservation.mutation_token,
                    &reservation.generation,
                    &observation.object_key,
                    to_i64(observation.size_bytes, "size_bytes")?,
                    &observation.etag,
                    observation.version.as_deref(),
                    observation.digest.as_ref().map(ContentDigest::as_str),
                    now_ms,
                ],
            )
            .map_err(|error| storage("publication_write", error))?;
        let reference = ArtifactReference {
            artifact_id: reservation.artifact_id.clone(),
            scope: reservation.scope.clone(),
            path: reservation.path.clone(),
            revision: reservation.revision,
            mutation_token: reservation.mutation_token.clone(),
            body,
        };
        reference.validate()?;
        insert_publication_receipt(&transaction, &reference, now_ms)?;
        delete_reservation(&transaction, reservation)?;
        transaction
            .commit()
            .map_err(|error| storage("publish_commit", error))?;
        Ok(reference)
    }

    /// Abort one live reservation without changing the last published path.
    /// Uploaded bytes remain unreachable and may be removed by a later GC pass.
    ///
    /// # Errors
    ///
    /// Returns conflict/not-found or storage errors for a stale reservation.
    pub fn abort(&mut self, reservation: &ArtifactReservation) -> Result<(), ArtifactStoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("abort_begin", error))?;
        queue_gc_candidate(
            &transaction,
            &reservation.scope,
            &reservation.generation,
            reservation
                .expires_at_ms
                .saturating_add(GC_SETTLEMENT_GRACE_MS),
        )?;
        let deleted = delete_reservation(&transaction, reservation)?;
        if deleted != 1 {
            return Err(ArtifactStoreError::NotFound);
        }
        transaction
            .commit()
            .map_err(|error| storage("abort_commit", error))
    }

    /// Resolve the currently published metadata for one logical path.
    ///
    /// # Errors
    ///
    /// Returns a validation or storage error when the catalog row is invalid.
    pub fn get(
        &self,
        scope: &ProjectScope,
        path: &ArtifactPath,
    ) -> Result<Option<ArtifactReference>, ArtifactStoreError> {
        load_published_row(&self.connection, scope, path)?.map_or(Ok(None), |row| {
            let reference = row.reference(scope.clone(), path.clone())?;
            reference.validate()?;
            Ok(Some(reference))
        })
    }

    /// Load one canonical reservation by its opaque mutation token.
    ///
    /// # Errors
    ///
    /// Returns a validation or storage error when the persisted row is invalid.
    pub fn reservation_by_token(
        &self,
        scope: &ProjectScope,
        mutation_token: &str,
    ) -> Result<Option<ArtifactReservation>, ArtifactStoreError> {
        let row: Option<(String, String, i64, String, i64)> = self
            .connection
            .query_row(
                "SELECT logical_path, artifact_id, revision, generation, expires_at_ms
                 FROM artifact_reservations
                 WHERE tenant_id = ?1 AND project_id = ?2 AND mutation_token = ?3",
                params![
                    scope.tenant_id.as_str(),
                    scope.project_id.as_str(),
                    mutation_token,
                ],
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
            .map_err(|error| storage("reservation_by_token", error))?;
        row.map(|(path, artifact_id, revision, generation, expires_at_ms)| {
            let reservation = ArtifactReservation {
                artifact_id: ArtifactId::try_from(artifact_id)?,
                scope: scope.clone(),
                path: ArtifactPath::try_from(path)?,
                revision: Revision::new(from_i64(revision, "revision")?)?,
                mutation_token: mutation_token.to_owned(),
                generation,
                expires_at_ms,
            };
            reservation.validate()?;
            Ok(reservation)
        })
        .transpose()
    }

    /// Load a reservation receipt by token after the live reservation was consumed.
    ///
    /// This supports an exact publish retry without making the upload authority
    /// live again.
    ///
    /// # Errors
    ///
    /// Returns a validation or storage error when the persisted receipt is invalid.
    pub fn reservation_receipt_by_token(
        &self,
        scope: &ProjectScope,
        mutation_token: &str,
    ) -> Result<Option<ArtifactReservation>, ArtifactStoreError> {
        let row: Option<(String, String, i64, String, i64)> = self
            .connection
            .query_row(
                "SELECT logical_path, artifact_id, revision, generation, expires_at_ms
                 FROM artifact_reservation_receipts
                 WHERE tenant_id = ?1 AND project_id = ?2 AND mutation_token = ?3",
                params![
                    scope.tenant_id.as_str(),
                    scope.project_id.as_str(),
                    mutation_token,
                ],
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
            .map_err(|error| storage("reservation_receipt_by_token", error))?;
        row.map(|(path, artifact_id, revision, generation, expires_at_ms)| {
            let reservation = ArtifactReservation {
                artifact_id: ArtifactId::try_from(artifact_id)?,
                scope: scope.clone(),
                path: ArtifactPath::try_from(path)?,
                revision: Revision::new(from_i64(revision, "revision")?)?,
                mutation_token: mutation_token.to_owned(),
                generation,
                expires_at_ms,
            };
            reservation.validate()?;
            Ok(reservation)
        })
        .transpose()
    }

    /// Load a committed publication receipt by its reservation token.
    ///
    /// This supports exact response replay even after a later replacement and
    /// garbage collection make the original generation unreachable.
    ///
    /// # Errors
    ///
    /// Returns a validation or storage error when the persisted receipt is invalid.
    pub fn publication_receipt_by_token(
        &self,
        scope: &ProjectScope,
        mutation_token: &str,
    ) -> Result<Option<ArtifactReference>, ArtifactStoreError> {
        load_publication_receipt(&self.connection, scope, mutation_token)
    }

    /// Resolve the current published metadata by stable artifact identity.
    ///
    /// # Errors
    ///
    /// Returns a validation or storage error when the persisted row is invalid.
    pub fn get_by_id(
        &self,
        scope: &ProjectScope,
        artifact_id: &ArtifactId,
    ) -> Result<Option<ArtifactReference>, ArtifactStoreError> {
        let path: Option<String> = self
            .connection
            .query_row(
                "SELECT logical_path FROM artifact_paths
                 WHERE tenant_id = ?1 AND project_id = ?2 AND artifact_id = ?3",
                params![
                    scope.tenant_id.as_str(),
                    scope.project_id.as_str(),
                    artifact_id.as_str(),
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage("published_by_id", error))?;
        path.map(ArtifactPath::try_from)
            .transpose()?
            .map_or(Ok(None), |path| self.get(scope, &path))
    }

    /// Durably retain the exact current generation through a direct-read grant.
    ///
    /// This transaction must commit before a body adapter signs the grant. A
    /// later replacement may enqueue the generation, but garbage collection
    /// will not lease it until this retention expires.
    ///
    /// # Errors
    ///
    /// Returns a stale/body-mismatch, validation, or storage error.
    pub fn retain_for_read(
        &mut self,
        reference: &ArtifactReference,
        now_ms: i64,
        retain_until_ms: i64,
    ) -> Result<(), ArtifactStoreError> {
        reference.validate()?;
        if now_ms <= 0
            || retain_until_ms <= now_ms
            || retain_until_ms.saturating_sub(now_ms) > MAX_READ_RETENTION_MS
        {
            return Err(DomainError::InvalidArtifactReference(
                "artifact read retention must be positive, future, and bounded".to_owned(),
            )
            .into());
        }
        if self
            .get_by_id(&reference.scope, &reference.artifact_id)?
            .as_ref()
            != Some(reference)
        {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        let ArtifactBodyRef::Object {
            object_key,
            generation,
            ..
        } = &reference.body
        else {
            return Err(ArtifactStoreError::BodyMismatch);
        };
        self.connection
            .execute(
                "INSERT INTO artifact_read_retentions
                    (tenant_id, project_id, generation, object_key, retain_until_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (tenant_id, project_id, generation) DO UPDATE SET
                    object_key = excluded.object_key,
                    retain_until_ms = max(
                        artifact_read_retentions.retain_until_ms,
                        excluded.retain_until_ms
                    )",
                params![
                    reference.scope.tenant_id.as_str(),
                    reference.scope.project_id.as_str(),
                    generation,
                    object_key,
                    retain_until_ms,
                ],
            )
            .map_err(|error| storage("read_retention_write", error))?;
        Ok(())
    }

    /// Reap metadata and durably lease a bounded batch of unreachable bodies.
    ///
    /// The caller must delete each generation through its configured body
    /// adapter, then call [`Self::acknowledge_garbage`] or
    /// [`Self::fail_garbage`].
    ///
    /// # Errors
    ///
    /// Returns a validation or catalog error.
    pub fn claim_garbage(
        &mut self,
        now_ms: i64,
        limit: usize,
    ) -> Result<GarbageClaim, ArtifactStoreError> {
        validate_gc_request(now_ms, limit)?;
        let mut report = GarbageCollectionReport::default();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage("gc_begin", error))?;
        report.expired_reservations =
            reap_expired_reservations(&transaction, now_ms, MAX_GC_BATCH)?;
        report.pruned_read_retentions =
            prune_expired_read_retentions(&transaction, now_ms, MAX_GC_BATCH)?;
        let leases = claim_gc_candidates(&transaction, now_ms, limit)?;
        report.claimed = leases.len();
        report.pruned_receipts = prune_expired_receipts(&transaction, now_ms, MAX_GC_BATCH)?;
        transaction
            .commit()
            .map_err(|error| storage("gc_claim_commit", error))?;
        Ok(GarbageClaim { report, leases })
    }

    /// Acknowledge successful or already-absent deletion for one exact lease.
    ///
    /// # Errors
    ///
    /// Returns a catalog error.
    pub fn acknowledge_garbage(&mut self, lease: &GarbageLease) -> Result<(), ArtifactStoreError> {
        acknowledge_gc(&self.connection, lease)
    }

    /// Release one failed lease with bounded exponential retry metadata.
    ///
    /// # Errors
    ///
    /// Returns a validation or catalog error.
    pub fn fail_garbage(
        &mut self,
        lease: &GarbageLease,
        now_ms: i64,
        detail: &str,
    ) -> Result<(), ArtifactStoreError> {
        if now_ms <= 0 {
            return Err(DomainError::InvalidArtifactReference(
                "artifact GC failure time must be positive".to_owned(),
            )
            .into());
        }
        fail_gc(&self.connection, lease, now_ms, detail)
    }

    /// Reap expired reservations and delete a bounded batch of unreachable bodies.
    ///
    /// Delete is idempotent. A crash after filesystem deletion but before the
    /// catalog acknowledgement leaves the candidate available for retry.
    ///
    /// # Errors
    ///
    /// Returns a validation, catalog, or filesystem error. Individual filesystem
    /// failures are recorded for retry and reflected in the report.
    pub fn drain_garbage(
        &mut self,
        now_ms: i64,
        limit: usize,
    ) -> Result<GarbageCollectionReport, ArtifactStoreError> {
        let GarbageClaim { mut report, leases } = self.claim_garbage(now_ms, limit)?;

        for lease in leases {
            let path = self.object_path(&lease.object_key)?;
            match fs::remove_file(&path) {
                Ok(()) => {
                    self.acknowledge_garbage(&lease)?;
                    report.deleted += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.acknowledge_garbage(&lease)?;
                    report.deleted += 1;
                }
                Err(error) => {
                    self.fail_garbage(&lease, now_ms, &error.to_string())?;
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }

    /// Read and re-verify the immutable bytes named by a published reference.
    ///
    /// # Errors
    ///
    /// Returns a body-mismatch or filesystem error for stale/corrupt metadata.
    pub fn read(&self, reference: &ArtifactReference) -> Result<Vec<u8>, ArtifactStoreError> {
        reference.validate()?;
        let ArtifactBodyRef::Object {
            object_key: stored_key,
            generation,
            size_bytes,
            etag,
            digest,
            ..
        } = &reference.body
        else {
            return Err(ArtifactStoreError::BodyMismatch);
        };
        let expected_key = object_key(&reference.scope, generation);
        let digest = digest.as_ref().ok_or(ArtifactStoreError::BodyMismatch)?;
        if stored_key != &expected_key || etag != digest.as_str() {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        let path = self.object_path(&expected_key)?;
        let bytes =
            fs::read(&path).map_err(|error| filesystem_error("object_read", &path, error))?;
        if bytes.len() as u64 != *size_bytes || digest_bytes(&bytes)? != *digest {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        Ok(bytes)
    }

    fn ensure_live_reservation(
        &self,
        reservation: &ArtifactReservation,
        now_ms: i64,
    ) -> Result<(), ArtifactStoreError> {
        ensure_live_reservation_connection(&self.connection, reservation, now_ms)
    }

    fn object_path(&self, object_key: &str) -> Result<PathBuf, ArtifactStoreError> {
        let components = Path::new(object_key).components().collect::<Vec<_>>();
        if object_key.contains('\\')
            || components.len() != 4
            || components[0] != Component::Normal("objects".as_ref())
            || components
                .iter()
                .skip(1)
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        Ok(self.root.join(object_key))
    }
}

fn configure_wal(connection: &Connection) -> Result<(), ArtifactStoreError> {
    let started = Instant::now();
    loop {
        match connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0)) {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => return Ok(()),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                ) && started.elapsed() < DEFAULT_BUSY_TIMEOUT =>
            {
                let remaining = DEFAULT_BUSY_TIMEOUT.saturating_sub(started.elapsed());
                std::thread::sleep(remaining.min(Duration::from_millis(20)));
                continue;
            }
            Err(error) => return Err(storage("journal_mode_read", error)),
        }
        match connection.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                ) && started.elapsed() < DEFAULT_BUSY_TIMEOUT =>
            {
                let remaining = DEFAULT_BUSY_TIMEOUT.saturating_sub(started.elapsed());
                std::thread::sleep(remaining.min(Duration::from_millis(20)));
            }
            Err(error) => return Err(storage("journal_mode", error)),
        }
    }
}

#[derive(Debug)]
struct PublishedRow {
    artifact_id: ArtifactId,
    revision: Revision,
    mutation_token: String,
    generation: String,
    object_key: String,
    size_bytes: u64,
    etag: String,
    version: Option<String>,
    digest: Option<ContentDigest>,
}

struct PublishedSqlRow {
    artifact_id: String,
    revision: i64,
    mutation_token: String,
    generation: String,
    object_key: String,
    size_bytes: i64,
    etag: String,
    version: Option<String>,
    digest: Option<String>,
}

impl PublishedRow {
    fn reference(
        self,
        scope: ProjectScope,
        path: ArtifactPath,
    ) -> Result<ArtifactReference, DomainError> {
        Ok(ArtifactReference {
            artifact_id: self.artifact_id,
            scope,
            path,
            revision: self.revision,
            mutation_token: self.mutation_token,
            body: ArtifactBodyRef::Object {
                object_key: self.object_key,
                generation: self.generation,
                size_bytes: self.size_bytes,
                etag: self.etag,
                version: self.version,
                digest: self.digest,
            },
        })
    }
}

fn migrate(connection: &Connection) -> Result<(), ArtifactStoreError> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| storage("schema_version", error))?;
    match version {
        0 => connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS artifact_paths (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    logical_path TEXT NOT NULL,
                    artifact_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0),
                    mutation_token TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    object_key TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
                    etag TEXT NOT NULL,
                    version TEXT,
                    digest TEXT,
                    published_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (tenant_id, project_id, logical_path)
                 ) STRICT;
                 CREATE TABLE IF NOT EXISTS artifact_reservations (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    logical_path TEXT NOT NULL,
                    artifact_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0),
                    mutation_token TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > 0),
                    PRIMARY KEY (tenant_id, project_id, logical_path)
                 ) STRICT;
                 CREATE UNIQUE INDEX IF NOT EXISTS artifact_reservation_token
                    ON artifact_reservations (tenant_id, project_id, mutation_token);
                 CREATE TABLE IF NOT EXISTS artifact_reservation_receipts (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    request_id TEXT NOT NULL,
                    logical_path TEXT NOT NULL,
                    expected_mutation_token TEXT,
                    ttl_ms INTEGER NOT NULL CHECK (ttl_ms > 0),
                    artifact_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0),
                    mutation_token TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > 0),
                    retain_until_ms INTEGER NOT NULL CHECK (retain_until_ms > 0),
                    PRIMARY KEY (tenant_id, project_id, request_id)
                 ) STRICT;
                 CREATE UNIQUE INDEX IF NOT EXISTS artifact_receipt_token
                    ON artifact_reservation_receipts (tenant_id, project_id, mutation_token);
                 CREATE TABLE IF NOT EXISTS artifact_publication_receipts (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    mutation_token TEXT NOT NULL,
                    reference_json TEXT NOT NULL,
                    retain_until_ms INTEGER NOT NULL CHECK (retain_until_ms > 0),
                    PRIMARY KEY (tenant_id, project_id, mutation_token)
                 ) STRICT;
                 CREATE TABLE IF NOT EXISTS artifact_gc_queue (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    object_key TEXT NOT NULL PRIMARY KEY,
                    not_before_ms INTEGER NOT NULL CHECK (not_before_ms > 0),
                    next_attempt_ms INTEGER NOT NULL CHECK (next_attempt_ms > 0),
                    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
                    lease_token TEXT,
                    lease_until_ms INTEGER,
                    last_error TEXT
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS artifact_gc_due
                    ON artifact_gc_queue (next_attempt_ms, not_before_ms);
                 CREATE TABLE IF NOT EXISTS artifact_read_retentions (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    object_key TEXT NOT NULL,
                    retain_until_ms INTEGER NOT NULL CHECK (retain_until_ms > 0),
                    PRIMARY KEY (tenant_id, project_id, generation)
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS artifact_read_retention_expiry
                    ON artifact_read_retentions (retain_until_ms);
                 CREATE TABLE IF NOT EXISTS artifact_repository_meta (
                    key TEXT NOT NULL PRIMARY KEY,
                    value TEXT NOT NULL
                 ) STRICT;
                 PRAGMA user_version = 4;
                 COMMIT;",
            )
            .map_err(|error| storage("schema_create", error)),
        1 => connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE UNIQUE INDEX IF NOT EXISTS artifact_reservation_token
                    ON artifact_reservations (tenant_id, project_id, mutation_token);
                 CREATE TABLE IF NOT EXISTS artifact_reservation_receipts (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    request_id TEXT NOT NULL,
                    logical_path TEXT NOT NULL,
                    expected_mutation_token TEXT,
                    ttl_ms INTEGER NOT NULL CHECK (ttl_ms > 0),
                    artifact_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0),
                    mutation_token TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > 0),
                    retain_until_ms INTEGER NOT NULL CHECK (retain_until_ms > 0),
                    PRIMARY KEY (tenant_id, project_id, request_id)
                 ) STRICT;
                 CREATE UNIQUE INDEX IF NOT EXISTS artifact_receipt_token
                    ON artifact_reservation_receipts (tenant_id, project_id, mutation_token);
                 CREATE TABLE IF NOT EXISTS artifact_publication_receipts (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    mutation_token TEXT NOT NULL,
                    reference_json TEXT NOT NULL,
                    retain_until_ms INTEGER NOT NULL CHECK (retain_until_ms > 0),
                    PRIMARY KEY (tenant_id, project_id, mutation_token)
                 ) STRICT;
                 CREATE TABLE IF NOT EXISTS artifact_gc_queue (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    object_key TEXT NOT NULL PRIMARY KEY,
                    not_before_ms INTEGER NOT NULL CHECK (not_before_ms > 0),
                    next_attempt_ms INTEGER NOT NULL CHECK (next_attempt_ms > 0),
                    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
                    lease_token TEXT,
                    lease_until_ms INTEGER,
                    last_error TEXT
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS artifact_gc_due
                    ON artifact_gc_queue (next_attempt_ms, not_before_ms);
                 CREATE TABLE IF NOT EXISTS artifact_repository_meta (
                    key TEXT NOT NULL PRIMARY KEY,
                    value TEXT NOT NULL
                 ) STRICT;
                 INSERT OR IGNORE INTO artifact_repository_meta (key, value)
                    VALUES ('body_store_identity', 'local:v1');
                 PRAGMA user_version = 2;
                 COMMIT;",
            )
            .map_err(|error| storage("schema_migrate_v2", error))
            .and_then(|()| migrate(connection)),
        2 => connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE artifact_paths RENAME TO artifact_paths_v2;
                 CREATE TABLE artifact_paths (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    logical_path TEXT NOT NULL,
                    artifact_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0),
                    mutation_token TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    object_key TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
                    etag TEXT NOT NULL,
                    version TEXT,
                    digest TEXT,
                    published_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (tenant_id, project_id, logical_path)
                 ) STRICT;
                 INSERT INTO artifact_paths
                    (tenant_id, project_id, logical_path, artifact_id, revision,
                     mutation_token, generation, object_key, size_bytes, etag,
                     version, digest, published_at_ms)
                 SELECT tenant_id, project_id, logical_path, artifact_id, revision,
                        mutation_token, generation, object_key, size_bytes, etag,
                        version, digest, published_at_ms
                 FROM artifact_paths_v2;
                 DROP TABLE artifact_paths_v2;
                 CREATE TABLE artifact_read_retentions (
                    tenant_id TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    generation TEXT NOT NULL,
                    object_key TEXT NOT NULL,
                    retain_until_ms INTEGER NOT NULL CHECK (retain_until_ms > 0),
                    PRIMARY KEY (tenant_id, project_id, generation)
                 ) STRICT;
                 CREATE INDEX artifact_read_retention_expiry
                    ON artifact_read_retentions (retain_until_ms);
                 CREATE TABLE IF NOT EXISTS artifact_repository_meta (
                    key TEXT NOT NULL PRIMARY KEY,
                    value TEXT NOT NULL
                 ) STRICT;
                 INSERT OR IGNORE INTO artifact_repository_meta (key, value)
                    VALUES ('body_store_identity', 'local:v1');
                 PRAGMA user_version = 3;
                 COMMIT;",
            )
            .map_err(|error| storage("schema_migrate_v3", error))
            .and_then(|()| migrate(connection)),
        3 => connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS artifact_repository_meta (
                    key TEXT NOT NULL PRIMARY KEY,
                    value TEXT NOT NULL
                 ) STRICT;
                 PRAGMA user_version = 4;
                 COMMIT;",
            )
            .map_err(|error| storage("schema_migrate_v4", error)),
        SCHEMA_VERSION => Ok(()),
        other => Err(ArtifactStoreError::Storage {
            operation: "schema_version",
            detail: format!("artifact schema {other} is not supported by version {SCHEMA_VERSION}"),
        }),
    }
}

fn validate_request_id(request_id: &str) -> Result<(), ArtifactStoreError> {
    if request_id.is_empty()
        || request_id.len() > MAX_REQUEST_ID_BYTES
        || request_id
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(DomainError::InvalidArtifactReference(
            "artifact reservation request_id must be non-empty, bounded, and contain no whitespace or control characters"
                .to_owned(),
        )
        .into());
    }
    Ok(())
}

fn validate_body_store_identity(identity: &str) -> Result<(), ArtifactStoreError> {
    if identity.is_empty()
        || identity.len() > 256
        || identity
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(DomainError::InvalidArtifactReference(
            "body-store identity must be non-empty, bounded, and contain no whitespace or control characters"
                .to_owned(),
        )
        .into());
    }
    Ok(())
}

fn load_reservation_receipt(
    transaction: &Transaction<'_>,
    request: &ReserveRequest,
) -> Result<Option<ArtifactReservation>, ArtifactStoreError> {
    type ReceiptRow = (
        String,
        Option<String>,
        i64,
        String,
        i64,
        String,
        String,
        i64,
    );
    let row: Option<ReceiptRow> = transaction
        .query_row(
            "SELECT logical_path, expected_mutation_token, ttl_ms, artifact_id,
                    revision, mutation_token, generation, expires_at_ms
             FROM artifact_reservation_receipts
             WHERE tenant_id = ?1 AND project_id = ?2 AND request_id = ?3",
            params![
                request.scope.tenant_id.as_str(),
                request.scope.project_id.as_str(),
                &request.request_id,
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage("reservation_receipt_read", error))?;
    let Some((
        path,
        expected_mutation_token,
        ttl_ms,
        artifact_id,
        revision,
        mutation_token,
        generation,
        expires_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    if path != request.path.as_str()
        || expected_mutation_token != request.expected_mutation_token
        || ttl_ms != request.ttl_ms
    {
        return Err(ArtifactStoreError::Conflict);
    }
    let reservation = ArtifactReservation {
        artifact_id: ArtifactId::try_from(artifact_id)?,
        scope: request.scope.clone(),
        path: ArtifactPath::try_from(path)?,
        revision: Revision::new(from_i64(revision, "revision")?)?,
        mutation_token,
        generation,
        expires_at_ms,
    };
    reservation.validate()?;
    Ok(Some(reservation))
}

fn insert_reservation_receipt(
    transaction: &Transaction<'_>,
    request: &ReserveRequest,
    reservation: &ArtifactReservation,
) -> Result<(), ArtifactStoreError> {
    let retain_until_ms = reservation
        .expires_at_ms
        .saturating_add(RECEIPT_RETENTION_MS);
    transaction
        .execute(
            "INSERT INTO artifact_reservation_receipts
                (tenant_id, project_id, request_id, logical_path,
                 expected_mutation_token, ttl_ms, artifact_id, revision,
                 mutation_token, generation, expires_at_ms, retain_until_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                reservation.scope.tenant_id.as_str(),
                reservation.scope.project_id.as_str(),
                &request.request_id,
                reservation.path.as_str(),
                request.expected_mutation_token.as_deref(),
                request.ttl_ms,
                reservation.artifact_id.as_str(),
                to_i64(reservation.revision.get(), "revision")?,
                &reservation.mutation_token,
                &reservation.generation,
                reservation.expires_at_ms,
                retain_until_ms,
            ],
        )
        .map_err(|error| storage("reservation_receipt_insert", error))?;
    Ok(())
}

fn insert_publication_receipt(
    transaction: &Transaction<'_>,
    reference: &ArtifactReference,
    now_ms: i64,
) -> Result<(), ArtifactStoreError> {
    let reference_json =
        serde_json::to_string(reference).map_err(|error| ArtifactStoreError::Storage {
            operation: "publication_receipt_encode",
            detail: error.to_string(),
        })?;
    let retain_until_ms = now_ms.saturating_add(RECEIPT_RETENTION_MS);
    transaction
        .execute(
            "INSERT INTO artifact_publication_receipts
                (tenant_id, project_id, mutation_token, reference_json, retain_until_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                reference.scope.tenant_id.as_str(),
                reference.scope.project_id.as_str(),
                &reference.mutation_token,
                reference_json,
                retain_until_ms,
            ],
        )
        .map_err(|error| storage("publication_receipt_insert", error))?;
    Ok(())
}

fn load_publication_receipt(
    connection: &Connection,
    scope: &ProjectScope,
    mutation_token: &str,
) -> Result<Option<ArtifactReference>, ArtifactStoreError> {
    let reference_json: Option<String> = connection
        .query_row(
            "SELECT reference_json FROM artifact_publication_receipts
             WHERE tenant_id = ?1 AND project_id = ?2 AND mutation_token = ?3",
            params![
                scope.tenant_id.as_str(),
                scope.project_id.as_str(),
                mutation_token,
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage("publication_receipt_read", error))?;
    reference_json
        .map(|json| {
            let reference: ArtifactReference =
                serde_json::from_str(&json).map_err(|error| ArtifactStoreError::Storage {
                    operation: "publication_receipt_decode",
                    detail: error.to_string(),
                })?;
            reference.validate()?;
            if &reference.scope != scope || reference.mutation_token != mutation_token {
                return Err(ArtifactStoreError::BodyMismatch);
            }
            Ok(reference)
        })
        .transpose()
}

fn queue_gc_candidate(
    transaction: &Transaction<'_>,
    scope: &ProjectScope,
    generation: &str,
    not_before_ms: i64,
) -> Result<(), ArtifactStoreError> {
    queue_gc_candidate_parts(
        transaction,
        scope.tenant_id.as_str(),
        scope.project_id.as_str(),
        generation,
        not_before_ms,
    )
}

fn queue_gc_candidate_parts(
    transaction: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    generation: &str,
    not_before_ms: i64,
) -> Result<(), ArtifactStoreError> {
    if not_before_ms <= 0 {
        return Err(DomainError::InvalidArtifactReference(
            "artifact GC not_before must be positive".to_owned(),
        )
        .into());
    }
    let object_key = format!("objects/{tenant_id}/{project_id}/{generation}.blob");
    transaction
        .execute(
            "INSERT INTO artifact_gc_queue
                (tenant_id, project_id, generation, object_key, not_before_ms,
                 next_attempt_ms, attempts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0)
             ON CONFLICT (object_key) DO UPDATE SET
                not_before_ms = max(artifact_gc_queue.not_before_ms, excluded.not_before_ms),
                next_attempt_ms = max(artifact_gc_queue.next_attempt_ms, excluded.next_attempt_ms)",
            params![tenant_id, project_id, generation, object_key, not_before_ms],
        )
        .map_err(|error| storage("gc_queue", error))?;
    Ok(())
}

fn reap_expired_reservations(
    transaction: &Transaction<'_>,
    now_ms: i64,
    limit: usize,
) -> Result<usize, ArtifactStoreError> {
    let limit = i64::try_from(limit).map_err(|_| ArtifactStoreError::Storage {
        operation: "gc_reap_limit",
        detail: "limit exceeds i64".to_owned(),
    })?;
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT tenant_id, project_id, logical_path, generation, expires_at_ms
                 FROM artifact_reservations
                 WHERE expires_at_ms <= ?1
                 ORDER BY expires_at_ms ASC
                 LIMIT ?2",
            )
            .map_err(|error| storage("gc_expired_prepare", error))?;
        statement
            .query_map(params![now_ms, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|error| storage("gc_expired_query", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage("gc_expired_collect", error))?
    };
    let mut reaped = 0;
    for (tenant_id, project_id, logical_path, generation, expires_at_ms) in rows {
        queue_gc_candidate_parts(
            transaction,
            &tenant_id,
            &project_id,
            &generation,
            expires_at_ms.saturating_add(GC_SETTLEMENT_GRACE_MS),
        )?;
        reaped += transaction
            .execute(
                "DELETE FROM artifact_reservations
                 WHERE tenant_id = ?1 AND project_id = ?2 AND logical_path = ?3
                   AND generation = ?4 AND expires_at_ms = ?5",
                params![
                    tenant_id,
                    project_id,
                    logical_path,
                    generation,
                    expires_at_ms,
                ],
            )
            .map_err(|error| storage("gc_expired_delete", error))?;
    }
    Ok(reaped)
}

fn prune_expired_receipts(
    transaction: &Transaction<'_>,
    now_ms: i64,
    limit: usize,
) -> Result<usize, ArtifactStoreError> {
    let sql_limit = i64::try_from(limit).map_err(|_| ArtifactStoreError::Storage {
        operation: "receipt_prune_limit",
        detail: "limit exceeds i64".to_owned(),
    })?;
    let reservations = transaction
        .execute(
            "DELETE FROM artifact_reservation_receipts
             WHERE rowid IN (
                 SELECT rowid FROM artifact_reservation_receipts
                 WHERE retain_until_ms <= ?1
                 ORDER BY retain_until_ms ASC
                 LIMIT ?2
             )",
            params![now_ms, sql_limit],
        )
        .map_err(|error| storage("reservation_receipt_prune", error))?;
    let remaining = limit.saturating_sub(reservations);
    if remaining == 0 {
        return Ok(reservations);
    }
    let remaining = i64::try_from(remaining).map_err(|_| ArtifactStoreError::Storage {
        operation: "receipt_prune_limit",
        detail: "remaining limit exceeds i64".to_owned(),
    })?;
    let publications = transaction
        .execute(
            "DELETE FROM artifact_publication_receipts
             WHERE rowid IN (
                 SELECT rowid FROM artifact_publication_receipts
                 WHERE retain_until_ms <= ?1
                 ORDER BY retain_until_ms ASC
                 LIMIT ?2
             )",
            params![now_ms, remaining],
        )
        .map_err(|error| storage("publication_receipt_prune", error))?;
    Ok(reservations.saturating_add(publications))
}

fn prune_expired_read_retentions(
    transaction: &Transaction<'_>,
    now_ms: i64,
    limit: usize,
) -> Result<usize, ArtifactStoreError> {
    let limit = i64::try_from(limit).map_err(|_| ArtifactStoreError::Storage {
        operation: "read_retention_prune_limit",
        detail: "limit exceeds i64".to_owned(),
    })?;
    transaction
        .execute(
            "DELETE FROM artifact_read_retentions
             WHERE rowid IN (
                 SELECT rowid FROM artifact_read_retentions
                 WHERE retain_until_ms <= ?1
                 ORDER BY retain_until_ms ASC
                 LIMIT ?2
             )",
            params![now_ms, limit],
        )
        .map_err(|error| storage("read_retention_prune", error))
}

fn validate_gc_request(now_ms: i64, limit: usize) -> Result<(), ArtifactStoreError> {
    if now_ms <= 0 || limit == 0 || limit > MAX_GC_BATCH {
        return Err(DomainError::InvalidArtifactReference(format!(
            "garbage collection requires positive now_ms and limit between 1 and {MAX_GC_BATCH}"
        ))
        .into());
    }
    Ok(())
}

fn claim_gc_candidates(
    transaction: &Transaction<'_>,
    now_ms: i64,
    limit: usize,
) -> Result<Vec<GarbageLease>, ArtifactStoreError> {
    let limit = i64::try_from(limit).map_err(|_| ArtifactStoreError::Storage {
        operation: "gc_claim_limit",
        detail: "limit exceeds i64".to_owned(),
    })?;
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT queue.tenant_id, queue.project_id, queue.generation,
                        queue.object_key, queue.attempts
                 FROM artifact_gc_queue AS queue
                 WHERE queue.not_before_ms <= ?1 AND queue.next_attempt_ms <= ?1
                   AND (queue.lease_until_ms IS NULL OR queue.lease_until_ms <= ?1)
                   AND NOT EXISTS (
                       SELECT 1 FROM artifact_paths AS paths
                       WHERE paths.object_key = queue.object_key
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM artifact_reservations AS reservations
                       WHERE reservations.tenant_id = queue.tenant_id
                         AND reservations.project_id = queue.project_id
                         AND reservations.generation = queue.generation
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM artifact_read_retentions AS retention
                       WHERE retention.tenant_id = queue.tenant_id
                         AND retention.project_id = queue.project_id
                         AND retention.generation = queue.generation
                         AND retention.retain_until_ms > ?1
                   )
                 ORDER BY queue.next_attempt_ms ASC, queue.object_key ASC
                 LIMIT ?2",
            )
            .map_err(|error| storage("gc_claim_prepare", error))?;
        statement
            .query_map(params![now_ms, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|error| storage("gc_claim_query", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage("gc_claim_collect", error))?
    };
    let lease_until_ms = now_ms.saturating_add(GC_LEASE_MS);
    let mut claimed = Vec::with_capacity(rows.len());
    for (tenant_id, project_id, generation, object_key, attempts) in rows {
        let lease_token = format!("lease_{}", new_id());
        let updated = transaction
            .execute(
                "UPDATE artifact_gc_queue
                 SET lease_token = ?2, lease_until_ms = ?3
                 WHERE object_key = ?1
                   AND (lease_until_ms IS NULL OR lease_until_ms <= ?4)",
                params![&object_key, &lease_token, lease_until_ms, now_ms],
            )
            .map_err(|error| storage("gc_claim_update", error))?;
        if updated == 1 {
            claimed.push(GarbageLease {
                scope: ProjectScope::new(
                    TenantId::try_from(tenant_id)?,
                    ProjectId::try_from(project_id)?,
                ),
                generation,
                object_key,
                lease_token,
                attempts: u32::try_from(attempts).map_err(|_| ArtifactStoreError::Storage {
                    operation: "gc_attempts_decode",
                    detail: "attempt count is outside u32".to_owned(),
                })?,
            });
        }
    }
    Ok(claimed)
}

fn acknowledge_gc(
    connection: &Connection,
    candidate: &GarbageLease,
) -> Result<(), ArtifactStoreError> {
    connection
        .execute(
            "DELETE FROM artifact_gc_queue WHERE object_key = ?1 AND lease_token = ?2",
            params![&candidate.object_key, &candidate.lease_token],
        )
        .map_err(|error| storage("gc_ack", error))?;
    Ok(())
}

fn fail_gc(
    connection: &Connection,
    candidate: &GarbageLease,
    now_ms: i64,
    detail: &str,
) -> Result<(), ArtifactStoreError> {
    let exponent = candidate.attempts.min(10);
    let delay_ms = 1_000_i64.saturating_mul(1_i64 << exponent);
    let next_attempt_ms = now_ms.saturating_add(delay_ms);
    let detail = detail
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(2_048)
        .collect::<String>();
    connection
        .execute(
            "UPDATE artifact_gc_queue
             SET attempts = attempts + 1, next_attempt_ms = ?3,
                 lease_token = NULL, lease_until_ms = NULL, last_error = ?4
             WHERE object_key = ?1 AND lease_token = ?2",
            params![
                &candidate.object_key,
                &candidate.lease_token,
                next_attempt_ms,
                detail,
            ],
        )
        .map_err(|storage_error| storage("gc_retry", storage_error))?;
    Ok(())
}

fn load_published_row(
    connection: &Connection,
    scope: &ProjectScope,
    path: &ArtifactPath,
) -> Result<Option<PublishedRow>, ArtifactStoreError> {
    let row: Option<PublishedSqlRow> = connection
        .query_row(
            "SELECT artifact_id, revision, mutation_token, generation, object_key,
                        size_bytes, etag, version, digest
                 FROM artifact_paths
                 WHERE tenant_id = ?1 AND project_id = ?2 AND logical_path = ?3",
            params![
                scope.tenant_id.as_str(),
                scope.project_id.as_str(),
                path.as_str()
            ],
            |row| {
                Ok(PublishedSqlRow {
                    artifact_id: row.get(0)?,
                    revision: row.get(1)?,
                    mutation_token: row.get(2)?,
                    generation: row.get(3)?,
                    object_key: row.get(4)?,
                    size_bytes: row.get(5)?,
                    etag: row.get(6)?,
                    version: row.get(7)?,
                    digest: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(|error| storage("published_read", error))?;
    row.map(|row| {
        Ok(PublishedRow {
            artifact_id: ArtifactId::try_from(row.artifact_id)?,
            revision: Revision::new(from_i64(row.revision, "revision")?)?,
            mutation_token: row.mutation_token,
            generation: row.generation,
            object_key: row.object_key,
            size_bytes: from_i64(row.size_bytes, "size_bytes")?,
            etag: row.etag,
            version: row.version,
            digest: row.digest.map(ContentDigest::try_from).transpose()?,
        })
    })
    .transpose()
}

fn ensure_live_reservation_connection(
    connection: &Connection,
    reservation: &ArtifactReservation,
    now_ms: i64,
) -> Result<(), ArtifactStoreError> {
    let row: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT mutation_token, generation, expires_at_ms
             FROM artifact_reservations
             WHERE tenant_id = ?1 AND project_id = ?2 AND logical_path = ?3
               AND artifact_id = ?4 AND revision = ?5",
            params![
                reservation.scope.tenant_id.as_str(),
                reservation.scope.project_id.as_str(),
                reservation.path.as_str(),
                reservation.artifact_id.as_str(),
                to_i64(reservation.revision.get(), "revision")?,
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage("reservation_read", error))?;
    check_live_reservation(row, reservation, now_ms)
}

fn ensure_live_reservation_tx(
    transaction: &Transaction<'_>,
    reservation: &ArtifactReservation,
    now_ms: i64,
) -> Result<(), ArtifactStoreError> {
    ensure_live_reservation_connection(transaction, reservation, now_ms)
}

fn check_live_reservation(
    row: Option<(String, String, i64)>,
    reservation: &ArtifactReservation,
    now_ms: i64,
) -> Result<(), ArtifactStoreError> {
    let Some((mutation_token, generation, expires_at_ms)) = row else {
        return Err(ArtifactStoreError::NotFound);
    };
    if mutation_token != reservation.mutation_token
        || generation != reservation.generation
        || expires_at_ms != reservation.expires_at_ms
    {
        return Err(ArtifactStoreError::Conflict);
    }
    if now_ms >= expires_at_ms {
        return Err(ArtifactStoreError::ReservationExpired);
    }
    Ok(())
}

fn delete_reservation(
    transaction: &Transaction<'_>,
    reservation: &ArtifactReservation,
) -> Result<usize, ArtifactStoreError> {
    transaction
        .execute(
            "DELETE FROM artifact_reservations
             WHERE tenant_id = ?1 AND project_id = ?2 AND logical_path = ?3
               AND artifact_id = ?4 AND revision = ?5
               AND mutation_token = ?6 AND generation = ?7 AND expires_at_ms = ?8",
            params![
                reservation.scope.tenant_id.as_str(),
                reservation.scope.project_id.as_str(),
                reservation.path.as_str(),
                reservation.artifact_id.as_str(),
                to_i64(reservation.revision.get(), "revision")?,
                &reservation.mutation_token,
                &reservation.generation,
                reservation.expires_at_ms,
            ],
        )
        .map_err(|error| storage("reservation_delete", error))
}

fn object_key(scope: &ProjectScope, generation: &str) -> String {
    format!(
        "objects/{}/{}/{}.blob",
        scope.tenant_id.as_str(),
        scope.project_id.as_str(),
        generation
    )
}

fn publication_matches(
    reference: &ArtifactReference,
    reservation: &ArtifactReservation,
    observation: &ArtifactObservation,
) -> bool {
    reference.artifact_id == reservation.artifact_id
        && reference.scope == reservation.scope
        && reference.path == reservation.path
        && reference.revision == reservation.revision
        && reference.mutation_token == reservation.mutation_token
        && matches!(
            &reference.body,
            ArtifactBodyRef::Object {
                object_key,
                generation,
                size_bytes,
                etag,
                version,
                digest,
            } if object_key == &observation.object_key
                && generation == &observation.generation
                && *size_bytes == observation.size_bytes
                && etag == &observation.etag
                && version == &observation.version
                && digest == &observation.digest
        )
}

fn ensure_object_parent(root: &Path, scope: &ProjectScope) -> Result<PathBuf, ArtifactStoreError> {
    let mut current = root.to_path_buf();
    for component in [
        "objects",
        scope.tenant_id.as_str(),
        scope.project_id.as_str(),
    ] {
        current.push(component);
        if fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(filesystem(
                "object_directory",
                &current,
                "object directory component must not be a symbolic link",
            ));
        }
        fs::create_dir(&current)
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .map_err(|error| filesystem_error("object_directory", &current, error))?;
    }
    Ok(current)
}

fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_digest: &ContentDigest,
) -> Result<(), ArtifactStoreError> {
    let bytes =
        fs::read(path).map_err(|error| filesystem_error("object_verify_read", path, error))?;
    if bytes.len() as u64 != expected_size || digest_bytes(&bytes)? != *expected_digest {
        return Err(ArtifactStoreError::BodyMismatch);
    }
    Ok(())
}

fn digest_bytes(bytes: &[u8]) -> Result<ContentDigest, DomainError> {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        FmtWrite::write_fmt(&mut encoded, format_args!("{byte:02x}")).map_err(|error| {
            DomainError::InvalidArtifactReference(format!("digest encoding failed: {error}"))
        })?;
    }
    ContentDigest::try_from(encoded)
}

fn new_id() -> String {
    Ulid::new().to_string().to_ascii_lowercase()
}

fn storage(operation: &'static str, error: rusqlite::Error) -> ArtifactStoreError {
    ArtifactStoreError::Storage {
        operation,
        detail: error.to_string(),
    }
}

fn filesystem_error(
    operation: &'static str,
    path: &Path,
    error: std::io::Error,
) -> ArtifactStoreError {
    filesystem(operation, path, &error.to_string())
}

fn filesystem(operation: &'static str, path: &Path, detail: &str) -> ArtifactStoreError {
    ArtifactStoreError::Filesystem {
        operation,
        path: path.to_path_buf(),
        detail: detail.to_owned(),
    }
}

fn to_i64(value: u64, field: &'static str) -> Result<i64, ArtifactStoreError> {
    i64::try_from(value)
        .map_err(|_| DomainError::InvalidArtifactReference(format!("{field} exceeds i64")).into())
}

fn from_i64(value: i64, field: &'static str) -> Result<u64, ArtifactStoreError> {
    u64::try_from(value).map_err(|_| ArtifactStoreError::Storage {
        operation: "integer_decode",
        detail: format!("{field} is negative"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidememo_domain::{ProjectId, TenantId};
    use std::sync::{Arc, Barrier};

    fn scope(tenant: &str, project: &str) -> Result<ProjectScope, DomainError> {
        Ok(ProjectScope::new(
            TenantId::try_from(tenant)?,
            ProjectId::try_from(project)?,
        ))
    }

    fn reserve_request(
        scope: ProjectScope,
        path: ArtifactPath,
        expected_mutation_token: Option<String>,
        now_ms: i64,
    ) -> ReserveRequest {
        ReserveRequest {
            request_id: format!("request_{}", new_id()),
            scope,
            path,
            expected_mutation_token,
            now_ms,
            ttl_ms: 60_000,
        }
    }

    #[test]
    fn reserve_upload_publish_read_and_replace_are_cas_guarded()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        let scope = scope("tenant_a", "project_a")?;
        let path = ArtifactPath::try_from("/sessions/session_a/result.json")?;
        let first = store.reserve(reserve_request(scope.clone(), path.clone(), None, 1_000))?;
        let observation = store.upload(&first, br#"{"result":"one"}"#, 1_001)?;
        let published = store.publish(&first, &observation, 1_002)?;
        assert_eq!(store.publish(&first, &observation, 1_003)?, published);
        assert_eq!(published.revision.get(), 1);
        assert_eq!(store.read(&published)?, br#"{"result":"one"}"#);
        assert!(matches!(
            store.reserve(reserve_request(scope.clone(), path.clone(), None, 2_000)),
            Err(ArtifactStoreError::Conflict)
        ));

        let second = store.reserve(reserve_request(
            scope.clone(),
            path.clone(),
            Some(published.mutation_token.clone()),
            2_000,
        ))?;
        assert_eq!(second.artifact_id, first.artifact_id);
        assert_eq!(second.revision.get(), 2);
        let second_observation = store.upload(&second, b"two", 2_001)?;
        let replaced = store.publish(&second, &second_observation, 2_002)?;
        assert_eq!(replaced.revision.get(), 2);
        assert_eq!(store.read(&replaced)?, b"two");
        let gc = store.drain_garbage(2_003, 100)?;
        assert_eq!(gc.deleted, 1);
        assert_eq!(store.publish(&first, &observation, 2_004)?, published);
        assert_eq!(store.get(&scope, &path)?, Some(replaced));
        Ok(())
    }

    #[test]
    fn expired_or_tampered_publication_never_replaces_current_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        let scope = scope("tenant_a", "project_a")?;
        let path = ArtifactPath::try_from("/sessions/session_a/canvas.md")?;
        let reservation = store.reserve(ReserveRequest {
            request_id: "request_expiring".to_owned(),
            scope: scope.clone(),
            path: path.clone(),
            expected_mutation_token: None,
            now_ms: 1_000,
            ttl_ms: 10,
        })?;
        assert!(matches!(
            store.upload(&reservation, b"late", 1_010),
            Err(ArtifactStoreError::ReservationExpired)
        ));

        let fresh = store.reserve(reserve_request(scope.clone(), path.clone(), None, 2_000))?;
        let mut observation = store.upload(&fresh, b"safe", 2_001)?;
        observation.size_bytes += 1;
        assert!(matches!(
            store.publish(&fresh, &observation, 2_002),
            Err(ArtifactStoreError::BodyMismatch)
        ));
        assert!(store.get(&scope, &path)?.is_none());
        Ok(())
    }

    #[test]
    fn active_reservations_are_exclusive_and_abort_preserves_previous_version()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        let scope = scope("tenant_a", "project_a")?;
        let path = ArtifactPath::try_from("/handoffs/handoff_a/result.json")?;
        let first = store.reserve(reserve_request(scope.clone(), path.clone(), None, 1_000))?;
        assert!(matches!(
            store.reserve(reserve_request(scope.clone(), path.clone(), None, 1_001)),
            Err(ArtifactStoreError::Conflict)
        ));
        let observed = store.upload(&first, b"v1", 1_002)?;
        let published = store.publish(&first, &observed, 1_003)?;

        let replacement = store.reserve(reserve_request(
            scope.clone(),
            path.clone(),
            Some(published.mutation_token.clone()),
            2_000,
        ))?;
        store.abort(&replacement)?;
        assert_eq!(store.get(&scope, &path)?, Some(published));
        assert!(matches!(
            store.publish(&replacement, &observed, 2_001),
            Err(ArtifactStoreError::BodyMismatch | ArtifactStoreError::NotFound)
        ));
        Ok(())
    }

    #[test]
    fn tenant_scope_and_logical_path_never_control_object_resolution()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        let scope_a = scope("tenant_a", "project_shared")?;
        let scope_b = scope("tenant_b", "project_shared")?;
        let path = ArtifactPath::try_from("/sessions/shared/result.json")?;
        let a = store.reserve(reserve_request(scope_a.clone(), path.clone(), None, 1_000))?;
        let b = store.reserve(reserve_request(scope_b.clone(), path.clone(), None, 1_000))?;
        let a_ref = store.publish(&a, &store.upload(&a, b"a", 1_001)?, 1_002)?;
        let b_ref = store.publish(&b, &store.upload(&b, b"b", 1_001)?, 1_002)?;
        assert_eq!(store.read(&a_ref)?, b"a");
        assert_eq!(store.read(&b_ref)?, b"b");
        assert_ne!(a_ref.body, b_ref.body);
        Ok(())
    }

    #[test]
    fn published_metadata_and_bytes_survive_reopen_and_exact_upload_retry()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let scope = scope("tenant_restart", "project_restart")?;
        let path = ArtifactPath::try_from("/sessions/restart/canvas.md")?;
        let published = {
            let mut store = LocalArtifactStore::open(directory.path())?;
            let reservation =
                store.reserve(reserve_request(scope.clone(), path.clone(), None, 1_000))?;
            let first = store.upload(&reservation, b"durable", 1_001)?;
            let retry = store.upload(&reservation, b"durable", 1_002)?;
            assert_eq!(retry, first);
            store.publish(&reservation, &first, 1_003)?
        };

        let reopened = LocalArtifactStore::open(directory.path())?;
        assert_eq!(reopened.get(&scope, &path)?, Some(published.clone()));
        assert_eq!(reopened.read(&published)?, b"durable");
        Ok(())
    }

    #[test]
    fn reservation_request_replays_exactly_and_rejects_changed_reuse()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        let scope = scope("tenant_replay", "project_replay")?;
        let path = ArtifactPath::try_from("/sessions/replay/result.json")?;
        let request = ReserveRequest {
            request_id: "request_replay".to_owned(),
            scope: scope.clone(),
            path: path.clone(),
            expected_mutation_token: None,
            now_ms: 1_000,
            ttl_ms: 60_000,
        };
        let first = store.reserve(request.clone())?;
        let mut retry = request.clone();
        retry.now_ms = 2_000;
        assert_eq!(store.reserve(retry)?, first);

        let mut changed = request;
        changed.path = ArtifactPath::try_from("/sessions/replay/other.json")?;
        assert!(matches!(
            store.reserve(changed),
            Err(ArtifactStoreError::Conflict)
        ));
        Ok(())
    }

    #[test]
    fn abort_and_replacement_queue_only_unreachable_generations()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        let scope = scope("tenant_gc", "project_gc")?;
        let aborted_path = ArtifactPath::try_from("/sessions/gc/aborted.bin")?;
        let aborted = store.reserve(reserve_request(scope.clone(), aborted_path, None, 1_000))?;
        let aborted_observation = store.upload(&aborted, b"aborted", 1_001)?;
        let aborted_object = store.object_path(&aborted_observation.object_key)?;
        store.abort(&aborted)?;
        assert_eq!(store.drain_garbage(120_999, 10)?.claimed, 0);
        assert!(aborted_object.exists());
        let aborted_report = store.drain_garbage(121_000, 10)?;
        assert_eq!(aborted_report.deleted, 1);
        assert!(!aborted_object.exists());

        let path = ArtifactPath::try_from("/sessions/gc/current.bin")?;
        let first = store.reserve(reserve_request(scope.clone(), path.clone(), None, 200_000))?;
        let first_observation = store.upload(&first, b"first", 200_001)?;
        let first_object = store.object_path(&first_observation.object_key)?;
        let first_reference = store.publish(&first, &first_observation, 200_002)?;
        let second = store.reserve(reserve_request(
            scope.clone(),
            path.clone(),
            Some(first_reference.mutation_token),
            201_000,
        ))?;
        let second_observation = store.upload(&second, b"second", 201_001)?;
        let second_reference = store.publish(&second, &second_observation, 201_002)?;
        let replacement_report = store.drain_garbage(201_002, 10)?;
        assert_eq!(replacement_report.deleted, 1);
        assert!(!first_object.exists());
        assert_eq!(store.read(&second_reference)?, b"second");
        Ok(())
    }

    #[test]
    fn expired_reservations_are_reaped_before_durable_gc() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        let reservation = store.reserve(ReserveRequest {
            request_id: "request_gc_expiry".to_owned(),
            scope: scope("tenant_expiry", "project_expiry")?,
            path: ArtifactPath::try_from("/sessions/expiry/body.bin")?,
            expected_mutation_token: None,
            now_ms: 1_000,
            ttl_ms: 10,
        })?;
        let observation = store.upload(&reservation, b"expires", 1_001)?;
        let object_path = store.object_path(&observation.object_key)?;
        let first = store.drain_garbage(1_010, 10)?;
        assert_eq!(first.expired_reservations, 1);
        assert_eq!(first.claimed, 0);
        assert!(object_path.exists());
        let second = store.drain_garbage(61_010, 10)?;
        assert_eq!(second.deleted, 1);
        assert!(!object_path.exists());
        Ok(())
    }

    #[test]
    fn schema_v1_catalog_migrates_to_replay_and_gc_tables() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let connection = Connection::open(directory.path().join("catalog.sqlite"))?;
        connection.execute_batch(
            "CREATE TABLE artifact_paths (
                tenant_id TEXT NOT NULL, project_id TEXT NOT NULL,
                logical_path TEXT NOT NULL, artifact_id TEXT NOT NULL,
                revision INTEGER NOT NULL, mutation_token TEXT NOT NULL,
                generation TEXT NOT NULL, object_key TEXT NOT NULL,
                size_bytes INTEGER NOT NULL, etag TEXT NOT NULL, version TEXT,
                digest TEXT NOT NULL, published_at_ms INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, project_id, logical_path)
             ) STRICT;
             CREATE TABLE artifact_reservations (
                tenant_id TEXT NOT NULL, project_id TEXT NOT NULL,
                logical_path TEXT NOT NULL, artifact_id TEXT NOT NULL,
                revision INTEGER NOT NULL, mutation_token TEXT NOT NULL,
                generation TEXT NOT NULL, expires_at_ms INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, project_id, logical_path)
             ) STRICT;
             PRAGMA user_version = 1;",
        )?;
        drop(connection);

        let mut store = LocalArtifactStore::open(directory.path())?;
        let version: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, SCHEMA_VERSION);
        let request = reserve_request(
            scope("tenant_migrate", "project_migrate")?,
            ArtifactPath::try_from("/sessions/migrate/result.bin")?,
            None,
            1_000,
        );
        let reservation = store.reserve(request.clone())?;
        assert_eq!(store.reserve(request)?, reservation);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_scope_directory_is_rejected_before_body_write()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        symlink(outside.path(), directory.path().join("objects/tenant_link"))?;
        let reservation = store.reserve(reserve_request(
            scope("tenant_link", "project_a")?,
            ArtifactPath::try_from("/sessions/link/result.json")?,
            None,
            1_000,
        ))?;
        assert!(matches!(
            store.upload(&reservation, b"blocked", 1_001),
            Err(ArtifactStoreError::Filesystem { .. })
        ));
        assert!(!outside.path().join("project_a").exists());
        Ok(())
    }

    #[test]
    fn concurrent_reservers_create_exactly_one_live_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().to_path_buf();
        let stores = [
            LocalArtifactStore::open(&root)?,
            LocalArtifactStore::open(&root)?,
        ];
        let scope = scope("tenant_race", "project_race")?;
        let path = ArtifactPath::try_from("/sessions/race/result.json")?;
        let barrier = Arc::new(Barrier::new(2));
        let handles = stores
            .into_iter()
            .map(|mut store| {
                let scope = scope.clone();
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.reserve(reserve_request(scope, path, None, 1_000))
                })
            })
            .collect::<Vec<_>>();
        let mut success = 0;
        let mut conflict = 0;
        for handle in handles {
            match handle
                .join()
                .map_err(|_| std::io::Error::other("reservation thread panicked"))?
            {
                Ok(_) => success += 1,
                Err(ArtifactStoreError::Conflict) => conflict += 1,
                Err(error) => return Err(error.into()),
            }
        }
        assert_eq!(success, 1);
        assert_eq!(conflict, 1);
        Ok(())
    }

    #[test]
    fn trusted_hosted_publication_preserves_optional_digest()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let scope = scope("tenant_hosted", "project_hosted")?;
        let path = ArtifactPath::try_from("/sessions/hosted/result.bin")?;
        let published = {
            let mut store = LocalArtifactStore::open(directory.path())?;
            let reservation =
                store.reserve(reserve_request(scope.clone(), path.clone(), None, 1_000))?;
            let observation = ArtifactObservation {
                object_key: format!(
                    "hosted/objects/{}/{}/{}.blob",
                    scope.tenant_id, scope.project_id, reservation.generation
                ),
                generation: reservation.generation.clone(),
                size_bytes: 12,
                etag: "provider-etag".to_owned(),
                version: Some("provider-version".to_owned()),
                digest: None,
            };
            let published = store.publish_trusted_observation(&reservation, &observation, 1_001)?;
            assert_eq!(
                store.publish_trusted_observation(&reservation, &observation, 1_002)?,
                published
            );
            published
        };
        let reopened = LocalArtifactStore::open(directory.path())?;
        assert_eq!(reopened.get(&scope, &path)?, Some(published.clone()));
        assert!(matches!(
            published.body,
            ArtifactBodyRef::Object { digest: None, .. }
        ));
        Ok(())
    }

    #[test]
    fn durable_read_retention_blocks_gc_leases_until_expiry()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        let scope = scope("tenant_retention", "project_retention")?;
        let path = ArtifactPath::try_from("/sessions/retention/result.bin")?;
        let first = store.reserve(reserve_request(scope.clone(), path.clone(), None, 1_000))?;
        let first_observation = store.upload(&first, b"first", 1_001)?;
        let first_reference = store.publish(&first, &first_observation, 1_002)?;
        store.retain_for_read(&first_reference, 1_003, 5_000)?;

        let second = store.reserve(reserve_request(
            scope,
            path,
            Some(first_reference.mutation_token.clone()),
            2_000,
        ))?;
        let second_observation = store.upload(&second, b"second", 2_001)?;
        store.publish(&second, &second_observation, 2_002)?;

        let early = store.claim_garbage(4_999, 10)?;
        assert_eq!(early.report.claimed, 0);
        assert!(early.leases.is_empty());
        let due = store.claim_garbage(5_000, 10)?;
        assert_eq!(due.report.claimed, 1);
        assert_eq!(due.report.pruned_read_retentions, 1);
        assert_eq!(due.leases[0].generation, first.generation);
        store.acknowledge_garbage(&due.leases[0])?;
        Ok(())
    }

    #[test]
    fn schema_v2_catalog_migrates_digest_to_optional_and_keeps_publication()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let scope = scope("tenant_v2", "project_v2")?;
        let path = ArtifactPath::try_from("/sessions/v2/result.bin")?;
        let published = {
            let mut store = LocalArtifactStore::open(directory.path())?;
            let reservation =
                store.reserve(reserve_request(scope.clone(), path.clone(), None, 1_000))?;
            let observation = store.upload(&reservation, b"v2-body", 1_001)?;
            store.publish(&reservation, &observation, 1_002)?
        };
        let database = directory.path().join("catalog.sqlite");
        let connection = Connection::open(&database)?;
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             DROP TABLE artifact_read_retentions;
             ALTER TABLE artifact_paths RENAME TO artifact_paths_v3;
             CREATE TABLE artifact_paths (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                logical_path TEXT NOT NULL,
                artifact_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                mutation_token TEXT NOT NULL,
                generation TEXT NOT NULL,
                object_key TEXT NOT NULL,
                size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
                etag TEXT NOT NULL,
                version TEXT,
                digest TEXT NOT NULL,
                published_at_ms INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, project_id, logical_path)
             ) STRICT;
             INSERT INTO artifact_paths SELECT * FROM artifact_paths_v3;
             DROP TABLE artifact_paths_v3;
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
        drop(connection);

        let reopened = LocalArtifactStore::open(directory.path())?;
        assert_eq!(reopened.get(&scope, &path)?, Some(published));
        let version: i64 = reopened
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, SCHEMA_VERSION);
        let digest_not_null: i64 = reopened.connection.query_row(
            "SELECT \"notnull\" FROM pragma_table_info('artifact_paths') WHERE name = 'digest'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(digest_not_null, 0);
        let identity: String = reopened.connection.query_row(
            "SELECT value FROM artifact_repository_meta WHERE key = 'body_store_identity'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(identity, LOCAL_BODY_STORE_IDENTITY);
        Ok(())
    }

    #[test]
    fn body_store_identity_binds_once_and_rejects_adapter_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        store.bind_body_store(LOCAL_BODY_STORE_IDENTITY)?;
        store.bind_body_store(LOCAL_BODY_STORE_IDENTITY)?;
        assert!(matches!(
            store.bind_body_store(
                "s3:v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            Err(ArtifactStoreError::BodyStoreMismatch)
        ));
        drop(store);

        let mut reopened = LocalArtifactStore::open(directory.path())?;
        assert!(matches!(
            reopened.bind_body_store(
                "s3:v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            Err(ArtifactStoreError::BodyStoreMismatch)
        ));
        Ok(())
    }

    #[test]
    fn populated_unidentified_catalog_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut store = LocalArtifactStore::open(directory.path())?;
        store.reserve(reserve_request(
            scope("tenant_unbound", "project_unbound")?,
            ArtifactPath::try_from("/sessions/unbound/result.bin")?,
            None,
            1_000,
        ))?;
        assert!(matches!(
            store.bind_body_store(LOCAL_BODY_STORE_IDENTITY),
            Err(ArtifactStoreError::BodyStoreIdentityMissing)
        ));
        Ok(())
    }

    #[test]
    fn concurrent_opens_both_configure_wal() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let root = root.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    LocalArtifactStore::open(root)
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle
                .join()
                .map_err(|_| std::io::Error::other("open thread panicked"))??;
        }
        Ok(())
    }
}
