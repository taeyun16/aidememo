use aidememo_domain::identity_conformance;
use aidememo_store_local::SqliteCommandStore;

#[test]
fn sqlite_server_identity_conformance() -> Result<(), Box<dyn std::error::Error>> {
    let mut store = SqliteCommandStore::open_in_memory()?;
    let report = identity_conformance::run(&mut store)?;
    assert_eq!(report.checks.len(), 10);
    Ok(())
}
