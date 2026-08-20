from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def function_span(text: str, marker: str) -> tuple[int, int]:
    start = text.find(marker)
    if start < 0:
        raise SystemExit(f"function marker not found: {marker}")
    brace = text.find("{", start)
    if brace < 0:
        raise SystemExit(f"opening brace not found: {marker}")
    depth = 0
    i = brace
    state = "normal"
    block_depth = 0
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if state == "line":
            if ch == "\n": state = "normal"
            i += 1; continue
        if state == "block":
            if ch == "/" and nxt == "*": block_depth += 1; i += 2; continue
            if ch == "*" and nxt == "/":
                block_depth -= 1; i += 2
                if block_depth == 0: state = "normal"
                continue
            i += 1; continue
        if state == "string":
            if ch == "\\": i += 2; continue
            if ch == '"': state = "normal"
            i += 1; continue
        if state == "char":
            if ch == "\\": i += 2; continue
            if ch == "'": state = "normal"
            i += 1; continue
        if ch == "/" and nxt == "/": state = "line"; i += 2; continue
        if ch == "/" and nxt == "*": state = "block"; block_depth = 1; i += 2; continue
        if ch == '"': state = "string"; i += 1; continue
        if ch == "'": state = "char"; i += 1; continue
        if ch == "{": depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0: return start, i + 1
        i += 1
    raise SystemExit(f"closing brace not found: {marker}")


def replace_function(text: str, marker: str, replacement: str) -> str:
    start, end = function_span(text, marker)
    return text[:start] + replacement.strip() + text[end:]


def block_span(text: str, marker: str) -> tuple[int, int]:
    return function_span(text, marker)


# ---------------------------------------------------------------------------
# PostgreSQL adapter: bounded session timeout configuration before migration.
# ---------------------------------------------------------------------------
pg_path = Path("crates/aidememo-store-postgres/src/lib.rs")
pg = pg_path.read_text()
pg = replace_once(
    pg,
    "//! It is intentionally not wired directly into Axum request handling yet: the\n//! production server boundary must place blocking database work behind a pool /\n//! blocking-executor boundary before enabling this adapter for HTTP traffic.",
    "//! Server HTTP use is mediated by the bounded blocking executor in\n//! `aidememo-server`; this adapter remains synchronous and transport-agnostic.",
    "postgres module docs",
)
pg = replace_once(
    pg,
    "    sync::{Mutex, MutexGuard},\n    time::{SystemTime, UNIX_EPOCH},",
    "    sync::{Mutex, MutexGuard},\n    time::{Duration, SystemTime, UNIX_EPOCH},",
    "postgres duration import",
)
old_ctor = '''    pub fn connect_no_tls(url: &str) -> Result<Self, DomainError> {
        let client = Client::connect(url, NoTls).map_err(|error| storage("connect", error))?;
        let store = Self {
            client: Mutex::new(client),
        };
        store.migrate()?;
        Ok(store)
    }
'''
new_ctor = '''    pub fn connect_no_tls(url: &str) -> Result<Self, DomainError> {
        let client = Client::connect(url, NoTls).map_err(|error| storage("connect", error))?;
        let store = Self {
            client: Mutex::new(client),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Connect without TLS, configure finite server-side timeouts, and migrate.
    ///
    /// This is the explicit local/development transport used by the server's
    /// `insecure-no-tls` profile. Production callers must use a TLS-capable path.
    /// Applying the settings before migration bounds advisory-lock and DDL waits
    /// as well as later request statements on this connection.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when the timeout is zero/out of PostgreSQL's
    /// millisecond range or when connection, timeout configuration, or migration
    /// fails.
    pub fn connect_no_tls_with_timeouts(
        url: &str,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, DomainError> {
        let mut client = Client::connect(url, NoTls).map_err(|error| storage("connect", error))?;
        configure_timeout(&mut client, "statement_timeout", statement_timeout)?;
        configure_timeout(&mut client, "lock_timeout", lock_timeout)?;
        let store = Self {
            client: Mutex::new(client),
        };
        store.migrate()?;
        Ok(store)
    }
'''
pg = replace_once(pg, old_ctor, new_ctor, "postgres timeout constructor")
insert_marker = "impl CommandStore for PostgresCommandStore {"
idx = pg.find(insert_marker)
if idx < 0: raise SystemExit("postgres command store impl marker missing")
helper = '''fn configure_timeout(
    client: &mut Client,
    setting: &'static str,
    timeout: Duration,
) -> Result<(), DomainError> {
    let millis = i32::try_from(timeout.as_millis()).map_err(|_| DomainError::StorageFailure {
        operation: "postgres_timeout_config",
        detail: format!("{setting} exceeds PostgreSQL millisecond range"),
    })?;
    if millis <= 0 {
        return Err(DomainError::StorageFailure {
            operation: "postgres_timeout_config",
            detail: format!("{setting} must be greater than zero"),
        });
    }
    let setting_name = setting.to_owned();
    let value = format!("{millis}ms");
    client
        .query_one(
            "SELECT set_config($1, $2, false)",
            &[&setting_name, &value],
        )
        .map_err(|error| storage("postgres_timeout_config", error))?;
    Ok(())
}

'''
pg = pg[:idx] + helper + pg[idx:]
pg_path.write_text(pg)


