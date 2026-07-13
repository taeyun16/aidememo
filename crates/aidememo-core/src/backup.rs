//! SQLite snapshot backup / restore helpers.
//!
//! This module treats object storage as a backup target, not as the hot
//! database. The hot store remains a local SQLite file; backup creates a
//! point-in-time SQLite snapshot, writes a manifest, and restore verifies the
//! manifest before replacing the local store.

use crate::{AideMemoError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use ulid::Ulid;

const MANIFEST_FILE: &str = "manifest.json";
const SQLITE_OBJECT: &str = "wiki.sqlite";
const COLD_SQLITE_OBJECT: &str = "wiki.cold.sqlite";
#[cfg(feature = "s3")]
const SQLITE_ZSTD_OBJECT: &str = "wiki.sqlite.zst";
#[cfg(feature = "s3")]
const COLD_SQLITE_ZSTD_OBJECT: &str = "wiki.cold.sqlite.zst";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    pub schema: u32,
    pub backup_id: String,
    pub created_at_ms: u64,
    pub backend: String,
    pub source_store: String,
    pub database: BackupDatabase,
    /// Optional backend-specific cold-tier snapshot. Older manifests omit
    /// this field and restore as a hot-only backup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold_database: Option<BackupDatabase>,
    /// High-water sync cursor for the snapshot contents. Branch push uses this
    /// as the compact delta base. Older backup manifests omit it; branch push
    /// can still fall back to a full-state segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_cursor: Option<BackupSyncCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupDatabase {
    pub object: String,
    pub compression: String,
    pub stored_bytes: u64,
    pub stored_sha256: String,
    pub sqlite_bytes: u64,
    pub sqlite_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupSyncCursor {
    pub entity: Option<String>,
    pub fact: Option<String>,
    pub entity_updated_at: Option<u64>,
    #[serde(default)]
    pub entity_updated_id: Option<String>,
    pub fact_updated_at: Option<u64>,
    #[serde(default)]
    pub fact_updated_id: Option<String>,
    #[serde(default)]
    pub relation_created_at: Option<u64>,
    #[serde(default)]
    pub relation_key: Option<String>,
    #[serde(default)]
    pub relation_generation: Option<String>,
    #[serde(default)]
    pub relation_scan_key: Option<String>,
}

impl BackupSyncCursor {
    pub fn from_sync(cursor: crate::sync::SyncCursor) -> Self {
        Self {
            entity: cursor.entity.map(|id| id.0.to_string()),
            fact: cursor.fact.map(|id| id.0.to_string()),
            entity_updated_at: cursor.entity_updated_at,
            entity_updated_id: cursor.entity_updated_id.map(|id| id.0.to_string()),
            fact_updated_at: cursor.fact_updated_at,
            fact_updated_id: cursor.fact_updated_id.map(|id| id.0.to_string()),
            relation_created_at: cursor.relation_created_at,
            relation_key: cursor.relation_key,
            relation_generation: cursor.relation_generation,
            relation_scan_key: cursor.relation_scan_key,
        }
    }

    pub fn to_sync(&self) -> Result<crate::sync::SyncCursor> {
        let parse = |name: &str, value: &Option<String>| -> Result<Option<Ulid>> {
            value
                .as_deref()
                .map(|raw| {
                    Ulid::from_string(raw).map_err(|source| {
                        AideMemoError::InvalidInput(format!(
                            "invalid backup sync cursor {name} ULID `{raw}`: {source}"
                        ))
                    })
                })
                .transpose()
        };
        Ok(crate::sync::SyncCursor {
            entity: parse("entity", &self.entity)?.map(crate::EntityId),
            fact: parse("fact", &self.fact)?.map(crate::FactId),
            entity_updated_at: self.entity_updated_at,
            entity_updated_id: parse("entity_updated_id", &self.entity_updated_id)?
                .map(crate::EntityId),
            fact_updated_at: self.fact_updated_at,
            fact_updated_id: parse("fact_updated_id", &self.fact_updated_id)?.map(crate::FactId),
            relation_created_at: self.relation_created_at,
            relation_key: self.relation_key.clone(),
            relation_generation: self.relation_generation.clone(),
            relation_scan_key: self.relation_scan_key.clone(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupCreateReport {
    pub manifest: BackupManifest,
    pub destination: String,
    pub manifest_uri: String,
    pub database_uri: String,
    pub cold_database_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRestoreReport {
    pub manifest: BackupManifest,
    pub source: String,
    pub restored_store: String,
    pub previous_store: Option<String>,
    /// Cold tier restored from the manifest, if one was present.
    pub restored_cold_store: Option<String>,
    /// Existing cold tier moved aside by `--force`. For hot-only legacy
    /// backups this is the explicit stale-archive policy: never leave
    /// unmanifested cold facts attached to the restored store.
    pub previous_cold_store: Option<String>,
    pub removed_sidecars: Vec<String>,
}

pub fn create_local_backup(
    store_path: &Path,
    backend: &str,
    destination_dir: &Path,
) -> Result<BackupCreateReport> {
    ensure_sqlite_backend(backend)?;
    let backup_id = format!("backup-{}", Ulid::new());
    let output_dir = destination_dir.join(&backup_id);
    std::fs::create_dir_all(&output_dir).map_err(|source| AideMemoError::StoreOpen {
        path: output_dir.clone(),
        source: Box::new(source),
    })?;

    let snapshots = snapshot_sqlite_tiers(store_path, backend)?;
    let snapshot = snapshots.hot;
    let sync_cursor = snapshot_sync_cursor(snapshot.path(), backend)?;

    let sqlite_bytes = std::fs::read(snapshot.path()).map_err(|source| {
        AideMemoError::FileRead(snapshot.path().to_path_buf(), source.to_string())
    })?;
    let sqlite_sha256 = sha256_hex(&sqlite_bytes);
    let db_path = output_dir.join(SQLITE_OBJECT);
    std::fs::write(&db_path, &sqlite_bytes).map_err(|source| AideMemoError::StoreWrite {
        table: "backup",
        key: db_path.display().to_string(),
        source: Box::new(source),
    })?;

    let (cold_database, cold_database_uri) = if let Some(cold_snapshot) = snapshots.cold {
        let cold_bytes = read_snapshot_bytes(cold_snapshot.path())?;
        let cold_path = output_dir.join(COLD_SQLITE_OBJECT);
        std::fs::write(&cold_path, &cold_bytes).map_err(|source| AideMemoError::StoreWrite {
            table: "backup",
            key: cold_path.display().to_string(),
            source: Box::new(source),
        })?;
        (
            Some(database_manifest(
                COLD_SQLITE_OBJECT,
                "none",
                &cold_bytes,
                &cold_bytes,
            )),
            Some(cold_path.display().to_string()),
        )
    } else {
        (None, None)
    };

    let manifest = BackupManifest {
        schema: 1,
        backup_id,
        created_at_ms: crate::time::current_epoch_ms(),
        backend: canonical_sqlite_backend(backend).to_string(),
        source_store: store_path.display().to_string(),
        database: BackupDatabase {
            object: SQLITE_OBJECT.to_string(),
            compression: "none".to_string(),
            stored_bytes: sqlite_bytes.len() as u64,
            stored_sha256: sqlite_sha256.clone(),
            sqlite_bytes: sqlite_bytes.len() as u64,
            sqlite_sha256,
        },
        cold_database,
        sync_cursor,
    };
    let manifest_path = output_dir.join(MANIFEST_FILE);
    write_manifest(&manifest_path, &manifest)?;

    Ok(BackupCreateReport {
        manifest,
        destination: output_dir.display().to_string(),
        manifest_uri: manifest_path.display().to_string(),
        database_uri: db_path.display().to_string(),
        cold_database_uri,
    })
}

pub fn restore_local_backup(
    source_dir: &Path,
    target_store_path: &Path,
    backend: &str,
    force: bool,
) -> Result<BackupRestoreReport> {
    ensure_sqlite_backend(backend)?;
    let manifest_path = source_dir.join(MANIFEST_FILE);
    let manifest = read_manifest(&manifest_path)?;
    validate_manifest_metadata(&manifest, Some(backend))?;
    let stored = std::fs::read(source_dir.join(&manifest.database.object)).map_err(|source| {
        AideMemoError::StoreRead {
            table: "backup",
            key: manifest.database.object.clone(),
            source: Box::new(source),
        }
    })?;
    let cold_stored = manifest
        .cold_database
        .as_ref()
        .map(|database| {
            std::fs::read(source_dir.join(&database.object)).map_err(|source| {
                AideMemoError::StoreRead {
                    table: "backup",
                    key: database.object.clone(),
                    source: Box::new(source),
                }
            })
        })
        .transpose()?;
    restore_from_stored_bytes(
        source_dir.display().to_string(),
        stored,
        cold_stored,
        manifest,
        target_store_path,
        backend,
        force,
    )
}

#[cfg(feature = "s3")]
pub async fn create_s3_backup(
    store_path: &Path,
    backend: &str,
    destination: &str,
) -> Result<BackupCreateReport> {
    ensure_sqlite_backend(backend)?;
    let location = S3Location::parse(destination)?;
    let backup_id = format!("backup-{}", Ulid::new());
    let prefix = location.join(&backup_id);

    let snapshots = snapshot_sqlite_tiers(store_path, backend)?;
    let snapshot = snapshots.hot;
    let sync_cursor = snapshot_sync_cursor(snapshot.path(), backend)?;

    let sqlite_bytes = std::fs::read(snapshot.path()).map_err(|source| {
        AideMemoError::FileRead(snapshot.path().to_path_buf(), source.to_string())
    })?;
    let sqlite_sha256 = sha256_hex(&sqlite_bytes);
    let stored = zstd::stream::encode_all(&sqlite_bytes[..], 0).map_err(|source| {
        AideMemoError::Internal(format!("backup compression failed: {source}"))
    })?;
    let stored_sha256 = sha256_hex(&stored);
    let db_key = prefix.object_key(SQLITE_ZSTD_OBJECT);
    let cold_backup = if let Some(cold_snapshot) = snapshots.cold {
        let cold_sqlite_bytes = read_snapshot_bytes(cold_snapshot.path())?;
        let cold_stored =
            zstd::stream::encode_all(&cold_sqlite_bytes[..], 0).map_err(|source| {
                AideMemoError::Internal(format!("cold backup compression failed: {source}"))
            })?;
        Some((
            database_manifest(
                COLD_SQLITE_ZSTD_OBJECT,
                "zstd",
                &cold_stored,
                &cold_sqlite_bytes,
            ),
            cold_stored,
        ))
    } else {
        None
    };
    let manifest = BackupManifest {
        schema: 1,
        backup_id,
        created_at_ms: crate::time::current_epoch_ms(),
        backend: canonical_sqlite_backend(backend).to_string(),
        source_store: store_path.display().to_string(),
        database: BackupDatabase {
            object: SQLITE_ZSTD_OBJECT.to_string(),
            compression: "zstd".to_string(),
            stored_bytes: stored.len() as u64,
            stored_sha256,
            sqlite_bytes: sqlite_bytes.len() as u64,
            sqlite_sha256,
        },
        cold_database: cold_backup.as_ref().map(|(database, _)| database.clone()),
        sync_cursor,
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|source| AideMemoError::Serialize {
            context: "backup manifest".to_string(),
            source,
        })?;
    let manifest_key = prefix.object_key(MANIFEST_FILE);
    let client = s3_client().await;
    put_s3_object(&client, &prefix.bucket, &db_key, stored).await?;
    let cold_database_uri = if let Some((database, cold_stored)) = cold_backup {
        let key = prefix.object_key(&database.object);
        put_s3_object(&client, &prefix.bucket, &key, cold_stored).await?;
        Some(format!("s3://{}/{}", prefix.bucket, key))
    } else {
        None
    };
    put_s3_object(&client, &prefix.bucket, &manifest_key, manifest_bytes).await?;

    Ok(BackupCreateReport {
        manifest,
        destination: prefix.to_uri(),
        manifest_uri: format!("s3://{}/{}", prefix.bucket, manifest_key),
        database_uri: format!("s3://{}/{}", prefix.bucket, db_key),
        cold_database_uri,
    })
}

#[cfg(feature = "s3")]
pub async fn restore_s3_backup(
    source: &str,
    target_store_path: &Path,
    backend: &str,
    force: bool,
) -> Result<BackupRestoreReport> {
    ensure_sqlite_backend(backend)?;
    let location = S3Location::parse(source)?;
    let client = s3_client().await;
    let manifest_key = location.object_key(MANIFEST_FILE);
    let manifest_bytes = get_s3_object(&client, &location.bucket, &manifest_key).await?;
    let manifest: BackupManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|source| AideMemoError::Deserialize {
            context: "backup manifest".to_string(),
            source,
        })?;
    validate_manifest_metadata(&manifest, Some(backend))?;
    let db_key = location.object_key(&manifest.database.object);
    let stored = get_s3_object(&client, &location.bucket, &db_key).await?;
    let cold_stored = if let Some(database) = &manifest.cold_database {
        let key = location.object_key(&database.object);
        Some(get_s3_object(&client, &location.bucket, &key).await?)
    } else {
        None
    };
    restore_from_stored_bytes(
        location.to_uri(),
        stored,
        cold_stored,
        manifest,
        target_store_path,
        backend,
        force,
    )
}

pub fn read_local_backup_manifest(source_dir: &Path) -> Result<BackupManifest> {
    let manifest = read_manifest(&source_dir.join(MANIFEST_FILE))?;
    validate_manifest_metadata(&manifest, None)?;
    Ok(manifest)
}

#[cfg(feature = "s3")]
pub async fn read_s3_backup_manifest(source: &str) -> Result<BackupManifest> {
    let location = S3Location::parse(source)?;
    let client = s3_client().await;
    let manifest_key = location.object_key(MANIFEST_FILE);
    let manifest_bytes = get_s3_object(&client, &location.bucket, &manifest_key).await?;
    let manifest =
        serde_json::from_slice(&manifest_bytes).map_err(|source| AideMemoError::Deserialize {
            context: "backup manifest".to_string(),
            source,
        })?;
    validate_manifest_metadata(&manifest, None)?;
    Ok(manifest)
}

pub fn is_s3_uri(value: &str) -> bool {
    value.starts_with("s3://")
}

fn restore_from_stored_bytes(
    source: String,
    stored: Vec<u8>,
    cold_stored: Option<Vec<u8>>,
    manifest: BackupManifest,
    target_store_path: &Path,
    backend: &str,
    force: bool,
) -> Result<BackupRestoreReport> {
    validate_manifest_metadata(&manifest, Some(backend))?;
    validate_manifest(&manifest, &stored)?;
    let sqlite_bytes = decode_validated_database("hot", &manifest.database, &stored)?;
    let cold_sqlite_bytes = match (&manifest.cold_database, cold_stored) {
        (Some(database), Some(stored)) => {
            Some(decode_validated_database("cold", database, &stored)?)
        }
        (Some(_), None) => {
            return Err(AideMemoError::InvalidInput(
                "backup manifest references a cold-tier payload that is missing".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(AideMemoError::InvalidInput(
                "backup supplied an unmanifested cold-tier payload".to_string(),
            ));
        }
        (None, None) => None,
    };

    let parent = target_store_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| AideMemoError::StoreOpen {
        path: parent.to_path_buf(),
        source: Box::new(source),
    })?;
    let tmp = restore_temp_file(parent, &sqlite_bytes)?;
    let cold_tmp = cold_sqlite_bytes
        .as_deref()
        .map(|bytes| restore_temp_file(parent, bytes))
        .transpose()?;
    let target_cold_path = crate::archive::cold_path_for_backend(target_store_path, backend);

    if (target_store_path.exists() || target_cold_path.exists()) && !force {
        return Err(AideMemoError::InvalidInput(format!(
            "target store {} or its cold tier already exists; pass --force to replace it",
            target_store_path.display(),
        )));
    }

    let previous_base = previous_store_path(target_store_path);
    let previous_cold_path = crate::archive::cold_path_for_backend(&previous_base, backend);

    // Preserve complete, checkpointed SQLite snapshots before touching WAL /
    // SHM. Renaming only the main file would make `.restore-prev` silently
    // omit committed pages that still live in WAL.
    let previous_store = preserve_existing_store(target_store_path, &previous_base)?;
    let previous_cold_store = preserve_existing_store(&target_cold_path, &previous_cold_path)?;

    let mut removed_sidecars = remove_restore_sidecars(target_store_path)?;
    // The cold AideMemo store has its own HNSW sidecar. Keeping that sidecar
    // while replacing (or detaching) the cold SQLite database can return
    // vectors and fact IDs from the pre-restore archive.
    removed_sidecars.extend(remove_restore_sidecars(&target_cold_path)?);

    if let Err(error) = remove_store_file(target_store_path) {
        return Err(restore_error_with_rollback(
            error,
            previous_store.as_deref(),
            target_store_path,
            previous_cold_store.as_deref(),
            &target_cold_path,
        ));
    }
    if let Err(error) = remove_store_file(&target_cold_path) {
        return Err(restore_error_with_rollback(
            error,
            previous_store.as_deref(),
            target_store_path,
            previous_cold_store.as_deref(),
            &target_cold_path,
        ));
    }

    if let Err(source_error) = std::fs::rename(tmp.path(), target_store_path) {
        let restore_error = AideMemoError::StoreWrite {
            table: "backup_restore",
            key: target_store_path.display().to_string(),
            source: Box::new(source_error),
        };
        return Err(restore_error_with_rollback(
            restore_error,
            previous_store.as_deref(),
            target_store_path,
            previous_cold_store.as_deref(),
            &target_cold_path,
        ));
    }

    let restored_cold_store = if let Some(cold_tmp) = cold_tmp {
        if let Err(source_error) = std::fs::rename(cold_tmp.path(), &target_cold_path) {
            let restore_error = AideMemoError::StoreWrite {
                table: "backup_restore",
                key: target_cold_path.display().to_string(),
                source: Box::new(source_error),
            };
            return Err(restore_error_with_rollback(
                restore_error,
                previous_store.as_deref(),
                target_store_path,
                previous_cold_store.as_deref(),
                &target_cold_path,
            ));
        }
        Some(target_cold_path.display().to_string())
    } else {
        None
    };

    Ok(BackupRestoreReport {
        manifest,
        source,
        restored_store: target_store_path.display().to_string(),
        previous_store: previous_store.map(|path| path.display().to_string()),
        restored_cold_store,
        previous_cold_store: previous_cold_store.map(|path| path.display().to_string()),
        removed_sidecars,
    })
}

fn restore_temp_file(parent: &Path, sqlite_bytes: &[u8]) -> Result<tempfile::NamedTempFile> {
    let tmp =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| AideMemoError::StoreOpen {
            path: parent.to_path_buf(),
            source: Box::new(source),
        })?;
    std::fs::write(tmp.path(), sqlite_bytes).map_err(|source| AideMemoError::StoreWrite {
        table: "backup_restore",
        key: tmp.path().display().to_string(),
        source: Box::new(source),
    })?;
    validate_sqlite_file(tmp.path())?;
    Ok(tmp)
}

fn preserve_existing_store(store_path: &Path, previous_path: &Path) -> Result<Option<PathBuf>> {
    if !store_path.exists() {
        return Ok(None);
    }

    checkpoint_sqlite_for_restore(store_path)?;
    let parent = previous_path.parent().unwrap_or_else(|| Path::new("."));
    let staged =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| AideMemoError::StoreOpen {
            path: parent.to_path_buf(),
            source: Box::new(source),
        })?;
    snapshot_sqlite(store_path, staged.path())?;
    validate_sqlite_file(staged.path())?;

    // A previous safety copy may itself have stale sidecars from a manual
    // inspection. Remove those only after the new snapshot is ready.
    let _ = remove_restore_sidecars(previous_path)?;
    remove_store_file(previous_path)?;
    std::fs::rename(staged.path(), previous_path).map_err(|source| AideMemoError::StoreWrite {
        table: "backup_restore_previous",
        key: previous_path.display().to_string(),
        source: Box::new(source),
    })?;
    Ok(Some(previous_path.to_path_buf()))
}

#[cfg(feature = "sqlite")]
fn checkpoint_sqlite_for_restore(store_path: &Path) -> Result<()> {
    let conn = rusqlite::Connection::open_with_flags(
        store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(|source| AideMemoError::StoreOpen {
        path: store_path.to_path_buf(),
        source: Box::new(source),
    })?;
    conn.busy_timeout(std::time::Duration::ZERO)
        .map_err(|source| AideMemoError::StoreWrite {
            table: "backup_restore_checkpoint",
            key: store_path.display().to_string(),
            source: Box::new(source),
        })?;
    let (busy, _log_frames, _checkpointed): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|source| checkpoint_restore_error(store_path, source))?;
    if busy != 0 {
        return Err(AideMemoError::InvalidInput(format!(
            "target store {} is busy; stop its daemon and all readers/writers before restore",
            store_path.display()
        )));
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn checkpoint_restore_error(store_path: &Path, source: rusqlite::Error) -> AideMemoError {
    let busy = matches!(
        &source,
        rusqlite::Error::SqliteFailure(error, _)
            if matches!(
                error.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    );
    if busy {
        AideMemoError::InvalidInput(format!(
            "target store {} is busy; stop its daemon and all readers/writers before restore",
            store_path.display()
        ))
    } else {
        AideMemoError::StoreWrite {
            table: "backup_restore_checkpoint",
            key: store_path.display().to_string(),
            source: Box::new(source),
        }
    }
}

#[cfg(not(feature = "sqlite"))]
fn checkpoint_sqlite_for_restore(_store_path: &Path) -> Result<()> {
    Err(AideMemoError::InvalidInput(
        "SQLite restore requires a build with the `sqlite` feature".to_string(),
    ))
}

fn remove_store_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AideMemoError::StoreWrite {
            table: "backup_restore",
            key: path.display().to_string(),
            source: Box::new(source),
        }),
    }
}

fn restore_error_with_rollback(
    restore_error: AideMemoError,
    previous_store: Option<&Path>,
    target_store: &Path,
    previous_cold_store: Option<&Path>,
    target_cold_store: &Path,
) -> AideMemoError {
    combine_restore_and_rollback_error(
        restore_error,
        rollback_preserved_stores(
            previous_store,
            target_store,
            previous_cold_store,
            target_cold_store,
        ),
    )
}

fn combine_restore_and_rollback_error(
    restore_error: AideMemoError,
    rollback_result: Result<()>,
) -> AideMemoError {
    match rollback_result {
        Ok(()) => restore_error,
        Err(rollback_error) => AideMemoError::Internal(format!(
            "backup restore failed: {restore_error}; rollback also failed: {rollback_error}"
        )),
    }
}

fn rollback_preserved_stores(
    previous_store: Option<&Path>,
    target_store: &Path,
    previous_cold_store: Option<&Path>,
    target_cold_store: &Path,
) -> Result<()> {
    rollback_preserved_stores_with(
        previous_store,
        target_store,
        previous_cold_store,
        target_cold_store,
        restore_preserved_copy,
    )
}

fn rollback_preserved_stores_with<F>(
    previous_store: Option<&Path>,
    target_store: &Path,
    previous_cold_store: Option<&Path>,
    target_cold_store: &Path,
    mut restore: F,
) -> Result<()>
where
    F: FnMut(Option<&Path>, &Path) -> Result<()>,
{
    let hot_result = restore(previous_store, target_store);
    let cold_result = restore(previous_cold_store, target_cold_store);
    match (hot_result, cold_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(hot_error), Err(cold_error)) => Err(AideMemoError::Internal(format!(
            "hot rollback failed: {hot_error}; cold rollback failed: {cold_error}"
        ))),
    }
}

fn restore_preserved_copy(previous: Option<&Path>, target: &Path) -> Result<()> {
    restore_preserved_copy_with(previous, target, |from, to| std::fs::copy(from, to))
}

fn restore_preserved_copy_with<F>(
    previous: Option<&Path>,
    target: &Path,
    copy_file: F,
) -> Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<u64>,
{
    if let Some(previous) = previous {
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        let staged =
            tempfile::NamedTempFile::new_in(parent).map_err(|source| AideMemoError::StoreOpen {
                path: parent.to_path_buf(),
                source: Box::new(source),
            })?;
        copy_file(previous, staged.path()).map_err(|source| AideMemoError::StoreWrite {
            table: "backup_restore_rollback",
            key: target.display().to_string(),
            source: Box::new(source),
        })?;
        validate_sqlite_file(staged.path())?;
        std::fs::rename(staged.path(), target).map_err(|source| AideMemoError::StoreWrite {
            table: "backup_restore_rollback",
            key: target.display().to_string(),
            source: Box::new(source),
        })?;
        validate_sqlite_file(target)
    } else {
        remove_store_file(target)
    }
}

#[cfg(feature = "sqlite")]
fn snapshot_sqlite(store_path: &Path, snapshot_path: &Path) -> Result<()> {
    if !store_path.exists() {
        return Err(AideMemoError::InvalidInput(format!(
            "store path does not exist: {}",
            store_path.display()
        )));
    }
    let source = rusqlite::Connection::open_with_flags(
        store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|source| AideMemoError::StoreOpen {
        path: store_path.to_path_buf(),
        source: Box::new(source),
    })?;
    source
        .backup(rusqlite::MAIN_DB, snapshot_path, None)
        .map_err(|source| AideMemoError::StoreWrite {
            table: "backup",
            key: snapshot_path.display().to_string(),
            source: Box::new(source),
        })
}

struct SqliteTierSnapshots {
    hot: tempfile::NamedTempFile,
    cold: Option<tempfile::NamedTempFile>,
}

/// Take the hot snapshot first, then the cold snapshot. Archiving commits the
/// cold write before deleting hot, so this ordering cannot lose a fact. If an
/// archive straddles the two snapshots, the FactId can appear in both; cold is
/// the later tier and wins, so remove those records from the hot snapshot.
///
/// A fact and its entity can also be created after the first hot snapshot and
/// archived before the cold snapshot. Take a second hot metadata snapshot so
/// reconciliation can copy only entities required by those cold facts into
/// the first hot snapshot.
fn snapshot_sqlite_tiers(store_path: &Path, backend: &str) -> Result<SqliteTierSnapshots> {
    let hot = new_snapshot_temp_file()?;
    snapshot_sqlite(store_path, hot.path())?;
    validate_sqlite_file(hot.path())?;

    let cold_source = crate::archive::cold_path_for_backend(store_path, backend);
    let cold = if cold_source.exists() {
        let snapshot = new_snapshot_temp_file()?;
        snapshot_sqlite(&cold_source, snapshot.path())?;
        validate_sqlite_file(snapshot.path())?;
        let later_hot = new_snapshot_temp_file()?;
        snapshot_sqlite(store_path, later_hot.path())?;
        validate_sqlite_file(later_hot.path())?;
        reconcile_hot_cold_snapshots(hot.path(), snapshot.path(), later_hot.path())?;
        validate_sqlite_file(hot.path())?;
        Some(snapshot)
    } else {
        None
    };

    Ok(SqliteTierSnapshots { hot, cold })
}

fn new_snapshot_temp_file() -> Result<tempfile::NamedTempFile> {
    tempfile::NamedTempFile::new().map_err(|source| AideMemoError::StoreOpen {
        path: std::env::temp_dir(),
        source: Box::new(source),
    })
}

fn read_snapshot_bytes(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path)
        .map_err(|source| AideMemoError::FileRead(path.to_path_buf(), source.to_string()))
}

#[cfg(feature = "sqlite")]
fn reconcile_hot_cold_snapshots(
    hot_snapshot: &Path,
    cold_snapshot: &Path,
    later_hot_snapshot: &Path,
) -> Result<()> {
    use rusqlite::TransactionBehavior;

    let mut hot =
        rusqlite::Connection::open(hot_snapshot).map_err(|source| AideMemoError::StoreOpen {
            path: hot_snapshot.to_path_buf(),
            source: Box::new(source),
        })?;
    hot.execute(
        "ATTACH DATABASE ?1 AS cold_snapshot",
        rusqlite::params![cold_snapshot.to_string_lossy().as_ref()],
    )
    .map_err(|source| AideMemoError::StoreRead {
        table: "backup_cold_snapshot",
        key: cold_snapshot.display().to_string(),
        source: Box::new(source),
    })?;
    hot.execute(
        "ATTACH DATABASE ?1 AS later_hot_snapshot",
        rusqlite::params![later_hot_snapshot.to_string_lossy().as_ref()],
    )
    .map_err(|source| AideMemoError::StoreRead {
        table: "backup_later_hot_snapshot",
        key: later_hot_snapshot.display().to_string(),
        source: Box::new(source),
    })?;

    let reconcile = (|| -> std::result::Result<(i64, i64), rusqlite::Error> {
        let tx = hot.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TEMP TABLE backup_missing_entities (
                 id TEXT PRIMARY KEY
             ) WITHOUT ROWID;

             INSERT INTO backup_missing_entities (id)
             SELECT DISTINCT cold_entity.entity_id
             FROM cold_snapshot.fact_entities AS cold_entity
             LEFT JOIN main.entities AS hot_entity
               ON hot_entity.id = cold_entity.entity_id
             WHERE hot_entity.id IS NULL;

             INSERT INTO main.entities (
                 id, name, name_lower, entity_type, updated_at, record_json
             )
             SELECT later_entity.id,
                    later_entity.name,
                    later_entity.name_lower,
                    later_entity.entity_type,
                    later_entity.updated_at,
                    later_entity.record_json
             FROM later_hot_snapshot.entities AS later_entity
             JOIN backup_missing_entities AS missing
               ON missing.id = later_entity.id;

             INSERT INTO main.entity_names (name_lower, entity_id)
             SELECT later_name.name_lower, later_name.entity_id
             FROM later_hot_snapshot.entity_names AS later_name
             JOIN backup_missing_entities AS missing
               ON missing.id = later_name.entity_id;",
        )?;
        tx.execute(
            "DELETE FROM facts_fts
             WHERE fact_id IN (SELECT id FROM cold_snapshot.facts)",
            [],
        )?;
        tx.execute(
            "DELETE FROM fact_entities
             WHERE fact_id IN (SELECT id FROM cold_snapshot.facts)",
            [],
        )?;
        tx.execute(
            "DELETE FROM facts
             WHERE id IN (SELECT id FROM cold_snapshot.facts)",
            [],
        )?;
        let missing_entity_refs = tx.query_row(
            "SELECT COUNT(DISTINCT cold_entity.entity_id)
             FROM cold_snapshot.fact_entities AS cold_entity
             LEFT JOIN main.entities AS hot_entity
               ON hot_entity.id = cold_entity.entity_id
             WHERE hot_entity.id IS NULL",
            [],
            |row| row.get(0),
        )?;
        let missing_canonical_names = tx.query_row(
            "SELECT COUNT(DISTINCT cold_entity.entity_id)
             FROM cold_snapshot.fact_entities AS cold_entity
             JOIN main.entities AS hot_entity
               ON hot_entity.id = cold_entity.entity_id
             LEFT JOIN main.entity_names AS hot_name
               ON hot_name.entity_id = hot_entity.id
              AND hot_name.name_lower = hot_entity.name_lower
             WHERE hot_name.entity_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        if missing_entity_refs == 0 && missing_canonical_names == 0 {
            tx.commit()?;
        } else {
            tx.rollback()?;
        }
        Ok((missing_entity_refs, missing_canonical_names))
    })();
    let detach_later = hot.execute_batch("DETACH DATABASE later_hot_snapshot");
    let detach_cold = hot.execute_batch("DETACH DATABASE cold_snapshot");
    let (missing_entity_refs, missing_canonical_names) =
        reconcile.map_err(|source| AideMemoError::StoreWrite {
            table: "backup_tier_reconcile",
            key: hot_snapshot.display().to_string(),
            source: Box::new(source),
        })?;
    detach_later.map_err(|source| AideMemoError::StoreWrite {
        table: "backup_tier_reconcile",
        key: later_hot_snapshot.display().to_string(),
        source: Box::new(source),
    })?;
    detach_cold.map_err(|source| AideMemoError::StoreWrite {
        table: "backup_tier_reconcile",
        key: cold_snapshot.display().to_string(),
        source: Box::new(source),
    })?;
    if missing_entity_refs != 0 || missing_canonical_names != 0 {
        return Err(AideMemoError::InvalidInput(format!(
            "backup cold snapshot has {missing_entity_refs} unresolved entity references and \
             {missing_canonical_names} entities without canonical name mappings"
        )));
    }
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
fn reconcile_hot_cold_snapshots(
    _hot_snapshot: &Path,
    _cold_snapshot: &Path,
    _later_hot_snapshot: &Path,
) -> Result<()> {
    Err(AideMemoError::InvalidInput(
        "SQLite backup requires a build with the `sqlite` feature".to_string(),
    ))
}

#[cfg(not(feature = "sqlite"))]
fn snapshot_sqlite(_store_path: &Path, _snapshot_path: &Path) -> Result<()> {
    Err(AideMemoError::InvalidInput(
        "SQLite backup requires a build with the `sqlite` feature".to_string(),
    ))
}

#[cfg(feature = "sqlite")]
fn validate_sqlite_file(path: &Path) -> Result<()> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| AideMemoError::StoreOpen {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
    let status: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|source| AideMemoError::StoreRead {
            table: "sqlite_integrity_check",
            key: path.display().to_string(),
            source: Box::new(source),
        })?;
    if status == "ok" {
        Ok(())
    } else {
        Err(AideMemoError::InvalidInput(format!(
            "SQLite integrity_check failed for {}: {}",
            path.display(),
            status
        )))
    }
}

#[cfg(not(feature = "sqlite"))]
fn validate_sqlite_file(_path: &Path) -> Result<()> {
    Err(AideMemoError::InvalidInput(
        "SQLite restore requires a build with the `sqlite` feature".to_string(),
    ))
}

fn ensure_sqlite_backend(backend: &str) -> Result<()> {
    match backend.trim().to_ascii_lowercase().as_str() {
        "" | "sqlite" | "libsqlite" => Ok(()),
        other => Err(AideMemoError::InvalidInput(format!(
            "backup/restore currently supports SQLite stores only, got backend `{other}`"
        ))),
    }
}

fn snapshot_sync_cursor(snapshot_path: &Path, backend: &str) -> Result<Option<BackupSyncCursor>> {
    let mut config = crate::Config::default();
    config.store.backend = canonical_sqlite_backend(backend).to_string();
    config.store.path = snapshot_path.display().to_string();
    let snapshot = crate::AideMemo::open(snapshot_path, config)?;
    let mut cursor = crate::sync::SyncCursor::default();

    // A full export first advances the insert high-water marks. Inserts do
    // not advance update watermarks, by design, so drain the immutable
    // snapshot until an export is empty before persisting the backup cursor.
    // Otherwise `branch push --base` would replay every baseline entity/fact
    // once as an update instead of exporting only post-backup changes.
    loop {
        let mut output = Vec::new();
        let emitted = snapshot.sync_export(
            crate::sync::SyncExportOpts {
                since: cursor,
                include_relations: true,
                ..Default::default()
            },
            &mut output,
        )?;
        let jsonl = std::str::from_utf8(&output).map_err(|source| {
            AideMemoError::Internal(format!("backup sync cursor UTF-8: {source}"))
        })?;
        cursor = crate::sync::cursor_from_jsonl(jsonl)?;
        if emitted == 0 {
            return Ok(Some(BackupSyncCursor::from_sync(cursor)));
        }
    }
}

fn canonical_sqlite_backend(backend: &str) -> &str {
    if backend.trim().eq_ignore_ascii_case("libsqlite") {
        "libsqlite"
    } else {
        "sqlite"
    }
}

fn write_manifest(path: &Path, manifest: &BackupManifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|source| AideMemoError::Serialize {
        context: "backup manifest".to_string(),
        source,
    })?;
    std::fs::write(path, bytes).map_err(|source| AideMemoError::StoreWrite {
        table: "backup_manifest",
        key: path.display().to_string(),
        source: Box::new(source),
    })
}

fn read_manifest(path: &Path) -> Result<BackupManifest> {
    let bytes = std::fs::read(path).map_err(|source| AideMemoError::StoreRead {
        table: "backup_manifest",
        key: path.display().to_string(),
        source: Box::new(source),
    })?;
    serde_json::from_slice(&bytes).map_err(|source| AideMemoError::Deserialize {
        context: "backup manifest".to_string(),
        source,
    })
}

fn validate_manifest(manifest: &BackupManifest, stored: &[u8]) -> Result<()> {
    validate_stored_database("hot", &manifest.database, stored)
}

/// Validate every manifest field that influences object lookup before any
/// referenced payload is read. Backup objects deliberately live directly
/// under one backup prefix; accepting paths here would turn a signed/checksummed
/// manifest into a local path traversal or an arbitrary S3-key selector.
fn validate_manifest_metadata(
    manifest: &BackupManifest,
    restore_backend: Option<&str>,
) -> Result<()> {
    if manifest.schema != 1 {
        return Err(AideMemoError::InvalidInput(format!(
            "unsupported backup manifest schema {}",
            manifest.schema
        )));
    }
    ensure_manifest_sqlite_backend(&manifest.backend)?;
    if let Some(backend) = restore_backend {
        ensure_sqlite_backend(backend)?;
    }
    validate_manifest_object("hot", &manifest.database.object)?;
    if let Some(cold) = &manifest.cold_database {
        validate_manifest_object("cold", &cold.object)?;
        if cold.object.eq_ignore_ascii_case(&manifest.database.object) {
            return Err(AideMemoError::InvalidInput(
                "backup hot and cold payload objects must be distinct".to_string(),
            ));
        }
    }
    Ok(())
}

fn ensure_manifest_sqlite_backend(backend: &str) -> Result<()> {
    match backend.trim().to_ascii_lowercase().as_str() {
        "sqlite" | "libsqlite" => Ok(()),
        other => Err(AideMemoError::InvalidInput(format!(
            "backup manifest backend `{other}` is not SQLite-compatible"
        ))),
    }
}

fn validate_manifest_object(tier: &str, object: &str) -> Result<()> {
    let path = Path::new(object);
    let simple_filename = !object.is_empty()
        && object != "."
        && object != ".."
        && !object.chars().any(char::is_control)
        && !object.contains('/')
        && !object.contains('\\')
        && path
            .file_name()
            .is_some_and(|name| name == path.as_os_str());
    if !simple_filename {
        return Err(AideMemoError::InvalidInput(format!(
            "backup {tier} object must be a simple relative filename"
        )));
    }
    Ok(())
}

fn validate_stored_database(tier: &str, database: &BackupDatabase, stored: &[u8]) -> Result<()> {
    if database.stored_bytes != stored.len() as u64 {
        return Err(AideMemoError::InvalidInput(format!(
            "backup {tier} stored payload size mismatch"
        )));
    }
    let actual = sha256_hex(stored);
    if database.stored_sha256 != actual {
        return Err(AideMemoError::InvalidInput(format!(
            "backup {tier} stored payload checksum mismatch"
        )));
    }
    Ok(())
}

fn decode_validated_database(
    tier: &str,
    database: &BackupDatabase,
    stored: &[u8],
) -> Result<Vec<u8>> {
    validate_stored_database(tier, database, stored)?;
    let sqlite_bytes = decode_database_bytes(database, stored)?;
    if sqlite_bytes.len() as u64 != database.sqlite_bytes {
        return Err(AideMemoError::InvalidInput(format!(
            "backup {tier} SQLite payload size mismatch"
        )));
    }
    if sha256_hex(&sqlite_bytes) != database.sqlite_sha256 {
        return Err(AideMemoError::InvalidInput(format!(
            "backup {tier} SQLite payload checksum mismatch"
        )));
    }
    Ok(sqlite_bytes)
}

fn decode_database_bytes(database: &BackupDatabase, stored: &[u8]) -> Result<Vec<u8>> {
    match database.compression.as_str() {
        "none" => Ok(stored.to_vec()),
        #[cfg(feature = "s3")]
        "zstd" => zstd::stream::decode_all(stored).map_err(|source| {
            AideMemoError::Internal(format!("backup decompression failed: {source}"))
        }),
        #[cfg(not(feature = "s3"))]
        "zstd" => Err(AideMemoError::InvalidInput(
            "zstd backup restore requires a build with the `s3` feature".to_string(),
        )),
        other => Err(AideMemoError::InvalidInput(format!(
            "unsupported backup compression `{other}`"
        ))),
    }
}

fn database_manifest(
    object: &str,
    compression: &str,
    stored: &[u8],
    sqlite: &[u8],
) -> BackupDatabase {
    BackupDatabase {
        object: object.to_string(),
        compression: compression.to_string(),
        stored_bytes: stored.len() as u64,
        stored_sha256: sha256_hex(stored),
        sqlite_bytes: sqlite.len() as u64,
        sqlite_sha256: sha256_hex(sqlite),
    }
}

fn remove_restore_sidecars(store_path: &Path) -> Result<Vec<String>> {
    let mut removed = remove_sqlite_sidecars(store_path)?;
    let path = store_path.with_extension("hnsw.bin");
    match std::fs::remove_file(&path) {
        Ok(()) => removed.push(path.display().to_string()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(AideMemoError::StoreWrite {
                table: "backup_restore",
                key: path.display().to_string(),
                source: Box::new(source),
            });
        }
    }
    Ok(removed)
}

fn remove_sqlite_sidecars(store_path: &Path) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for path in [
        append_suffix(store_path, "-wal"),
        append_suffix(store_path, "-shm"),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(path.display().to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(AideMemoError::StoreWrite {
                    table: "backup_restore",
                    key: path.display().to_string(),
                    source: Box::new(source),
                });
            }
        }
    }
    Ok(removed)
}

