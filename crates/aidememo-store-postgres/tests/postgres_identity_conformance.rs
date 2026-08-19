use aidememo_domain::identity_conformance;
use aidememo_store_postgres::PostgresCommandStore;

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn postgres_server_identity_conformance() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let mut store = PostgresCommandStore::connect_no_tls(&url)?;
    let report = identity_conformance::run(&mut store)?;
    assert_eq!(report.checks.len(), 10);
    Ok(())
}
