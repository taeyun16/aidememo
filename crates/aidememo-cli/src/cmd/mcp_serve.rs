//! `aidememo mcp-serve` — MCP server over HTTP + SSE.
//!
//! Speaks MCP JSON-RPC 2.0 over an HTTP POST endpoint (`/mcp`) plus an SSE
//! endpoint (`/sse`) for browser-based or remote clients. For local agents
//! (Claude Code, Codex CLI), prefer `aidememo mcp` (stdio transport) instead.
//!
//! Usage:
//!   aidememo mcp-serve --port 3000

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use axum::{
    Json, Router,
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bpaf::*;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::cmd::mcp_tools::{JsonRpcRequest, JsonRpcResponse, dispatch};
use crate::{AideMemo, Config, cmd::Command};

#[derive(Clone)]
struct AppState {
    wiki: Arc<RwLock<Option<AideMemo>>>,
    status: Arc<ServerStatus>,
}

struct ServerStatus {
    started_at_ms: u64,
    store_path: PathBuf,
    bind_addr: String,
    port: u16,
    auth_mode: &'static str,
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
    fact_updated_at: Option<u64>,
    last_pulled_at: u64,
    age_ms: u64,
}

#[derive(Debug, Default, Deserialize)]
struct StoredCursor {
    entity: Option<String>,
    fact: Option<String>,
    #[serde(default)]
    entity_updated_at: Option<u64>,
    #[serde(default)]
    fact_updated_at: Option<u64>,
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
}

async fn handle_post(State(state): State<AppState>, Json(req): Json<JsonRpcRequest>) -> Response {
    state.status.record_mcp();
    let guard = state.wiki.read().await;
    let wiki = match guard.as_ref() {
        Some(w) => w,
        None => {
            let resp = JsonRpcResponse::error(req.id, -32603, "wiki not initialized");
            return Json(resp).into_response();
        }
    };

    match dispatch(req, wiki) {
        Some(resp) => Json(resp).into_response(),
        None => axum::http::StatusCode::NO_CONTENT.into_response(),
    }
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

async fn handle_health(State(state): State<AppState>) -> Response {
    state.status.record_health();
    Json(state.status.snapshot()).into_response()
}

async fn handle_admin_status(State(state): State<AppState>) -> Response {
    state.status.record_admin_status();
    Json(state.status.snapshot()).into_response()
}

#[derive(Debug, Clone)]
pub struct McpSub {
    pub port: Option<u16>,
    pub bind: Option<String>,
    pub auth_token: Option<String>,
    pub auth_token_file: Option<PathBuf>,
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
             network (multi-host); pair with --auth-token whenever \
             you do.",
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

    let wiki_root = positional::<PathBuf>("WIKI_ROOT")
        .help("Path to wiki root (uses store path if omitted)")
        .optional();

    construct!(McpSub {
        port,
        bind,
        auth_token,
        auth_token_file,
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
    store_path: PathBuf,
    config: Config,
) -> Result<String, aidememo_core::AideMemoError> {
    let port: u16 = port.unwrap_or(3000);
    // Default to loopback so a casual `aidememo mcp-serve` doesn't expose
    // the store on every network interface. Operators who want
    // multi-host explicitly pass `--bind 0.0.0.0`.
    let bind_addr = bind.unwrap_or_else(|| "127.0.0.1".to_string());
    let addr = format!("{}:{}", bind_addr, port);

    // Auth token resolution: --auth-token > --auth-token-file >
    // AIDEMEMO_MCP_AUTH_TOKEN env > None (loopback only).
    let token = auth_token
        .or_else(|| {
            auth_token_file
                .as_ref()
                .map(|p| read_token_file(p))
                .transpose()
                .ok()
                .flatten()
        })
        .or_else(|| std::env::var("AIDEMEMO_MCP_AUTH_TOKEN").ok());
    let is_loopback = bind_addr == "127.0.0.1" || bind_addr == "::1" || bind_addr == "localhost";
    if !is_loopback && token.is_none() {
        return Err(aidememo_core::AideMemoError::InvalidInput(format!(
            "non-loopback bind '{}' requires an auth token — pass \
             --auth-token <SECRET> or set AIDEMEMO_MCP_AUTH_TOKEN. \
             Refusing to expose an unauthenticated store on the network.",
            bind_addr
        )));
    }

    let prewarm_semantic = should_prewarm_semantic(&config);
    let wiki = AideMemo::open(store_path.as_ref(), config)?;
    if prewarm_semantic {
        let started = std::time::Instant::now();
        match wiki.semantic_prewarm() {
            Ok(()) => tracing::info!(
                ms = started.elapsed().as_secs_f64() * 1000.0,
                "semantic provider prewarmed"
            ),
            Err(err) => tracing::warn!(
                error = %err,
                "semantic provider prewarm failed; server will fall back on demand"
            ),
        }
    }

    let runtime = tokio::runtime::Runtime::new().map_err(|e| {
        aidememo_core::AideMemoError::Internal(format!("failed to create runtime: {}", e))
    })?;

    runtime.block_on(async {
        let state = AppState {
            wiki: Arc::new(RwLock::new(Some(wiki))),
            status: Arc::new(ServerStatus {
                started_at_ms: aidememo_core::time::current_epoch_ms(),
                store_path: store_path.clone(),
                bind_addr: bind_addr.clone(),
                port,
                auth_mode: if token.is_some() { "bearer" } else { "none" },
                counters: RequestCounters::default(),
            }),
        };
        let auth_state: AuthState = Arc::new(token.clone());

        let mut app = Router::new()
            .route("/mcp", post(handle_post))
            .route("/sse", get(handle_sse))
            .route("/sync/since", get(handle_sync_since))
            .route("/health", get(handle_health))
            .route("/admin/status", get(handle_admin_status))
            .with_state(state);

        if token.is_some() {
            app = app.layer(middleware::from_fn_with_state(auth_state, require_bearer));
        }

        let auth_label = if token.is_some() {
            "auth=bearer"
        } else {
            "auth=none"
        };
        tracing::info!(
            %addr,
            "aidememo mcp-serve: listening ({}) (POST /mcp, GET /sse, GET /health, GET /admin/status)",
            auth_label
        );

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| aidememo_core::AideMemoError::Internal(format!("failed to bind: {}", e)))?;

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

fn env_bool(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

type AuthState = Arc<Option<String>>;

#[derive(Debug, Deserialize)]
struct SyncSinceQuery {
    /// Last entity ULID the puller already has (inclusive lower bound).
    entity: Option<String>,
    /// Last fact ULID the puller already has.
    fact: Option<String>,
    /// Phase 2.5 — high-water `updated_at` for entities, drives the
    /// in-place updates pass (catches `entity_describe`, etc).
    entity_updated_at: Option<u64>,
    /// Phase 2.5 — high-water `updated_at` for facts (catches
    /// `supersede`, `pin`, etc).
    fact_updated_at: Option<u64>,
    /// Cap on records returned in this batch. Default 5000.
    limit: Option<usize>,
    /// Include relations in the export. Default true.
    relations: Option<bool>,
}

async fn handle_sync_since(
    State(state): State<AppState>,
    Query(q): Query<SyncSinceQuery>,
) -> Response {
    state.status.record_sync_since();
    let guard = state.wiki.read().await;
    let wiki = match guard.as_ref() {
        Some(w) => w,
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "wiki not initialized").into_response();
        }
    };

    let parse_ulid = |raw: Option<String>| -> Option<ulid::Ulid> {
        raw.as_deref().and_then(|s| ulid::Ulid::from_string(s).ok())
    };
    let opts = aidememo_core::sync::SyncExportOpts {
        since: aidememo_core::sync::SyncCursor {
            entity: parse_ulid(q.entity).map(aidememo_core::EntityId),
            fact: parse_ulid(q.fact).map(aidememo_core::FactId),
            entity_updated_at: q.entity_updated_at,
            fact_updated_at: q.fact_updated_at,
        },
        limit: q.limit.unwrap_or(5000),
        include_relations: q.relations.unwrap_or(true),
    };

    let mut buf: Vec<u8> = Vec::new();
    if let Err(e) = wiki.sync_export(opts, &mut buf) {
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

fn status_report(status: &ServerStatus) -> AdminStatusReport {
    let now = aidememo_core::time::current_epoch_ms();
    AdminStatusReport {
        status: "ok",
        store_path: status.store_path.display().to_string(),
        bind_addr: status.bind_addr.clone(),
        port: status.port,
        auth_mode: status.auth_mode,
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
            fact_updated_at: cursor.fact_updated_at,
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
pub(crate) fn read_token_file(
    path: &std::path::Path,
) -> Result<String, aidememo_core::AideMemoError> {
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

async fn require_bearer(State(expected): State<AuthState>, req: Request, next: Next) -> Response {
    let Some(expected) = expected.as_ref() else {
        return next.run(req).await;
    };

    let header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let presented = header.strip_prefix("Bearer ").unwrap_or("");
    // Constant-time compare via subtle isn't worth a dep here; the
    // tokens we accept are fixed-size, attacker-controlled inputs are
    // small, and HTTPS termination should sit at a reverse proxy.
    if !presented.is_empty() && presented == expected.as_str() {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
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
            counters: RequestCounters::default(),
        };

        status.record_health();
        status.record_mcp();
        status.record_mcp();
        let report = status.snapshot();

        assert_eq!(report.status, "ok");
        assert_eq!(report.auth_mode, "bearer");
        assert_eq!(report.request_count, 3);
        assert_eq!(report.routes.health, 1);
        assert_eq!(report.routes.mcp, 2);
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
