//! `wg watch` — watch wiki files and automatically re-ingest on changes.
//!
//! Uses the `notify` crate to monitor the wiki root directory for changes
//! to `.md` files and triggers an incremental re-ingest when files change.

use bpaf::*;
use std::path::PathBuf;
use std::time::Duration;

use crate::cmd::Command;
use wg_core::{Config, WgError, WikiGraph};

#[derive(Debug, Clone)]
pub struct WatchSub {
    /// Polling interval in seconds (default: 5).
    pub interval: Option<u64>,
    pub wiki_root: PathBuf,
}

pub fn watch_command() -> impl Parser<Command> {
    let wiki_root = positional::<PathBuf>("WIKI_ROOT").help("Path to the wiki root directory");

    let interval = long("interval")
        .short('i')
        .help("Polling interval in seconds (default: 5)")
        .argument::<u64>("SECS")
        .optional();

    construct!(WatchSub {
        interval,
        wiki_root,
    })
    .map(Command::Watch)
    .to_options()
    .command("watch")
    .help("Watch wiki files and automatically re-ingest on changes")
}

/// Run `wg watch` — monitor the wiki root and re-ingest on file changes.
pub fn run_watch(wiki_root: PathBuf, interval_secs: Option<u64>) -> Result<String, WgError> {
    use notify::{RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc;

    let wiki_root = wiki_root
        .canonicalize()
        .unwrap_or_else(|_| wiki_root.clone());

    if !wiki_root.is_dir() {
        return Err(WgError::InvalidInput(format!(
            "Wiki root '{}' is not a directory",
            wiki_root.display()
        )));
    }

    let interval = Duration::from_secs(interval_secs.unwrap_or(5));

    // Load config
    let config = Config::load().unwrap_or_default();
    let store_path = PathBuf::from(&config.store.path);

    // Open the store
    let mut wiki = WikiGraph::open(&store_path, config)?;

    // Do an initial ingest
    println!("Initial ingest...");
    let stats = wiki
        .ingest(&wiki_root, false)
        .map_err(|e| WgError::IngestFailed(e.to_string()))?;
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
        .map_err(|e| WgError::Internal(format!("failed to create watcher: {}", e)))?;

    watcher
        .watch(&wiki_root, RecursiveMode::Recursive)
        .map_err(|e| WgError::Internal(format!("failed to watch directory: {}", e)))?;

    // Event loop
    loop {
        match rx.recv_timeout(interval) {
            Ok(event) => {
                // Filter to relevant file change events
                let is_md = event
                    .paths
                    .iter()
                    .any(|p| p.extension().map_or(false, |e| e == "md"));
                if !is_md {
                    continue;
                }

                let action = match event.kind {
                    notify::EventKind::Create(_) => "created",
                    notify::EventKind::Modify(_) => "modified",
                    notify::EventKind::Remove(_) => "removed",
                    _ => continue,
                };

                eprintln!("[wg watch] file {}: {}", action, event.paths[0].display());

                // Re-ingest
                match wiki.ingest(&wiki_root, true) {
                    Ok(stats) => {
                        println!("[wg watch] re-ingested: {}", format_watch_stats(&stats));
                    }
                    Err(e) => {
                        eprintln!("[wg watch] ingest error: {}", e);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Periodic re-ingest as a fallback (handles deletes, etc.)
                match wiki.ingest(&wiki_root, true) {
                    Ok(stats) => {
                        if stats.files_scanned > 0 {
                            println!("[wg watch] periodic check: {}", format_watch_stats(&stats));
                        }
                    }
                    Err(e) => {
                        eprintln!("[wg watch] periodic ingest error: {}", e);
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

fn format_watch_stats(stats: &wg_core::IngestStats) -> String {
    format!(
        "+{} entities, +{} relations, +{} facts ({} files scanned)",
        stats.entities_added, stats.relations_added, stats.facts_added, stats.files_scanned
    )
}
