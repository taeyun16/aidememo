//! PostgreSQL logical backup / restore helpers via pg_dump / pg_restore.
//!
//! This module implements PostgreSQL-backed logical backup using pg_dump custom
//! format, manifest checksums, and tenant-scoped export. The hot store is the
//! canonical PostgreSQL database; backup creates a logical dump plus a manifest,
//! and restore verifies the manifest before importing.

use aidememo_domain::{DomainError, ProjectScope, TenantId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use ulid::Ulid;

const MANIFEST_FILE: &str = "manifest.json";
const POSTGRES_DUMP_OBJECT: &str = "canonical.pgdump";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostgresBackupManifest {
    pub schema: u32,
    pub backup_id: String,
    pub created_at_ms: u64,
    pub backend: String,
    pub source_database: String,
    pub database: PostgresBackupDatabase,
    /// Optional project sequence high-water mark for delta backup base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_high_water: Option<SequenceHighWater>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostgresBackupDatabase {
    pub object: String,
    pub compression: String,
    pub stored_bytes: u64,
    pub stored_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SequenceHighWater {
    pub tenant_id: String,
    pub project_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresBackupCreateReport {
    pub manifest: PostgresBackupManifest,
    pub destination: String,
    pub manifest_uri: String,
    pub database_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresBackupRestoreReport {
    pub manifest: PostgresBackupManifest,
    pub source: String,
    pub target_database: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantExportReport {
    pub tenant_id: String,
    pub destination: String,
    pub resource_count: usize,
    pub manifest_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantDeleteReport {
    pub tenant_id: String,
    pub deleted_resources: usize,
    pub deleted_receipts: usize,
    pub deleted_changes: usize,
}

/// Create a PostgreSQL logical backup using pg_dump custom format.
pub fn create_postgres_backup(
    database_url: &str,
    destination_dir: &Path,
) -> Result<PostgresBackupCreateReport, DomainError> {
    let backup_id = format!("backup-{}", Ulid::new());
    let output_dir = destination_dir.join(&backup_id);
    std::fs::create_dir_all(&output_dir).map_err(|error| {
        DomainError::StorageFailure {
            operation: "backup_create_dir",
            detail: format!(
                "failed to create backup directory {}: {error}",
                output_dir.display()
            ),
        }
    })?;

    let dump_path = output_dir.join(POSTGRES_DUMP_OBJECT);
    
    // Execute pg_dump with custom format (compressed)
    let output = Command::new("pg_dump")
        .arg(database_url)
        .arg("--format=custom")
        .arg("--file")
        .arg(&dump_path)
        .arg("--no-owner")
        .arg("--no-acl")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            DomainError::StorageFailure {
                operation: "pg_dump_execute",
                detail: format!("pg_dump execution failed: {error}"),
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DomainError::StorageFailure {
            operation: "pg_dump",
            detail: format!("pg_dump failed: {stderr}"),
        });
    }

    let dump_bytes = std::fs::read(&dump_path).map_err(|error| {
        DomainError::StorageFailure {
            operation: "backup_read_dump",
            detail: format!(
                "failed to read dump file {}: {error}",
                dump_path.display()
            ),
        }
    })?;
    let dump_sha256 = sha256_hex(&dump_bytes);

    let manifest = PostgresBackupManifest {
        schema: 1,
        backup_id,
        created_at_ms: current_epoch_ms(),
        backend: "postgres".to_string(),
        source_database: sanitize_database_url(database_url),
        database: PostgresBackupDatabase {
            object: POSTGRES_DUMP_OBJECT.to_string(),
            compression: "pg_custom".to_string(),
            stored_bytes: dump_bytes.len() as u64,
            stored_sha256: dump_sha256,
        },
        sequence_high_water: None,
    };

    let manifest_path = output_dir.join(MANIFEST_FILE);
    write_manifest(&manifest_path, &manifest)?;

    Ok(PostgresBackupCreateReport {
        manifest,
        destination: output_dir.display().to_string(),
        manifest_uri: manifest_path.display().to_string(),
        database_uri: dump_path.display().to_string(),
    })
}

/// Restore a PostgreSQL logical backup using pg_restore.
/// 
/// **WARNING**: This drops and recreates the target database schema.
/// Ensure the target database URL points to a database that can be safely replaced.
pub fn restore_postgres_backup(
    source_dir: &Path,
    target_database_url: &str,
) -> Result<PostgresBackupRestoreReport, DomainError> {
    let manifest_path = source_dir.join(MANIFEST_FILE);
    let manifest = read_manifest(&manifest_path)?;
    validate_manifest_metadata(&manifest)?;

    let dump_path = source_dir.join(&manifest.database.object);
    let dump_bytes = std::fs::read(&dump_path).map_err(|error| {
        DomainError::StorageFailure {
            operation: "backup_restore_read",
            detail: format!(
                "failed to read dump file {}: {error}",
                dump_path.display()
            ),
        }
    })?;

    // Validate checksum
    if dump_bytes.len() as u64 != manifest.database.stored_bytes {
        return Err(DomainError::InvalidInput(
            "backup dump file size mismatch".to_string(),
        ));
    }
    let actual_sha256 = sha256_hex(&dump_bytes);
    if actual_sha256 != manifest.database.stored_sha256 {
        return Err(DomainError::InvalidInput(
            "backup dump checksum mismatch".to_string(),
        ));
    }

    // Execute pg_restore with clean option (drops existing objects first)
    let output = Command::new("pg_restore")
        .arg("--dbname")
        .arg(target_database_url)
        .arg("--clean")
        .arg("--if-exists")
        .arg("--no-owner")
        .arg("--no-acl")
        .arg(&dump_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            DomainError::StorageFailure {
                operation: "pg_restore_execute",
                detail: format!("pg_restore execution failed: {error}"),
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DomainError::StorageFailure {
            operation: "pg_restore",
            detail: format!("pg_restore failed: {stderr}"),
        });
    }

    Ok(PostgresBackupRestoreReport {
        manifest,
        source: source_dir.display().to_string(),
        target_database: sanitize_database_url(target_database_url),
    })
}

/// Export all resources for a specific tenant to a manifest file.
pub fn export_tenant(
    store: &crate::PostgresCommandStore,
    tenant_id: &TenantId,
    destination_dir: &Path,
) -> Result<TenantExportReport, DomainError> {
    let export_id = format!("tenant-export-{}-{}", tenant_id, Ulid::new());
    let output_dir = destination_dir.join(&export_id);
    std::fs::create_dir_all(&output_dir).map_err(|error| {
        DomainError::StorageFailure {
            operation: "tenant_export_create_dir",
            detail: format!(
                "failed to create export directory {}: {error}",
                output_dir.display()
            ),
        }
    })?;

    // Query all resources for this tenant
    let resources = store.export_tenant_resources(tenant_id)?;
    let resource_count = resources.len();

    let manifest = serde_json::json!({
        "schema": 1,
        "export_id": export_id,
        "tenant_id": tenant_id.to_string(),
        "created_at_ms": current_epoch_ms(),
        "resource_count": resource_count,
        "resources": resources,
    });

    let manifest_path = output_dir.join("tenant_export.json");
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|error| DomainError::SerializeFailure {
            context: "tenant export manifest".to_string(),
            source: error,
        })?;
    std::fs::write(&manifest_path, manifest_bytes).map_err(|error| {
        DomainError::StorageFailure {
            operation: "tenant_export_write",
            detail: format!(
                "failed to write export manifest {}: {error}",
                manifest_path.display()
            ),
        }
    })?;

    Ok(TenantExportReport {
        tenant_id: tenant_id.to_string(),
        destination: output_dir.display().to_string(),
        resource_count,
        manifest_uri: manifest_path.display().to_string(),
    })
}

/// Delete all resources for a specific tenant from the PostgreSQL store.
/// 
/// **WARNING**: This is a destructive operation that removes all data for the tenant.
pub fn delete_tenant(
    store: &crate::PostgresCommandStore,
    tenant_id: &TenantId,
) -> Result<TenantDeleteReport, DomainError> {
    store.delete_tenant_data(tenant_id)
}

fn write_manifest(
    path: &Path,
    manifest: &PostgresBackupManifest,
) -> Result<(), DomainError> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
        DomainError::SerializeFailure {
            context: "postgres backup manifest".to_string(),
            source: error,
        }
    })?;
    std::fs::write(path, bytes).map_err(|error| {
        DomainError::StorageFailure {
            operation: "backup_manifest_write",
            detail: format!(
                "failed to write manifest {}: {error}",
                path.display()
            ),
        }
    })
}

fn read_manifest(path: &Path) -> Result<PostgresBackupManifest, DomainError> {
    let bytes = std::fs::read(path).map_err(|error| {
        DomainError::StorageFailure {
            operation: "backup_manifest_read",
            detail: format!(
                "failed to read manifest {}: {error}",
                path.display()
            ),
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|error| DomainError::DeserializeFailure {
        context: "postgres backup manifest".to_string(),
        source: error,
    })
}

fn validate_manifest_metadata(manifest: &PostgresBackupManifest) -> Result<(), DomainError> {
    if manifest.schema != 1 {
        return Err(DomainError::InvalidInput(format!(
            "unsupported postgres backup manifest schema {}",
            manifest.schema
        )));
    }
    if manifest.backend != "postgres" {
        return Err(DomainError::InvalidInput(format!(
            "postgres backup manifest backend `{}` is not postgres-compatible",
            manifest.backend
        )));
    }
    Ok(())
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

fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn sanitize_database_url(url: &str) -> String {
    // Remove password from URL for logging
    url.split('@')
        .last()
        .map(|s| format!("postgres://<credentials>@{}", s))
        .unwrap_or_else(|| "postgres://<redacted>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_schema_validation() {
        let mut manifest = PostgresBackupManifest {
            schema: 1,
            backup_id: "test".to_string(),
            created_at_ms: 0,
            backend: "postgres".to_string(),
            source_database: "test".to_string(),
            database: PostgresBackupDatabase {
                object: POSTGRES_DUMP_OBJECT.to_string(),
                compression: "pg_custom".to_string(),
                stored_bytes: 0,
                stored_sha256: String::new(),
            },
            sequence_high_water: None,
        };

        assert!(validate_manifest_metadata(&manifest).is_ok());

        manifest.schema = 2;
        assert!(validate_manifest_metadata(&manifest)
            .unwrap_err()
            .to_string()
            .contains("unsupported"));

        manifest.schema = 1;
        manifest.backend = "sqlite".to_string();
        assert!(validate_manifest_metadata(&manifest)
            .unwrap_err()
            .to_string()
            .contains("not postgres-compatible"));
    }

    #[test]
    fn database_url_sanitization_removes_credentials() {
        assert_eq!(
            sanitize_database_url("postgres://user:pass@localhost:5432/db"),
            "postgres://<credentials>@localhost:5432/db"
        );
        assert_eq!(
            sanitize_database_url("invalid"),
            "postgres://<redacted>"
        );
    }
}
