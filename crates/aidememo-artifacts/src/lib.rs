//! Local Phase-1 artifact repository with immutable body generations.
//!
//! Canonical logical paths and publication metadata live in a private SQLite
//! catalog. Body bytes are written once under adapter-owned generation keys.
//! A caller must reserve a path, upload the selected generation, and publish
//! the server-observed metadata before readers can discover it.

use aidememo_domain::{
    ArtifactBodyRef, ArtifactId, ArtifactObservation, ArtifactPath, ArtifactReference,
    ArtifactReservation, ContentDigest, DomainError, ProjectScope, Revision,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tempfile::NamedTempFile;
use thiserror::Error;
use ulid::Ulid;

const SCHEMA_VERSION: i64 = 1;
const MAX_TTL_MS: i64 = 24 * 60 * 60 * 1_000;
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// The bounded direct-upload API rejected an oversized body.
    #[error("artifact upload exceeds the local limit of {limit_bytes} bytes")]
    UploadTooLarge {
        /// Maximum accepted body size.
        limit_bytes: usize,
    },
}

/// Input to an exclusive path reservation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReserveRequest {
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
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| storage("journal_mode", error))?;
        migrate(&connection)?;
        Ok(Self {
            root: root.to_path_buf(),
            connection,
        })
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
        let published = load_published_row(&transaction, &request.scope, &request.path)?;
        match (&published, request.expected_mutation_token.as_deref()) {
            (None, None) => {}
            (Some(current), Some(expected)) if current.mutation_token == expected => {}
            _ => return Err(ArtifactStoreError::Conflict),
        }
        let active_expiry: Option<i64> = transaction
            .query_row(
                "SELECT expires_at_ms FROM artifact_reservations
                 WHERE tenant_id = ?1 AND project_id = ?2 AND logical_path = ?3",
                params![
                    request.scope.tenant_id.as_str(),
                    request.scope.project_id.as_str(),
                    request.path.as_str(),
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage("reservation_current", error))?;
        if active_expiry.is_some_and(|expiry| expiry > request.now_ms) {
            return Err(ArtifactStoreError::Conflict);
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
            scope: request.scope,
            path: request.path,
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
        if bytes.len() > MAX_UPLOAD_BYTES {
            return Err(ArtifactStoreError::UploadTooLarge {
                limit_bytes: MAX_UPLOAD_BYTES,
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
        let body = observation.body_ref()?;
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
        verify_file(
            &self.object_path(&expected_key)?,
            observation.size_bytes,
            digest,
        )?;

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
                    digest.as_str(),
                    now_ms,
                ],
            )
            .map_err(|error| storage("publication_write", error))?;
        delete_reservation(&transaction, reservation)?;
        transaction
            .commit()
            .map_err(|error| storage("publish_commit", error))?;
        let reference = ArtifactReference {
            artifact_id: reservation.artifact_id.clone(),
            scope: reservation.scope.clone(),
            path: reservation.path.clone(),
            revision: reservation.revision,
            mutation_token: reservation.mutation_token.clone(),
            body,
        };
        reference.validate()?;
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
    digest: ContentDigest,
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
    digest: String,
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
                digest: Some(self.digest),
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
                    digest TEXT NOT NULL,
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
                 PRAGMA user_version = 1;
                 COMMIT;",
            )
            .map_err(|error| storage("schema_create", error)),
        SCHEMA_VERSION => Ok(()),
        other => Err(ArtifactStoreError::Storage {
            operation: "schema_version",
            detail: format!("artifact schema {other} is not supported by version {SCHEMA_VERSION}"),
        }),
    }
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
            digest: ContentDigest::try_from(row.digest)?,
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
        let scope = scope("tenant_race", "project_race")?;
        let path = ArtifactPath::try_from("/sessions/race/result.json")?;
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let root = root.clone();
                let scope = scope.clone();
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut store = LocalArtifactStore::open(root)?;
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
}
