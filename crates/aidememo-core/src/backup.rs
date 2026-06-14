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
#[cfg(feature = "s3")]
const SQLITE_ZSTD_OBJECT: &str = "wiki.sqlite.zst";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    pub schema: u32,
    pub backup_id: String,
    pub created_at_ms: u64,
    pub backend: String,
    pub source_store: String,
    pub database: BackupDatabase,
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
    pub fact_updated_at: Option<u64>,
}

impl BackupSyncCursor {
    pub fn from_sync(cursor: crate::sync::SyncCursor) -> Self {
        Self {
            entity: cursor.entity.map(|id| id.0.to_string()),
            fact: cursor.fact.map(|id| id.0.to_string()),
            entity_updated_at: cursor.entity_updated_at,
            fact_updated_at: cursor.fact_updated_at,
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
            fact_updated_at: self.fact_updated_at,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupCreateReport {
    pub manifest: BackupManifest,
    pub destination: String,
    pub manifest_uri: String,
    pub database_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRestoreReport {
    pub manifest: BackupManifest,
    pub source: String,
    pub restored_store: String,
    pub previous_store: Option<String>,
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

    let snapshot = tempfile::NamedTempFile::new().map_err(|source| AideMemoError::StoreOpen {
        path: std::env::temp_dir(),
        source: Box::new(source),
    })?;
    snapshot_sqlite(store_path, snapshot.path())?;
    validate_sqlite_file(snapshot.path())?;
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
        sync_cursor,
    };
    let manifest_path = output_dir.join(MANIFEST_FILE);
    write_manifest(&manifest_path, &manifest)?;

    Ok(BackupCreateReport {
        manifest,
        destination: output_dir.display().to_string(),
        manifest_uri: manifest_path.display().to_string(),
        database_uri: db_path.display().to_string(),
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
    let stored = std::fs::read(source_dir.join(&manifest.database.object)).map_err(|source| {
        AideMemoError::StoreRead {
            table: "backup",
            key: manifest.database.object.clone(),
            source: Box::new(source),
        }
    })?;
    restore_from_stored_bytes(
        source_dir.display().to_string(),
        stored,
        manifest,
        target_store_path,
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

    let snapshot = tempfile::NamedTempFile::new().map_err(|source| AideMemoError::StoreOpen {
        path: std::env::temp_dir(),
        source: Box::new(source),
    })?;
    snapshot_sqlite(store_path, snapshot.path())?;
    validate_sqlite_file(snapshot.path())?;
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
    put_s3_object(&client, &prefix.bucket, &manifest_key, manifest_bytes).await?;

    Ok(BackupCreateReport {
        manifest,
        destination: prefix.to_uri(),
        manifest_uri: format!("s3://{}/{}", prefix.bucket, manifest_key),
        database_uri: format!("s3://{}/{}", prefix.bucket, db_key),
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
    let db_key = location.object_key(&manifest.database.object);
    let stored = get_s3_object(&client, &location.bucket, &db_key).await?;
    restore_from_stored_bytes(
        location.to_uri(),
        stored,
        manifest,
        target_store_path,
        force,
    )
}

pub fn read_local_backup_manifest(source_dir: &Path) -> Result<BackupManifest> {
    read_manifest(&source_dir.join(MANIFEST_FILE))
}

#[cfg(feature = "s3")]
pub async fn read_s3_backup_manifest(source: &str) -> Result<BackupManifest> {
    let location = S3Location::parse(source)?;
    let client = s3_client().await;
    let manifest_key = location.object_key(MANIFEST_FILE);
    let manifest_bytes = get_s3_object(&client, &location.bucket, &manifest_key).await?;
    serde_json::from_slice(&manifest_bytes).map_err(|source| AideMemoError::Deserialize {
        context: "backup manifest".to_string(),
        source,
    })
}

pub fn is_s3_uri(value: &str) -> bool {
    value.starts_with("s3://")
}

fn restore_from_stored_bytes(
    source: String,
    stored: Vec<u8>,
    manifest: BackupManifest,
    target_store_path: &Path,
    force: bool,
) -> Result<BackupRestoreReport> {
    validate_manifest(&manifest, &stored)?;
    let sqlite_bytes = decode_database_bytes(&manifest, &stored)?;
    let sqlite_sha256 = sha256_hex(&sqlite_bytes);
    if sqlite_sha256 != manifest.database.sqlite_sha256 {
        return Err(AideMemoError::InvalidInput(
            "backup SQLite payload checksum mismatch".to_string(),
        ));
    }

    let parent = target_store_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| AideMemoError::StoreOpen {
        path: parent.to_path_buf(),
        source: Box::new(source),
    })?;
    let tmp =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| AideMemoError::StoreOpen {
            path: parent.to_path_buf(),
            source: Box::new(source),
        })?;
    std::fs::write(tmp.path(), &sqlite_bytes).map_err(|source| AideMemoError::StoreWrite {
        table: "backup_restore",
        key: tmp.path().display().to_string(),
        source: Box::new(source),
    })?;
    validate_sqlite_file(tmp.path())?;

    if target_store_path.exists() && !force {
        return Err(AideMemoError::InvalidInput(format!(
            "target store {} already exists; pass --force to replace it",
            target_store_path.display()
        )));
    }

    let previous_store = if target_store_path.exists() {
        let previous = previous_store_path(target_store_path);
        let _ = std::fs::remove_file(&previous);
        std::fs::rename(target_store_path, &previous).map_err(|source| {
            AideMemoError::StoreWrite {
                table: "backup_restore",
                key: target_store_path.display().to_string(),
                source: Box::new(source),
            }
        })?;
        Some(previous)
    } else {
        None
    };

    let removed_sidecars = remove_restore_sidecars(target_store_path)?;
    if let Err(source_error) = std::fs::rename(tmp.path(), target_store_path) {
        if let Some(previous) = &previous_store {
            let _ = std::fs::rename(previous, target_store_path);
        }
        return Err(AideMemoError::StoreWrite {
            table: "backup_restore",
            key: target_store_path.display().to_string(),
            source: Box::new(source_error),
        });
    }

    Ok(BackupRestoreReport {
        manifest,
        source,
        restored_store: target_store_path.display().to_string(),
        previous_store: previous_store.map(|path| path.display().to_string()),
        removed_sidecars,
    })
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
    let mut output = Vec::new();
    snapshot.sync_export(
        crate::sync::SyncExportOpts {
            include_relations: true,
            ..Default::default()
        },
        &mut output,
    )?;
    let jsonl = std::str::from_utf8(&output)
        .map_err(|source| AideMemoError::Internal(format!("backup sync cursor UTF-8: {source}")))?;
    let cursor = crate::sync::cursor_from_jsonl(jsonl)?;
    Ok(Some(BackupSyncCursor::from_sync(cursor)))
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
    if manifest.schema != 1 {
        return Err(AideMemoError::InvalidInput(format!(
            "unsupported backup manifest schema {}",
            manifest.schema
        )));
    }
    if manifest.database.stored_bytes != stored.len() as u64 {
        return Err(AideMemoError::InvalidInput(
            "backup stored payload size mismatch".to_string(),
        ));
    }
    let actual = sha256_hex(stored);
    if manifest.database.stored_sha256 != actual {
        return Err(AideMemoError::InvalidInput(
            "backup stored payload checksum mismatch".to_string(),
        ));
    }
    Ok(())
}

fn decode_database_bytes(manifest: &BackupManifest, stored: &[u8]) -> Result<Vec<u8>> {
    match manifest.database.compression.as_str() {
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

fn remove_restore_sidecars(store_path: &Path) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for path in [
        append_suffix(store_path, "-wal"),
        append_suffix(store_path, "-shm"),
        store_path.with_extension("hnsw.bin"),
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
    fn s3_uri_detection_is_prefix_based() {
        assert!(is_s3_uri("s3://bucket/prefix"));
        assert!(!is_s3_uri("/tmp/backup"));
    }
}
