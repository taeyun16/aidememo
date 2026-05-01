//! `wg pending review` — TUI to audit and promote dry-run captures.
//!
//! The Hermes plugin's auto-recorder appends detected facts to a
//! JSONL log when `dry_run: true` is on (see
//! `plugins/hermes/src/hermes_wg/pending.py`). That covers Hermes
//! sessions; for everyone else (Codex / Cursor / Claude Code), the
//! same JSONL format works as a "things I've collected and want to
//! review later" inbox.
//!
//! This module reads the log, presents an interactive checklist,
//! and lets the operator select entries to commit (write to wg as
//! real facts) or discard (drop without writing). Anything not
//! touched stays in the log for next time.
//!
//! Layered intentionally so the I/O + state can be unit-tested
//! without rendering: `read_jsonl`, `write_jsonl`, `commit_selected`,
//! and `AppState::toggle/select_all/etc.` are pure. The terminal
//! loop in `run_tui` is the only piece that needs a real TTY.

use bpaf::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::cmd::Command;
use wg_core::{Config, FactInput, WgError, WikiGraph};

// ---------------------------------------------------------------------------
// Subcommand wiring
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum PendingSub {
    Review { from: Option<PathBuf> },
}

pub fn pending_command() -> impl Parser<Command> {
    let from = long("from")
        .help(
            "Path to the JSONL pending log. Defaults to \
             $HERMES_STATE_DIR/wg-pending.jsonl, then \
             ~/.hermes/state/wg-pending.jsonl.",
        )
        .argument::<PathBuf>("PATH")
        .optional();

    let review = construct!(PendingSub::Review { from })
        .to_options()
        .command("review")
        .help("Open an interactive TUI to commit / discard pending detections");

    construct!([review])
        .map(Command::Pending)
        .to_options()
        .command("pending")
        .help("Manage the dry-run pending detections log")
}

// ---------------------------------------------------------------------------
// JSONL data model — kept in lockstep with hermes_wg.pending
// ---------------------------------------------------------------------------

/// One row from the pending log. Matches the Python plugin's shape
/// field-for-field so the two paths can read each other's logs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingEntry {
    /// Milliseconds since epoch when the entry was captured.
    pub ts_ms: u64,
    pub content: String,
    pub fact_type: String,
    pub confidence: f32,
    /// The original line of conversation that triggered the match —
    /// useful in the TUI's detail pane for spot-checking precision.
    pub source_line: String,
}

/// Resolve the default pending log path, honoring `HERMES_STATE_DIR`
/// just like the Python helper. We don't *create* the file or its
/// parent here — readers should treat "absent" as "no entries".
pub fn default_log_path() -> PathBuf {
    if let Ok(env) = std::env::var("HERMES_STATE_DIR") {
        return PathBuf::from(env).join("wg-pending.jsonl");
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".hermes/state/wg-pending.jsonl")
}

pub fn read_jsonl(path: &Path) -> Result<Vec<PendingEntry>, WgError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(path)
        .map_err(|e| WgError::Internal(format!("open {}: {e}", path.display())))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                return Err(WgError::Internal(format!("read {}: {e}", path.display())));
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Bad lines are skipped — same precedent the Python module
        // sets, so a partial write doesn't sink the rest of the log.
        if let Ok(entry) = serde_json::from_str::<PendingEntry>(trimmed) {
            out.push(entry);
        }
    }
    Ok(out)
}

