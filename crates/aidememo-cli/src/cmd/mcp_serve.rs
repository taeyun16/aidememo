//! `aidememo mcp-serve` — MCP server over HTTP + SSE.
//!
//! Speaks MCP JSON-RPC 2.0 over an HTTP POST endpoint (`/mcp`) plus an SSE
//! endpoint (`/sse`) for browser-based or remote clients. For local agents
//! (Claude Code, Codex CLI), prefer `aidememo mcp` (stdio transport) instead.
//!
//! Usage:
//!   aidememo mcp-serve --port 3000

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};

use crate::cmd::mcp_tools::{JsonRpcRequest, JsonRpcResponse, dispatch};
use crate::{AideMemo, Config, cmd::Command};
use axum::{
    Extension, Json, Router,
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bpaf::*;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
struct AppState {
    wiki: Arc<AideMemo>,
    status: Arc<ServerStatus>,
}

struct ServerStatus {
    started_at_ms: u64,
    store_path: PathBuf,
    bind_addr: String,
    port: u16,
    auth_mode: &'static str,
    semantic_prewarm: AtomicU8,
    counters: RequestCounters,
}

#[derive(Default)]
struct RequestCounters {
    total: AtomicU64,
    mcp: AtomicU64,
    sse: AtomicU64,
    sync_since: AtomicU64,
    health: AtomicU64,
    admin_status: AtomicU64,
}

#[derive(Debug, Serialize)]
struct RouteCounts {
    mcp: u64,
    sse: u64,
    sync_since: u64,
    health: u64,
    admin_status: u64,
}

#[derive(Debug, Serialize)]
struct AdminStatusReport {
    status: &'static str,
    store_path: String,
    bind_addr: String,
    port: u16,
    auth_mode: &'static str,
    semantic_prewarm: &'static str,
    started_at_ms: u64,
    uptime_ms: u64,
    request_count: u64,
    routes: RouteCounts,
    sync: SyncStatusReport,
}

#[derive(Debug, Serialize)]
struct SyncStatusReport {
    cursor_file: String,
    exists: bool,
    remotes_count: usize,
    remotes: Vec<SyncRemoteReport>,
}

#[derive(Debug, Serialize)]
struct SyncRemoteReport {
    url: String,
    entity: Option<String>,
    fact: Option<String>,
    entity_updated_at: Option<u64>,
    entity_updated_id: Option<String>,
    fact_updated_at: Option<u64>,
    fact_updated_id: Option<String>,
    relation_created_at: Option<u64>,
    relation_key: Option<String>,
    relation_generation: Option<String>,
    relation_scan_key: Option<String>,
    last_pulled_at: u64,
    age_ms: u64,
}

#[derive(Debug, Serialize)]
struct ScopedHealthReport {
    status: &'static str,
    semantic_prewarm: &'static str,
}

#[derive(Debug, Default, Deserialize)]
struct StoredCursor {
    entity: Option<String>,
    fact: Option<String>,
    #[serde(default)]
    entity_updated_at: Option<u64>,
    #[serde(default)]
    entity_updated_id: Option<String>,
    #[serde(default)]
    fact_updated_at: Option<u64>,
    #[serde(default)]
    fact_updated_id: Option<String>,
    #[serde(default)]
    relation_created_at: Option<u64>,
    #[serde(default)]
    relation_key: Option<String>,
    #[serde(default)]
    relation_generation: Option<String>,
    #[serde(default)]
    relation_scan_key: Option<String>,
    last_pulled_at: u64,
}

#[derive(Debug, Default, Deserialize)]
struct SyncCursorFile {
    remotes: std::collections::HashMap<String, StoredCursor>,
}

impl ServerStatus {
    fn record_mcp(&self) {
        self.record(&self.counters.mcp);
    }

    fn record_sse(&self) {
        self.record(&self.counters.sse);
    }

    fn record_sync_since(&self) {
        self.record(&self.counters.sync_since);
    }

    fn record_health(&self) {
        self.record(&self.counters.health);
    }

    fn record_admin_status(&self) {
        self.record(&self.counters.admin_status);
    }

    fn record(&self, route: &AtomicU64) {
        self.counters.total.fetch_add(1, Ordering::Relaxed);
        route.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> AdminStatusReport {
        status_report(self)
    }

    fn semantic_prewarm_label(&self) -> &'static str {
        match self.semantic_prewarm.load(Ordering::Relaxed) {
            PREWARM_WARMING => "warming",
            PREWARM_READY => "ready",
            PREWARM_FAILED => "failed",
            _ => "disabled",
        }
    }
}

const PREWARM_DISABLED: u8 = 0;
const PREWARM_WARMING: u8 = 1;
const PREWARM_READY: u8 = 2;
const PREWARM_FAILED: u8 = 3;

#[derive(Clone)]
struct BoundIdentity {
    source_id: String,
    actor_id: String,
}

enum AuthConfig {
    None,
    Single(String),
    Bound(HashMap<String, BoundIdentity>),
}

impl AuthConfig {
    fn enabled(&self) -> bool {
        !matches!(self, Self::None)
    }

    fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Single(_) => "bearer",
            Self::Bound(_) => "bearer-bound",
        }
    }

    fn authenticate(&self, token: &str) -> Option<AuthenticatedPrincipal> {
        match self {
            Self::None => Some(AuthenticatedPrincipal::Unbound),
            Self::Single(expected) if !token.is_empty() && token == expected => {
                Some(AuthenticatedPrincipal::Unbound)
            }
            Self::Bound(bindings) => bindings
                .get(token)
                .cloned()
                .map(AuthenticatedPrincipal::Bound),
            _ => None,
        }
    }
}