# ---------------------------------------------------------------------------
# Executor: make PostgreSQL path live and pass DB-side timeout policy.
# ---------------------------------------------------------------------------
ex_path = Path("crates/aidememo-server/src/executor.rs")
ex = ex_path.read_text()
ex = ex.replace(
    '''    /// The configured execution policy is invalid.
    // Constructed by the PostgreSQL backend constructor, which is intentionally
    // wired into the server CLI in the next backend-selection slice.
    #[allow(dead_code)]
    Configuration(String),''',
    '''    /// The configured execution policy is invalid.
    Configuration(String),''',
)
ex = ex.replace(
    '''    Sqlite(Arc<Mutex<SqliteCommandStore>>),
    // The PostgreSQL executor is validated by its dedicated integration test;
    // production CLI selection is deliberately deferred to the next slice.
    #[allow(dead_code)]
    Postgres(Arc<PostgresPool>),''',
    '''    Sqlite(Arc<Mutex<SqliteCommandStore>>),
    Postgres(Arc<PostgresPool>),''',
)
ex = ex.replace("    #[allow(dead_code)]\n    fn new() -> Result<Self, BlockingStoreError> {", "    fn new() -> Result<Self, BlockingStoreError> {", 1)
ex = ex.replace("    #[allow(dead_code)]\n    fn new(store: PostgresCommandStore, reaper: PostgresDropReaper) -> Self {", "    fn new(store: PostgresCommandStore, reaper: PostgresDropReaper) -> Self {", 1)
ex = ex.replace("    #[allow(dead_code)]\n    pub(crate) async fn postgres_no_tls(", "    pub(crate) async fn postgres_no_tls(", 1)
ex = replace_once(
    ex,
    '''        acquire_timeout: Duration,
        operation_timeout: Duration,
    ) -> Result<Self, BlockingStoreError> {''',
    '''        acquire_timeout: Duration,
        operation_timeout: Duration,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, BlockingStoreError> {''',
    "executor postgres timeout signature",
)
ex = replace_once(
    ex,
    '''        if pool_size == 0 {
            return Err(BlockingStoreError::Configuration(
                "PostgreSQL pool size must be greater than zero".to_owned(),
            ));
        }
        let reaper = PostgresDropReaper::new()?;''',
    '''        if pool_size == 0 {
            return Err(BlockingStoreError::Configuration(
                "PostgreSQL pool size must be greater than zero".to_owned(),
            ));
        }
        if acquire_timeout.is_zero()
            || operation_timeout.is_zero()
            || statement_timeout.is_zero()
            || lock_timeout.is_zero()
        {
            return Err(BlockingStoreError::Configuration(
                "PostgreSQL acquire/operation/statement/lock timeouts must be greater than zero"
                    .to_owned(),
            ));
        }
        if statement_timeout >= operation_timeout {
            return Err(BlockingStoreError::Configuration(
                "PostgreSQL statement timeout must be shorter than the outer operation timeout"
                    .to_owned(),
            ));
        }
        if lock_timeout > statement_timeout {
            return Err(BlockingStoreError::Configuration(
                "PostgreSQL lock timeout must not exceed statement timeout".to_owned(),
            ));
        }
        let reaper = PostgresDropReaper::new()?;''',
    "executor postgres policy validation",
)
ex = replace_once(
    ex,
    '''                    PostgresCommandStore::connect_no_tls(&url)
                        .map(|store| PooledPostgresStore::new(store, build_reaper.clone()))''',
    '''                    PostgresCommandStore::connect_no_tls_with_timeouts(
                        &url,
                        statement_timeout,
                        lock_timeout,
                    )
                    .map(|store| PooledPostgresStore::new(store, build_reaper.clone()))''',
    "executor timed postgres connect",
)
ex_path.write_text(ex)


# ---------------------------------------------------------------------------
# Server state: public development-only PostgreSQL constructor, no artifacts.
# ---------------------------------------------------------------------------
lib_path = Path("crates/aidememo-server/src/lib.rs")
lib = lib_path.read_text()
lib = replace_once(
    lib,
    "//! active membership is loaded from the same SQLite ledger before every read or\n//! mutation. This crate does not modify or expose the existing embedded store.",
    "//! active membership is loaded from the selected canonical ledger before every\n//! read or mutation. This crate does not modify or expose the existing embedded store.",
    "server module docs backend-neutral",
)
marker = "    /// Wrap the command ledger together with an isolated local artifact repository."
idx = lib.find(marker)
if idx < 0: raise SystemExit("ServerState insertion marker missing")
pg_state_ctor = '''    /// Build an artifact-disabled PostgreSQL server state without TLS.
    ///
    /// This constructor exists only for explicit local/development profiles. The
    /// server CLI fails closed to TLS-required mode unless the operator selects
    /// `insecure-no-tls`; production PostgreSQL transport is a separate TLS slice.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when pool construction, timeout policy,
    /// connection, or schema initialization fails.
    pub async fn postgres_no_tls_for_development(
        url: String,
        pool_size: usize,
        acquire_timeout: Duration,
        operation_timeout: Duration,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, DomainError> {
        let canonical = BlockingStoreExecutor::postgres_no_tls(
            url,
            pool_size,
            acquire_timeout,
            operation_timeout,
            statement_timeout,
            lock_timeout,
        )
        .await
        .map_err(blocking_store_initialization_error)?;
        Ok(Self {
            canonical,
            artifacts: None,
            #[cfg(feature = "semantic")]
            semantic_provider: None,
            #[cfg(feature = "semantic")]
            semantic_projection: Arc::new(Mutex::new(None)),
        })
    }

'''
lib = lib[:idx] + pg_state_ctor + lib[idx:]
# Insert startup error normalization before router.
router_marker = "/// Build the authenticated server router."
idx = lib.find(router_marker)
if idx < 0: raise SystemExit("router marker missing")
startup_helper = '''fn blocking_store_initialization_error(error: BlockingStoreError) -> DomainError {
    match error {
        BlockingStoreError::Domain(error) => error,
        error => DomainError::StorageFailure {
            operation: "server_canonical_backend_init",
            detail: error.to_string(),
        },
    }
}

'''
lib = lib[:idx] + startup_helper + lib[idx:]
lib_path.write_text(lib)


