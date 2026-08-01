//! `aidememo mcp` — MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! This is the transport used by local agents that spawn the server as a
//! subprocess: Claude Code (`claude mcp add aidememo -- aidememo --backend libsqlite mcp`),
//! OpenAI Codex CLI (`[mcp_servers.aidememo] command = "aidememo"
//! args = ["--backend", "libsqlite", "mcp"]`), and any other client that follows the MCP
//! stdio convention.
//!
//! Protocol:
//! - Each request is one JSON object on a single line read from stdin.
//! - Each response is one JSON object on a single line written to stdout.
//! - Logs and diagnostics go to stderr (never stdout — that channel is
//!   reserved for protocol traffic).

use std::path::PathBuf;

use bpaf::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::cmd::mcp_tools::{JsonRpcRequest, JsonRpcResponse, dispatch_with_remote_profile};
use crate::{AideMemo, Config, cmd::Command};

#[derive(Debug, Clone)]
pub struct McpStdioSub {
    pub remote_profile: Option<String>,
    pub wiki_root: Option<PathBuf>,
}

pub fn mcp_command() -> impl Parser<Command> {
    let remote_profile = long("remote-profile")
        .help("Route handoff tools through a named authenticated remote SSOT profile")
        .argument::<String>("NAME")
        .optional();
    let wiki_root = positional::<PathBuf>("WIKI_ROOT")
        .help("Path to wiki root (uses store path if omitted)")
        .optional();

    construct!(McpStdioSub {
        remote_profile,
        wiki_root,
    })
    .map(Command::Mcp)
    .to_options()
    .command("mcp")
    .help("Start MCP server over stdio (for Claude Code / Codex CLI)")
}

pub fn run_mcp(
    store_path: PathBuf,
    config: Config,
    remote_profile: Option<String>,
) -> Result<String, aidememo_core::AideMemoError> {
    let remote_profile = remote_profile.or_else(|| {
        std::env::var("AIDEMEMO_REMOTE_PROFILE")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    });
    if let Some(profile) = remote_profile.as_deref() {
        let authenticated_actor = crate::cmd::remote_handoff::actor_id_for_profile(profile)?;
        let configured_actor = std::env::var("AIDEMEMO_ACTOR_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                aidememo_core::AideMemoError::InvalidInput(
                    "remote MCP requires AIDEMEMO_ACTOR_ID; use `aidememo mcp-install --remote-profile <NAME>` to derive it from the bearer binding"
                        .to_owned(),
                )
            })?;
        if configured_actor != authenticated_actor {
            return Err(aidememo_core::AideMemoError::InvalidInput(format!(
                "AIDEMEMO_ACTOR_ID {configured_actor:?} does not match remote profile actor {authenticated_actor:?}"
            )));
        }
    }
    let wiki = AideMemo::open(store_path.as_ref(), config)?;

    let runtime = tokio::runtime::Runtime::new().map_err(|e| {
        aidememo_core::AideMemoError::Internal(format!("failed to create runtime: {}", e))
    })?;

    runtime.block_on(async move {
        tracing::info!(store = %store_path.display(), "aidememo mcp: stdio transport ready");

        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();
        let mut stdout = tokio::io::stdout();

        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| aidememo_core::AideMemoError::Internal(format!("stdin read: {}", e)))?
        {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let response = match serde_json::from_str::<JsonRpcRequest>(line) {
                Ok(req) => dispatch_with_remote_profile(req, &wiki, remote_profile.as_deref()),
                Err(e) => Some(JsonRpcResponse::error(
                    serde_json::Value::Null,
                    -32700,
                    &format!("Parse error: {}", e),
                )),
            };

            if let Some(resp) = response {
                let payload = serde_json::to_string(&resp).map_err(|e| {
                    aidememo_core::AideMemoError::Internal(format!("serialize response: {}", e))
                })?;
                stdout.write_all(payload.as_bytes()).await.map_err(|e| {
                    aidememo_core::AideMemoError::Internal(format!("stdout write: {}", e))
                })?;
                stdout.write_all(b"\n").await.map_err(|e| {
                    aidememo_core::AideMemoError::Internal(format!("stdout write: {}", e))
                })?;
                stdout.flush().await.map_err(|e| {
                    aidememo_core::AideMemoError::Internal(format!("stdout flush: {}", e))
                })?;
            }
        }

        tracing::info!("aidememo mcp: stdin closed, shutting down");
        Ok::<(), aidememo_core::AideMemoError>(())
    })?;

    Ok(String::new())
}