enum AuthenticatedPrincipal {
    Unbound,
    Bound(BoundIdentity),
}

#[derive(Deserialize)]
struct TokenBindingEntry {
    token: String,
    source_id: String,
    actor_id: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TokenBindingsDocument {
    Object { tokens: Vec<TokenBindingEntry> },
    Array(Vec<TokenBindingEntry>),
}

type AuthState = Arc<AuthConfig>;

fn build_router(state: AppState, auth_state: AuthState, auth_enabled: bool) -> Router {
    let mut app = Router::new()
        .route("/mcp", post(handle_post))
        .route("/sse", get(handle_sse))
        .route("/sync/since", get(handle_sync_since))
        .route("/health", get(handle_health))
        .route("/admin/status", get(handle_admin_status))
        .with_state(state);

    if auth_enabled {
        app = app.layer(middleware::from_fn_with_state(auth_state, require_bearer));
    }
    app
}

async fn handle_post(
    State(state): State<AppState>,
    identity: Option<Extension<BoundIdentity>>,
    Json(mut req): Json<JsonRpcRequest>,
) -> Response {
    state.status.record_mcp();
    if let Some(Extension(identity)) = identity
        && let Err(message) = bind_request_identity(&mut req, &identity)
    {
        let resp = JsonRpcResponse::error(req.id, -32602, &message);
        return Json(resp).into_response();
    }

    match dispatch(req, state.wiki.as_ref()) {
        Some(resp) => Json(resp).into_response(),
        None => axum::http::StatusCode::NO_CONTENT.into_response(),
    }
}

fn bind_request_identity(req: &mut JsonRpcRequest, identity: &BoundIdentity) -> Result<(), String> {
    if req.method != "tools/call" {
        return Ok(());
    }

    let params = req
        .params
        .get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let params = params
        .as_object_mut()
        .ok_or_else(|| "authenticated tools/call params must be an object".to_string())?;
    let tool_name = params
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let arguments = params
        .entry("arguments")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if arguments.is_null() {
        *arguments = serde_json::Value::Object(serde_json::Map::new());
    }
    let arguments = arguments
        .as_object_mut()
        .ok_or_else(|| "authenticated tool arguments must be an object".to_string())?;

    enforce_identity_field(arguments, "source_id", &identity.source_id)?;
    enforce_identity_field(arguments, "actor_id", &identity.actor_id)?;

    if tool_name == "aidememo_fact_add_many"
        && let Some(items) = arguments
            .get_mut("items")
            .and_then(|value| value.as_array_mut())
    {
        for (index, item) in items.iter_mut().enumerate() {
            let item = item
                .as_object_mut()
                .ok_or_else(|| format!("items[{index}] must be an object"))?;
            enforce_identity_field(item, "source_id", &identity.source_id)?;
            enforce_identity_field(item, "actor_id", &identity.actor_id)?;
        }
    }

    Ok(())
}

fn enforce_identity_field(
    object: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    expected: &str,
) -> Result<(), String> {
    if let Some(value) = object.get(field)
        && !value.is_null()
    {
        let presented = value
            .as_str()
            .ok_or_else(|| format!("{field} must be a string or null"))?;
        if presented.trim() != expected {
            return Err(format!(
                "authenticated token fixes {field}={expected:?}; caller override denied"
            ));
        }
    }
    object.insert(
        field.to_string(),
        serde_json::Value::String(expected.to_string()),
    );
    Ok(())
}

async fn handle_sse(State(state): State<AppState>) -> Response {
    use axum::response::sse::{Event, Sse};
    use std::convert::Infallible;

    state.status.record_sse();

    async fn event_stream()
    -> impl tokio_stream::Stream<Item = Result<Event, Infallible>> + Send + 'static {
        tokio_stream::iter(vec![Ok(Event::default()
            .event("message")
            .data(r#"{"jsonrpc":"2.0","method":"initialized","params":{} }"#))])
    }

    let stream = event_stream().await;
    Sse::new(stream).into_response()
}

async fn handle_health(
    State(state): State<AppState>,
    identity: Option<Extension<BoundIdentity>>,
) -> Response {
    state.status.record_health();
    if identity.is_some() {
        return Json(ScopedHealthReport {
            status: "ok",
            semantic_prewarm: state.status.semantic_prewarm_label(),
        })
        .into_response();
    }
    Json(state.status.snapshot()).into_response()
}

async fn handle_admin_status(
    State(state): State<AppState>,
    identity: Option<Extension<BoundIdentity>>,
) -> Response {
    state.status.record_admin_status();
    if identity.is_some() {
        return (
            StatusCode::FORBIDDEN,
            "source-bound tokens cannot inspect global server status",
        )
            .into_response();
    }
    Json(state.status.snapshot()).into_response()
}

#[derive(Debug, Clone)]
pub struct McpSub {
    pub port: Option<u16>,
    pub bind: Option<String>,
    pub auth_token: Option<String>,
    pub auth_token_file: Option<PathBuf>,
    pub auth_bindings_file: Option<PathBuf>,
    pub wiki_root: Option<PathBuf>,
}

pub fn mcp_serve_command() -> impl Parser<Command> {
    let port = long("port")
        .short('p')
        .help("Port to listen on (default: 3000)")
        .argument::<u16>("PORT")
        .optional();

    let bind = long("bind")
        .help(
            "Address to bind. Default 127.0.0.1 (loopback only — \
             same-host agents). Pass 0.0.0.0 to expose to the \
             network (multi-host); pair with --auth-token-file or \
             --auth-bindings-file whenever you do.",
        )
        .argument::<String>("ADDR")
        .optional();

    let auth_token = long("auth-token")
        .help(
            "Bearer token. When set, every request must include \
             `Authorization: Bearer <TOKEN>`. Falls back to \
             --auth-token-file, then AIDEMEMO_MCP_AUTH_TOKEN env. Required \
             for any non-loopback bind. Avoid passing on the command \
             line in production — use --auth-token-file or env var \
             so the secret doesn't land in shell history / `ps aux`.",
        )
        .argument::<String>("TOKEN")
        .optional();

    let auth_token_file = long("auth-token-file")
        .help(
            "Path to a file holding the bearer token (single line, \
             trimmed). Mode 0600 recommended. Use this instead of \
             --auth-token in production so the token doesn't appear \
             in shell history or `ps aux`.",
        )
        .argument::<PathBuf>("PATH")
        .optional();

    let auth_bindings_file = long("auth-bindings-file")
        .help(
            "JSON file containing bearer-token identity bindings. Each entry must provide \
             token, source_id, and actor_id. Authenticated MCP calls are pinned to that \
             identity and caller overrides are rejected. Falls back to \
             AIDEMEMO_MCP_AUTH_BINDINGS_FILE. Mode 0600 recommended.",
        )
        .argument::<PathBuf>("PATH")
        .optional();

    let wiki_root = positional::<PathBuf>("WIKI_ROOT")
        .help("Path to wiki root (uses store path if omitted)")
        .optional();

    construct!(McpSub {
        port,
        bind,
        auth_token,
        auth_token_file,
        auth_bindings_file,
        wiki_root,
    })
    .map(Command::McpServe)
    .to_options()
    .command("mcp-serve")
    .help("Start MCP server over HTTP + SSE (use `aidememo mcp` for stdio)")
}

pub fn run_mcp_serve(
    port: Option<u16>,
    bind: Option<String>,
    auth_token: Option<String>,
    auth_token_file: Option<PathBuf>,
    auth_bindings_file: Option<PathBuf>,
    store_path: PathBuf,
    config: Config,
) -> Result<String, aidememo_core::AideMemoError> {
    let port: u16 = port.unwrap_or(3000);
    // Default to loopback so a casual `aidememo mcp-serve` doesn't expose
    // the store on every network interface. Operators who want
    // multi-host explicitly pass `--bind 0.0.0.0`.
    let bind_addr = bind.unwrap_or_else(|| "127.0.0.1".to_string());

    let auth = resolve_auth_config(auth_token, auth_token_file, auth_bindings_file)?;
    let prewarm_semantic = should_prewarm_semantic(&config);
    let wiki = Arc::new(AideMemo::open(store_path.as_ref(), config)?);
    let auth_label = auth.label();
    let auth_enabled = auth.enabled();

    let runtime = tokio::runtime::Runtime::new().map_err(|e| {
        aidememo_core::AideMemoError::Internal(format!("failed to create runtime: {}", e))
    })?;

    runtime.block_on(async {
        // Bind using the socket tuple rather than string concatenation so
        // IPv6 literals such as `::1` are handled correctly. Authorisation is
        // decided from the address the OS actually bound, not from a hostname
        // spelling such as `localhost` whose resolver entry may be unsafe.
        let listener = tokio::net::TcpListener::bind((bind_addr.as_str(), port))
            .await
            .map_err(|e| aidememo_core::AideMemoError::Internal(format!("failed to bind: {e}")))?;
        let local_addr = listener.local_addr().map_err(|e| {
            aidememo_core::AideMemoError::Internal(format!(
                "failed to inspect bound server address: {e}"
            ))
        })?;
        if !local_addr.ip().is_loopback() && !auth_enabled {
            return Err(aidememo_core::AideMemoError::InvalidInput(format!(
                "non-loopback bind '{}' requires an auth token — pass \
                 --auth-token <SECRET>, --auth-bindings-file <PATH>, or set \
                 AIDEMEMO_MCP_AUTH_TOKEN / AIDEMEMO_MCP_AUTH_BINDINGS_FILE. \
                 Refusing to expose an unauthenticated store on the network.",
                local_addr
            )));
        }

        let state = AppState {
            wiki,
            status: Arc::new(ServerStatus {
                started_at_ms: aidememo_core::time::current_epoch_ms(),
                store_path: store_path.clone(),
                bind_addr: local_addr.ip().to_string(),
                port: local_addr.port(),
                auth_mode: auth_label,
                semantic_prewarm: AtomicU8::new(if prewarm_semantic {
                    PREWARM_WARMING
                } else {
                    PREWARM_DISABLED
                }),
                counters: RequestCounters::default(),
            }),
        };
        let auth_state: AuthState = Arc::new(auth);

        let app = build_router(state.clone(), auth_state, auth_enabled);

        tracing::info!(
            address = %local_addr,
            auth = auth_label,
            "aidememo mcp-serve: listening (POST /mcp, GET /sse, GET /health, GET /admin/status)"
        );

        if prewarm_semantic {
            spawn_semantic_prewarm(Arc::clone(&state.wiki), Arc::clone(&state.status));
        }

        axum::serve(listener, app)
            .await
            .map_err(|e| aidememo_core::AideMemoError::Internal(format!("server error: {}", e)))?;

        Ok::<(), aidememo_core::AideMemoError>(())
    })?;

    Ok("MCP server stopped".into())
}

fn should_prewarm_semantic(config: &Config) -> bool {
    config.search.auto_hybrid || env_bool("AIDEMEMO_PREWARM_SEMANTIC")
}

fn spawn_semantic_prewarm(wiki: Arc<AideMemo>, status: Arc<ServerStatus>) {
    tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        match wiki.semantic_prewarm() {
            Ok(()) => {
                status
                    .semantic_prewarm
                    .store(PREWARM_READY, Ordering::Relaxed);
                tracing::info!(
                    ms = started.elapsed().as_secs_f64() * 1000.0,
                    "semantic provider prewarmed"
                );
            }
            Err(err) => {
                status
                    .semantic_prewarm
                    .store(PREWARM_FAILED, Ordering::Relaxed);
                tracing::warn!(
                    error = %err,
                    "semantic provider prewarm failed; server will fall back on demand"
                );
            }
        }
    });
}

fn env_bool(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[derive(Debug, Deserialize)]
struct SyncSinceQuery {
    /// Last entity ULID the puller already has (inclusive lower bound).
    entity: Option<String>,
    /// Last fact ULID the puller already has.
    fact: Option<String>,
    /// Phase 2.5 — high-water `updated_at` for entities, drives the
    /// in-place updates pass (catches `entity_describe`, etc).
    entity_updated_at: Option<u64>,
    /// Stable tie-breaker for entities sharing `entity_updated_at`.
    entity_updated_id: Option<String>,
    /// Phase 2.5 — high-water `updated_at` for facts (catches
    /// `supersede`, `pin`, etc).
    fact_updated_at: Option<u64>,
    /// Stable tie-breaker for facts sharing `fact_updated_at`.
    fact_updated_id: Option<String>,
    /// High-water relation creation time and deterministic tie-breaker.
    relation_created_at: Option<u64>,
    relation_key: Option<String>,
    /// Full relation-snapshot digest and in-generation scan cursor.
    relation_generation: Option<String>,
    relation_scan_key: Option<String>,
    /// Cap on records returned in this batch. Default 5000.
    limit: Option<usize>,
    /// Include relations in the export. Default true.
    relations: Option<bool>,
}

async fn handle_sync_since(
    State(state): State<AppState>,
    identity: Option<Extension<BoundIdentity>>,
    Query(q): Query<SyncSinceQuery>,
) -> Response {
    state.status.record_sync_since();
    if identity.is_some() {
        return (
            StatusCode::FORBIDDEN,
            "source-bound tokens cannot export an unscoped store; use an unbound admin token",
        )
            .into_response();
    }

    let entity = match parse_sync_query_ulid(q.entity, "entity") {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let fact = match parse_sync_query_ulid(q.fact, "fact") {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let entity_updated_id = match parse_sync_query_ulid(q.entity_updated_id, "entity_updated_id") {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let fact_updated_id = match parse_sync_query_ulid(q.fact_updated_id, "fact_updated_id") {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let opts = aidememo_core::sync::SyncExportOpts {
        since: aidememo_core::sync::SyncCursor {
            entity: entity.map(aidememo_core::EntityId),
            fact: fact.map(aidememo_core::FactId),
            entity_updated_at: q.entity_updated_at,
            entity_updated_id: entity_updated_id.map(aidememo_core::EntityId),
            fact_updated_at: q.fact_updated_at,
            fact_updated_id: fact_updated_id.map(aidememo_core::FactId),
            relation_created_at: q.relation_created_at,
            relation_key: q.relation_key,
            relation_generation: q.relation_generation,
            relation_scan_key: q.relation_scan_key,
        },
        limit: q.limit.unwrap_or(5000),
        include_relations: q.relations.unwrap_or(true),
    };

    let mut buf: Vec<u8> = Vec::new();
    if let Err(e) = state.wiki.sync_export(opts, &mut buf) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("sync_export: {e}"),
        )
            .into_response();
    }
    // Plain text — JSONL isn't a registered MIME but `application/x-ndjson`
    // is the convention; pin it so curl + the `aidememo` client both recognise it.
    (
        StatusCode::OK,
        [("content-type", "application/x-ndjson")],
        buf,
    )
        .into_response()
}

fn parse_sync_query_ulid(raw: Option<String>, field: &str) -> Result<Option<ulid::Ulid>, String> {
    raw.map(|value| {
        ulid::Ulid::from_string(&value)
            .map_err(|source| format!("invalid {field} cursor ULID `{value}`: {source}"))
    })
    .transpose()
}

fn status_report(status: &ServerStatus) -> AdminStatusReport {
    let now = aidememo_core::time::current_epoch_ms();
    AdminStatusReport {
        status: "ok",
        store_path: status.store_path.display().to_string(),
        bind_addr: status.bind_addr.clone(),
        port: status.port,
        auth_mode: status.auth_mode,
        semantic_prewarm: status.semantic_prewarm_label(),
        started_at_ms: status.started_at_ms,
        uptime_ms: now.saturating_sub(status.started_at_ms),
        request_count: status.counters.total.load(Ordering::Relaxed),
        routes: RouteCounts {
            mcp: status.counters.mcp.load(Ordering::Relaxed),
            sse: status.counters.sse.load(Ordering::Relaxed),
            sync_since: status.counters.sync_since.load(Ordering::Relaxed),
            health: status.counters.health.load(Ordering::Relaxed),
            admin_status: status.counters.admin_status.load(Ordering::Relaxed),
        },
        sync: sync_status_report(&status.store_path, now),
    }
}

fn sync_status_report(store_path: &std::path::Path, now_ms: u64) -> SyncStatusReport {
    let cursor_path = sync_cursor_path(store_path);
    let exists = cursor_path.exists();
    let cursor = load_sync_cursor(&cursor_path);
    let mut remotes: Vec<SyncRemoteReport> = cursor
        .remotes
        .into_iter()
        .map(|(url, cursor)| SyncRemoteReport {
            url,
            entity: cursor.entity,
            fact: cursor.fact,
            entity_updated_at: cursor.entity_updated_at,
            entity_updated_id: cursor.entity_updated_id,
            fact_updated_at: cursor.fact_updated_at,
            fact_updated_id: cursor.fact_updated_id,
            relation_created_at: cursor.relation_created_at,
            relation_key: cursor.relation_key,
            relation_generation: cursor.relation_generation,
            relation_scan_key: cursor.relation_scan_key,
            last_pulled_at: cursor.last_pulled_at,
            age_ms: now_ms.saturating_sub(cursor.last_pulled_at),
        })
        .collect();
    remotes.sort_by(|a, b| a.url.cmp(&b.url));
    SyncStatusReport {
        cursor_file: cursor_path.display().to_string(),
        exists,
        remotes_count: remotes.len(),
        remotes,
    }
}

fn sync_cursor_path(store_path: &std::path::Path) -> std::path::PathBuf {
    let mut p = store_path.to_path_buf();
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wiki".into());
    p.set_file_name(format!("{stem}.sync.json"));
    p
}

fn load_sync_cursor(path: &std::path::Path) -> SyncCursorFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Read a bearer token from a file. Trims surrounding whitespace
/// (operators commonly `echo $TOKEN > file` which appends a newline)
/// and rejects empty contents so a misconfigured file doesn't get
/// silently accepted as "no auth".
pub(crate) fn read_token_file(path: &Path) -> Result<String, aidememo_core::AideMemoError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        aidememo_core::AideMemoError::InvalidInput(format!(
            "failed to read auth token file {}: {e}",
            path.display()
        ))
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(aidememo_core::AideMemoError::InvalidInput(format!(
            "auth token file {} is empty after trimming whitespace",
            path.display()
        )));
    }
    Ok(trimmed.to_string())
}

fn resolve_auth_config(
    auth_token: Option<String>,
    auth_token_file: Option<PathBuf>,
    auth_bindings_file: Option<PathBuf>,
) -> Result<AuthConfig, aidememo_core::AideMemoError> {
    if auth_bindings_file.is_some() && (auth_token.is_some() || auth_token_file.is_some()) {
        return Err(aidememo_core::AideMemoError::InvalidInput(
            "--auth-bindings-file cannot be combined with --auth-token or --auth-token-file"
                .to_string(),
        ));
    }

    if let Some(path) = auth_bindings_file {
        return read_token_bindings_file(&path).map(AuthConfig::Bound);
    }

    if let Some(token) = auth_token {
        return normalise_inline_token(token, "--auth-token").map(AuthConfig::Single);
    }

    if let Some(path) = auth_token_file {
        return read_token_file(&path).map(AuthConfig::Single);
    }

    if let Some(path) = std::env::var_os("AIDEMEMO_MCP_AUTH_BINDINGS_FILE") {
        return read_token_bindings_file(Path::new(&path)).map(AuthConfig::Bound);
    }

    match std::env::var("AIDEMEMO_MCP_AUTH_TOKEN") {
        Ok(token) => {
            normalise_inline_token(token, "AIDEMEMO_MCP_AUTH_TOKEN").map(AuthConfig::Single)
        }
        Err(std::env::VarError::NotPresent) => Ok(AuthConfig::None),
        Err(err) => Err(aidememo_core::AideMemoError::InvalidInput(format!(
            "failed to read AIDEMEMO_MCP_AUTH_TOKEN: {err}"
        ))),
    }
}

fn normalise_inline_token(
    token: String,
    origin: &str,
) -> Result<String, aidememo_core::AideMemoError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(aidememo_core::AideMemoError::InvalidInput(format!(
            "{origin} is empty after trimming whitespace"
        )));
    }
    Ok(token.to_string())
}

fn read_token_bindings_file(
    path: &Path,
) -> Result<HashMap<String, BoundIdentity>, aidememo_core::AideMemoError> {
    let raw = std::fs::read_to_string(path).map_err(|err| {
        aidememo_core::AideMemoError::InvalidInput(format!(
            "failed to read auth bindings file {}: {err}",
            path.display()
        ))
    })?;
    let document: TokenBindingsDocument = serde_json::from_str(&raw).map_err(|err| {
        aidememo_core::AideMemoError::InvalidInput(format!(
            "failed to parse auth bindings file {}: {err}",
            path.display()
        ))
    })?;
    let entries = match document {
        TokenBindingsDocument::Object { tokens } | TokenBindingsDocument::Array(tokens) => tokens,
    };
    if entries.is_empty() {
        return Err(aidememo_core::AideMemoError::InvalidInput(format!(
            "auth bindings file {} contains no token bindings",
            path.display()
        )));
    }

    let mut bindings = HashMap::with_capacity(entries.len());
    for (index, entry) in entries.into_iter().enumerate() {
        let token = normalise_binding_value(entry.token, "token", index, path)?;
        let source_id = normalise_binding_value(entry.source_id, "source_id", index, path)?;
        let actor_id = normalise_binding_value(entry.actor_id, "actor_id", index, path)?;
        if bindings
            .insert(
                token,
                BoundIdentity {
                    source_id,
                    actor_id,
                },
            )
            .is_some()
        {
            return Err(aidememo_core::AideMemoError::InvalidInput(format!(
                "auth bindings file {} contains a duplicate token at entry {index}",
                path.display()
            )));
        }
    }
    Ok(bindings)
}

fn normalise_binding_value(
    value: String,
    field: &str,
    index: usize,
    path: &Path,
) -> Result<String, aidememo_core::AideMemoError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(aidememo_core::AideMemoError::InvalidInput(format!(
            "auth bindings file {} entry {index} has an empty {field}",
            path.display()
        )));
    }
    Ok(value.to_string())
}

