use aidememo_artifacts::LocalArtifactStore;
#[cfg(feature = "s3")]
use aidememo_artifacts::{S3BodyStore, S3BodyStoreConfig};
use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, DomainError, MembershipRole, MembershipStatus, ProjectEpoch,
    ProjectId, ProjectMembership, ProjectRecord, ProjectScope, RecordStatus, Revision,
    ServerCanonicalStore, TenantId, TenantRecord,
};
#[cfg(feature = "semantic")]
use aidememo_server::HttpEmbeddingProvider;
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_local::SqliteCommandStore;
use aidememo_store_postgres::PostgresCommandStore;
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    error::Error,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::info;

#[derive(Parser)]
#[command(
    name = "aidememo-server",
    about = "AideMemo single-node SSOT foundation"
)]
struct Args {
    #[command(subcommand)]
    command: ServerCommand,
}

#[derive(Subcommand)]
enum ServerCommand {
    /// Provision one tenant, project, actor, membership, and bearer token.
    Bootstrap(BootstrapArgs),
    /// Serve the authenticated resource command and change-feed API.
    Serve(ServeArgs),
}

#[derive(clap::Args)]
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
    /// PostgreSQL transport policy. Verified TLS is the production default.
    #[arg(long, value_enum, default_value_t = PostgresTransportArg::RequireTls)]
    postgres_transport: PostgresTransportArg,
    /// Optional PEM root CA file added to the platform trust store for PostgreSQL TLS.
    #[arg(long)]
    postgres_root_ca_file: Option<PathBuf>,
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

#[derive(clap::Args)]
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
    /// Optional PEM root CA file added to the platform trust store for PostgreSQL TLS.
    #[arg(long)]
    postgres_root_ca_file: Option<PathBuf>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CanonicalBackendArg {
    Sqlite,
    Postgres,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PostgresTransportArg {
    RequireTls,
    InsecureNoTls,
}

#[derive(Clone, Copy, ValueEnum)]
enum RoleArg {
    Owner,
    Admin,
    Writer,
    Reader,
}

impl From<RoleArg> for MembershipRole {
    fn from(role: RoleArg) -> Self {
        match role {
            RoleArg::Owner => Self::Owner,
            RoleArg::Admin => Self::Admin,
            RoleArg::Writer => Self::Writer,
            RoleArg::Reader => Self::Reader,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ActorKindArg {
    Human,
    Agent,
    Service,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ArtifactBackendArg {
    Disabled,
    Local,
    S3,
}

impl From<ActorKindArg> for ActorKind {
    fn from(kind: ActorKindArg) -> Self {
        match kind {
            ActorKindArg::Human => Self::Human,
            ActorKindArg::Agent => Self::Agent,
            ActorKindArg::Service => Self::Service,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aidememo_server=info".into()),
        )
        .init();
    match Args::parse().command {
        ServerCommand::Bootstrap(args) => bootstrap(args).await?,
        ServerCommand::Serve(args) => serve(args).await?,
    }
    Ok(())
}

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
    let postgres_transport = args.postgres_transport;
    let postgres_root_ca =
        read_postgres_root_ca(postgres_transport, args.postgres_root_ca_file.as_deref())?;
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
                let mut store = match postgres_transport {
                    PostgresTransportArg::RequireTls => {
                        PostgresCommandStore::connect_tls_with_timeouts(
                            &url,
                            postgres_root_ca.as_deref(),
                            statement_timeout,
                            lock_timeout,
                        )?
                    }
                    PostgresTransportArg::InsecureNoTls => {
                        PostgresCommandStore::connect_no_tls_with_timeouts(
                            &url,
                            statement_timeout,
                            lock_timeout,
                        )?
                    }
                };
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
            let root_ca = read_postgres_root_ca(
                args.postgres_transport,
                args.postgres_root_ca_file.as_deref(),
            )?;
            let state = match args.postgres_transport {
                PostgresTransportArg::RequireTls => {
                    ServerState::postgres_tls(
                        url,
                        root_ca,
                        args.postgres_pool_size,
                        Duration::from_millis(args.postgres_acquire_timeout_ms),
                        Duration::from_millis(args.postgres_operation_timeout_ms),
                        Duration::from_millis(args.postgres_statement_timeout_ms),
                        Duration::from_millis(args.postgres_lock_timeout_ms),
                    )
                    .await?
                }
                PostgresTransportArg::InsecureNoTls => {
                    ServerState::postgres_no_tls_for_development(
                        url,
                        args.postgres_pool_size,
                        Duration::from_millis(args.postgres_acquire_timeout_ms),
                        Duration::from_millis(args.postgres_operation_timeout_ms),
                        Duration::from_millis(args.postgres_statement_timeout_ms),
                        Duration::from_millis(args.postgres_lock_timeout_ms),
                    )
                    .await?
                }
            };
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

fn validate_bootstrap_args(args: &BootstrapArgs) -> Result<(), std::io::Error> {
    match args.canonical_backend {
        CanonicalBackendArg::Sqlite => {
            if args.postgres_root_ca_file.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--postgres-root-ca-file requires --canonical-backend postgres",
                ));
            }
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
            validate_postgres_transport(
                args.postgres_transport,
                args.postgres_root_ca_file.as_deref(),
            )?;
            validate_postgres_session_timeouts(
                args.postgres_statement_timeout_ms,
                args.postgres_lock_timeout_ms,
            )?;
            validate_env_name(&args.postgres_url_env)?;
        }
    }
    Ok(())
}

fn validate_postgres_transport(
    transport: PostgresTransportArg,
    root_ca_file: Option<&Path>,
) -> Result<(), std::io::Error> {
    if transport == PostgresTransportArg::InsecureNoTls && root_ca_file.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--postgres-root-ca-file is valid only with --postgres-transport require-tls",
        ));
    }
    Ok(())
}

