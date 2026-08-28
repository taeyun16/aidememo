//! PostgreSQL backup/restore and tenant operations integration tests.

#[cfg(test)]
mod tests {
    use aidememo_domain::{ProjectEpoch, ProjectScope, TenantId};
    use aidememo_store_postgres::{
        PostgresCommandStore,
        backup::{create_postgres_backup, export_tenant, restore_postgres_backup},
    };
    use std::path::Path;
    use tempfile::TempDir;

    fn test_database_url() -> Option<String> {
        std::env::var("AIDEMEMO_TEST_POSTGRES_URL").ok()
    }

    #[test]
    fn backup_manifest_schema_validation() {
        use aidememo_store_postgres::backup::{PostgresBackupDatabase, PostgresBackupManifest};

        let manifest = PostgresBackupManifest {
            schema: 1,
            backup_id: "test".to_string(),
            created_at_ms: 0,
            backend: "postgres".to_string(),
            source_database: "test".to_string(),
            database: PostgresBackupDatabase {
                object: "canonical.pgdump".to_string(),
                compression: "pg_custom".to_string(),
                stored_bytes: 0,
                stored_sha256: String::new(),
            },
            sequence_high_water: None,
        };

        // Serialization should work
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(json.contains("postgres"));

        // Deserialization should work
        let parsed: PostgresBackupManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.backend, "postgres");
    }

    #[test]
    #[ignore] // Requires PostgreSQL instance
    fn postgres_backup_restore_roundtrip() {
        let Some(db_url) = test_database_url() else {
            eprintln!("Skipping: AIDEMEMO_TEST_POSTGRES_URL not set");
            return;
        };

        let backup_dir = TempDir::new().unwrap();

        // Create backup
        let report = create_postgres_backup(&db_url, backup_dir.path())
            .expect("backup creation should succeed");

        assert!(Path::new(&report.database_uri).exists());
        assert!(Path::new(&report.manifest_uri).exists());
        assert_eq!(report.manifest.backend, "postgres");

        // Restore would require a separate test database
        // This is a basic smoke test that backup creation works
    }

    #[test]
    #[ignore] // Requires PostgreSQL instance
    fn tenant_export_creates_manifest() {
        let Some(db_url) = test_database_url() else {
            eprintln!("Skipping: AIDEMEMO_TEST_POSTGRES_URL not set");
            return;
        };

        let store =
            PostgresCommandStore::connect_no_tls(&db_url).expect("store connection should succeed");

        let tenant_id = TenantId::try_from("test-tenant").unwrap();
        let project_id = aidememo_domain::ProjectId::try_from("test-project").unwrap();
        let scope = ProjectScope::new(tenant_id.clone(), project_id);
        let epoch = ProjectEpoch::try_from("epoch-test").unwrap();

        // Initialize a test project
        store
            .initialize_project(&scope, &epoch)
            .expect("project initialization should succeed");

        let export_dir = TempDir::new().unwrap();
        let report = export_tenant(&store, &tenant_id, export_dir.path())
            .expect("tenant export should succeed");

        assert!(Path::new(&report.manifest_uri).exists());
        assert_eq!(report.tenant_id, tenant_id.to_string());
    }
}
