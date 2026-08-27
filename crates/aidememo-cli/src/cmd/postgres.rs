//! `aidememo postgres` — PostgreSQL-specific backup, restore, and tenant operations.

use aidememo_core::AideMemoError;
use bpaf::*;
use serde_json::json;
use std::path::{Path, PathBuf};

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub enum PostgresSub {
    BackupCreate {
        database_url: String,
        destination: PathBuf,
        json: bool,
    },
    BackupRestore {
        source: PathBuf,
        database_url: String,
        json: bool,
    },
    TenantExport {
        tenant_id: String,
        destination: PathBuf,
        database_url: String,
        json: bool,
    },
    TenantDelete {
        tenant_id: String,
        database_url: String,
        confirm: bool,
        json: bool,
    },
}

pub fn postgres_command() -> impl Parser<Command> {
    let database_url = long("database-url")
        .help("PostgreSQL connection URL (postgres://user:pass@host:port/db)")
        .argument::<String>("URL");
    let destination = positional::<PathBuf>("DESTINATION")
        .help("Local backup directory");
    let json = long("json")
        .help("Emit JSON output")
        .switch();
    let backup_create = construct!(PostgresSub::BackupCreate {
        database_url,
        destination,
        json,
    })
    .to_options()
    .command("backup-create")
    .help("Create a PostgreSQL logical backup using pg_dump");

    let source = positional::<PathBuf>("SOURCE").help("Local backup directory");
    let database_url = long("database-url")
        .help("PostgreSQL connection URL (postgres://user:pass@host:port/db)")
        .argument::<String>("URL");
    let json = long("json")
        .help("Emit JSON output")
        .switch();
    let backup_restore = construct!(PostgresSub::BackupRestore {
        source,
        database_url,
        json,
    })
    .to_options()
    .command("backup-restore")
    .help("Restore a PostgreSQL logical backup using pg_restore");

    let tenant_id = positional::<String>("TENANT_ID").help("Tenant ID to export");
    let destination = positional::<PathBuf>("DESTINATION")
        .help("Local export directory");
    let database_url = long("database-url")
        .help("PostgreSQL connection URL (postgres://user:pass@host:port/db)")
        .argument::<String>("URL");
    let json = long("json")
        .help("Emit JSON output")
        .switch();
    let tenant_export = construct!(PostgresSub::TenantExport {
        tenant_id,
        destination,
        database_url,
        json,
    })
    .to_options()
    .command("tenant-export")
    .help("Export all resources for a specific tenant");

    let tenant_id = positional::<String>("TENANT_ID").help("Tenant ID to delete");
    let database_url = long("database-url")
        .help("PostgreSQL connection URL (postgres://user:pass@host:port/db)")
        .argument::<String>("URL");
    let confirm = long("confirm")
        .help("Confirm deletion of all tenant data")
        .switch();
    let json = long("json")
        .help("Emit JSON output")
        .switch();
    let tenant_delete = construct!(PostgresSub::TenantDelete {
        tenant_id,
        database_url,
        confirm,
        json,
    })
    .to_options()
    .command("tenant-delete")
    .help("Delete all data for a specific tenant (DESTRUCTIVE)");

    construct!([backup_create, backup_restore, tenant_export, tenant_delete])
        .map(Command::Postgres)
        .to_options()
        .command("postgres")
        .help("PostgreSQL-specific backup, restore, and tenant operations")
}

