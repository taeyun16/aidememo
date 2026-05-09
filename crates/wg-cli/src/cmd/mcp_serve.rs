//! `wg mcp-serve` — MCP server over HTTP + SSE.
//!
//! Speaks MCP JSON-RPC 2.0 over an HTTP POST endpoint (`/mcp`) plus an SSE
//! endpoint (`/sse`) for browser-based or remote clients. For local agents
//! (Claude Code, Codex CLI), prefer `wg mcp` (stdio transport) instead.
//!
//! Usage:
//!   wg mcp-serve --port 3000

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bpaf::*;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::cmd::mcp_tools::{JsonRpcRequest, JsonRpcResponse, dispatch};
use crate::{Config, WikiGraph, cmd::Command};

type SharedState = Arc<RwLock<Option<WikiGraph>>>;

async fn handle_post(
    State(state): State<SharedState>,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    let guard = state.read().await;
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

async fn handle_sse(State(_state): State<SharedState>) -> Response {
    use axum::response::sse::{Event, Sse};
    use std::convert::Infallible;

    async fn event_stream()
    -> impl tokio_stream::Stream<Item = Result<Event, Infallible>> + Send + 'static {
        tokio_stream::iter(vec![Ok(Event::default()
            .event("message")
            .data(r#"{"jsonrpc":"2.0","method":"initialized","params":{} }"#))])
    }

    let stream = event_stream().await;
    Sse::new(stream).into_response()
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
             --auth-token-file, then WG_MCP_AUTH_TOKEN env. Required \
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
    .help("Start MCP server over HTTP + SSE (use `wg mcp` for stdio)")
}

pub fn run_mcp_serve(
    port: Option<u16>,
    bind: Option<String>,
    auth_token: Option<String>,
    auth_token_file: Option<PathBuf>,
    wiki_root: Option<PathBuf>,
) -> Result<String, wg_core::WgError> {
    let config = Config::load().unwrap_or_default();
    let store_path = match wiki_root {
        Some(p) => p,
        None => PathBuf::from(&config.store.path),
    };

    let port: u16 = port.unwrap_or(3000);
    // Default to loopback so a casual `wg mcp-serve` doesn't expose
    // the store on every network interface. Operators who want
    // multi-host explicitly pass `--bind 0.0.0.0`.
    let bind_addr = bind.unwrap_or_else(|| "127.0.0.1".to_string());
    let addr = format!("{}:{}", bind_addr, port);

    // Auth token resolution: --auth-token > --auth-token-file >
    // WG_MCP_AUTH_TOKEN env > None (loopback only).
    let token = auth_token
        .or_else(|| {
            auth_token_file
                .as_ref()
                .map(|p| read_token_file(p))
                .transpose()
                .ok()
                .flatten()
        })
        .or_else(|| std::env::var("WG_MCP_AUTH_TOKEN").ok());
    let is_loopback = bind_addr == "127.0.0.1" || bind_addr == "::1" || bind_addr == "localhost";
    if !is_loopback && token.is_none() {
        return Err(wg_core::WgError::InvalidInput(format!(
            "non-loopback bind '{}' requires an auth token — pass \
             --auth-token <SECRET> or set WG_MCP_AUTH_TOKEN. \
             Refusing to expose an unauthenticated store on the network.",
            bind_addr
        )));
    }

    let wiki = WikiGraph::open(store_path.as_ref(), config)?;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| wg_core::WgError::Internal(format!("failed to create runtime: {}", e)))?;

    runtime.block_on(async {
        let state: SharedState = Arc::new(RwLock::new(Some(wiki)));
        let auth_state: AuthState = Arc::new(token.clone());

        let mut app = Router::new()
            .route("/mcp", post(handle_post))
            .route("/sse", get(handle_sse))
            .route("/sync/since", get(handle_sync_since))
            .route("/health", get(|| async { "ok" }))
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
            "wg mcp-serve: listening ({}) (POST /mcp, GET /sse, GET /health)",
            auth_label
        );

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| wg_core::WgError::Internal(format!("failed to bind: {}", e)))?;

        axum::serve(listener, app)
            .await
            .map_err(|e| wg_core::WgError::Internal(format!("server error: {}", e)))?;

        Ok::<(), wg_core::WgError>(())
    })?;

    Ok("MCP server stopped".into())
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
    State(state): State<SharedState>,
    Query(q): Query<SyncSinceQuery>,
) -> Response {
    let guard = state.read().await;
    let wiki = match guard.as_ref() {
        Some(w) => w,
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "wiki not initialized").into_response();
        }
    };

    let parse_ulid = |raw: Option<String>| -> Option<ulid::Ulid> {
        raw.as_deref().and_then(|s| ulid::Ulid::from_string(s).ok())
    };
    let opts = wg_core::sync::SyncExportOpts {
        since: wg_core::sync::SyncCursor {
            entity: parse_ulid(q.entity).map(wg_core::EntityId),
            fact: parse_ulid(q.fact).map(wg_core::FactId),
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
    // is the convention; pin it so curl + the wg client both recognise it.
    (
        StatusCode::OK,
        [("content-type", "application/x-ndjson")],
        buf,
    )
        .into_response()
}

/// Read a bearer token from a file. Trims surrounding whitespace
/// (operators commonly `echo $TOKEN > file` which appends a newline)
/// and rejects empty contents so a misconfigured file doesn't get
/// silently accepted as "no auth".
pub(crate) fn read_token_file(path: &std::path::Path) -> Result<String, wg_core::WgError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        wg_core::WgError::InvalidInput(format!(
            "failed to read auth token file {}: {e}",
            path.display()
        ))
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(wg_core::WgError::InvalidInput(format!(
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
