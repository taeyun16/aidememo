from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


lib_path = Path("crates/aidememo-server/src/lib.rs")
lib = lib_path.read_text()
lib = replace_once(
    lib,
    '''    let Json(request) = payload
        .map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
    validate_resource_command(&request)?;
    let digest = bearer_digest_from_headers(&headers)?;''',
    '''    let Json(request) = payload
        .map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
    let digest = bearer_digest_from_headers(&headers)?;''',
    "preserve authentication precedence before command validation",
)
lib = replace_once(
    lib,
    '''            let membership = service
                .store()
                .membership(&authenticated, &request.project_id)?
                .ok_or_else(|| DomainError::ProjectUnauthorized {
                    project_id: request.project_id.clone(),
                })?;
            let envelope = CommandEnvelope {''',
    '''            let membership = service
                .store()
                .membership(&authenticated, &request.project_id)?
                .ok_or_else(|| DomainError::ProjectUnauthorized {
                    project_id: request.project_id.clone(),
                })?;
            validate_resource_command(&request)?;
            let envelope = CommandEnvelope {''',
    "validate command only after authenticated membership",
)
lib_path.write_text(lib)

executor_path = Path("crates/aidememo-server/src/executor.rs")
executor = executor_path.read_text()
executor = replace_once(
    executor,
    '''    /// The configured execution policy is invalid.
    Configuration(String),''',
    '''    /// The configured execution policy is invalid.
    // Constructed by the PostgreSQL backend constructor, which is intentionally
    // wired into the server CLI in the next backend-selection slice.
    #[allow(dead_code)]
    Configuration(String),''',
    "allow deferred postgres configuration variant",
)
executor = replace_once(
    executor,
    '''    Sqlite(Arc<Mutex<SqliteCommandStore>>),
    Postgres(Arc<PostgresPool>),''',
    '''    Sqlite(Arc<Mutex<SqliteCommandStore>>),
    // The PostgreSQL executor is validated by its dedicated integration test;
    // production CLI selection is deliberately deferred to the next slice.
    #[allow(dead_code)]
    Postgres(Arc<PostgresPool>),''',
    "allow deferred postgres backend variant",
)
executor = replace_once(
    executor,
    '''impl PostgresDropReaper {
    fn new() -> Result<Self, BlockingStoreError> {''',
    '''impl PostgresDropReaper {
    #[allow(dead_code)]
    fn new() -> Result<Self, BlockingStoreError> {''',
    "allow deferred postgres drop reaper constructor",
)
executor = replace_once(
    executor,
    '''impl PooledPostgresStore {
    fn new(store: PostgresCommandStore, reaper: PostgresDropReaper) -> Self {''',
    '''impl PooledPostgresStore {
    #[allow(dead_code)]
    fn new(store: PostgresCommandStore, reaper: PostgresDropReaper) -> Self {''',
    "allow deferred pooled postgres constructor",
)
executor = replace_once(
    executor,
    '''    pub(crate) async fn postgres_no_tls(
        url: String,''',
    '''    #[allow(dead_code)]
    pub(crate) async fn postgres_no_tls(
        url: String,''',
    "allow deferred postgres executor constructor",
)
executor_path.write_text(executor)

print("HTTP executor follow-up fixes applied")