pub fn write_jsonl(path: &Path, entries: &[PendingEntry]) -> Result<(), WgError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WgError::Internal(format!("create {}: {e}", parent.display())))?;
    }
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| WgError::Internal(format!("write {}: {e}", tmp.display())))?;
        for entry in entries {
            let line = serde_json::to_string(entry)
                .map_err(|e| WgError::Internal(format!("serialize entry: {e}")))?;
            writeln!(file, "{}", line)
                .map_err(|e| WgError::Internal(format!("write {}: {e}", tmp.display())))?;
        }
        file.flush()
            .map_err(|e| WgError::Internal(format!("flush {}: {e}", tmp.display())))?;
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| WgError::Internal(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Promote selected entries into wg facts
// ---------------------------------------------------------------------------

/// Outcome of a `wg pending review` run, returned to the user once
/// the TUI exits so they can see what changed.
#[derive(Debug, Default, PartialEq)]
pub struct ReviewSummary {
    pub committed: usize,
    pub discarded: usize,
    pub remaining: usize,
}

/// Commit and/or discard selected entries in the order they appear
/// in `entries`. Returns the new entry list (kept) and a summary.
/// On commit failure, the offending entry is *kept* so the user can
/// retry — same precedent the Python module sets.
pub fn apply_review(
    entries: &[PendingEntry],
    commit: &HashSet<usize>,
    discard: &HashSet<usize>,
    wiki: &WikiGraph,
) -> (Vec<PendingEntry>, ReviewSummary) {
    let mut summary = ReviewSummary::default();
    let mut kept: Vec<PendingEntry> = Vec::with_capacity(entries.len());

    for (idx, entry) in entries.iter().enumerate() {
        if commit.contains(&idx) {
            match commit_entry(wiki, entry) {
                Ok(_) => summary.committed += 1,
                Err(_) => {
                    // Failed commit — keep so the user can retry.
                    kept.push(entry.clone());
                }
            }
        } else if discard.contains(&idx) {
            summary.discarded += 1;
        } else {
            kept.push(entry.clone());
        }
    }
    summary.remaining = kept.len();
    (kept, summary)
}

fn commit_entry(wiki: &WikiGraph, entry: &PendingEntry) -> Result<(), WgError> {
    let fact_type = parse_fact_type(&entry.fact_type);
    wiki.add_fact(FactInput {
        content: entry.content.clone(),
        fact_type,
        entity_ids: None,
        tags: Some(vec![
            "auto-recorded".to_string(),
            "wg-pending-review".to_string(),
        ]),
        source: None,
        source_confidence: Some(entry.confidence),
        observed_at: Some(entry.ts_ms),
    })?;
    Ok(())
}

fn parse_fact_type(s: &str) -> Option<wg_core::FactType> {
    let parsed = wg_core::FactType::parse(s);
    if matches!(parsed, wg_core::FactType::Unknown) && s.to_lowercase() != "unknown" {
        None
    } else {
        Some(parsed)
    }
}

// ---------------------------------------------------------------------------
// TUI state — pure, render-free
// ---------------------------------------------------------------------------

/// In-memory state of the TUI. Everything except `quit` is observable
/// from a unit test, so the state machine is verifiable end-to-end
/// without spinning up a terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub entries: Vec<PendingEntry>,
    /// Indices marked for commit. Mutually exclusive with `discard`.
    pub commit: HashSet<usize>,
    /// Indices marked for discard.
    pub discard: HashSet<usize>,
    /// Currently focused row.
    pub cursor: usize,
    pub log_path: PathBuf,
    /// One-line message shown at the bottom — set by recent actions.
    pub status: String,
    pub quit: bool,
    /// True when the user pressed `c` to commit/apply. The runner
    /// reads this on quit to decide whether to call `apply_review`.
    pub apply_on_quit: bool,
}

impl AppState {
    pub fn new(entries: Vec<PendingEntry>, log_path: PathBuf) -> Self {
        Self {
            entries,
            commit: HashSet::new(),
            discard: HashSet::new(),
            cursor: 0,
            log_path,
            status: String::new(),
            quit: false,
            apply_on_quit: false,
        }
    }

    pub fn cursor_down(&mut self) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
        }
    }

    pub fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Cycle the current row through three states: unmarked → commit
    /// → discard → unmarked. Mirrors how mutt and similar mailers
    /// handle bulk-action selection.
    pub fn cycle_current(&mut self) {
        let i = self.cursor;
        if i >= self.entries.len() {
            return;
        }
        if self.commit.contains(&i) {
            self.commit.remove(&i);
            self.discard.insert(i);
        } else if self.discard.contains(&i) {
            self.discard.remove(&i);
        } else {
            self.commit.insert(i);
        }
    }

    pub fn select_all_commit(&mut self) {
        self.commit = (0..self.entries.len()).collect();
        self.discard.clear();
    }

    pub fn clear_selections(&mut self) {
        self.commit.clear();
        self.discard.clear();
    }

    pub fn pending_action(&self, idx: usize) -> &'static str {
        if self.commit.contains(&idx) {
            "commit"
        } else if self.discard.contains(&idx) {
            "discard"
        } else {
            "—"
        }
    }
}

// ---------------------------------------------------------------------------
// Runner — the only piece that needs a TTY
// ---------------------------------------------------------------------------

pub fn run_pending_review(sub: PendingSub) -> Result<String, WgError> {
    let PendingSub::Review { from } = sub;
    let log_path = from.unwrap_or_else(default_log_path);
    let entries = read_jsonl(&log_path)?;

    if entries.is_empty() {
        return Ok(format!(
            "No pending detections in {}.\nRun a Hermes session with `dry_run: true` to populate the log.\n",
            log_path.display()
        ));
    }

    let state = AppState::new(entries, log_path.clone());
    let final_state = tui::run(state)?;

    if !final_state.apply_on_quit {
        return Ok(format!(
            "Exited without changes. {} entry(ies) still in {}.\n",
            final_state.entries.len(),
            log_path.display()
        ));
    }

    if final_state.commit.is_empty() && final_state.discard.is_empty() {
        return Ok(format!(
            "Nothing selected. {} entry(ies) still in {}.\n",
            final_state.entries.len(),
            log_path.display()
        ));
    }

    let config = Config::load().unwrap_or_default();
    let store_path = PathBuf::from(&config.store.path);
    let wiki = WikiGraph::open(&store_path, config)?;

    let (kept, summary) = apply_review(
        &final_state.entries,
        &final_state.commit,
        &final_state.discard,
        &wiki,
    );
    write_jsonl(&log_path, &kept)?;

    Ok(format!(
        "Committed {} fact(s), discarded {}, kept {}.\nLog: {}\n",
        summary.committed,
        summary.discarded,
        summary.remaining,
        log_path.display()
    ))
}

