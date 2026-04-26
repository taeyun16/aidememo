//! `wg recent` — discover what changed lately.
//!
//! Sugar on top of `wg fact list --last <duration>` with friendlier defaults.

use bpaf::*;
use std::path::PathBuf;
use wg_core::{Config, FactListOpts, WgError, WikiGraph};

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
    store_path: &PathBuf,
    config: Config,
    sub: RecentSub,
    global_json: bool,
) -> Result<String, WgError> {
    let wiki = WikiGraph::open(store_path, config)?;

    // Default to last 7 days if user didn't specify.
    let last = sub.last.unwrap_or_else(|| "7d".to_string());
    let since = crate::resolve_since(None, Some(&last))?;

    let opts = FactListOpts {
        fact_type: crate::parse_fact_type(sub.fact_type),
        entity_id: None,
        min_confidence: None,
        limit: Some(sub.limit.unwrap_or(20)),
        offset: 0,
        since,
        until: None,
    };
    let facts = wiki.fact_list(opts)?;

    let format = if sub.json || global_json {
        output::Format::Json
    } else {
        output::Format::Table
    };
    output::format_fact_list(&facts, &wiki, format)
}
