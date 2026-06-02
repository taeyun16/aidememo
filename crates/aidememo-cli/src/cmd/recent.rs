//! `aidememo recent` — discover what changed lately.
//!
//! Sugar on top of `aidememo fact list --last <duration>` with friendlier defaults.

use aidememo_core::{AideMemo, AideMemoError, Config, FactListOpts};
use bpaf::*;
use std::path::Path;

use crate::cmd::Command;
use crate::output;

#[derive(Debug, Clone)]
pub struct RecentSub {
    pub limit: Option<usize>,
    pub fact_type: Option<String>,
    pub last: Option<String>,
    pub json: bool,
}

pub fn recent_command() -> impl Parser<Command> {
    let limit = long("limit")
        .short('n')
        .help("Max facts to return (default 20)")
        .argument::<usize>("N")
        .optional();
    let fact_type = long("type")
        .short('t')
        .help("Filter by fact type (decision, pattern, …)")
        .argument::<String>("TYPE")
        .optional();
    let last = long("last")
        .help("Window from now: 30d, 12h, 4w (default 7d)")
        .argument::<String>("DURATION")
        .optional();
    let json = long("json").short('j').help("Output as JSON").switch();

    construct!(RecentSub {
        limit,
        fact_type,
        last,
        json
    })
    .map(Command::Recent)
    .to_options()
    .command("recent")
    .help("Show recently added/updated facts")
}

pub fn run_recent(
    store_path: &Path,
    config: Config,
    sub: RecentSub,
    global_json: bool,
) -> Result<String, AideMemoError> {
    // Default to last 7 days if user didn't specify.
    let last = sub.last.unwrap_or_else(|| "7d".to_string());

    // Daemon discovery — same pattern as `aidememo search`/`aidememo query`.
    // The aidememo_recent MCP tool returns {"facts": [...]} JSON; we print
    // it verbatim. Local `--type` filter falls through to the
    // in-process path below because the tool doesn't expose it
    // (the tool is the daemon's surface, not a 1:1 of the CLI).
    if sub.fact_type.is_none() {
        if let Some(via) = crate::cmd::daemon::registered_endpoint(store_path) {
            tracing::debug!(via = %via, "auto-discovered daemon for recent");
            return run_recent_via_daemon(&via, sub.limit.unwrap_or(20), &last);
        }
    }

    let wiki = AideMemo::open(store_path, config)?;
    let since = crate::resolve_since(None, Some(&last))?;

    let opts = FactListOpts {
        fact_type: crate::parse_fact_type(sub.fact_type),
        entity_id: None,
        min_confidence: None,
        source_id: None,
        limit: Some(sub.limit.unwrap_or(20)),
        offset: 0,
        since,
        until: None,
        current_only: false,
        as_of: None,
    };
    let facts = wiki.fact_list(opts)?;

    let format = if sub.json || global_json {
        output::Format::Json
    } else {
        output::Format::Table
    };
    output::format_fact_list(&facts, &wiki, format)
}

fn run_recent_via_daemon(
    base_url: &str,
    limit: usize,
    last: &str,
) -> Result<String, AideMemoError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {
            "name": "aidememo_recent",
            "arguments": {
                "limit": limit,
                "last": last,
            }
        }
    });
    crate::daemon_tool_call(&url, body, "aidememo_recent")
}