pub fn run_postgres(
    sub: PostgresSub,
    global_json: bool,
) -> Result<String, AideMemoError> {
    match sub {
        PostgresSub::BackupCreate {
            database_url,
            destination,
            json,
        } => {
            let json = json || global_json;
            #[cfg(feature = "postgres")]
            {
                use aidememo_store_postgres::backup;
                let report = backup::create_postgres_backup(&database_url, &destination)
                    .map_err(domain_error_to_core)?;
                if json {
                    serde_json::to_string_pretty(&report).map_err(|source| {
                        AideMemoError::Serialize {
                            context: "postgres backup create report".to_string(),
                            source,
                        }
                    })
                } else {
                    Ok(format!(
                        "postgres backup created: {}\nmanifest: {}\ndatabase: {}\nsha256: {}",
                        report.destination,
                        report.manifest_uri,
                        report.database_uri,
                        report.manifest.database.stored_sha256
                    ))
                }
            }
            #[cfg(not(feature = "postgres"))]
            {
                let _ = (database_url, destination, json);
                Err(AideMemoError::InvalidInput(
                    "PostgreSQL backup requires the `postgres` feature".to_string(),
                ))
            }
        }
        PostgresSub::BackupRestore {
            source,
            database_url,
            json,
        } => {
            let json = json || global_json;
            #[cfg(feature = "postgres")]
            {
                use aidememo_store_postgres::backup;
                let report = backup::restore_postgres_backup(&source, &database_url)
                    .map_err(domain_error_to_core)?;
                if json {
                    serde_json::to_string_pretty(&report).map_err(|source| {
                        AideMemoError::Serialize {
                            context: "postgres backup restore report".to_string(),
                            source,
                        }
                    })
                } else {
                    Ok(format!(
                        "postgres backup restored: {} -> {}",
                        report.source, report.target_database
                    ))
                }
            }
            #[cfg(not(feature = "postgres"))]
            {
                let _ = (source, database_url, json);
                Err(AideMemoError::InvalidInput(
                    "PostgreSQL restore requires the `postgres` feature".to_string(),
                ))
            }
        }
        PostgresSub::TenantExport {
            tenant_id,
            destination,
            database_url,
            json,
        } => {
            let json = json || global_json;
            #[cfg(feature = "postgres")]
            {
                use aidememo_domain::TenantId;
                use aidememo_store_postgres::{PostgresCommandStore, backup};
                let tenant = TenantId::try_from(tenant_id.as_str())
                    .map_err(|error| {
                        AideMemoError::InvalidInput(format!("invalid tenant ID: {error}"))
                    })?;
                let store = PostgresCommandStore::connect_no_tls(&database_url)
                    .map_err(domain_error_to_core)?;
                let report = backup::export_tenant(&store, &tenant, &destination)
                    .map_err(domain_error_to_core)?;
                if json {
                    serde_json::to_string_pretty(&report).map_err(|source| {
                        AideMemoError::Serialize {
                            context: "tenant export report".to_string(),
                            source,
                        }
                    })
                } else {
                    Ok(format!(
                        "tenant exported: {}\nresources: {}\nmanifest: {}",
                        report.tenant_id, report.resource_count, report.manifest_uri
                    ))
                }
            }
            #[cfg(not(feature = "postgres"))]
            {
                let _ = (tenant_id, destination, database_url, json);
                Err(AideMemoError::InvalidInput(
                    "Tenant export requires the `postgres` feature".to_string(),
                ))
            }
        }
        PostgresSub::TenantDelete {
            tenant_id,
            database_url,
            confirm,
            json,
        } => {
            if !confirm {
                return Err(AideMemoError::InvalidInput(
                    "tenant delete is destructive; pass --confirm to proceed".to_string(),
                ));
            }
            let json = json || global_json;
            #[cfg(feature = "postgres")]
            {
                use aidememo_domain::TenantId;
                use aidememo_store_postgres::{PostgresCommandStore, backup};
                let tenant = TenantId::try_from(tenant_id.as_str())
                    .map_err(|error| {
                        AideMemoError::InvalidInput(format!("invalid tenant ID: {error}"))
                    })?;
                let store = PostgresCommandStore::connect_no_tls(&database_url)
                    .map_err(domain_error_to_core)?;
                let report = backup::delete_tenant(&store, &tenant)
                    .map_err(domain_error_to_core)?;
                if json {
                    serde_json::to_string_pretty(&report).map_err(|source| {
                        AideMemoError::Serialize {
                            context: "tenant delete report".to_string(),
                            source,
                        }
                    })
                } else {
                    Ok(format!(
                        "tenant deleted: {}\nresources: {}\nreceipts: {}\nchanges: {}",
                        report.tenant_id,
                        report.deleted_resources,
                        report.deleted_receipts,
                        report.deleted_changes
                    ))
                }
            }
            #[cfg(not(feature = "postgres"))]
            {
                let _ = (tenant_id, database_url, confirm, json);
                Err(AideMemoError::InvalidInput(
                    "Tenant delete requires the `postgres` feature".to_string(),
                ))
            }
        }
    }
}

#[cfg(feature = "postgres")]
fn domain_error_to_core(error: aidememo_domain::DomainError) -> AideMemoError {
    match error {
        aidememo_domain::DomainError::InvalidInput(msg) => AideMemoError::InvalidInput(msg),
        aidememo_domain::DomainError::StorageFailure { operation, detail } => {
            AideMemoError::Internal(format!("storage {operation}: {detail}"))
        }
        aidememo_domain::DomainError::SerializeFailure { context, source } => {
            AideMemoError::Serialize { context, source }
        }
        aidememo_domain::DomainError::DeserializeFailure { context, source } => {
            AideMemoError::Deserialize { context, source }
        }
        _ => AideMemoError::Internal(error.to_string()),
    }
}
