use aidememo_domain::{ProjectEpoch, ProjectId, ProjectScope, TenantId, conformance};
use aidememo_store_postgres::PostgresCommandStore;

/// Run against a disposable, empty PostgreSQL database.
///
/// The test is ignored by default because normal CI does not provision a
/// database service. Set `AIDEMEMO_POSTGRES_URL` and run with `--ignored` in
/// the dedicated PostgreSQL conformance job.
#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn portable_command_store_conformance() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let epoch = ProjectEpoch::try_from("epoch_fixture")?;
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_fixture")?,
        ProjectId::try_from("project_fixture")?,
    );
    let mut store = PostgresCommandStore::connect_no_tls(&url)?;
    store.initialize_project(&scope, &epoch)?;
    let report = conformance::run(&mut store, epoch)?;
    assert_eq!(report.final_sequence.get(), 3);
    assert_eq!(report.tombstone_revision.get(), 3);
    assert_eq!(report.checks.len(), 15);
    Ok(())
}
