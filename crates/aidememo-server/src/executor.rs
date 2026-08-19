use aidememo_domain::{DomainError, ServerCanonicalStore};
use aidememo_service::CommandService;
use aidememo_store_local::SqliteCommandStore;
use aidememo_store_postgres::PostgresCommandStore;
use std::{
    error::Error,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, mpsc as std_mpsc},
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore, mpsc};

/// Failure at the async-to-blocking storage execution boundary.
#[derive(Debug)]
pub(crate) enum BlockingStoreError {
    /// Portable domain/storage behavior rejected the operation.
    Domain(DomainError),
    /// No bounded execution permit became available before the queue deadline.
    Saturated,
    /// The caller stopped waiting before the blocking operation completed.
    TimedOut,
    /// The backend synchronization primitive or connection pool became unusable.
    BackendUnavailable,
    /// The configured execution policy is invalid.
    Configuration(String),
    /// The Tokio blocking task terminated unexpectedly or handler code panicked.
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
            Self::Configuration(detail) => {
                write!(formatter, "invalid canonical store executor configuration: {detail}")
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

#[derive(Clone)]
enum BlockingBackend {
    Sqlite(Arc<Mutex<SqliteCommandStore>>),
    Postgres(Arc<PostgresPool>),
}

/// Dedicated ordinary thread that owns destruction of synchronous PostgreSQL clients.
///
/// The synchronous `postgres` crate drives its own Tokio runtime internally even
/// during `Client::drop`. Destroying one on an Axum/Tokio runtime thread can
/// therefore panic with a nested-runtime error. Every pooled store carries a
/// reaper handle so even channel/pool teardown on an async thread transfers the
/// actual client destruction to this ordinary thread.
#[derive(Clone)]
struct PostgresDropReaper {
    sender: std_mpsc::Sender<PostgresCommandStore>,
}

impl PostgresDropReaper {
    fn new() -> Result<Self, BlockingStoreError> {
        let (sender, receiver) = std_mpsc::channel::<PostgresCommandStore>();
        std::thread::Builder::new()
            .name("aidememo-postgres-drop".to_owned())
            .spawn(move || {
                while let Ok(store) = receiver.recv() {
                    let _ = catch_unwind(AssertUnwindSafe(|| drop(store)));
                }
            })
            .map_err(|error| {
                BlockingStoreError::Configuration(format!(
                    "failed to start PostgreSQL drop reaper: {error}"
                ))
            })?;
        Ok(Self { sender })
    }

    fn reap(&self, store: PostgresCommandStore) {
        if let Err(error) = self.sender.send(store) {
            // The reaper is designed to outlive every sender. If it somehow
            // terminated unexpectedly, leaking the client is safer than
            // synchronously dropping it on a Tokio worker and aborting the process.
            std::mem::forget(error.0);
        }
    }
}

struct PooledPostgresStore {
    store: Option<PostgresCommandStore>,
    reaper: PostgresDropReaper,
}

impl PooledPostgresStore {
    fn new(store: PostgresCommandStore, reaper: PostgresDropReaper) -> Self {
        Self {
            store: Some(store),
            reaper,
        }
    }

    fn store_mut(&mut self) -> Result<&mut PostgresCommandStore, BlockingStoreError> {
        self.store
            .as_mut()
            .ok_or(BlockingStoreError::BackendUnavailable)
    }
}

impl Drop for PooledPostgresStore {
    fn drop(&mut self) {
        if let Some(store) = self.store.take() {
            self.reaper.reap(store);
        }
    }
}

struct PostgresPool {
    sender: mpsc::Sender<PooledPostgresStore>,
    receiver: AsyncMutex<mpsc::Receiver<PooledPostgresStore>>,
}

impl PostgresPool {
    async fn take(&self, timeout: Duration) -> Result<PooledPostgresStore, BlockingStoreError> {
        tokio::time::timeout(timeout, async {
            let mut receiver = self.receiver.lock().await;
            receiver.recv().await
        })
        .await
        .map_err(|_| BlockingStoreError::BackendUnavailable)?
        .ok_or(BlockingStoreError::BackendUnavailable)
    }

    fn put(&self, store: PooledPostgresStore) -> Result<(), BlockingStoreError> {
        self.sender
            .try_send(store)
            .map_err(|_| BlockingStoreError::BackendUnavailable)
    }
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
    backend: BlockingBackend,
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
            backend: BlockingBackend::Sqlite(Arc::new(Mutex::new(store))),
            permits: Arc::new(Semaphore::new(1)),
            acquire_timeout,
            operation_timeout,
        }
    }