fn previous_store_path(store_path: &Path) -> PathBuf {
    append_suffix(store_path, ".restore-prev")
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(feature = "s3")]
#[derive(Debug, Clone)]
struct S3Location {
    bucket: String,
    prefix: String,
}

#[cfg(feature = "s3")]
impl S3Location {
    fn parse(uri: &str) -> Result<Self> {
        let Some(rest) = uri.strip_prefix("s3://") else {
            return Err(AideMemoError::InvalidInput(format!(
                "expected s3:// URI, got {uri}"
            )));
        };
        let mut parts = rest.splitn(2, '/');
        let bucket = parts.next().unwrap_or_default().trim();
        if bucket.is_empty() {
            return Err(AideMemoError::InvalidInput(
                "S3 backup URI must include a bucket".to_string(),
            ));
        }
        let prefix = parts
            .next()
            .unwrap_or_default()
            .trim_matches('/')
            .to_string();
        Ok(Self {
            bucket: bucket.to_string(),
            prefix,
        })
    }

    fn join(&self, child: &str) -> Self {
        let prefix = if self.prefix.is_empty() {
            child.to_string()
        } else {
            format!("{}/{}", self.prefix, child.trim_matches('/'))
        };
        Self {
            bucket: self.bucket.clone(),
            prefix,
        }
    }