async fn require_bearer(State(auth): State<AuthState>, mut req: Request, next: Next) -> Response {
    let header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let presented = header.strip_prefix("Bearer ").unwrap_or("");
    // Constant-time compare via subtle isn't worth a dep here; the
    // tokens we accept are fixed-size, attacker-controlled inputs are
    // small, and HTTPS termination should sit at a reverse proxy.
    match auth.authenticate(presented) {
        Some(AuthenticatedPrincipal::Unbound) => next.run(req).await,
        Some(AuthenticatedPrincipal::Bound(identity)) => {
            req.extensions_mut().insert(identity);
            next.run(req).await
        }
        None => (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_status_report_loads_cursor_file_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("team.sqlite");
        let cursor = dir.path().join("team.sync.json");
        std::fs::write(
            &cursor,
            r#"{
              "remotes": {
                "http://team-host:3000": {
                  "entity": "01H00000000000000000000000",
                  "fact": "01H00000000000000000000001",
                  "entity_updated_at": 1000,
                  "fact_updated_at": 2000,
                  "last_pulled_at": 3000
                }
              }
            }"#,
        )
        .unwrap();

        let report = sync_status_report(&store, 4500);

        assert_eq!(report.cursor_file, cursor.display().to_string());
        assert!(report.exists);
        assert_eq!(report.remotes_count, 1);
        assert_eq!(report.remotes[0].url, "http://team-host:3000");
        assert_eq!(report.remotes[0].age_ms, 1500);
    }

    #[test]
    fn status_report_counts_routes_and_reports_auth_mode() {
        let dir = tempfile::tempdir().unwrap();
        let status = ServerStatus {
            started_at_ms: aidememo_core::time::current_epoch_ms(),
            store_path: dir.path().join("wiki.sqlite"),
            bind_addr: "127.0.0.1".to_string(),
            port: 3000,
            auth_mode: "bearer",
            semantic_prewarm: AtomicU8::new(PREWARM_WARMING),
            counters: RequestCounters::default(),
        };

        status.record_health();
        status.record_mcp();
        status.record_mcp();
        let report = status.snapshot();

        assert_eq!(report.status, "ok");
        assert_eq!(report.auth_mode, "bearer");
        assert_eq!(report.semantic_prewarm, "warming");
        assert_eq!(report.request_count, 3);
        assert_eq!(report.routes.health, 1);
        assert_eq!(report.routes.mcp, 2);
    }

    #[test]
    fn token_bindings_file_accepts_wrapped_entries_and_trims_identity_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bindings.json");
        std::fs::write(
            &path,
            r#"{"tokens":[{"token":" alpha-token ","source_id":" project:alpha ","actor_id":" codex:a "}]}"#,
        )
        .unwrap();

        let bindings = read_token_bindings_file(&path).unwrap();
        let identity = bindings.get("alpha-token").unwrap();
        assert_eq!(identity.source_id, "project:alpha");
        assert_eq!(identity.actor_id, "codex:a");
    }

    #[test]
    fn token_bindings_file_rejects_duplicate_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bindings.json");
        std::fs::write(
            &path,
            r#"[
              {"token":"same","source_id":"alpha","actor_id":"codex"},
              {"token":"same","source_id":"beta","actor_id":"claude"}
            ]"#,
        )
        .unwrap();

        let err = read_token_bindings_file(&path).err().unwrap();
        assert!(err.to_string().contains("duplicate token"));
    }

    #[test]
    fn bound_identity_is_injected_and_caller_override_is_rejected() {
        let identity = BoundIdentity {
            source_id: "project:alpha".to_string(),
            actor_id: "codex:a".to_string(),
        };
        let mut request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "aidememo_fact_add",
                "arguments": {"content": "remember this"}
            })),
        };

        bind_request_identity(&mut request, &identity).unwrap();
        let arguments = request
            .params
            .as_ref()
            .and_then(|params| params.get("arguments"))
            .unwrap();
        assert_eq!(arguments["source_id"], "project:alpha");
        assert_eq!(arguments["actor_id"], "codex:a");

        let mut override_request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(2),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "aidememo_search",
                "arguments": {"query": "secret", "source_id": "project:beta"}
            })),
        };
        let err = bind_request_identity(&mut override_request, &identity)
            .err()
            .unwrap();
        assert!(err.contains("caller override denied"));
    }

    #[test]
    fn bound_identity_rejects_nested_batch_override() {
        let identity = BoundIdentity {
            source_id: "project:alpha".to_string(),
            actor_id: "codex:a".to_string(),
        };
        let mut request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(3),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "aidememo_fact_add_many",
                "arguments": {
                    "items": [{"content": "leak", "actor_id": "claude:b"}]
                }
            })),
        };

        let err = bind_request_identity(&mut request, &identity)
            .err()
            .unwrap();
        assert!(err.contains("actor_id"));
        assert!(err.contains("caller override denied"));
    }

    fn bound_route_test_state(dir: &tempfile::TempDir) -> AppState {
        let store_path = dir.path().join("wiki.sqlite");
        AppState {
            wiki: Arc::new(AideMemo::open(&store_path, Config::default()).unwrap()),
            status: Arc::new(ServerStatus {
                started_at_ms: aidememo_core::time::current_epoch_ms(),
                store_path,
                bind_addr: "127.0.0.1".to_string(),
                port: 3000,
                auth_mode: "bearer-bound",
                semantic_prewarm: AtomicU8::new(PREWARM_READY),
                counters: RequestCounters::default(),
            }),
        }
    }

    fn bound_route_identity() -> Extension<BoundIdentity> {
        Extension(BoundIdentity {
            source_id: "project:alpha".to_string(),
            actor_id: "codex:a".to_string(),
        })
    }

    fn bound_test_auth() -> AuthState {
        Arc::new(AuthConfig::Bound(HashMap::from([
            (
                "alpha-token".to_string(),
                BoundIdentity {
                    source_id: "alpha".to_string(),
                    actor_id: "actor-alpha".to_string(),
                },
            ),
            (
                "beta-token".to_string(),
                BoundIdentity {
                    source_id: "beta".to_string(),
                    actor_id: "actor-beta".to_string(),
                },
            ),
        ])))
    }

    async fn test_http_request(
        base_url: &str,
        path: &str,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> (u16, String) {
        let url = format!("{base_url}{path}");
        let token = token.to_string();
        tokio::task::spawn_blocking(move || {
            let request = match body.as_ref() {
                Some(_) => ureq::post(&url),
                None => ureq::get(&url),
            }
            .set("Authorization", &format!("Bearer {token}"));
            let response = match body {
                Some(body) => request.send_json(body),
                None => request.call(),
            };
            match response {
                Ok(response) => {
                    let status = response.status();
                    let body = response.into_string().unwrap_or_default();
                    (status, body)
                }
                Err(ureq::Error::Status(status, response)) => {
                    let body = response.into_string().unwrap_or_default();
                    (status, body)
                }
                Err(error) => panic!("test HTTP request failed: {error}"),
            }
        })
        .await
        .unwrap()
    }

    async fn call_bound_tool(
        base_url: &str,
        token: &str,
        id: u64,
        name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let (status, body) = test_http_request(
            base_url,
            "/mcp",
            token,
            Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK.as_u16(), "response body: {body}");
        serde_json::from_str(&body).unwrap()
    }

    fn tool_payload(response: &serde_json::Value) -> serde_json::Value {
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool response text");
        serde_json::from_str(text).expect("JSON tool response text")
    }

    fn assert_bound_override_denied(response: &serde_json::Value, field: &str) {
        assert_eq!(response["error"]["code"], -32602);
        let message = response["error"]["message"]
            .as_str()
            .expect("JSON-RPC error message");
        assert!(message.contains(field), "unexpected message: {message}");
        assert!(
            message.contains("caller override denied"),
            "unexpected message: {message}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bound_token_router_enforces_identity_and_source_isolation_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let state = bound_route_test_state(&dir);
        let app = build_router(state.clone(), bound_test_auth(), true);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let base_url = format!("http://{address}");

        let alpha_add = call_bound_tool(
            &base_url,
            "alpha-token",
            1,
            "aidememo_fact_add",
            serde_json::json!({
                "content": "same content in two bound sources",
                "dedup_check": false,
            }),
        )
        .await;
        let alpha_payload = tool_payload(&alpha_add);
        assert_eq!(alpha_payload["source_id"], "alpha");
        assert_eq!(alpha_payload["actor_id"], "actor-alpha");
        let alpha_id = alpha_payload["id"].as_str().unwrap().to_string();

        let beta_add = call_bound_tool(
            &base_url,
            "beta-token",
            2,
            "aidememo_fact_add",
            serde_json::json!({
                "content": "same content in two bound sources",
                "dedup_check": false,
            }),
        )
        .await;
        let beta_payload = tool_payload(&beta_add);
        assert_eq!(beta_payload["source_id"], "beta");
        assert_eq!(beta_payload["actor_id"], "actor-beta");
        assert_ne!(alpha_id, beta_payload["id"].as_str().unwrap());

        let beta_get_alpha = call_bound_tool(
            &base_url,
            "beta-token",
            3,
            "aidememo_fact_get",
            serde_json::json!({"id": alpha_id}),
        )
        .await;
        assert_eq!(beta_get_alpha["result"]["isError"], true);
        assert_eq!(tool_payload(&beta_get_alpha)["error_kind"], "not_found");

        let top_level_source = call_bound_tool(
            &base_url,
            "alpha-token",
            4,
            "aidememo_fact_add_many",
            serde_json::json!({
                "source_id": "beta",
                "items": [{"content": "top-level source override"}],
            }),
        )
        .await;
        assert_bound_override_denied(&top_level_source, "source_id");

        let top_level_actor = call_bound_tool(
            &base_url,
            "alpha-token",
            5,
            "aidememo_fact_add_many",
            serde_json::json!({
                "actor_id": "actor-beta",
                "items": [{"content": "top-level actor override"}],
            }),
        )
        .await;
        assert_bound_override_denied(&top_level_actor, "actor_id");

        let nested_source = call_bound_tool(
            &base_url,
            "alpha-token",
            6,
            "aidememo_fact_add_many",
            serde_json::json!({
                "items": [{
                    "content": "nested source override",
                    "source_id": "beta",
                }],
            }),
        )
        .await;
        assert_bound_override_denied(&nested_source, "source_id");

        let nested_actor = call_bound_tool(
            &base_url,
            "alpha-token",
            7,
            "aidememo_fact_add_many",
            serde_json::json!({
                "items": [{
                    "content": "nested actor override",
                    "actor_id": "actor-beta",
                }],
            }),
        )
        .await;
        assert_bound_override_denied(&nested_actor, "actor_id");
        assert_eq!(state.wiki.stats().unwrap().fact_count, 2);

        let (health_status, health_body) =
            test_http_request(&base_url, "/health", "alpha-token", None).await;
        assert_eq!(health_status, StatusCode::OK.as_u16());
        let health: serde_json::Value = serde_json::from_str(&health_body).unwrap();
        assert_eq!(health["status"], "ok");
        assert_eq!(health["semantic_prewarm"], "ready");
        assert_eq!(health.as_object().unwrap().len(), 2);
        assert!(health.get("store_path").is_none());
        assert!(health.get("sync").is_none());

        let (admin_status, _) =
            test_http_request(&base_url, "/admin/status", "alpha-token", None).await;
        assert_eq!(admin_status, StatusCode::FORBIDDEN.as_u16());
        let (sync_status, _) =
            test_http_request(&base_url, "/sync/since", "alpha-token", None).await;
        assert_eq!(sync_status, StatusCode::FORBIDDEN.as_u16());

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn bound_token_health_omits_global_status_and_admin_is_forbidden() {
        let dir = tempfile::tempdir().unwrap();
        let state = bound_route_test_state(&dir);

        let health = handle_health(State(state.clone()), Some(bound_route_identity())).await;
        assert_eq!(health.status(), StatusCode::OK);
        let body = axum::body::to_bytes(health.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["semantic_prewarm"], "ready");
        assert!(value.get("store_path").is_none());
        assert!(value.get("sync").is_none());

        let admin = handle_admin_status(State(state), Some(bound_route_identity())).await;
        assert_eq!(admin.status(), StatusCode::FORBIDDEN);

        let sync_state = bound_route_test_state(&dir);
        let sync = handle_sync_since(
            State(sync_state),
            Some(bound_route_identity()),
            Query(SyncSinceQuery {
                entity: None,
                fact: None,
                entity_updated_at: None,
                entity_updated_id: None,
                fact_updated_at: None,
                fact_updated_id: None,
                relation_created_at: None,
                relation_key: None,
                relation_generation: None,
                relation_scan_key: None,
                limit: None,
                relations: None,
            }),
        )
        .await;
        assert_eq!(sync.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn semantic_prewarm_follows_auto_hybrid_config() {
        let mut config = Config::default();
        config.search.auto_hybrid = false;
        assert!(!should_prewarm_semantic(&config));

        config.search.auto_hybrid = true;
        assert!(should_prewarm_semantic(&config));
    }
}