    /// Build a bounded local/development PostgreSQL pool without TLS.
    ///
    /// Production server wiring must use a TLS-capable constructor rather than
    /// silently selecting this path. Connection creation itself runs on Tokio's
    /// blocking pool so startup does not block an async runtime worker.
    pub(crate) async fn postgres_no_tls(
        url: String,
        pool_size: usize,
        acquire_timeout: Duration,
        operation_timeout: Duration,
    ) -> Result<Self, BlockingStoreError> {
        if pool_size == 0 {
            return Err(BlockingStoreError::Configuration(
                "PostgreSQL pool size must be greater than zero".to_owned(),
            ));
        }
        let reaper = PostgresDropReaper::new()?;
        let build_reaper = reaper.clone();
        let stores = tokio::task::spawn_blocking(move || {
            (0..pool_size)
                .map(|_| {
                    PostgresCommandStore::connect_no_tls(&url)
                        .map(|store| PooledPostgresStore::new(store, build_reaper.clone()))
                })
                .collect::<Result<Vec<_>, DomainError>>()
        })
        .await
        .map_err(|error| BlockingStoreError::Join(error.to_string()))?
        .map_err(BlockingStoreError::Domain)?;
        let (sender, receiver) = mpsc::channel(pool_size);
        for store in stores {
            sender
                .try_send(store)
                .map_err(|_| BlockingStoreError::BackendUnavailable)?;
        }
        drop(reaper);
        Ok(Self {
            backend: BlockingBackend::Postgres(Arc::new(PostgresPool {
                sender,
                receiver: AsyncMutex::new(receiver),
            })),
            permits: Arc::new(Semaphore::new(pool_size)),
            acquire_timeout,
            operation_timeout,
        })
    }

    /// Execute one synchronous canonical-store session away from Tokio workers.
    ///
    /// The closure may perform several reads and one mutation on the same leased
    /// store. A timed-out caller stops waiting, but the blocking task keeps its
    /// permit and any PostgreSQL connection until the closure actually exits.
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

        let task = match &self.backend {
            BlockingBackend::Sqlite(store) => {
                let store = Arc::clone(store);
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let mut store = store
                        .lock()
                        .map_err(|_| BlockingStoreError::BackendUnavailable)?;
                    run_operation(&mut *store, operation)
                })
            }
            BlockingBackend::Postgres(pool) => {
                let pool = Arc::clone(pool);
                let mut store = pool.take(self.acquire_timeout).await?;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let result = run_operation(store.store_mut()?, operation);
                    pool.put(store)?;
                    result
                })
            }
        };

        match tokio::time::timeout(self.operation_timeout, task).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(BlockingStoreError::Join(error.to_string())),
            Err(_) => Err(BlockingStoreError::TimedOut),
        }
    }

    /// Execute one existing [`CommandService`] orchestration against a leased store.
    ///
    /// The service borrows only for the lifetime of the blocking closure, so the
    /// same handler implementation can operate on SQLite or pooled PostgreSQL
    /// without owning or naming the concrete adapter.
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

fn run_operation<R, F>(
    store: &mut dyn ServerCanonicalStore,
    operation: F,
) -> Result<R, BlockingStoreError>
where
    F: FnOnce(&mut dyn ServerCanonicalStore) -> Result<R, DomainError>,
{
    catch_unwind(AssertUnwindSafe(|| operation(store)))
        .map_err(|_| BlockingStoreError::Join("canonical store operation panicked".to_owned()))?
        .map_err(BlockingStoreError::Domain)
}
