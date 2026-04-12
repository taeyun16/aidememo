//! `wg mcp-serve` — MCP server mode.
//!
//! This module currently provides the CLI plumbing and a lightweight runtime
//! entry point so the command builds cleanly with the rest of the workspace.

use std::path::PathBuf;

use bpaf::*;

use crate::{cmd::Command, Config, WikiGraph};

#[derive(Debug, Clone)]
pub struct McpSub {
    pub port: u16,
    pub wiki_root: Option<PathBuf>,
}

pub fn mcp_serve_command() -> impl Parser<Command> {
    let port = long("port")
        .short('p')
        .help("Port to listen on (default: 3000)")
        .argument::<u16>("PORT")
        .optional();

    let wiki_root = positional::<PathBuf>("WIKI_ROOT")
        .help("Path to wiki root")
        .optional();

    construct!(port, wiki_root)
        .map(|(port, wiki_root)| McpSub {
            port: port.unwrap_or(3000),
            wiki_root,
        })
        .map(Command::McpServe)
        .to_options()
        .command("mcp-serve")
        .help("Start MCP server")
}

pub fn run_mcp_serve(port: u16, wiki_root: Option<PathBuf>) -> Result<String, wg_core::WgError> {
    let config = Config::load().unwrap_or_default();
    let store_path = match wiki_root {
        Some(path) => path,
        None => PathBuf::from(&config.store.path),
    };

    // Open the store so the command verifies the configured path is valid.
    let _wiki = WikiGraph::open(store_path.as_ref(), config)?;

    Ok(format!(
        "MCP server build is available. Use port {} for future HTTP serving.",
        port
    ))
}
