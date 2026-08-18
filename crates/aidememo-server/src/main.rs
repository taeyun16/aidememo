use aidememo_artifacts::LocalArtifactStore;
#[cfg(feature = "s3")]
use aidememo_artifacts::{S3BodyStore, S3BodyStoreConfig};
use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, ProjectScope, RecordStatus, Revision, TenantId, TenantRecord,
};
#[cfg(feature = "semantic")]
use aidememo_server::HttpEmbeddingProvider;
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_local::SqliteCommandStore;
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    error::Error,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
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
    /// Separate server ledger SQLite path.
    #[arg(long)]
    database: PathBuf,
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
    /// Separate server ledger SQLite path.
    #[arg(long)]
    database: PathBuf,
    /// Separate local artifact catalog and immutable-object directory.
    /// Defaults to `<database>.artifacts`.
    #[arg(long)]
    artifact_root: Option<PathBuf>,
    /// Immutable artifact body backend.
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

#[derive(Clone, Copy, ValueEnum)]
enum ArtifactBackendArg {
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
        ServerCommand::Bootstrap(args) => bootstrap(args)?,
        ServerCommand::Serve(args) => serve(args).await?,
    }
    Ok(())
}

fn bootstrap(args: BootstrapArgs) -> Result<(), Box<dyn Error>> {
    let token = read_token_file(&args.token_file)?;
    let digest = bearer_token_digest(&token)?;
    let timestamp = unix_time_ms()?;
    let tenant_id = TenantId::try_from(args.tenant_id)?;
    let project_id = ProjectId::try_from(args.project_id)?;
    let actor_id = ActorId::try_from(args.actor_id)?;
    let scope = ProjectScope::new(tenant_id.clone(), project_id.clone());
    let mut store = SqliteCommandStore::open(&args.database)?;
    let epoch = match args.project_epoch {
        Some(epoch) => ProjectEpoch::try_from(epoch)?,
        None => match store.project_epoch(&scope)? {
            Some(epoch) => epoch,
            None => ProjectEpoch::try_from(ulid::Ulid::new().to_string())?,
        },
    };
    let tenant = TenantRecord {
        display_name: args
            .tenant_name
            .unwrap_or_else(|| tenant_id.as_str().to_owned()),
        tenant_id: tenant_id.clone(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant_id.clone(),
        display_name: args
            .project_name
            .unwrap_or_else(|| project_id.as_str().to_owned()),
        project_id: project_id.clone(),
        project_epoch: epoch,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let actor = ActorRecord {
        tenant_id: tenant_id.clone(),
        display_name: args
            .actor_name
            .unwrap_or_else(|| actor_id.as_str().to_owned()),
        actor_id: actor_id.clone(),
        kind: args.actor_kind.into(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let membership = ProjectMembership {
        tenant_id,
        project_id,
        actor_id,
        role: args.role.into(),
        status: MembershipStatus::Active,
    };
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(&actor, &membership, &digest, timestamp)?;
    println!(
        "bootstrapped tenant={} project={} actor={} epoch={}",
        tenant.tenant_id, project.project_id, actor.actor_id, project.project_epoch
    );
    Ok(())
}

async fn serve(args: ServeArgs) -> Result<(), Box<dyn Error>> {
    validate_serve_args(&args)?;
    let store = SqliteCommandStore::open(&args.database)?;
    let artifact_root = args
        .artifact_root
        .unwrap_or_else(|| default_artifact_root(&args.database));
    let artifacts = LocalArtifactStore::open(&artifact_root)?;
    let state = match args.artifact_backend {
        ArtifactBackendArg::Local => ServerState::with_artifacts(store, artifacts)?,
        ArtifactBackendArg::S3 => {
            #[cfg(feature = "s3")]
            {
                let bucket = args.artifact_s3_bucket.ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "--artifact-s3-bucket is required for --artifact-backend s3",
                    )
                })?;
                let config = S3BodyStoreConfig::new(
                    bucket,
                    args.artifact_s3_prefix,
                    args.artifact_s3_region,
                    args.artifact_s3_endpoint,
                    args.artifact_s3_force_path_style,
                )?;
                let bodies = S3BodyStore::from_environment(config).await;
                ServerState::with_s3_artifacts(store, artifacts, bodies)?
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
    let artifact_backend = match args.artifact_backend {
        ArtifactBackendArg::Local => "local",
        ArtifactBackendArg::S3 => "s3",
    };
    info!(address = %args.bind, artifact_root = %artifact_root.display(), artifact_backend, "AideMemo single-node SSOT foundation listening");
    let gc_state = state.clone();
    let gc_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
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
    match args.artifact_backend {
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
            database: PathBuf::from("server.sqlite"),
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
