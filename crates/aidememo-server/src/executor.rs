use aidememo_domain::{DomainError, ServerCanonicalStore};
use aidememo_service::CommandService;
use aidememo_store_local::SqliteCommandStore;
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;

/// Failure at the async-to-blocking storage execution boundary.
#[derive(Debug)]
pub(crate) enum BlockingStoreError {
    /// Portable domain/storage behavior rejected the operation.
    Domain(DomainError),
    /// No bounded execution permit became available before the queue deadline.
    Saturated,
    /// The caller stopped waiting before the blocking operation completed.
    TimedOut,
    /// The backend synchronization primitive became unusable.
    BackendUnavailable,
    /// The Tokio blocking task terminated unexpectedly.
    Join(String),
}

impl fmt::Display for BlockingStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::Saturated => formatter.write_str("canonical store executor is saturated"),
            Self::TimedOut => formatter.write_str("canonical store operation timed out"),
            Self::BackendUnavailable => {
                formatter.write_str("canonical store backend is unavailable")
            }
            Self::Join(detail) => {
                write!(formatter, "canonical store blocking task failed: {detail}")
            }
        }
    }
}

impl Error for BlockingStoreError {}

impl From<DomainError> for BlockingStoreError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

enum BlockingBackend {
    Sqlite(Mutex<SqliteCommandStore>),
}

/// Bounded bridge from async HTTP code to synchronous canonical stores.
///
/// A permit is acquired before `spawn_blocking` and moved into the blocking
/// closure. If the async caller times out, the blocking task remains detached
/// but retains that permit until the synchronous database work actually exits.
/// This prevents request cancellation from creating an unbounded blocking-task
/// backlog.
#[derive(Clone)]
pub(crate) struct BlockingStoreExecutor {
    backend: Arc<BlockingBackend>,
    permits: Arc<Semaphore>,
    acquire_timeout: Duration,
    operation_timeout: Duration,
}

impl BlockingStoreExecutor {
    /// Wrap the single-node SQLite canonical store with one execution permit.
    #[must_use]
    pub(crate) fn sqlite(
        store: SqliteCommandStore,
        acquire_timeout: Duration,
        operation_timeout: Duration,
    ) -> Self {
        Self {
            backend: Arc::new(BlockingBackend::Sqlite(Mutex::new(store))),
            permits: Arc::new(Semaphore::new(1)),
            acquire_timeout,
            operation_timeout,
        }
    }

    /// Execute one synchronous canonical-store session away from Tokio workers.
    ///
    /// The closure may perform several reads and one mutation on the same leased
    /// store. PostgreSQL pooling can therefore reuse this boundary later without
    /// duplicating transport/domain orchestration.
    pub(crate) async fn run<R, F>(&self, operation: F) -> Result<R, BlockingStoreError>
    where
        R: Send + 'static,
        F: FnOnce(&mut dyn ServerCanonicalStore) -> Result<R, DomainError> + Send + 'static,
    {
        let permit = tokio::time::timeout(
            self.acquire_timeout,
            Arc::clone(&self.permits).acquire_owned(),
        )
        .await
        .map_err(|_| BlockingStoreError::Saturated)?
        .map_err(|_| BlockingStoreError::BackendUnavailable)?;
        let backend = Arc::clone(&self.backend);
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match backend.as_ref() {
                BlockingBackend::Sqlite(store) => {
                    let mut store = store
                        .lock()
                        .map_err(|_| BlockingStoreError::BackendUnavailable)?;
                    operation(&mut *store).map_err(BlockingStoreError::Domain)
                }
            }
        });
        match tokio::time::timeout(self.operation_timeout, task).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(BlockingStoreError::Join(error.to_string())),
            Err(_) => Err(BlockingStoreError::TimedOut),
        }
    }

    /// Execute one existing [`CommandService`] orchestration against a leased store.
    ///
    /// The service borrows only for the lifetime of the blocking closure, so the
    /// same handler implementation can operate on SQLite today and a pooled
    /// PostgreSQL store later without owning or naming the concrete adapter.
    pub(crate) async fn run_service<R, F>(&self, operation: F) -> Result<R, BlockingStoreError>
    where
        R: Send + 'static,
        F: for<'store> FnOnce(
                &mut CommandService<&'store mut dyn ServerCanonicalStore>,
            ) -> Result<R, DomainError>
            + Send
            + 'static,
    {
        self.run(move |store| {
            let mut service = CommandService::new(store);
            operation(&mut service)
        })
        .await
    }
}
