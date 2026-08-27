//! `aidememo postgres` — PostgreSQL-specific backup, restore, and tenant operations.

use aidememo_core::AideMemoError;
use bpaf::*;
use std::path::PathBuf;

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub enum PostgresSub {
    BackupCreate {
        destination: PathBuf,
        json: bool,
    },
    BackupRestore {
        source: PathBuf,
        json: bool,
    },
    TenantExport {
        tenant_id: String,
        destination: PathBuf,
        json: bool,
    },
    TenantDelete {
        tenant_id: String,
        confirm: bool,
        json: bool,
    },
}

pub fn postgres_command() -> impl Parser<Command> {
    let destination = positional::<PathBuf>("DESTINATION")
        .help("Local backup directory");
    let json = long("json")
        .help("Emit JSON output")
        .switch();
    let backup_create = construct!(PostgresSub::BackupCreate {
        destination,
        json,
    })
    .to_options()
    .command("backup-create")
    .help("Create a PostgreSQL logical backup using pg_dump (requires AIDEMEMO_POSTGRES_URL)");

    let source = positional::<PathBuf>("SOURCE").help("Local backup directory");
    let json = long("json")
        .help("Emit JSON output")
        .switch();
    let backup_restore = construct!(PostgresSub::BackupRestore {
        source,
        json,
    })
    .to_options()
    .command("backup-restore")
    .help("Restore a PostgreSQL logical backup using pg_restore (requires AIDEMEMO_POSTGRES_URL)");

    let tenant_id = positional::<String>("TENANT_ID").help("Tenant ID to export");
    let destination = positional::<PathBuf>("DESTINATION")
        .help("Local export directory");
    let json = long("json")
        .help("Emit JSON output")
        .switch();
    let tenant_export = construct!(PostgresSub::TenantExport {
        tenant_id,
        destination,
        json,
    })
    .to_options()
    .command("tenant-export")
    .help("Export all resources for a specific tenant (requires AIDEMEMO_POSTGRES_URL)");

    let tenant_id = positional::<String>("TENANT_ID").help("Tenant ID to delete");
    let confirm = long("confirm")
        .help("Confirm deletion of all tenant data")
        .switch();
    let json = long("json")
        .help("Emit JSON output")
        .switch();
    let tenant_delete = construct!(PostgresSub::TenantDelete {
        tenant_id,
        confirm,
        json,
    })
    .to_options()
    .command("tenant-delete")
    .help("Delete all data for a specific tenant (requires AIDEMEMO_POSTGRES_URL and --confirm)");

    construct!([backup_create, backup_restore, tenant_export, tenant_delete])
        .to_options()
        .command("postgres")
        .help("PostgreSQL-specific backup, restore, and tenant operations (all require AIDEMEMO_POSTGRES_URL)")
        .map(Command::Postgres)
}

pub fn run_postgres(sub: PostgresSub, global_json: bool) -> Result<(), AideMemoError> {
    let database_url = std::env::var("AIDEMEMO_POSTGRES_URL").map_err(|_| {
        AideMemoError::InvalidInput(
            "AIDEMEMO_POSTGRES_URL environment variable is required for PostgreSQL operations"
                .to_string(),
        )
    })?;

    match sub {
        PostgresSub::BackupCreate { destination, json } => {
            let json = json || global_json;
            let _ = (database_url, destination, json);
            Err(AideMemoError::InvalidInput(
                "PostgreSQL backup requires aidememo-store-postgres; use --backend postgres with aidememo backup for SQLite-like stores".to_string(),
            ))
        }
        PostgresSub::BackupRestore { source, json } => {
            let json = json || global_json;
            let _ = (database_url, source, json);
            Err(AideMemoError::InvalidInput(
                "PostgreSQL restore requires aidememo-store-postgres; use --backend postgres with aidememo backup for SQLite-like stores".to_string(),
            ))
        }
        PostgresSub::TenantExport {
            tenant_id,
            destination,
            json,
        } => {
            let json = json || global_json;
            let _ = (database_url, tenant_id, destination, json);
            Err(AideMemoError::InvalidInput(
                "Tenant export requires aidememo-store-postgres".to_string(),
            ))
        }
        PostgresSub::TenantDelete {
            tenant_id,
            confirm,
            json,
        } => {
            if !confirm {
                return Err(AideMemoError::InvalidInput(
                    "tenant delete is destructive; pass --confirm to proceed".to_string(),
                ));
            }
            let json = json || global_json;
            let _ = (database_url, tenant_id, confirm, json);
            Err(AideMemoError::InvalidInput(
                "Tenant delete requires aidememo-store-postgres".to_string(),
            ))
        }
    }
}
