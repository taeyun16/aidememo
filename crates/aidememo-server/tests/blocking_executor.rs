#[path = "../src/executor.rs"]
mod executor;

use aidememo_store_local::SqliteCommandStore;
use executor::{BlockingStoreError, BlockingStoreExecutor};
use std::{sync::mpsc, time::Duration};

#[tokio::test]
async fn sqlite_executor_runs_borrowed_server_store() -> Result<(), Box<dyn std::error::Error>> {
    let store = SqliteCommandStore::open_in_memory()?;
    let executor =
        BlockingStoreExecutor::sqlite(store, Duration::from_secs(1), Duration::from_secs(1));

    let schema_version = executor.run(|store| store.schema_version()).await?;
    assert_eq!(schema_version, 4);
    Ok(())
}

#[tokio::test]
async fn timed_out_blocking_task_retains_capacity_until_it_exits()
-> Result<(), Box<dyn std::error::Error>> {
    let store = SqliteCommandStore::open_in_memory()?;
    let executor = BlockingStoreExecutor::sqlite(
        store,
        Duration::from_millis(100),
        Duration::from_millis(40),
    );
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let first = executor.clone();
    let first_task = tokio::spawn(async move {
        first
            .run(move |_| {
                let _ = started_tx.send(());
                release_rx.recv().map_err(|error| {
                    aidememo_domain::DomainError::StorageFailure {
                        operation: "blocking_executor_test_release",
                        detail: error.to_string(),
                    }
                })?;
                Ok(())
            })
            .await
    });

    started_rx.await?;
    let first_result = first_task.await?;
    assert!(matches!(first_result, Err(BlockingStoreError::TimedOut)));

    let second_result = executor.run(|store| store.schema_version()).await;
    assert!(matches!(second_result, Err(BlockingStoreError::Saturated)));

    release_tx.send(())?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let schema_version = executor.run(|store| store.schema_version()).await?;
    assert_eq!(schema_version, 4);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_store_work_does_not_stall_current_thread_runtime()
-> Result<(), Box<dyn std::error::Error>> {
    let store = SqliteCommandStore::open_in_memory()?;
    let executor =
        BlockingStoreExecutor::sqlite(store, Duration::from_secs(1), Duration::from_secs(1));
    let (release_tx, release_rx) = mpsc::channel();
    let release_thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let _ = release_tx.send(());
    });

    let operation = executor.run(move |_| {
        release_rx
            .recv()
            .map_err(|error| aidememo_domain::DomainError::StorageFailure {
                operation: "blocking_executor_test_release",
                detail: error.to_string(),
            })?;
        Ok(())
    });
    tokio::pin!(operation);

    tokio::select! {
        result = &mut operation => {
            return Err(format!("blocking store operation completed before runtime timer: {result:?}").into());
        }
        () = tokio::time::sleep(Duration::from_millis(30)) => {}
    }

    release_thread
        .join()
        .map_err(|_| "release thread panicked")?;
    Ok(())
}
