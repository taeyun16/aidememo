//! `aidememo postgres` — PostgreSQL-specific backup, restore, and tenant operations.

use aidememo_core::AideMemoError;
use bpaf::*;
use std::path::PathBuf;

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
            let _ = (database_url, destination, json);
            Err(AideMemoError::InvalidInput(
                "PostgreSQL backup requires aidememo-store-postgres; use --backend postgres with aidememo backup for SQLite-like stores".to_string(),
            ))
        }
        PostgresSub::BackupRestore {
            source,
            database_url,
            json,
        } => {
            let json = json || global_json;
            let _ = (source, database_url, json);
            Err(AideMemoError::InvalidInput(
                "PostgreSQL restore requires aidememo-store-postgres; use --backend postgres with aidememo backup for SQLite-like stores".to_string(),
            ))
        }
        PostgresSub::TenantExport {
            tenant_id,
            destination,
            database_url,
            json,
        } => {
            let json = json || global_json;
            let _ = (tenant_id, destination, database_url, json);
            Err(AideMemoError::InvalidInput(
                "Tenant export requires aidememo-store-postgres".to_string(),
            ))
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
            let _ = (tenant_id, database_url, confirm, json);
            Err(AideMemoError::InvalidInput(
                "Tenant delete requires aidememo-store-postgres".to_string(),
            ))
        }
    }
}
