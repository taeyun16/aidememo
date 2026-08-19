#[allow(dead_code)]
#[path = "../src/executor.rs"]
mod executor;

use executor::{BlockingStoreError, BlockingStoreExecutor};
use std::{sync::mpsc, time::Duration};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
async fn postgres_executor_runs_bounded_service_sessions() -> Result<(), Box<dyn std::error::Error>>
{
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let executor = BlockingStoreExecutor::postgres_no_tls(
        url,
        2,
        Duration::from_millis(100),
        Duration::from_secs(1),
    )
    .await?;

    let schema_version = executor
        .run_service(|service| service.store().schema_version())
        .await?;
    assert_eq!(schema_version, 2);

    let (started_one_tx, started_one_rx) = tokio::sync::oneshot::channel();
    let (started_two_tx, started_two_rx) = tokio::sync::oneshot::channel();
    let (release_one_tx, release_one_rx) = mpsc::channel();
    let (release_two_tx, release_two_rx) = mpsc::channel();

    let first = executor.clone();
    let first_task = tokio::spawn(async move {
        first
            .run(move |_| {
                let _ = started_one_tx.send(());
                release_one_rx.recv().map_err(|error| {
                    aidememo_domain::DomainError::StorageFailure {
                        operation: "postgres_executor_test_release",
                        detail: error.to_string(),
                    }
                })?;
                Ok(())
            })
            .await
    });
    let second = executor.clone();
    let second_task = tokio::spawn(async move {
        second
            .run(move |_| {
                let _ = started_two_tx.send(());
                release_two_rx.recv().map_err(|error| {
                    aidememo_domain::DomainError::StorageFailure {
                        operation: "postgres_executor_test_release",
                        detail: error.to_string(),
                    }
                })?;
                Ok(())
            })
            .await
    });

    started_one_rx.await?;
    started_two_rx.await?;
    let saturated = executor
        .run_service(|service| service.store().schema_version())
        .await;
    assert!(matches!(saturated, Err(BlockingStoreError::Saturated)));

    release_one_tx.send(())?;
    release_two_tx.send(())?;
    first_task.await??;
    second_task.await??;

    let schema_version = executor
        .run_service(|service| service.store().schema_version())
        .await?;
    assert_eq!(schema_version, 2);
    Ok(())
}