# ---------------------------------------------------------------------------
# Server binary: backend selection, env-secret URL, explicit artifact-disabled PG.
# ---------------------------------------------------------------------------
main_path = Path("crates/aidememo-server/src/main.rs")
main = main_path.read_text()
main = replace_once(
    main,
    '''    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, ProjectScope, RecordStatus, Revision, TenantId, TenantRecord,
};''',
    '''    ActorId, ActorKind, ActorRecord, DomainError, MembershipRole, MembershipStatus,
    ProjectEpoch, ProjectId, ProjectMembership, ProjectRecord, ProjectScope, RecordStatus, Revision,
    ServerCanonicalStore, TenantId, TenantRecord,
};''',
    "main domain imports",
)
main = replace_once(
    main,
    "use aidememo_store_local::SqliteCommandStore;",
    "use aidememo_store_local::SqliteCommandStore;\nuse aidememo_store_postgres::PostgresCommandStore;",
    "main postgres import",
)
main = replace_once(
    main,
    "    time::{SystemTime, UNIX_EPOCH},",
    "    time::{Duration, SystemTime, UNIX_EPOCH},",
    "main duration import",
)
# BootstrapArgs block.
start = main.find("#[derive(clap::Args)]\nstruct BootstrapArgs")
end = main.find("#[derive(clap::Args)]\nstruct ServeArgs", start)
if start < 0 or end < 0: raise SystemExit("bootstrap args block missing")
bootstrap_args = '''#[derive(clap::Args)]
struct BootstrapArgs {
    /// Canonical SSOT backend. SQLite remains the default profile.
    #[arg(long, value_enum, default_value_t = CanonicalBackendArg::Sqlite)]
    canonical_backend: CanonicalBackendArg,
    /// Separate server ledger SQLite path. Required for the SQLite backend.
    #[arg(long)]
    database: Option<PathBuf>,
    /// Environment variable containing the PostgreSQL URL. Plaintext URL is never accepted on CLI.
    #[arg(long, default_value = "AIDEMEMO_POSTGRES_URL")]
    postgres_url_env: String,
    /// PostgreSQL transport policy. TLS-required fails closed until the TLS connector slice lands.
    #[arg(long, value_enum, default_value_t = PostgresTransportArg::RequireTls)]
    postgres_transport: PostgresTransportArg,
    /// PostgreSQL statement timeout in milliseconds.
    #[arg(long, default_value_t = 8_000)]
    postgres_statement_timeout_ms: u64,
    /// PostgreSQL lock timeout in milliseconds.
    #[arg(long, default_value_t = 2_000)]
    postgres_lock_timeout_ms: u64,
    #[arg(long)]
    tenant_id: String,
    #[arg(long)]
    project_id: String,
    #[arg(long)]
    actor_id: String,
    /// Read bearer-token plaintext from a regular file. On Unix it must be 0600.
    #[arg(long)]
    token_file: PathBuf,
    #[arg(long)]
    tenant_name: Option<String>,
    #[arg(long)]
    project_name: Option<String>,
    #[arg(long)]
    actor_name: Option<String>,
    /// Existing epoch when restoring; otherwise a new ULID is generated.
    #[arg(long)]
    project_epoch: Option<String>,
    #[arg(long, value_enum, default_value_t = RoleArg::Writer)]
    role: RoleArg,
    #[arg(long, value_enum, default_value_t = ActorKindArg::Agent)]
    actor_kind: ActorKindArg,
}

'''
main = main[:start] + bootstrap_args + main[end:]
# ServeArgs block.
start = main.find("#[derive(clap::Args)]\nstruct ServeArgs")
end = main.find("#[derive(Clone, Copy, ValueEnum)]\nenum RoleArg", start)
if start < 0 or end < 0: raise SystemExit("serve args block missing")
serve_args = '''#[derive(clap::Args)]
struct ServeArgs {
    /// Canonical SSOT backend. SQLite remains the default profile.
    #[arg(long, value_enum, default_value_t = CanonicalBackendArg::Sqlite)]
    canonical_backend: CanonicalBackendArg,
    /// Separate server ledger SQLite path. Required for the SQLite backend.
    #[arg(long)]
    database: Option<PathBuf>,
    /// Environment variable containing the PostgreSQL URL. Plaintext URL is never accepted on CLI.
    #[arg(long, default_value = "AIDEMEMO_POSTGRES_URL")]
    postgres_url_env: String,
    /// PostgreSQL transport policy. Use insecure-no-tls only for explicit local/test environments.
    #[arg(long, value_enum, default_value_t = PostgresTransportArg::RequireTls)]
    postgres_transport: PostgresTransportArg,
    /// Maximum live PostgreSQL connections and blocking canonical sessions.
    #[arg(long, default_value_t = 8)]
    postgres_pool_size: usize,
    /// Maximum wait to acquire bounded PostgreSQL execution capacity.
    #[arg(long, default_value_t = 500)]
    postgres_acquire_timeout_ms: u64,
    /// Outer maximum wait for one synchronous canonical session.
    #[arg(long, default_value_t = 10_000)]
    postgres_operation_timeout_ms: u64,
    /// PostgreSQL statement timeout; must be shorter than the outer operation timeout.
    #[arg(long, default_value_t = 8_000)]
    postgres_statement_timeout_ms: u64,
    /// PostgreSQL lock timeout; must not exceed statement timeout.
    #[arg(long, default_value_t = 2_000)]
    postgres_lock_timeout_ms: u64,
    /// Separate local artifact catalog and immutable-object directory.
    /// Defaults to `<database>.artifacts` for SQLite artifact-enabled profiles.
    #[arg(long)]
    artifact_root: Option<PathBuf>,
    /// Artifact API mode. PostgreSQL canonical storage currently requires disabled.
    #[arg(long, value_enum, default_value_t = ArtifactBackendArg::Local)]
    artifact_backend: ArtifactBackendArg,
    /// S3-compatible bucket. Required when `--artifact-backend s3` is selected.
    #[arg(long)]
    artifact_s3_bucket: Option<String>,
    /// Adapter-owned S3 key prefix.
    #[arg(long, default_value = "aidememo/v1")]
    artifact_s3_prefix: String,
    /// S3 signing region (`auto` for Cloudflare R2).
    #[arg(long, default_value = "auto")]
    artifact_s3_region: String,
    /// Optional S3-compatible endpoint, such as an R2 account endpoint.
    #[arg(long)]
    artifact_s3_endpoint: Option<String>,
    /// Force path-style bucket addressing for providers such as local MinIO.
    #[arg(long)]
    artifact_s3_force_path_style: bool,
    /// OpenAI-compatible embedding endpoint used by semantic/hybrid retrieval.
    #[arg(long)]
    embedding_endpoint: Option<String>,
    /// Embedding model sent to the configured endpoint.
    #[arg(long, default_value = "text-embedding-3-small")]
    embedding_model: String,
    /// Exact embedding dimension expected from the configured model.
    #[arg(long, default_value_t = 0)]
    embedding_dimension: usize,
    /// Optional environment variable containing the embedding API key.
    #[arg(long)]
    embedding_api_key_env: Option<String>,
    /// HTTP bind address. Loopback is required unless explicitly overridden.
    #[arg(long, default_value = "127.0.0.1:3030")]
    bind: SocketAddr,
    /// Permit plaintext bearer authentication on a non-loopback socket.
    #[arg(long)]
    allow_insecure_http: bool,
}

'''
main = main[:start] + serve_args + main[end:]
# Insert backend enums before RoleArg.
enum_marker = "#[derive(Clone, Copy, ValueEnum)]\nenum RoleArg"
idx = main.find(enum_marker)
if idx < 0: raise SystemExit("role enum marker missing")
backend_enums = '''#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CanonicalBackendArg {
    Sqlite,
    Postgres,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PostgresTransportArg {
    RequireTls,
    InsecureNoTls,
}

'''
main = main[:idx] + backend_enums + main[idx:]
# Artifact Disabled variant.
main = replace_once(
    main,
    "enum ArtifactBackendArg {\n    Local,\n    S3,\n}",
    "enum ArtifactBackendArg {\n    Disabled,\n    Local,\n    S3,\n}",
    "artifact disabled variant",
)
# main async bootstrap call.
main = replace_once(
    main,
    "        ServerCommand::Bootstrap(args) => bootstrap(args)?,",
    "        ServerCommand::Bootstrap(args) => bootstrap(args).await?,",
    "async bootstrap dispatch",
)
# Replace bootstrap.
main = replace_function(
    main,
    "fn bootstrap(args: BootstrapArgs)",
    r'''
async fn bootstrap(args: BootstrapArgs) -> Result<(), Box<dyn Error>> {
    validate_bootstrap_args(&args)?;
    let token = read_token_file(&args.token_file)?;
    let digest = bearer_token_digest(&token)?;
    let timestamp = unix_time_ms()?;
    let tenant_id = TenantId::try_from(args.tenant_id)?;
    let project_id = ProjectId::try_from(args.project_id)?;
    let actor_id = ActorId::try_from(args.actor_id)?;
    let requested_epoch = args.project_epoch.map(ProjectEpoch::try_from).transpose()?;
    let postgres_url = if args.canonical_backend == CanonicalBackendArg::Postgres {
        Some(postgres_url_from_env(&args.postgres_url_env)?)
    } else {
        None
    };
    let backend = args.canonical_backend;
    let database = args.database;
    let statement_timeout = Duration::from_millis(args.postgres_statement_timeout_ms);
    let lock_timeout = Duration::from_millis(args.postgres_lock_timeout_ms);
    let tenant_name = args.tenant_name;
    let project_name = args.project_name;
    let actor_name = args.actor_name;
    let role = args.role.into();
    let actor_kind = args.actor_kind.into();

    let result = tokio::task::spawn_blocking(move || -> Result<BootstrapResult, DomainError> {
        match backend {
            CanonicalBackendArg::Sqlite => {
                let database = database.ok_or_else(|| DomainError::StorageFailure {
                    operation: "server_bootstrap_config",
                    detail: "SQLite bootstrap requires --database".to_owned(),
                })?;
                let mut store = SqliteCommandStore::open(database)?;
                bootstrap_store(
                    &mut store,
                    tenant_id,
                    project_id,
                    actor_id,
                    requested_epoch,
                    tenant_name,
                    project_name,
                    actor_name,
                    role,
                    actor_kind,
                    digest,
                    timestamp,
                )
            }
            CanonicalBackendArg::Postgres => {
                let url = postgres_url.ok_or_else(|| DomainError::StorageFailure {
                    operation: "server_bootstrap_config",
                    detail: "PostgreSQL URL environment variable was not resolved".to_owned(),
                })?;
                let mut store = PostgresCommandStore::connect_no_tls_with_timeouts(
                    &url,
                    statement_timeout,
                    lock_timeout,
                )?;
                bootstrap_store(
                    &mut store,
                    tenant_id,
                    project_id,
                    actor_id,
                    requested_epoch,
                    tenant_name,
                    project_name,
                    actor_name,
                    role,
                    actor_kind,
                    digest,
                    timestamp,
                )
            }
        }
    })
    .await??;

    println!(
        "bootstrapped tenant={} project={} actor={} epoch={}",
        result.tenant_id, result.project_id, result.actor_id, result.project_epoch
    );
    Ok(())
}
''',
)
# Insert bootstrap helper structures/functions before serve.
serve_idx = main.find("async fn serve(args: ServeArgs)")
if serve_idx < 0: raise SystemExit("serve marker missing")
bootstrap_helpers = r'''
struct BootstrapResult {
    tenant_id: TenantId,
    project_id: ProjectId,
    actor_id: ActorId,
    project_epoch: ProjectEpoch,
}

#[allow(clippy::too_many_arguments)]
fn bootstrap_store(
    store: &mut dyn ServerCanonicalStore,
    tenant_id: TenantId,
    project_id: ProjectId,
    actor_id: ActorId,
    requested_epoch: Option<ProjectEpoch>,
    tenant_name: Option<String>,
    project_name: Option<String>,
    actor_name: Option<String>,
    role: MembershipRole,
    actor_kind: ActorKind,
    digest: [u8; 32],
    timestamp: i64,
) -> Result<BootstrapResult, DomainError> {
    let scope = ProjectScope::new(tenant_id.clone(), project_id.clone());
    let epoch = match requested_epoch {
        Some(epoch) => epoch,
        None => match store.project_epoch(&scope)? {
            Some(epoch) => epoch,
            None => ProjectEpoch::try_from(ulid::Ulid::new().to_string())?,
        },
    };
    let tenant = TenantRecord {
        display_name: tenant_name.unwrap_or_else(|| tenant_id.as_str().to_owned()),
        tenant_id: tenant_id.clone(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant_id.clone(),
        display_name: project_name.unwrap_or_else(|| project_id.as_str().to_owned()),
        project_id: project_id.clone(),
        project_epoch: epoch.clone(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let actor = ActorRecord {
        tenant_id: tenant_id.clone(),
        display_name: actor_name.unwrap_or_else(|| actor_id.as_str().to_owned()),
        actor_id: actor_id.clone(),
        kind: actor_kind,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let membership = ProjectMembership {
        tenant_id: tenant_id.clone(),
        project_id: project_id.clone(),
        actor_id: actor_id.clone(),
        role,
        status: MembershipStatus::Active,
    };
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(&actor, &membership, &digest, timestamp)?;
    Ok(BootstrapResult {
        tenant_id,
        project_id,
        actor_id,
        project_epoch: epoch,
    })
}

'''
main = main[:serve_idx] + bootstrap_helpers + main[serve_idx:]
# Replace serve.
main = replace_function(
    main,
    "async fn serve(args: ServeArgs)",
    r'''
async fn serve(args: ServeArgs) -> Result<(), Box<dyn Error>> {
    validate_serve_args(&args)?;
    let canonical_backend = match args.canonical_backend {
        CanonicalBackendArg::Sqlite => "sqlite",
        CanonicalBackendArg::Postgres => "postgres",
    };

    let (state, artifact_root, artifact_backend) = match args.canonical_backend {
        CanonicalBackendArg::Sqlite => {
            let database = args.database.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "SQLite serve requires --database",
                )
            })?;
            let store = SqliteCommandStore::open(database)?;
            match args.artifact_backend {
                ArtifactBackendArg::Disabled => (ServerState::new(store), None, "disabled"),
                ArtifactBackendArg::Local => {
                    let artifact_root = args
                        .artifact_root
                        .clone()
                        .unwrap_or_else(|| default_artifact_root(database));
                    let artifacts = LocalArtifactStore::open(&artifact_root)?;
                    (
                        ServerState::with_artifacts(store, artifacts)?,
                        Some(artifact_root),
                        "local",
                    )
                }
                ArtifactBackendArg::S3 => {
                    #[cfg(feature = "s3")]
                    {
                        let artifact_root = args
                            .artifact_root
                            .clone()
                            .unwrap_or_else(|| default_artifact_root(database));
                        let artifacts = LocalArtifactStore::open(&artifact_root)?;
                        let bucket = args.artifact_s3_bucket.clone().ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "--artifact-s3-bucket is required for --artifact-backend s3",
                            )
                        })?;
                        let config = S3BodyStoreConfig::new(
                            bucket,
                            args.artifact_s3_prefix.clone(),
                            args.artifact_s3_region.clone(),
                            args.artifact_s3_endpoint.clone(),
                            args.artifact_s3_force_path_style,
                        )?;
                        let bodies = S3BodyStore::from_environment(config).await;
                        (
                            ServerState::with_s3_artifacts(store, artifacts, bodies)?,
                            Some(artifact_root),
                            "s3",
                        )
                    }
                    #[cfg(not(feature = "s3"))]
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Unsupported,
                            "S3 artifact bodies require an aidememo-server build with --features s3",
                        )
                        .into());
                    }
                }
            }
        }
        CanonicalBackendArg::Postgres => {
            let url = postgres_url_from_env(&args.postgres_url_env)?;
            let state = ServerState::postgres_no_tls_for_development(
                url,
                args.postgres_pool_size,
                Duration::from_millis(args.postgres_acquire_timeout_ms),
                Duration::from_millis(args.postgres_operation_timeout_ms),
                Duration::from_millis(args.postgres_statement_timeout_ms),
                Duration::from_millis(args.postgres_lock_timeout_ms),
            )
            .await?;
            (state, None, "disabled")
        }
    };

    #[cfg(feature = "semantic")]
    let state = if let Some(endpoint) = args.embedding_endpoint.as_deref() {
        if args.embedding_dimension == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "--embedding-dimension must be greater than zero when --embedding-endpoint is set",
            )
            .into());
        }
        let api_key = args
            .embedding_api_key_env
            .as_deref()
            .and_then(|name| std::env::var(name).ok());
        let provider = HttpEmbeddingProvider::new(
            endpoint,
            &args.embedding_model,
            args.embedding_dimension,
            api_key,
        )?;
        state.with_semantic_provider(std::sync::Arc::new(provider))
    } else {
        state
    };
    #[cfg(not(feature = "semantic"))]
    let state = {
        if args.embedding_endpoint.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "embedding endpoint requires an aidememo-server build with --features semantic",
            )
            .into());
        }
        state
    };

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    let artifact_root_log = artifact_root
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "disabled".to_owned());
    info!(
        address = %args.bind,
        canonical_backend,
        artifact_root = %artifact_root_log,
        artifact_backend,
        "AideMemo SSOT server listening"
    );
    let gc_state = state.clone();
    let gc_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let now_ms = match unix_time_ms() {
                Ok(now_ms) => now_ms,
                Err(error) => {
                    tracing::error!(%error, "artifact garbage collection clock failed");
                    continue;
                }
            };
            match gc_state.drain_artifact_garbage(now_ms, 100).await {
                Ok(Some(report))
                    if report.claimed > 0
                        || report.expired_reservations > 0
                        || report.pruned_receipts > 0
                        || report.pruned_read_retentions > 0 =>
                {
                    tracing::info!(
                        expired_reservations = report.expired_reservations,
                        claimed = report.claimed,
                        deleted = report.deleted,
                        failed = report.failed,
                        pruned_receipts = report.pruned_receipts,
                        pruned_read_retentions = report.pruned_read_retentions,
                        "artifact garbage collection pass completed"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(%error, "artifact garbage collection pass failed");
                }
            }
        }
    });
    let result = axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await;
    gc_task.abort();
    let _ = gc_task.await;
    result?;
    Ok(())
}
''',
)
# Replace validate_serve_args and insert bootstrap validator/helpers.
main = replace_function(
    main,
    "fn validate_serve_args(args: &ServeArgs)",
    r'''
fn validate_serve_args(args: &ServeArgs) -> Result<(), std::io::Error> {
    if args.embedding_endpoint.is_none()
        && (args.embedding_dimension != 0 || args.embedding_api_key_env.is_some())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "embedding dimension/API-key options require --embedding-endpoint",
        ));
    }
    if !args.bind.ip().is_loopback() && !args.allow_insecure_http {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "non-loopback plaintext HTTP requires --allow-insecure-http; prefer a TLS ingress",
        ));
    }

    match args.canonical_backend {
        CanonicalBackendArg::Sqlite => {
            if args.database.is_none() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "SQLite canonical backend requires --database",
                ));
            }
        }
        CanonicalBackendArg::Postgres => {
            if args.database.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--database is SQLite-only and must be omitted for PostgreSQL",
                ));
            }
            if args.artifact_backend != ArtifactBackendArg::Disabled {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "PostgreSQL canonical backend currently requires --artifact-backend disabled because artifact catalog metadata is node-local SQLite",
                ));
            }
            validate_postgres_transport(args.postgres_transport)?;
            validate_postgres_timeouts(
                args.postgres_pool_size,
                args.postgres_acquire_timeout_ms,
                args.postgres_operation_timeout_ms,
                args.postgres_statement_timeout_ms,
                args.postgres_lock_timeout_ms,
            )?;
            validate_env_name(&args.postgres_url_env)?;
        }
    }

    match args.artifact_backend {
        ArtifactBackendArg::Disabled => {
            if args.artifact_root.is_some()
                || args.artifact_s3_bucket.is_some()
                || args.artifact_s3_endpoint.is_some()
                || args.artifact_s3_force_path_style
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "artifact root/S3 options are invalid when --artifact-backend disabled",
                ));
            }
        }
        ArtifactBackendArg::Local => {
            if args.artifact_s3_bucket.is_some()
                || args.artifact_s3_endpoint.is_some()
                || args.artifact_s3_force_path_style
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "S3 artifact options require --artifact-backend s3",
                ));
            }
        }
        ArtifactBackendArg::S3 => {
            #[cfg(not(feature = "s3"))]
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "S3 artifact bodies require an aidememo-server build with --features s3",
            ));
            #[cfg(feature = "s3")]
            if args.artifact_s3_bucket.is_none() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--artifact-s3-bucket is required for --artifact-backend s3",
                ));
            }
        }
    }
    Ok(())
}
''',
)
validate_idx = main.find("fn validate_serve_args")
if validate_idx < 0: raise SystemExit("validate serve missing after replacement")
validators = r'''
fn validate_bootstrap_args(args: &BootstrapArgs) -> Result<(), std::io::Error> {
    match args.canonical_backend {
        CanonicalBackendArg::Sqlite => {
            if args.database.is_none() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "SQLite bootstrap requires --database",
                ));
            }
        }
        CanonicalBackendArg::Postgres => {
            if args.database.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--database is SQLite-only and must be omitted for PostgreSQL",
                ));
            }
            validate_postgres_transport(args.postgres_transport)?;
            validate_postgres_session_timeouts(
                args.postgres_statement_timeout_ms,
                args.postgres_lock_timeout_ms,
            )?;
            validate_env_name(&args.postgres_url_env)?;
        }
    }
    Ok(())
}

fn validate_postgres_transport(transport: PostgresTransportArg) -> Result<(), std::io::Error> {
    if transport == PostgresTransportArg::RequireTls {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "PostgreSQL TLS is required by default and is not yet implemented; local/test use must explicitly select --postgres-transport insecure-no-tls",
        ));
    }
    Ok(())
}

fn validate_postgres_timeouts(
    pool_size: usize,
    acquire_timeout_ms: u64,
    operation_timeout_ms: u64,
    statement_timeout_ms: u64,
    lock_timeout_ms: u64,
) -> Result<(), std::io::Error> {
    if pool_size == 0 || acquire_timeout_ms == 0 || operation_timeout_ms == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PostgreSQL pool size, acquire timeout, and operation timeout must be greater than zero",
        ));
    }
    validate_postgres_session_timeouts(statement_timeout_ms, lock_timeout_ms)?;
    if statement_timeout_ms >= operation_timeout_ms {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PostgreSQL statement timeout must be shorter than operation timeout",
        ));
    }
    Ok(())
}

fn validate_postgres_session_timeouts(
    statement_timeout_ms: u64,
    lock_timeout_ms: u64,
) -> Result<(), std::io::Error> {
    if statement_timeout_ms == 0 || lock_timeout_ms == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PostgreSQL statement and lock timeouts must be greater than zero",
        ));
    }
    if lock_timeout_ms > statement_timeout_ms {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PostgreSQL lock timeout must not exceed statement timeout",
        ));
    }
    if statement_timeout_ms > i32::MAX as u64 || lock_timeout_ms > i32::MAX as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PostgreSQL statement/lock timeout exceeds supported millisecond range",
        ));
    }
    Ok(())
}

fn validate_env_name(name: &str) -> Result<(), std::io::Error> {
    if name.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PostgreSQL URL environment variable name must not be empty",
        ));
    }
    Ok(())
}

fn postgres_url_from_env(name: &str) -> Result<String, std::io::Error> {
    validate_env_name(name)?;
    let value = std::env::var(name).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("PostgreSQL URL environment variable {name} is not set"),
        )
    })?;
    if value.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("PostgreSQL URL environment variable {name} is empty"),
        ));
    }
    Ok(value)
}

'''
main = main[:validate_idx] + validators + main[validate_idx:]
# Replace tests module in full.
test_idx = main.find("#[cfg(test)]\nmod tests")
if test_idx < 0: raise SystemExit("main tests marker missing")
main = main[:test_idx] + r'''#[cfg(test)]
mod tests {
    use super::*;

    fn serve_args(artifact_backend: ArtifactBackendArg) -> ServeArgs {
        ServeArgs {
            canonical_backend: CanonicalBackendArg::Sqlite,
            database: Some(PathBuf::from("server.sqlite")),
            postgres_url_env: "AIDEMEMO_POSTGRES_URL".to_owned(),
            postgres_transport: PostgresTransportArg::RequireTls,
            postgres_pool_size: 8,
            postgres_acquire_timeout_ms: 500,
            postgres_operation_timeout_ms: 10_000,
            postgres_statement_timeout_ms: 8_000,
            postgres_lock_timeout_ms: 2_000,
            artifact_root: None,
            artifact_backend,
            artifact_s3_bucket: None,
            artifact_s3_prefix: "aidememo/v1".to_owned(),
            artifact_s3_region: "auto".to_owned(),
            artifact_s3_endpoint: None,
            artifact_s3_force_path_style: false,
            embedding_endpoint: None,
            embedding_model: "text-embedding-3-small".to_owned(),
            embedding_dimension: 0,
            embedding_api_key_env: None,
            bind: SocketAddr::from(([127, 0, 0, 1], 3030)),
            allow_insecure_http: false,
        }
    }

    #[test]
    fn local_backend_rejects_s3_only_options() -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Local);
        args.artifact_s3_bucket = Some("artifacts".to_owned());
        let Err(error) = validate_serve_args(&args) else {
            return Err(std::io::Error::other("S3 option was accepted by local backend").into());
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        Ok(())
    }

    #[test]
    fn postgres_requires_explicit_insecure_transport_until_tls_lands()
    -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Disabled);
        args.canonical_backend = CanonicalBackendArg::Postgres;
        args.database = None;
        let Err(error) = validate_serve_args(&args) else {
            return Err(std::io::Error::other("PostgreSQL silently accepted non-TLS transport").into());
        };
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        Ok(())
    }

    #[test]
    fn postgres_requires_artifacts_disabled() -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Local);
        args.canonical_backend = CanonicalBackendArg::Postgres;
        args.database = None;
        args.postgres_transport = PostgresTransportArg::InsecureNoTls;
        let Err(error) = validate_serve_args(&args) else {
            return Err(std::io::Error::other("PostgreSQL accepted node-local artifact catalog").into());
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        Ok(())
    }

    #[test]
    fn explicit_postgres_development_profile_is_valid() -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Disabled);
        args.canonical_backend = CanonicalBackendArg::Postgres;
        args.database = None;
        args.postgres_transport = PostgresTransportArg::InsecureNoTls;
        validate_serve_args(&args)?;
        Ok(())
    }

    #[test]
    fn postgres_timeout_order_is_fail_closed() -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Disabled);
        args.canonical_backend = CanonicalBackendArg::Postgres;
        args.database = None;
        args.postgres_transport = PostgresTransportArg::InsecureNoTls;
        args.postgres_statement_timeout_ms = args.postgres_operation_timeout_ms;
        let Err(error) = validate_serve_args(&args) else {
            return Err(std::io::Error::other("outer timeout race was accepted").into());
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        Ok(())
    }

    #[test]
    fn disabled_artifacts_reject_artifact_paths() -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Disabled);
        args.artifact_root = Some(PathBuf::from("artifacts"));
        let Err(error) = validate_serve_args(&args) else {
            return Err(std::io::Error::other("disabled artifact profile accepted artifact root").into());
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        Ok(())
    }

    #[cfg(not(feature = "s3"))]
    #[test]
    fn s3_backend_requires_feature_before_server_state_is_opened() -> Result<(), Box<dyn Error>> {
        let args = serve_args(ArtifactBackendArg::S3);
        let Err(error) = validate_serve_args(&args) else {
            return Err(
                std::io::Error::other("S3 backend was accepted without its feature").into(),
            );
        };
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        Ok(())
    }

    #[cfg(feature = "s3")]
    #[test]
    fn s3_backend_requires_bucket_before_server_state_is_opened() -> Result<(), Box<dyn Error>> {
        let args = serve_args(ArtifactBackendArg::S3);
        let Err(error) = validate_serve_args(&args) else {
            return Err(std::io::Error::other("S3 backend was accepted without a bucket").into());
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        Ok(())
    }
}
'''
main_path.write_text(main)


# ---------------------------------------------------------------------------
# Existing PostgreSQL executor integration test signature.
# ---------------------------------------------------------------------------
test_path = Path("crates/aidememo-server/tests/postgres_executor.rs")
test = test_path.read_text()
test = replace_once(
    test,
    '''        Duration::from_millis(100),
        Duration::from_secs(1),
    )''',
    '''        Duration::from_millis(100),
        Duration::from_secs(1),
        Duration::from_millis(800),
        Duration::from_millis(100),
    )''',
    "postgres executor test timeout args",
)
test_path.write_text(test)

print("PostgreSQL server profile patch applied")