    fn object_key(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", self.prefix, name.trim_start_matches('/'))
        }
    }

    fn to_uri(&self) -> String {
        if self.prefix.is_empty() {
            format!("s3://{}", self.bucket)
        } else {
            format!("s3://{}/{}", self.bucket, self.prefix)
        }
    }
}

#[cfg(feature = "s3")]
async fn s3_client() -> aws_sdk_s3::Client {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    aws_sdk_s3::Client::new(&config)
}

#[cfg(feature = "s3")]
async fn put_s3_object(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    bytes: Vec<u8>,
) -> Result<()> {
    use aws_sdk_s3::primitives::ByteStream;
    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(bytes))
        .send()
        .await
        .map_err(|source| {
            AideMemoError::Internal(format!("S3 put s3://{bucket}/{key} failed: {source}"))
        })?;
    Ok(())
}

#[cfg(feature = "s3")]
async fn get_s3_object(client: &aws_sdk_s3::Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    let output = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(|source| {
            AideMemoError::Internal(format!("S3 get s3://{bucket}/{key} failed: {source}"))
        })?;
    let bytes = output.body.collect().await.map_err(|source| {
        AideMemoError::Internal(format!("S3 read s3://{bucket}/{key} failed: {source}"))
    })?;
    Ok(bytes.into_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "sqlite")]
    fn local_backup_round_trips_sqlite_store() {
        use crate::{AideMemo, Config, EntityInput, EntityType, FactInput, FactType};

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("wiki.sqlite");
        let backup_dir = dir.path().join("backups");
        let restored = dir.path().join("restored.sqlite");
        let mut config = Config::default();
        config.store.backend = "sqlite".to_string();
        let wiki = AideMemo::open(&store, config.clone()).unwrap();
        let entity = wiki
            .entity_add(EntityInput {
                name: "Backup".to_string(),
                entity_type: Some(EntityType::Concept),
                ..Default::default()
            })
            .unwrap();
        wiki.fact_add(FactInput {
            content: "SQLite backup restore smoke".to_string(),
            entity_ids: Some(vec![entity]),
            fact_type: Some(FactType::Claim),
            ..Default::default()
        })
        .unwrap();

        let report = create_local_backup(&store, "sqlite", &backup_dir).unwrap();
        let restore =
            restore_local_backup(Path::new(&report.destination), &restored, "sqlite", false)
                .unwrap();
        assert_eq!(restore.manifest.database.compression, "none");

        let reopened = AideMemo::open(&restored, config).unwrap();
        let stats = reopened.stats().unwrap();
        assert_eq!(stats.entity_count, 1);
        assert_eq!(stats.fact_count, 1);
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn local_backup_round_trips_cold_tier_and_checksums_it() {
        use crate::{AideMemo, Config, FactInput};

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("wiki.sqlite");
        let backup_dir = dir.path().join("backups");
        let restored = dir.path().join("restored.sqlite");
        let mut config = Config::default();
        config.store.backend = "sqlite".to_string();
        let wiki = AideMemo::open(&store, config.clone()).unwrap();
        let archived = wiki
            .fact_add(FactInput {
                content: "archived backup fact".to_string(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(wiki.archive_facts(&[archived]).unwrap(), 1);

        let report = create_local_backup(&store, "sqlite", &backup_dir).unwrap();
        let cold_manifest = report.manifest.cold_database.as_ref().unwrap();
        assert_eq!(cold_manifest.object, COLD_SQLITE_OBJECT);
        assert!(
            report
                .cold_database_uri
                .as_deref()
                .is_some_and(|p| Path::new(p).exists())
        );

        let restore =
            restore_local_backup(Path::new(&report.destination), &restored, "sqlite", false)
                .unwrap();
        assert_eq!(
            restore.restored_cold_store.as_deref(),
            Some(
                crate::archive::cold_path_for_backend(&restored, "sqlite")
                    .to_string_lossy()
                    .as_ref()
            )
        );
        let reopened = AideMemo::open(&restored, config).unwrap();
        assert_eq!(
            reopened.fact_get(&archived).unwrap().content,
            "archived backup fact"
        );
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn restore_rejects_tampered_cold_payload() {
        use crate::{AideMemo, Config, FactInput};

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("wiki.sqlite");
        let backup_dir = dir.path().join("backups");
        let mut config = Config::default();
        config.store.backend = "sqlite".to_string();
        let wiki = AideMemo::open(&store, config).unwrap();
        let id = wiki
            .fact_add(FactInput {
                content: "tamper target".to_string(),
                ..Default::default()
            })
            .unwrap();
        wiki.archive_facts(&[id]).unwrap();
        let report = create_local_backup(&store, "sqlite", &backup_dir).unwrap();
        let cold = Path::new(&report.destination).join(COLD_SQLITE_OBJECT);
        let mut bytes = std::fs::read(&cold).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(&cold, bytes).unwrap();

        let error = restore_local_backup(
            Path::new(&report.destination),
            &dir.path().join("restored.sqlite"),
            "sqlite",
            false,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cold stored payload checksum mismatch")
        );
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn restoring_hot_only_backup_moves_stale_cold_tier_aside() {
        use crate::{AideMemo, Config, FactInput};

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.sqlite");
        let target = dir.path().join("target.sqlite");
        let backup_dir = dir.path().join("backups");
        let mut config = Config::default();
        config.store.backend = "sqlite".to_string();
        let source_wiki = AideMemo::open(&source, config.clone()).unwrap();
        source_wiki
            .fact_add(FactInput {
                content: "hot-only backup fact".to_string(),
                ..Default::default()
            })
            .unwrap();
        let report = create_local_backup(&source, "sqlite", &backup_dir).unwrap();
        assert!(report.manifest.cold_database.is_none());

        let target_wiki = AideMemo::open(&target, config).unwrap();
        let stale = target_wiki
            .fact_add(FactInput {
                content: "stale cold fact".to_string(),
                ..Default::default()
            })
            .unwrap();
        target_wiki.archive_facts(&[stale]).unwrap();
        drop(target_wiki);
        let target_cold = crate::archive::cold_path_for_backend(&target, "sqlite");
        assert!(target_cold.exists());
        let stale_cold_hnsw = target_cold.with_extension("hnsw.bin");
        std::fs::write(&stale_cold_hnsw, b"stale cold vectors").unwrap();
        std::fs::remove_file(&target).unwrap();

        let error = restore_local_backup(Path::new(&report.destination), &target, "sqlite", false)
            .unwrap_err();
        assert!(error.to_string().contains("cold tier already exists"));
        assert!(target_cold.exists());

        let restored =
            restore_local_backup(Path::new(&report.destination), &target, "sqlite", true).unwrap();
        assert!(restored.previous_store.is_none());
        assert!(restored.restored_cold_store.is_none());
        let previous_cold = restored.previous_cold_store.unwrap();
        assert!(Path::new(&previous_cold).exists());
        assert!(!target_cold.exists());
        assert!(!stale_cold_hnsw.exists());
        assert!(
            restored
                .removed_sidecars
                .contains(&stale_cold_hnsw.display().to_string())
        );

        let previous_cold_wiki = AideMemo::open(Path::new(&previous_cold), Config::default())
            .expect("preserved cold tier opens as a complete SQLite snapshot");
        assert_eq!(
            previous_cold_wiki.fact_get(&stale).unwrap().content,
            "stale cold fact"
        );
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn tier_reconciliation_removes_cold_fact_from_hot_snapshot_indexes() {
        use crate::{AideMemo, Config, EntityInput, FactInput};

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("wiki.sqlite");
        let wiki = AideMemo::open(&store, Config::default()).unwrap();
        let entity = wiki
            .entity_add(EntityInput {
                name: "Archive race".to_string(),
                ..Default::default()
            })
            .unwrap();
        let fact = wiki
            .fact_add(FactInput {
                content: "fact moved between tier snapshots".to_string(),
                entity_ids: Some(vec![entity]),
                ..Default::default()
            })
            .unwrap();

        let hot_snapshot = new_snapshot_temp_file().unwrap();
        snapshot_sqlite(&store, hot_snapshot.path()).unwrap();
        wiki.archive_facts(&[fact]).unwrap();
        let cold_path = crate::archive::cold_path_for_backend(&store, "sqlite");
        let cold_snapshot = new_snapshot_temp_file().unwrap();
        snapshot_sqlite(&cold_path, cold_snapshot.path()).unwrap();
        let later_hot_snapshot = new_snapshot_temp_file().unwrap();
        snapshot_sqlite(&store, later_hot_snapshot.path()).unwrap();

        reconcile_hot_cold_snapshots(
            hot_snapshot.path(),
            cold_snapshot.path(),
            later_hot_snapshot.path(),
        )
        .unwrap();

        let hot = rusqlite::Connection::open(hot_snapshot.path()).unwrap();
        for table in ["facts", "fact_entities", "facts_fts"] {
            let count: i64 = hot
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} must not retain the cold-tier FactId");
        }
        let cold = rusqlite::Connection::open(cold_snapshot.path()).unwrap();
        let cold_count: i64 = cold
            .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cold_count, 1);
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn tier_reconciliation_copies_only_entities_needed_by_late_archived_facts() {
        use crate::{AideMemo, Config, EntityInput, FactInput};

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("wiki.sqlite");
        let restored = dir.path().join("restored.sqlite");
        let wiki = AideMemo::open(&store, Config::default()).unwrap();

        let hot_snapshot = new_snapshot_temp_file().unwrap();
        snapshot_sqlite(&store, hot_snapshot.path()).unwrap();

        let required_entity = wiki
            .entity_add(EntityInput {
                name: "Late archive entity".to_string(),
                aliases: Some(vec!["Late archive alias".to_string()]),
                ..Default::default()
            })
            .unwrap();
        wiki.entity_add(EntityInput {
            name: "Unrelated late entity".to_string(),
            ..Default::default()
        })
        .unwrap();
        let archived_fact = wiki
            .fact_add(FactInput {
                content: "created and archived between tier snapshots".to_string(),
                entity_ids: Some(vec![required_entity]),
                ..Default::default()
            })
            .unwrap();
        wiki.archive_facts(&[archived_fact]).unwrap();

        let cold_path = crate::archive::cold_path_for_backend(&store, "sqlite");
        let cold_snapshot = new_snapshot_temp_file().unwrap();
        snapshot_sqlite(&cold_path, cold_snapshot.path()).unwrap();
        let later_hot_snapshot = new_snapshot_temp_file().unwrap();
        snapshot_sqlite(&store, later_hot_snapshot.path()).unwrap();

        reconcile_hot_cold_snapshots(
            hot_snapshot.path(),
            cold_snapshot.path(),
            later_hot_snapshot.path(),
        )
        .unwrap();

        std::fs::copy(hot_snapshot.path(), &restored).unwrap();
        let restored_cold = crate::archive::cold_path_for_backend(&restored, "sqlite");
        std::fs::copy(cold_snapshot.path(), &restored_cold).unwrap();
        let reopened = AideMemo::open(&restored, Config::default()).unwrap();

        assert_eq!(
            reopened.entity_get("Late archive entity").unwrap().id,
            required_entity
        );
        assert_eq!(
            reopened.entity_get("Late archive alias").unwrap().id,
            required_entity
        );
        let restored_fact = reopened.fact_get(&archived_fact).unwrap();
        assert_eq!(restored_fact.entity_ids, vec![required_entity]);
        assert!(reopened.entity_get("Unrelated late entity").is_err());
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn force_restore_preserves_previous_hot_store_as_complete_snapshot() {
        use crate::{AideMemo, Config, FactInput};

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.sqlite");
        let target = dir.path().join("target.sqlite");
        let backup_dir = dir.path().join("backups");
        let source_wiki = AideMemo::open(&source, Config::default()).unwrap();
        source_wiki
            .fact_add(FactInput {
                content: "incoming restore fact".to_string(),
                ..Default::default()
            })
            .unwrap();
        let report = create_local_backup(&source, "sqlite", &backup_dir).unwrap();

        let target_wiki = AideMemo::open(&target, Config::default()).unwrap();
        let old_fact = target_wiki
            .fact_add(FactInput {
                content: "fact from before restore".to_string(),
                ..Default::default()
            })
            .unwrap();
        drop(target_wiki);

        let restored =
            restore_local_backup(Path::new(&report.destination), &target, "sqlite", true).unwrap();
        let previous = restored.previous_store.expect("previous hot snapshot");
        let previous_wiki = AideMemo::open(Path::new(&previous), Config::default()).unwrap();
        assert_eq!(
            previous_wiki.fact_get(&old_fact).unwrap().content,
            "fact from before restore"
        );
    }

    #[test]
    fn rollback_attempts_both_tiers_and_reports_an_injected_failure() {
        let hot_previous = Path::new("hot.previous");
        let hot_target = Path::new("hot.target");
        let cold_previous = Path::new("cold.previous");
        let cold_target = Path::new("cold.target");
        let mut attempted = Vec::new();

        let error = rollback_preserved_stores_with(
            Some(hot_previous),
            hot_target,
            Some(cold_previous),
            cold_target,
            |previous, target| {
                attempted.push((previous.map(Path::to_path_buf), target.to_path_buf()));
                if target == hot_target {
                    Err(AideMemoError::Internal(
                        "injected hot rollback failure".to_string(),
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();

        assert_eq!(
            attempted,
            vec![
                (Some(hot_previous.to_path_buf()), hot_target.to_path_buf()),
                (Some(cold_previous.to_path_buf()), cold_target.to_path_buf()),
            ]
        );
        assert!(error.to_string().contains("injected hot rollback failure"));
    }

    #[test]
    fn restore_and_rollback_failures_are_reported_together() {
        let error = combine_restore_and_rollback_error(
            AideMemoError::InvalidInput("injected restore failure".to_string()),
            Err(AideMemoError::Internal(
                "injected rollback failure".to_string(),
            )),
        );
        let message = error.to_string();
        assert!(message.contains("injected restore failure"));
        assert!(message.contains("injected rollback failure"));
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn rollback_revalidates_the_preserved_sqlite_copy() {
        let dir = tempfile::tempdir().unwrap();
        let previous = dir.path().join("previous.sqlite");
        let target = dir.path().join("target.sqlite");
        std::fs::write(&previous, b"not a sqlite database").unwrap();

        let error = restore_preserved_copy(Some(&previous), &target).unwrap_err();

        assert!(
            error.to_string().contains("SQLite")
                || error.to_string().contains("database disk image")
                || error.to_string().contains("file is not a database")
        );
        assert!(!target.exists());
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn restore_checkpoint_refuses_a_busy_wal_reader() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("busy.sqlite");
        let writer = rusqlite::Connection::open(&store).unwrap();
        writer
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .unwrap();
        writer
            .execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE t (v INTEGER);")
            .unwrap();
        writer.execute("INSERT INTO t VALUES (1)", []).unwrap();

        let reader = rusqlite::Connection::open(&store).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();
        writer.execute("INSERT INTO t VALUES (2)", []).unwrap();

        let error = checkpoint_sqlite_for_restore(&store).unwrap_err();
        assert!(error.to_string().contains("is busy; stop its daemon"));
        drop(reader);
        checkpoint_sqlite_for_restore(&store).unwrap();
    }

    fn manifest_with_objects(hot: &str, cold: Option<&str>, backend: &str) -> BackupManifest {
        BackupManifest {
            schema: 1,
            backup_id: "test".to_string(),
            created_at_ms: 0,
            backend: backend.to_string(),
            source_store: "source.sqlite".to_string(),
            database: database_manifest(hot, "none", b"", b""),
            cold_database: cold.map(|object| database_manifest(object, "none", b"", b"")),
            sync_cursor: None,
        }
    }

    #[test]
    fn manifest_rejects_unsafe_payload_paths_before_lookup() {
        for object in [
            "",
            ".",
            "..",
            "../outside.sqlite",
            "nested/wiki.sqlite",
            "/tmp/wiki.sqlite",
            r"..\outside.sqlite",
        ] {
            let manifest = manifest_with_objects(object, None, "sqlite");
            let error = validate_manifest_metadata(&manifest, Some("sqlite")).unwrap_err();
            assert!(
                error.to_string().contains("simple relative filename"),
                "unexpected validation for {object:?}: {error}"
            );
        }
    }

    #[test]
    fn manifest_rejects_same_hot_cold_object_and_non_sqlite_backend() {
        let same = manifest_with_objects("wiki.sqlite", Some("wiki.sqlite"), "sqlite");
        assert!(
            validate_manifest_metadata(&same, Some("sqlite"))
                .unwrap_err()
                .to_string()
                .contains("must be distinct")
        );

        let redb = manifest_with_objects("wiki.sqlite", None, "redb");
        assert!(
            validate_manifest_metadata(&redb, Some("sqlite"))
                .unwrap_err()
                .to_string()
                .contains("not SQLite-compatible")
        );
    }

    #[test]
    fn s3_uri_detection_is_prefix_based() {
        assert!(is_s3_uri("s3://bucket/prefix"));
        assert!(!is_s3_uri("/tmp/backup"));
    }
}
