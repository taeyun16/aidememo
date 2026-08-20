use aidememo_store_postgres::PostgresCommandStore;
use std::{fs, time::Duration};

#[test]
#[ignore = "requires disposable TLS PostgreSQL via AIDEMEMO_POSTGRES_TLS_URL"]
fn verified_tls_connects_even_when_url_requests_sslmode_disable()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_TLS_URL")?;
    let ca = fs::read(std::env::var("AIDEMEMO_POSTGRES_TLS_CA")?)?;
    let store = PostgresCommandStore::connect_tls_with_timeouts(
        &url,
        Some(&ca),
        Duration::from_millis(1_500),
        Duration::from_millis(250),
    )?;
    assert_eq!(store.schema_version()?, 2);
    Ok(())
}

#[test]
#[ignore = "requires disposable TLS PostgreSQL and wrong CA fixture"]
fn verified_tls_rejects_untrusted_ca() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_TLS_URL")?;
    let wrong_ca = fs::read(std::env::var("AIDEMEMO_POSTGRES_TLS_WRONG_CA")?)?;
    let result = PostgresCommandStore::connect_tls_with_timeouts(
        &url,
        Some(&wrong_ca),
        Duration::from_millis(1_500),
        Duration::from_millis(250),
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
#[ignore = "requires plaintext PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn tls_required_rejects_plaintext_postgres_without_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let result = PostgresCommandStore::connect_tls_with_timeouts(
        &url,
        None,
        Duration::from_millis(1_500),
        Duration::from_millis(250),
    );
    assert!(result.is_err());
    Ok(())
}