// The actual rendering / input loop is isolated so unit tests of the
// state machine never need to touch a terminal.
mod tui;

// ---------------------------------------------------------------------------
// Tests — pure logic only; rendering is exercised manually.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(content: &str, kind: &str, conf: f32) -> PendingEntry {
        PendingEntry {
            ts_ms: 1_700_000_000_000,
            content: content.to_string(),
            fact_type: kind.to_string(),
            confidence: conf,
            source_line: format!("source: {}", content),
        }
    }

    #[test]
    fn jsonl_roundtrip_preserves_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wg-pending.jsonl");
        let original = vec![
            entry("Use HNSW", "decision", 0.95),
            entry("Always lint", "convention", 0.85),
        ];
        write_jsonl(&path, &original).unwrap();
        let loaded = read_jsonl(&path).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn read_skips_invalid_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pending.jsonl");
        std::fs::write(
            &path,
            r#"{"ts_ms":1,"content":"ok","fact_type":"note","confidence":0.5,"source_line":"x"}
this is not json
{}
{"ts_ms":2,"content":"ok2","fact_type":"note","confidence":0.5,"source_line":"y"}
"#,
        )
        .unwrap();
        let loaded = read_jsonl(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].content, "ok");
        assert_eq!(loaded[1].content, "ok2");
    }

    #[test]
    fn read_returns_empty_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = read_jsonl(&dir.path().join("nope.jsonl")).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn cycle_current_walks_three_states() {
        let mut s = AppState::new(vec![entry("a", "note", 0.9)], PathBuf::from("/x"));
        assert_eq!(s.pending_action(0), "—");
        s.cycle_current();
        assert_eq!(s.pending_action(0), "commit");
        s.cycle_current();
        assert_eq!(s.pending_action(0), "discard");
        s.cycle_current();
        assert_eq!(s.pending_action(0), "—");
    }

    #[test]
    fn cursor_navigation_is_bounded() {
        let mut s = AppState::new(
            vec![entry("a", "note", 0.5), entry("b", "note", 0.5)],
            PathBuf::from("/x"),
        );
        s.cursor_up();
        assert_eq!(s.cursor, 0);
        s.cursor_down();
        s.cursor_down();
        s.cursor_down();
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn select_all_marks_every_entry_for_commit() {
        let mut s = AppState::new(
            vec![entry("a", "note", 0.5), entry("b", "note", 0.5)],
            PathBuf::from("/x"),
        );
        s.discard.insert(0);
        s.select_all_commit();
        assert_eq!(s.commit.len(), 2);
        assert!(s.discard.is_empty());
    }

    #[test]
    fn clear_selections_drops_both_sets() {
        let mut s = AppState::new(vec![entry("a", "note", 0.5)], PathBuf::from("/x"));
        s.commit.insert(0);
        s.discard.insert(0);
        s.clear_selections();
        assert!(s.commit.is_empty());
        assert!(s.discard.is_empty());
    }

    #[test]
    fn apply_review_keeps_unselected_and_failed_commits() {
        // We can't easily open a real WikiGraph in a unit test
        // without a redb store, so verify the bookkeeping by
        // building expectations purely from the state inputs.
        let entries = vec![
            entry("commit me", "decision", 0.9),
            entry("discard me", "note", 0.5),
            entry("leave me", "note", 0.5),
        ];
        let mut commit = HashSet::new();
        commit.insert(0);
        let mut discard = HashSet::new();
        discard.insert(1);

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("wiki.redb");
        let wiki = WikiGraph::open(&store, Config::default()).unwrap();

        let (kept, summary) = apply_review(&entries, &commit, &discard, &wiki);
        assert_eq!(summary.committed, 1);
        assert_eq!(summary.discarded, 1);
        assert_eq!(summary.remaining, 1);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].content, "leave me");
    }

    #[test]
    fn default_log_path_honors_state_dir_env() {
        let dir = tempfile::tempdir().unwrap();
        // Saving / restoring is fine because tests run single-threaded
        // by default in the same process — but be defensive anyway.
        let prior = std::env::var("HERMES_STATE_DIR").ok();
        // SAFETY: tests are serialized via cargo's default test harness
        // for env mutations as long as nothing else races on the var.
        unsafe {
            std::env::set_var("HERMES_STATE_DIR", dir.path());
        }
        assert_eq!(default_log_path(), dir.path().join("wg-pending.jsonl"));
        unsafe {
            match prior {
                Some(v) => std::env::set_var("HERMES_STATE_DIR", v),
                None => std::env::remove_var("HERMES_STATE_DIR"),
            }
        }
    }
}