fn read_postgres_root_ca(
    transport: PostgresTransportArg,
    root_ca_file: Option<&Path>,
) -> Result<Option<Vec<u8>>, std::io::Error> {
    validate_postgres_transport(transport, root_ca_file)?;
    match root_ca_file {
        Some(path) => std::fs::read(path).map(Some),
        None => Ok(None),
    }
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
            if args.postgres_root_ca_file.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--postgres-root-ca-file requires --canonical-backend postgres",
                ));
            }
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
            validate_postgres_transport(
                args.postgres_transport,
                args.postgres_root_ca_file.as_deref(),
            )?;
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

fn default_artifact_root(database: &Path) -> PathBuf {
    let mut path = database.as_os_str().to_os_string();
    path.push(".artifacts");
    PathBuf::from(path)
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal");
    }
}

fn read_token_file(path: &Path) -> Result<String, Box<dyn Error>> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "token path must be a regular file",
        )
        .into());
    }
    require_private_permissions(&metadata)?;
    let mut token = std::fs::read_to_string(path)?;
    while token.ends_with('\n') || token.ends_with('\r') {
        token.pop();
    }
    bearer_token_digest(&token)?;
    Ok(token)
}

#[cfg(unix)]
fn require_private_permissions(metadata: &std::fs::Metadata) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "token file must not be accessible by group or others; use chmod 600",
        )
        .into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_private_permissions(_metadata: &std::fs::Metadata) -> Result<(), Box<dyn Error>> {
    Ok(())
}

fn unix_time_ms() -> Result<i64, Box<dyn Error>> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(i64::try_from(duration.as_millis())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serve_args(artifact_backend: ArtifactBackendArg) -> ServeArgs {
        ServeArgs {
            canonical_backend: CanonicalBackendArg::Sqlite,
            database: Some(PathBuf::from("server.sqlite")),
            postgres_url_env: "AIDEMEMO_POSTGRES_URL".to_owned(),
            postgres_transport: PostgresTransportArg::RequireTls,
            postgres_root_ca_file: None,
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
    fn postgres_tls_is_the_valid_default_transport() -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Disabled);
        args.canonical_backend = CanonicalBackendArg::Postgres;
        args.database = None;
        validate_serve_args(&args)?;
        Ok(())
    }

    #[test]
    fn insecure_postgres_rejects_root_ca_file() -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Disabled);
        args.canonical_backend = CanonicalBackendArg::Postgres;
        args.database = None;
        args.postgres_transport = PostgresTransportArg::InsecureNoTls;
        args.postgres_root_ca_file = Some(PathBuf::from("private-ca.pem"));
        let Err(error) = validate_serve_args(&args) else {
            return Err(std::io::Error::other("insecure PostgreSQL accepted a TLS root CA").into());
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        Ok(())
    }

    #[test]
    fn postgres_requires_artifacts_disabled() -> Result<(), Box<dyn Error>> {
        let mut args = serve_args(ArtifactBackendArg::Local);
        args.canonical_backend = CanonicalBackendArg::Postgres;
        args.database = None;
        args.postgres_transport = PostgresTransportArg::InsecureNoTls;
        let Err(error) = validate_serve_args(&args) else {
            return Err(
                std::io::Error::other("PostgreSQL accepted node-local artifact catalog").into(),
            );
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
            return Err(
                std::io::Error::other("disabled artifact profile accepted artifact root").into(),
            );
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
