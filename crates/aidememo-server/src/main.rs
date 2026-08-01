use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, ProjectScope, RecordStatus, Revision, TenantId, TenantRecord,
};
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
    if !args.bind.ip().is_loopback() && !args.allow_insecure_http {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "non-loopback plaintext HTTP requires --allow-insecure-http; prefer a TLS ingress",
        )
        .into());
    }
    let store = SqliteCommandStore::open(&args.database)?;
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    info!(address = %args.bind, "AideMemo single-node SSOT foundation listening");
    axum::serve(listener, router(ServerState::new(store)))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
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
