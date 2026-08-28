# PostgreSQL Backup, Restore, and Tenant Operations

This document describes the PostgreSQL-specific backup, restore, tenant export, and tenant delete operations for AideMemo's canonical SSOT store.

## Overview

The PostgreSQL adapter provides logical backup and restore functionality using `pg_dump` and `pg_restore`, along with tenant-scoped export and delete operations for multi-tenant environments.

## Prerequisites

- PostgreSQL client tools (`pg_dump`, `pg_restore`) must be available in PATH
- Database access with appropriate permissions
- For production use, use TLS-enabled connections

## Commands

### Backup Create

Create a logical backup of a PostgreSQL canonical store:

```bash
export AIDEMEMO_POSTGRES_URL="postgres://user:pass@localhost:5432/aidememo"
aidememo postgres backup-create /path/to/backup
```

This creates:
- A `pg_dump` custom-format dump file (`canonical.pgdump`)
- A manifest file (`manifest.json`) with checksums and metadata
- Optional sequence high-water mark for delta backups

The manifest includes:
- Schema version
- Backup ID (ULID-based)
- Creation timestamp
- Backend identifier
- Database object with SHA-256 checksum
- Compressed size information

### Backup Restore

Restore a logical backup to a PostgreSQL database:

```bash
export AIDEMEMO_POSTGRES_URL="postgres://user:pass@localhost:5432/aidememo_restored"
aidememo postgres backup-restore /path/to/backup/backup-<ULID>
```

**WARNING**: This uses `pg_restore --clean`, which:
- Drops existing objects before restoring
- Requires an empty or expendable target database
- Cannot be run against a production database without downtime

For production restores:
1. Create a new database
2. Restore to the new database
3. Verify the restore
4. Switch application traffic to the new database

### Tenant Export

Export all resources for a specific tenant:

```bash
export AIDEMEMO_POSTGRES_URL="postgres://user:pass@localhost:5432/aidememo"
aidememo postgres tenant-export <TENANT_ID> /path/to/export
```

This creates a JSON manifest containing:
- All resources for the tenant
- Resource counts
- Metadata for audit and compliance

Use cases:
- Tenant offboarding
- Compliance data exports
- Migration to another AideMemo instance
- Backup before tenant deletion

### Tenant Delete

**DESTRUCTIVE OPERATION**: Delete all data for a tenant:

```bash
export AIDEMEMO_POSTGRES_URL="postgres://user:pass@localhost:5432/aidememo"
aidememo postgres tenant-delete --confirm <TENANT_ID>
```

This permanently deletes:
- All resources (`ssot_resources`)
- All command receipts (`ssot_receipts`)
- All changes (`ssot_changes`)
- All audit records (`ssot_audit`)
- All project metadata (`ssot_projects`)
- All handoff indexes (`ssot_handoff_index`)

The `--confirm` flag is required to prevent accidental deletion.

**Best practice**: Always run `tenant-export` before `tenant-delete`.

## JSON Output

All commands support `--json` for machine-readable output:

```bash
export AIDEMEMO_POSTGRES_URL="postgres://user:pass@localhost:5432/aidememo"
aidememo postgres backup-create --json /path/to/backup
```

## Replica Rebuild

The PostgreSQL canonical store works with the existing replica rebuild drills:

```bash
# Bootstrap a replica from canonical PostgreSQL via HTTP SSOT
aidememo replica pull --remote-profile production

# Verify replica status
aidememo replica status

# Reset and rebuild replica if needed
aidememo replica reset --force
aidememo replica pull --remote-profile production
```

The replica pulls changes from the canonical PostgreSQL store via the HTTP SSOT server's change feed, maintaining a separate exact-read cache.

## Integration with aidememo-server

The PostgreSQL backup/restore operations integrate with the bounded executor pattern:

```rust
use aidememo_store_postgres::{PostgresCommandStore, backup};

// Server initialization with bounded executor
let store = PostgresCommandStore::connect_tls_with_timeouts(
    &database_url,
    Some(root_ca_pem),
    statement_timeout,
    lock_timeout,
)?;

// Backup operation (run via blocking executor, not Axum worker)
let report = backup::create_postgres_backup(
    &database_url,
    destination_path,
)?;
```

Backup and tenant operations are blocking I/O and should not run on Axum runtime worker threads. Use the bounded executor pool for these operations.

## Manifest Format

PostgreSQL backup manifests differ from SQLite manifests:

- **Backend**: `"postgres"` instead of `"sqlite"` or `"libsqlite"`
- **Compression**: `"pg_custom"` (pg_dump's custom format) instead of `"none"` or `"zstd"`
- **Object**: `"canonical.pgdump"` instead of `"wiki.sqlite"`

Example manifest:

```json
{
  "schema": 1,
  "backup_id": "backup-01JAB2C3D4E5F6G7H8J9K0M1N2",
  "created_at_ms": 1735574400000,
  "backend": "postgres",
  "source_database": "postgres://<credentials>@localhost:5432/aidememo",
  "database": {
    "object": "canonical.pgdump",
    "compression": "pg_custom",
    "stored_bytes": 524288,
    "stored_sha256": "abc123..."
  },
  "sequence_high_water": {
    "tenant_id": "tenant-001",
    "project_id": "project-main",
    "sequence": 42
  }
}
```

## Testing

Run PostgreSQL-specific tests with:

```bash
export AIDEMEMO_TEST_POSTGRES_URL="postgres://localhost:5432/aidememo_test"
cargo test -p aidememo-store-postgres --test postgres_backup -- --ignored
```

## Security Considerations

1. **Connection URLs**: Never log or display database URLs with passwords. The backup module sanitizes URLs in reports.

2. **TLS**: Use `connect_tls_with_timeouts` in production:
   ```rust
   PostgresCommandStore::connect_tls_with_timeouts(
       url,
       Some(ca_pem),
       statement_timeout,
       lock_timeout,
   )
   ```

3. **Tenant Isolation**: Tenant delete operations are scoped by `tenant_id`. Verify the tenant ID before confirming deletion.

4. **Backup Encryption**: pg_dump custom format files are not encrypted. Use filesystem encryption or encrypted storage for backup directories.

## Conformance

PostgreSQL backup/restore, tenant export/delete, and replica rebuild operations are part of the Phase 2 exit gate conformance:

- ✅ Logical backup/restore with manifest validation
- ✅ Tenant export with resource counts
- ✅ Tenant delete with audit trail
- ✅ Replica rebuild from canonical PostgreSQL store
- ✅ Index rebuild (via standard `aidememo vector-rebuild`)

These operations pass the same conformance suite as SQLite, ensuring portable operations across backends.
