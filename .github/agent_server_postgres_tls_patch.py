from pathlib import Path


def one(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


# PostgreSQL adapter dependencies.
cargo_path = Path("crates/aidememo-store-postgres/Cargo.toml")
cargo = cargo_path.read_text()
cargo = one(
    cargo,
    'aidememo-domain = { version = "0.1.0", path = "../aidememo-domain" }\npostgres = "0.19.14"\nserde_json = "1.0"',
    'aidememo-domain = { version = "0.1.0", path = "../aidememo-domain" }\nnative-tls = "0.2"\npostgres = "0.19.14"\npostgres-native-tls = "0.5"\nserde_json = "1.0"',
    "postgres TLS dependencies",
)
cargo_path.write_text(cargo)


# PostgreSQL adapter TLS constructor.
pg_path = Path("crates/aidememo-store-postgres/src/lib.rs")
pg = pg_path.read_text()
pg = one(
    pg,
    'use postgres::{Client, GenericClient, IsolationLevel, NoTls, Row, Transaction};',
    'use native_tls::{Certificate, TlsConnector};\nuse postgres::{Client, GenericClient, IsolationLevel, NoTls, Row, Transaction, config::SslMode};\nuse postgres_native_tls::MakeTlsConnector;',
    "postgres TLS imports",
)
old = '''    pub fn connect_no_tls_with_timeouts(
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
new = '''    pub fn connect_no_tls_with_timeouts(
        url: &str,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, DomainError> {
        let client = Client::connect(url, NoTls).map_err(|error| storage("connect", error))?;
        Self::from_connected_client(client, statement_timeout, lock_timeout)
    }

    /// Connect with certificate- and hostname-verified TLS and finite server-side timeouts.
    ///
    /// The PostgreSQL URL is parsed into [`postgres::Config`] and its SSL mode is
    /// forcibly set to [`SslMode::Require`]. This prevents a URL-provided
    /// `sslmode=disable`/`prefer` value from downgrading the transport. The
    /// platform trust store is used by default; an optional PEM root certificate
    /// may be added for private/internal CAs. Certificate and hostname validation
    /// are never disabled.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when URL parsing, CA parsing, TLS connector
    /// construction, TLS connection, timeout configuration, or migration fails.
    pub fn connect_tls_with_timeouts(
        url: &str,
        root_ca_pem: Option<&[u8]>,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, DomainError> {
        let mut config = url
            .parse::<postgres::Config>()
            .map_err(|error| storage("tls_config_parse", error))?;
        config.ssl_mode(SslMode::Require);

        let mut builder = TlsConnector::builder();
        if let Some(root_ca_pem) = root_ca_pem {
            let certificate = Certificate::from_pem(root_ca_pem)
                .map_err(|error| storage("tls_root_ca_parse", error))?;
            builder.add_root_certificate(certificate);
        }
        let connector = builder
            .build()
            .map_err(|error| storage("tls_connector_build", error))?;
        let client = config
            .connect(MakeTlsConnector::new(connector))
            .map_err(|error| storage("tls_connect", error))?;
        Self::from_connected_client(client, statement_timeout, lock_timeout)
    }

    fn from_connected_client(
        mut client: Client,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, DomainError> {
        configure_timeout(&mut client, "statement_timeout", statement_timeout)?;
        configure_timeout(&mut client, "lock_timeout", lock_timeout)?;
        let store = Self {
            client: Mutex::new(client),
        };
        store.migrate()?;
        Ok(store)
    }
'''
pg = one(pg, old, new, "postgres TLS constructor")
pg_path.write_text(pg)


# Bounded executor TLS pool constructor.
ex_path = Path("crates/aidememo-server/src/executor.rs")
ex = ex_path.read_text()
marker = '''    /// Execute one synchronous canonical-store session away from Tokio workers.
'''
idx = ex.find(marker)
if idx < 0:
    raise SystemExit("executor insertion marker missing")
tls_ctor = '''    /// Build a bounded PostgreSQL pool with verified TLS.
    ///
    /// Connection creation runs on Tokio's blocking pool. TLS verification is
    /// performed by the PostgreSQL adapter; no plaintext fallback is permitted.
    pub(crate) async fn postgres_tls(
        url: String,
        root_ca_pem: Option<Vec<u8>>,
        pool_size: usize,
        acquire_timeout: Duration,
        operation_timeout: Duration,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, BlockingStoreError> {
        validate_postgres_policy(
            pool_size,
            acquire_timeout,
            operation_timeout,
            statement_timeout,
            lock_timeout,
        )?;
        let reaper = PostgresDropReaper::new()?;
        let build_reaper = reaper.clone();
        let stores = tokio::task::spawn_blocking(move || {
            (0..pool_size)
                .map(|_| {
                    PostgresCommandStore::connect_tls_with_timeouts(
                        &url,
                        root_ca_pem.as_deref(),
                        statement_timeout,
                        lock_timeout,
                    )
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

'''
ex = ex[:idx] + tls_ctor + ex[idx:]
# Deduplicate policy validation through a helper while retaining no-TLS behavior.
old_policy = '''        if pool_size == 0 {
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
'''
ex = one(
    ex,
    old_policy,
    '''        validate_postgres_policy(
            pool_size,
            acquire_timeout,
            operation_timeout,
            statement_timeout,
            lock_timeout,
        )?;
''',
    "executor no-TLS policy helper",
)
helper_marker = 'fn run_operation<R, F>(\n'
idx = ex.find(helper_marker)
if idx < 0:
    raise SystemExit("executor helper marker missing")
policy_helper = '''fn validate_postgres_policy(
    pool_size: usize,
    acquire_timeout: Duration,
    operation_timeout: Duration,
    statement_timeout: Duration,
    lock_timeout: Duration,
) -> Result<(), BlockingStoreError> {
    if pool_size == 0 {
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
    Ok(())
}

'''
ex = ex[:idx] + policy_helper + ex[idx:]
ex_path.write_text(ex)


# Public server state TLS constructor.
lib_path = Path("crates/aidememo-server/src/lib.rs")
lib = lib_path.read_text()
marker = '''    /// Build an artifact-disabled PostgreSQL server state without TLS.
'''
idx = lib.find(marker)
if idx < 0:
    raise SystemExit("ServerState PostgreSQL marker missing")
tls_state = '''    /// Build an artifact-disabled PostgreSQL server state with verified TLS.
    ///
    /// System trust roots are used by default. `root_ca_pem` may contain one
    /// additional PEM root certificate for a private/internal CA. The adapter
    /// always forces PostgreSQL SSL mode to `require` and keeps hostname and
    /// certificate verification enabled.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when pool construction, TLS, timeout policy,
    /// connection, or schema initialization fails.
    pub async fn postgres_tls(
        url: String,
        root_ca_pem: Option<Vec<u8>>,
        pool_size: usize,
        acquire_timeout: Duration,
        operation_timeout: Duration,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, DomainError> {
        let canonical = BlockingStoreExecutor::postgres_tls(
            url,
            root_ca_pem,
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
lib = lib[:idx] + tls_state + lib[idx:]
lib = lib.replace(
    '''    /// This constructor exists only for explicit local/development profiles. The
    /// server CLI fails closed to TLS-required mode unless the operator selects
    /// `insecure-no-tls`; production PostgreSQL transport is a separate TLS slice.
''',
    '''    /// This constructor exists only for explicit local/development profiles.
    /// Production profiles should use [`Self::postgres_tls`].
''',
)
lib_path.write_text(lib)


# Server CLI: root CA file + real RequireTls path.
main_path = Path("crates/aidememo-server/src/main.rs")
main = main_path.read_text()
main = main.replace(
    '''    /// PostgreSQL transport policy. TLS-required fails closed until the TLS connector slice lands.
    #[arg(long, value_enum, default_value_t = PostgresTransportArg::RequireTls)]
    postgres_transport: PostgresTransportArg,
''',
    '''    /// PostgreSQL transport policy. Verified TLS is the production default.
    #[arg(long, value_enum, default_value_t = PostgresTransportArg::RequireTls)]
    postgres_transport: PostgresTransportArg,
    /// Optional PEM root CA file added to the platform trust store for PostgreSQL TLS.
    #[arg(long)]
    postgres_root_ca_file: Option<PathBuf>,
''',
    1,
)
main = main.replace(
    '''    /// PostgreSQL transport policy. Use insecure-no-tls only for explicit local/test environments.
    #[arg(long, value_enum, default_value_t = PostgresTransportArg::RequireTls)]
    postgres_transport: PostgresTransportArg,
''',
    '''    /// PostgreSQL transport policy. Use insecure-no-tls only for explicit local/test environments.
    #[arg(long, value_enum, default_value_t = PostgresTransportArg::RequireTls)]
    postgres_transport: PostgresTransportArg,
    /// Optional PEM root CA file added to the platform trust store for PostgreSQL TLS.
    #[arg(long)]
    postgres_root_ca_file: Option<PathBuf>,
''',
    1,
)
# Bootstrap transport data.
main = one(
    main,
    '''    let backend = args.canonical_backend;
    let database = args.database;
    let statement_timeout = Duration::from_millis(args.postgres_statement_timeout_ms);''',
    '''    let backend = args.canonical_backend;
    let database = args.database;
    let postgres_transport = args.postgres_transport;
    let postgres_root_ca = read_postgres_root_ca(
        postgres_transport,
        args.postgres_root_ca_file.as_deref(),
    )?;
    let statement_timeout = Duration::from_millis(args.postgres_statement_timeout_ms);''',
    "bootstrap TLS inputs",
)
main = one(
    main,
    '''                let mut store = PostgresCommandStore::connect_no_tls_with_timeouts(
                    &url,
                    statement_timeout,
                    lock_timeout,
                )?;''',
    '''                let mut store = match postgres_transport {
                    PostgresTransportArg::RequireTls => PostgresCommandStore::connect_tls_with_timeouts(
                        &url,
                        postgres_root_ca.as_deref(),
                        statement_timeout,
                        lock_timeout,
                    )?,
                    PostgresTransportArg::InsecureNoTls => {
                        PostgresCommandStore::connect_no_tls_with_timeouts(
                            &url,
                            statement_timeout,
                            lock_timeout,
                        )?
                    }
                };''',
    "bootstrap TLS transport selection",
)
# Serve transport selection.
main = one(
    main,
    '''        CanonicalBackendArg::Postgres => {
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
        }''',
    '''        CanonicalBackendArg::Postgres => {
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
        }''',
    "serve TLS transport selection",
)
# Validation calls and implementation.
main = main.replace(
    '            validate_postgres_transport(args.postgres_transport)?;',
    '            validate_postgres_transport(\n                args.postgres_transport,\n                args.postgres_root_ca_file.as_deref(),\n            )?;',
)
old_validate = '''fn validate_postgres_transport(transport: PostgresTransportArg) -> Result<(), std::io::Error> {
    if transport == PostgresTransportArg::RequireTls {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "PostgreSQL TLS is required by default and is not yet implemented; local/test use must explicitly select --postgres-transport insecure-no-tls",
        ));
    }
    Ok(())
}
'''
new_validate = '''fn validate_postgres_transport(
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
'''
main = one(main, old_validate, new_validate, "PostgreSQL TLS validation")
# SQLite must not silently accept a PostgreSQL CA option.
main = main.replace(
    '''        CanonicalBackendArg::Sqlite => {
            if args.database.is_none() {''',
    '''        CanonicalBackendArg::Sqlite => {
            if args.postgres_root_ca_file.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--postgres-root-ca-file requires --canonical-backend postgres",
                ));
            }
            if args.database.is_none() {''',
)
# Test fixture field.
main = one(
    main,
    '''            postgres_transport: PostgresTransportArg::RequireTls,
            postgres_pool_size: 8,''',
    '''            postgres_transport: PostgresTransportArg::RequireTls,
            postgres_root_ca_file: None,
            postgres_pool_size: 8,''',
    "serve test TLS field",
)
# Replace obsolete fail-closed test with valid TLS default and add insecure CA rejection.
old_test = '''    #[test]
    fn postgres_requires_explicit_insecure_transport_until_tls_lands() -> Result<(), Box<dyn Error>>
    {
        let mut args = serve_args(ArtifactBackendArg::Disabled);
        args.canonical_backend = CanonicalBackendArg::Postgres;
        args.database = None;
        let Err(error) = validate_serve_args(&args) else {
            return Err(
                std::io::Error::other("PostgreSQL silently accepted non-TLS transport").into(),
            );
        };
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        Ok(())
    }
'''
new_test = '''    #[test]
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
'''
main = one(main, old_test, new_test, "TLS main tests")
main_path.write_text(main)


# TLS adapter integration tests.
tls_test_path = Path("crates/aidememo-store-postgres/tests/tls.rs")
tls_test_path.write_text(r'''use aidememo_store_postgres::PostgresCommandStore;
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
''')

# TLS HTTP profile integration test.
http_tls_path = Path("crates/aidememo-server/tests/postgres_tls_http.rs")
http_tls_path.write_text(r'''use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, DomainError, MembershipRole, MembershipStatus, ProjectEpoch,
    ProjectId, ProjectMembership, ProjectRecord, RecordStatus, Revision, ServerIdentityStore,
    TenantId, TenantRecord,
};
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_postgres::PostgresCommandStore;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{fs, time::Duration};
use tower::ServiceExt;

const WRITER_TOKEN: &str = "postgres-tls-http-writer-token-0123456789";

fn bootstrap_postgres(url: &str, ca: &[u8]) -> Result<(), DomainError> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_pg_tls_http")?,
        display_name: "PostgreSQL TLS HTTP tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_pg_tls_http")?,
        display_name: "PostgreSQL TLS HTTP project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_pg_tls_http")?,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let actor = ActorRecord {
        tenant_id: tenant.tenant_id.clone(),
        actor_id: ActorId::try_from("writer_pg_tls_http")?,
        display_name: "PostgreSQL TLS HTTP writer".to_owned(),
        kind: ActorKind::Agent,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let membership = ProjectMembership {
        tenant_id: tenant.tenant_id.clone(),
        project_id: project.project_id.clone(),
        actor_id: actor.actor_id.clone(),
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    let mut store = PostgresCommandStore::connect_tls_with_timeouts(
        url,
        Some(ca),
        Duration::from_millis(1_500),
        Duration::from_millis(250),
    )?;
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(
        &actor,
        &membership,
        &bearer_token_digest(WRITER_TOKEN)?,
        timestamp,
    )?;
    Ok(())
}

fn get_request(uri: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {WRITER_TOKEN}"))
        .body(Body::empty())
}

fn command_request(body: &Value) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("POST")
        .uri("/v1/commands")
        .header("authorization", format!("Bearer {WRITER_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
}

async fn response_json(response: axum::response::Response) -> Result<Value, axum::Error> {
    let bytes = response.into_body().collect().await?.to_bytes();
    serde_json::from_slice(&bytes).map_err(axum::Error::new)
}

#[tokio::test]
#[ignore = "requires disposable TLS PostgreSQL via AIDEMEMO_POSTGRES_TLS_URL"]
async fn tls_postgres_profile_serves_authenticated_canonical_http()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_TLS_URL")?;
    let ca = fs::read(std::env::var("AIDEMEMO_POSTGRES_TLS_CA")?)?;
    let bootstrap_url = url.clone();
    let bootstrap_ca = ca.clone();
    tokio::task::spawn_blocking(move || bootstrap_postgres(&bootstrap_url, &bootstrap_ca)).await??;

    let state = ServerState::postgres_tls(
        url,
        Some(ca),
        2,
        Duration::from_millis(250),
        Duration::from_secs(2),
        Duration::from_millis(1_500),
        Duration::from_millis(250),
    )
    .await?;
    let app = router(state);

    let health = app
        .clone()
        .oneshot(Request::builder().uri("/health").body(Body::empty())?)
        .await?;
    assert_eq!(health.status(), 200);
    assert_eq!(response_json(health).await?["schema_version"], 2);

    let identity = app
        .clone()
        .oneshot(get_request("/v1/projects/project_pg_tls_http/identity")?)
        .await?;
    assert_eq!(identity.status(), 200);
    assert_eq!(response_json(identity).await?["actor_id"], "writer_pg_tls_http");

    let command = json!({
        "command_id": "command_pg_tls_http",
        "project_id": "project_pg_tls_http",
        "expected_revision": null,
        "operation": "resource.put",
        "payload": {"content": "persisted through verified PostgreSQL TLS"},
        "resource": {"kind": "custom.note", "id": "note_pg_tls_http"},
        "change": "upsert"
    });
    let first = app.clone().oneshot(command_request(&command)?).await?;
    assert_eq!(first.status(), 200);
    let receipt = response_json(first).await?;
    let replay = app.clone().oneshot(command_request(&command)?).await?;
    assert_eq!(replay.status(), 200);
    assert_eq!(response_json(replay).await?, receipt);

    let resource = app
        .oneshot(get_request(
            "/v1/projects/project_pg_tls_http/resources/custom.note/note_pg_tls_http",
        )?)
        .await?;
    assert_eq!(resource.status(), 200);
    Ok(())
}
''')

# Extend PostgreSQL conformance with a real TLS PostgreSQL container.
workflow_path = Path(".github/workflows/postgres-conformance.yml")
workflow = workflow_path.read_text()
checkout = '      - uses: actions/checkout@v7\n'
setup_tls = r'''      - name: Start verified-TLS PostgreSQL 17 fixture
        shell: bash
        run: |
          set -euo pipefail
          tls_dir="$RUNNER_TEMP/aidememo-postgres-tls"
          mkdir -p "$tls_dir"
          openssl req -x509 -newkey rsa:2048 -nodes \
            -keyout "$tls_dir/ca.key" -out "$tls_dir/ca.crt" \
            -subj "/CN=AideMemo Test Root CA" -days 1
          openssl req -newkey rsa:2048 -nodes \
            -keyout "$tls_dir/server.key" -out "$tls_dir/server.csr" \
            -subj "/CN=localhost"
          cat > "$tls_dir/server.ext" <<'EOF'
          subjectAltName=DNS:localhost,IP:127.0.0.1
          extendedKeyUsage=serverAuth
          EOF
          openssl x509 -req -in "$tls_dir/server.csr" \
            -CA "$tls_dir/ca.crt" -CAkey "$tls_dir/ca.key" -CAcreateserial \
            -out "$tls_dir/server.crt" -days 1 -extfile "$tls_dir/server.ext"
          openssl req -x509 -newkey rsa:2048 -nodes \
            -keyout "$tls_dir/wrong-ca.key" -out "$tls_dir/wrong-ca.crt" \
            -subj "/CN=AideMemo Wrong Root CA" -days 1

          docker run -d --name aidememo-postgres-tls \
            -p 55432:5432 \
            -e POSTGRES_USER=postgres \
            -e POSTGRES_PASSWORD=postgres \
            -e POSTGRES_DB=aidememo_tls \
            -v "$tls_dir:/tls:ro" \
            --entrypoint bash postgres:17 -ceu '
              cp /tls/server.crt /tmp/server.crt
              cp /tls/server.key /tmp/server.key
              chown postgres:postgres /tmp/server.crt /tmp/server.key
              chmod 600 /tmp/server.key
              exec /usr/local/bin/docker-entrypoint.sh postgres \
                -c ssl=on \
                -c ssl_cert_file=/tmp/server.crt \
                -c ssl_key_file=/tmp/server.key
            '
          for attempt in $(seq 1 30); do
            if docker exec aidememo-postgres-tls pg_isready -U postgres -d aidememo_tls; then
              break
            fi
            if [ "$attempt" -eq 30 ]; then
              docker logs aidememo-postgres-tls
              exit 1
            fi
            sleep 1
          done
          echo "AIDEMEMO_POSTGRES_TLS_URL=postgres://postgres:postgres@localhost:55432/aidememo_tls?sslmode=disable" >> "$GITHUB_ENV"
          echo "AIDEMEMO_POSTGRES_TLS_CA=$tls_dir/ca.crt" >> "$GITHUB_ENV"
          echo "AIDEMEMO_POSTGRES_TLS_WRONG_CA=$tls_dir/wrong-ca.crt" >> "$GITHUB_ENV"
'''
workflow = one(workflow, checkout, checkout + setup_tls, "TLS workflow fixture")
workflow = workflow.replace(
    '''      - name: Run PostgreSQL server HTTP profile conformance
        run: cargo test -p aidememo-server --test postgres_http -- --ignored --nocapture
''',
    '''      - name: Run PostgreSQL server HTTP profile conformance
        run: cargo test -p aidememo-server --test postgres_http -- --ignored --nocapture
      - name: Run verified TLS adapter conformance
        run: cargo test -p aidememo-store-postgres --test tls -- --ignored --nocapture
      - name: Run verified TLS server HTTP profile conformance
        run: cargo test -p aidememo-server --test postgres_tls_http -- --ignored --nocapture
      - name: Stop TLS PostgreSQL fixture
        if: always()
        run: docker rm -f aidememo-postgres-tls || true
''',
)
workflow_path.write_text(workflow)

print("PostgreSQL TLS patch applied")
