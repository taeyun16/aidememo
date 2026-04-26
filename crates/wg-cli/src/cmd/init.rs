//! `wg init` — initialize a new wiki graph.
//!
//! Creates the store, runs the first ingest, and prints a quick-start example.

use bpaf::*;
use std::path::PathBuf;

use crate::cmd::Command;
use wg_core::{Config, IngestStats, WgError, WikiGraph};

#[derive(Debug, Clone)]
pub struct InitSub {
    /// Skip the initial ingest and only create the store.
    pub no_ingest: bool,
    /// Wiki root directory path.
    pub wiki_root: PathBuf,
}

pub fn init_command() -> impl Parser<Command> {
    let no_ingest = long("no-ingest")
        .short('n')
        .help("Skip the initial ingest (create store only)")
        .switch();

    let wiki_root = positional::<PathBuf>("WIKI_ROOT").help("Path to the wiki root directory");

    construct!(InitSub {
        no_ingest,
        wiki_root,
    })
    .map(Command::Init)
    .to_options()
    .command("init")
    .help("Initialize a wiki graph store and optionally ingest the wiki")
}

/// Run `wg init` — create config/store and optionally ingest.
pub fn run_init(wiki_root: PathBuf, no_ingest: bool) -> Result<String, WgError> {
    let wiki_root = wiki_root
        .canonicalize()
        .unwrap_or_else(|_| wiki_root.clone());

    // 1. Ensure the wiki root exists
    if !wiki_root.is_dir() {
        return Err(WgError::InvalidInput(format!(
            "Wiki root '{}' is not a directory",
            wiki_root.display()
        )));
    }

    // 2. Load config and ensure the store directory exists
    let config = Config::load().unwrap_or_default();
    let store_path = PathBuf::from(&config.store.path);
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WgError::Internal(format!("failed to create store dir: {}", e)))?;
    }

    // 3. Open (or create) the store
    let wiki = WikiGraph::open(&store_path, config.clone())?;

    // 4. Ingest if not skipped
    let stats = if no_ingest {
        None
    } else {
        let s = wiki.ingest(&wiki_root, false)?;
        Some(s)
    };

    // 5. Print summary
    let mut out = format!("WikiGraph initialized at {}\n", store_path.display());
    out.push_str(&format!("Wiki root: {}\n\n", wiki_root.display()));

    if let Some(stats) = stats {
        out.push_str(&format_summary(&stats));
        out.push_str("\nQuick start:\n");
        out.push_str("  wg entity list\n");
        out.push_str("  wg traverse <entity> --depth 2\n");
        out.push_str("  wg search <query>\n");
    } else {
        out.push_str("Store created (ingest skipped).\n");
        out.push_str("Run `wg ingest <wiki-root>` when ready.\n");
    }

    Ok(out)
}

fn format_summary(stats: &IngestStats) -> String {
    let mut lines = vec![];
    if stats.entities_added > 0 || stats.relations_added > 0 || stats.facts_added > 0 {
        lines.push(format!(
            "Ingested {} files: +{} entities, +{} relations, +{} facts",
            stats.files_scanned, stats.entities_added, stats.relations_added, stats.facts_added
        ));
    } else if stats.files_scanned == 0 {
        lines.push("No .md files found in wiki root.".to_string());
    }
    if stats.entities_updated > 0 {
        lines.push(format!(
            "  ({} entities refreshed from frontmatter)",
            stats.entities_updated
        ));
    }
    if !stats.errors.is_empty() {
        lines.push(format!("  {} parse errors (see logs)", stats.errors.len()));
        for e in stats.errors.iter().take(3) {
            lines.push(format!("    - {}", e));
        }
    }
    lines.join("\n")
}
