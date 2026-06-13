//! `aidememo watch` — watch wiki files and automatically re-ingest on changes.
//!
//! Uses the `notify` crate to monitor the wiki root directory for changes
//! to `.md` files and triggers an incremental re-ingest when files change.

use bpaf::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cmd::Command;
use aidememo_core::{AideMemo, AideMemoError, Config, SearchOpts};

#[derive(Debug, Clone)]
pub struct WatchSub {
    /// Polling interval in seconds (default: 5).
    pub interval: Option<u64>,
    /// Optional search query — if set, prints fresh top-N hits after each
    /// re-ingest instead of just the ingest stats.
    pub search: Option<String>,
    pub wiki_root: PathBuf,
}

pub fn watch_command() -> impl Parser<Command> {
    let wiki_root = positional::<PathBuf>("WIKI_ROOT").help("Path to the wiki root directory");

    let interval = long("interval")
        .short('i')
        .help("Polling interval in seconds (default: 5)")
        .argument::<u64>("SECS")
        .optional();

    let search = long("search")
        .help("Live-search topic — prints fresh top hits after each re-ingest")
        .argument::<String>("QUERY")
        .optional();

    construct!(WatchSub {
        interval,
        search,
        wiki_root,
    })
    .map(Command::Watch)
    .to_options()
    .command("watch")
    .help("Watch wiki files and re-ingest (with optional --search)")
}

/// Run `aidememo watch` — monitor the wiki root and re-ingest on file changes.
/// If `search_query` is set, also re-runs that hybrid search after each
/// ingest and prints the top-5 hits.
pub fn run_watch(
    wiki_root: PathBuf,
    store_path: &Path,
    config: Config,
    interval_secs: Option<u64>,
    search_query: Option<String>,
) -> Result<String, AideMemoError> {
    use notify::{RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc;

    let wiki_root = wiki_root
        .canonicalize()
        .unwrap_or_else(|_| wiki_root.clone());

    if !wiki_root.is_dir() {
        return Err(AideMemoError::InvalidInput(format!(
            "Wiki root '{}' is not a directory",
            wiki_root.display()
        )));
    }

    let interval = Duration::from_secs(interval_secs.unwrap_or(5));

    // Open the store
    let wiki = AideMemo::open(store_path, config)?;

    // Do an initial ingest
    println!("Initial ingest...");
    let stats = wiki
        .ingest(&wiki_root, false)
        .map_err(|e| AideMemoError::IngestFailed(e.to_string()))?;
    println!("{}", format_watch_stats(&stats));

    println!(
        "Watching {} for changes (Ctrl+C to stop)...",
        wiki_root.display()
    );

    // Set up the watcher using polling (notify-pollbox or recommended)
    let (tx, rx) = mpsc::channel();

    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })
        .map_err(|e| AideMemoError::Internal(format!("failed to create watcher: {}", e)))?;

    watcher
        .watch(&wiki_root, RecursiveMode::Recursive)
        .map_err(|e| AideMemoError::Internal(format!("failed to watch directory: {}", e)))?;

    // Event loop
    loop {
        match rx.recv_timeout(interval) {
            Ok(event) => {
                // Filter to relevant file change events
                let is_md = event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|e| e == "md"));
                if !is_md {
                    continue;
                }

                let action = match event.kind {
                    notify::EventKind::Create(_) => "created",
                    notify::EventKind::Modify(_) => "modified",
                    notify::EventKind::Remove(_) => "removed",
                    _ => continue,
                };

                eprintln!(
                    "[aidememo watch] file {}: {}",
                    action,
                    event.paths[0].display()
                );

                // Re-ingest
                match wiki.ingest(&wiki_root, true) {
                    Ok(stats) => {
                        println!(
                            "[aidememo watch] re-ingested: {}",
                            format_watch_stats(&stats)
                        );
                        if let Some(ref q) = search_query {
                            print_search_snapshot(&wiki, q);
                        }
                    }
                    Err(e) => {
                        eprintln!("[aidememo watch] ingest error: {}", e);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Periodic re-ingest as a fallback (handles deletes, etc.)
                match wiki.ingest(&wiki_root, true) {
                    Ok(stats) => {
                        if stats.files_scanned > 0 {
                            println!(
                                "[aidememo watch] periodic check: {}",
                                format_watch_stats(&stats)
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("[aidememo watch] periodic ingest error: {}", e);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    Ok(String::new())
}

fn format_watch_stats(stats: &aidememo_core::IngestStats) -> String {
    format!(
        "+{} entities, +{} relations, +{} facts ({} files scanned)",
        stats.entities_added, stats.relations_added, stats.facts_added, stats.files_scanned
    )
}

fn print_search_snapshot(wiki: &AideMemo, query: &str) {
    let opts = SearchOpts {
        limit: Some(5),
        current_only: true,
        ..Default::default()
    };
    match wiki.hybrid_search(query, opts) {
        Ok(results) if !results.is_empty() => {
            println!("[aidememo watch] search '{query}' (top 5):");
            for r in &results {
                let snippet: String = r.content.chars().take(70).collect();
                println!("  {}. {}  (score={:.3})", r.rank, snippet, r.score);
            }
        }
        Ok(_) => {
            println!("[aidememo watch] search '{query}': no current matches");
        }
        Err(e) => {
            eprintln!("[aidememo watch] search error: {e}");
        }
    }
}
