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
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bpaf::*;
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
    pub wiki_root: Option<PathBuf>,
}

pub fn mcp_serve_command() -> impl Parser<Command> {
    let port = long("port")
        .short('p')
        .help("Port to listen on (default: 3000)")
        .argument::<u16>("PORT")
        .optional();

    let wiki_root = positional::<PathBuf>("WIKI_ROOT")
        .help("Path to wiki root (uses store path if omitted)")
        .optional();

    construct!(McpSub { port, wiki_root })
        .map(Command::McpServe)
        .to_options()
        .command("mcp-serve")
        .help("Start MCP server over HTTP + SSE (use `wg mcp` for stdio)")
}

pub fn run_mcp_serve(
    port: Option<u16>,
    wiki_root: Option<PathBuf>,
) -> Result<String, wg_core::WgError> {
    let config = Config::load().unwrap_or_default();
    let store_path = match wiki_root {
        Some(p) => p,
        None => PathBuf::from(&config.store.path),
    };

    let port: u16 = port.unwrap_or(3000);
    let wiki = WikiGraph::open(store_path.as_ref(), config)?;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| wg_core::WgError::Internal(format!("failed to create runtime: {}", e)))?;

    runtime.block_on(async {
        let state: SharedState = Arc::new(RwLock::new(Some(wiki)));

        let app = Router::new()
            .route("/mcp", post(handle_post))
            .route("/sse", get(handle_sse))
            .route("/health", get(|| async { "ok" }))
            .with_state(state);

        let addr = format!("0.0.0.0:{}", port);
        tracing::info!(%addr, "wg mcp-serve: listening (POST /mcp, GET /sse, GET /health)");

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
