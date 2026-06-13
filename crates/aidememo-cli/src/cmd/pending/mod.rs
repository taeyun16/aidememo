//! `aidememo pending review` — TUI to audit and promote dry-run captures.
//!
//! The Hermes plugin's auto-recorder appends detected facts to a
//! JSONL log when `dry_run: true` is on (see
//! `plugins/hermes/src/hermes_aidememo/pending.py`). That covers Hermes
//! sessions; for everyone else (Codex / Cursor / Claude Code), the
//! same JSONL format works as a "things I've collected and want to
//! review later" inbox.
//!
//! This module reads the log, presents an interactive checklist,
//! and lets the operator select entries to commit (write to aidememo as
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
use aidememo_core::{AideMemo, AideMemoError, Config, FactInput};

// ---------------------------------------------------------------------------
// Subcommand wiring
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum PendingSub {
    Review {
        from: Option<PathBuf>,
    },
    List {
        from: Option<PathBuf>,
        limit: Option<usize>,
    },
    Approve {
        from: Option<PathBuf>,
        all: bool,
        indices: Option<String>,
    },
    Reject {
        from: Option<PathBuf>,
        all: bool,
        indices: Option<String>,
    },
    Stats {
        from: Option<PathBuf>,
    },
}

pub fn pending_command() -> impl Parser<Command> {
    let from = long("from")
        .help(
            "Path to the JSONL pending log. Defaults to \
             $HERMES_STATE_DIR/aidememo-pending.jsonl, then \
             ~/.hermes/state/aidememo-pending.jsonl.",
        )
        .argument::<PathBuf>("PATH")
        .optional();

    let review = construct!(PendingSub::Review { from })
        .to_options()
        .command("review")
        .help("Open an interactive TUI to commit / discard pending detections");

    let from = long("from")
        .help(
            "Path to the JSONL pending log. Defaults to \
             $HERMES_STATE_DIR/aidememo-pending.jsonl, then \
             ~/.hermes/state/aidememo-pending.jsonl.",
        )
        .argument::<PathBuf>("PATH")
        .optional();
    let limit = long("limit")
        .short('l')
        .help("Maximum entries to print")
        .argument::<usize>("N")
        .optional();
    let list = construct!(PendingSub::List { from, limit })
        .to_options()
        .command("list")
        .help("List pending detections with stable 1-based indices");

    let from = long("from")
        .help(
            "Path to the JSONL pending log. Defaults to \
             $HERMES_STATE_DIR/aidememo-pending.jsonl, then \
             ~/.hermes/state/aidememo-pending.jsonl.",
        )
        .argument::<PathBuf>("PATH")
        .optional();
    let all = long("all").help("Approve every pending entry").switch();
    let indices = long("indices")
        .short('i')
        .help("Comma-separated 1-based indices/ranges to approve, e.g. 1,3-5")
        .argument::<String>("LIST")
        .optional();
    let approve = construct!(PendingSub::Approve { from, all, indices })
        .to_options()
        .command("approve")
        .help("Commit selected pending detections without opening the TUI");

    let from = long("from")
        .help(
            "Path to the JSONL pending log. Defaults to \
             $HERMES_STATE_DIR/aidememo-pending.jsonl, then \
             ~/.hermes/state/aidememo-pending.jsonl.",
        )
        .argument::<PathBuf>("PATH")
        .optional();
    let all = long("all").help("Reject every pending entry").switch();
    let indices = long("indices")
        .short('i')
        .help("Comma-separated 1-based indices/ranges to reject, e.g. 1,3-5")
        .argument::<String>("LIST")
        .optional();
    let reject = construct!(PendingSub::Reject { from, all, indices })
        .to_options()
        .command("reject")
        .help("Discard selected pending detections without opening the TUI");

    let from = long("from")
        .help(
            "Path to the JSONL pending log. Defaults to \
             $HERMES_STATE_DIR/aidememo-pending.jsonl, then \
             ~/.hermes/state/aidememo-pending.jsonl.",
        )
        .argument::<PathBuf>("PATH")
        .optional();
    let stats = construct!(PendingSub::Stats { from })
        .to_options()
        .command("stats")
        .help("Summarize pending detections by type and confidence");

    construct!([review, list, approve, reject, stats])
        .map(Command::Pending)
        .to_options()
        .command("pending")
        .help("Manage the dry-run pending detections log")
}

// ---------------------------------------------------------------------------
// JSONL data model — kept in lockstep with hermes_aidememo.pending
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
        return PathBuf::from(env).join("aidememo-pending.jsonl");
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".hermes/state/aidememo-pending.jsonl")
}

pub fn read_jsonl(path: &Path) -> Result<Vec<PendingEntry>, AideMemoError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(path)
        .map_err(|e| AideMemoError::Internal(format!("open {}: {e}", path.display())))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                return Err(AideMemoError::Internal(format!(
                    "read {}: {e}",
                    path.display()
                )));
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

pub fn write_jsonl(path: &Path, entries: &[PendingEntry]) -> Result<(), AideMemoError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AideMemoError::Internal(format!("create {}: {e}", parent.display())))?;
    }
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| AideMemoError::Internal(format!("write {}: {e}", tmp.display())))?;
        for entry in entries {
            let line = serde_json::to_string(entry)
                .map_err(|e| AideMemoError::Internal(format!("serialize entry: {e}")))?;
            writeln!(file, "{}", line)
                .map_err(|e| AideMemoError::Internal(format!("write {}: {e}", tmp.display())))?;
        }
        file.flush()
            .map_err(|e| AideMemoError::Internal(format!("flush {}: {e}", tmp.display())))?;
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| AideMemoError::Internal(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Promote selected entries into aidememo facts
// ---------------------------------------------------------------------------

/// Outcome of a `aidememo pending review` run, returned to the user once
/// the TUI exits so they can see what changed.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ReviewSummary {
    pub attempted: usize,
    pub committed: usize,
    pub discarded: usize,
    pub failed: usize,
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
    wiki: &AideMemo,
) -> (Vec<PendingEntry>, ReviewSummary) {
    let mut summary = ReviewSummary::default();
    let mut kept: Vec<PendingEntry> = Vec::with_capacity(entries.len());

    for (idx, entry) in entries.iter().enumerate() {
        if commit.contains(&idx) {
            summary.attempted += 1;
            match commit_entry(wiki, entry) {
                Ok(_) => summary.committed += 1,
                Err(_) => {
                    // Failed commit — keep so the user can retry.
                    summary.failed += 1;
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

fn commit_entry(wiki: &AideMemo, entry: &PendingEntry) -> Result<(), AideMemoError> {
    let fact_type = parse_fact_type(&entry.fact_type);
    wiki.add_fact(FactInput {
        content: entry.content.clone(),
        fact_type,
        entity_ids: None,
        tags: Some(vec![
            "auto-recorded".to_string(),
            "aidememo-pending-review".to_string(),
        ]),
        source: None,
        source_id: None,
        source_confidence: Some(entry.confidence),
        observed_at: Some(entry.ts_ms),
    })?;
    Ok(())
}

fn parse_fact_type(s: &str) -> Option<aidememo_core::FactType> {
    let parsed = aidememo_core::FactType::parse(s);
    if matches!(parsed, aidememo_core::FactType::Unknown) && s.to_lowercase() != "unknown" {
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
// Runner
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct IndexedPendingEntry {
    index: usize,
    ts_ms: u64,
    content: String,
    fact_type: String,
    confidence: f32,
    source_line: String,
}

#[derive(Debug, Serialize)]
struct PendingListReport {
    log_path: String,
    count: usize,
    shown: usize,
    entries: Vec<IndexedPendingEntry>,
}

#[derive(Debug, Serialize)]
struct PendingApplyReport {
    log_path: String,
    selected: usize,
    summary: ReviewSummary,
}

#[derive(Debug, Serialize)]
struct PendingStatsReport {
    log_path: String,
    total: usize,
    by_type: std::collections::BTreeMap<String, usize>,
    confidence_min: Option<f32>,
    confidence_max: Option<f32>,
    confidence_avg: Option<f32>,
    confidence_buckets: std::collections::BTreeMap<String, usize>,
}

pub fn run_pending(
    sub: PendingSub,
    store_path: &Path,
    config: Config,
    json: bool,
) -> Result<String, AideMemoError> {
    match sub {
        PendingSub::Review { from } => run_pending_review(from, store_path, config, json),
        PendingSub::List { from, limit } => run_pending_list(from, limit, json),
        PendingSub::Approve { from, all, indices } => {
            run_pending_apply(from, all, indices, true, store_path, config, json)
        }
        PendingSub::Reject { from, all, indices } => {
            run_pending_apply(from, all, indices, false, store_path, config, json)
        }
        PendingSub::Stats { from } => run_pending_stats(from, json),
    }
}

fn run_pending_list(
    from: Option<PathBuf>,
    limit: Option<usize>,
    json: bool,
) -> Result<String, AideMemoError> {
    let log_path = from.unwrap_or_else(default_log_path);
    let entries = read_jsonl(&log_path)?;
    let shown = limit.unwrap_or(entries.len()).min(entries.len());

    let indexed: Vec<IndexedPendingEntry> = entries
        .iter()
        .take(shown)
        .enumerate()
        .map(|(idx, e)| IndexedPendingEntry {
            index: idx + 1,
            ts_ms: e.ts_ms,
            content: e.content.clone(),
            fact_type: e.fact_type.clone(),
            confidence: e.confidence,
            source_line: e.source_line.clone(),
        })
        .collect();

    let report = PendingListReport {
        log_path: log_path.display().to_string(),
        count: entries.len(),
        shown,
        entries: indexed,
    };
    if json {
        return serde_json::to_string_pretty(&report).map_err(|e| AideMemoError::Serialize {
            context: "pending list".to_string(),
            source: e,
        });
    }

    if entries.is_empty() {
        return Ok(format!(
            "No pending detections in {}.\n",
            log_path.display()
        ));
    }

    let mut out = format!(
        "Pending detections in {}: {} total, showing {}\n",
        log_path.display(),
        entries.len(),
        shown
    );
    for e in &report.entries {
        out.push_str(&format!(
            "{:>4}. [{:<10} {:.2}] {}\n",
            e.index, e.fact_type, e.confidence, e.content
        ));
    }
    Ok(out)
}

fn run_pending_stats(from: Option<PathBuf>, json: bool) -> Result<String, AideMemoError> {
    let log_path = from.unwrap_or_else(default_log_path);
    let entries = read_jsonl(&log_path)?;
    let mut by_type = std::collections::BTreeMap::new();
    let mut buckets = std::collections::BTreeMap::from([
        ("0.0-0.5".to_string(), 0usize),
        ("0.5-0.7".to_string(), 0usize),
        ("0.7-0.9".to_string(), 0usize),
        ("0.9-1.0".to_string(), 0usize),
    ]);
    let mut min: Option<f32> = None;
    let mut max: Option<f32> = None;
    let mut sum = 0.0_f32;

    for entry in &entries {
        *by_type.entry(entry.fact_type.clone()).or_insert(0) += 1;
        min = Some(min.map_or(entry.confidence, |v| v.min(entry.confidence)));
        max = Some(max.map_or(entry.confidence, |v| v.max(entry.confidence)));
        sum += entry.confidence;
        let bucket = if entry.confidence < 0.5 {
            "0.0-0.5"
        } else if entry.confidence < 0.7 {
            "0.5-0.7"
        } else if entry.confidence < 0.9 {
            "0.7-0.9"
        } else {
            "0.9-1.0"
        };
        if let Some(n) = buckets.get_mut(bucket) {
            *n += 1;
        }
    }

    let report = PendingStatsReport {
        log_path: log_path.display().to_string(),
        total: entries.len(),
        by_type,
        confidence_min: min,
        confidence_max: max,
        confidence_avg: if entries.is_empty() {
            None
        } else {
            Some(sum / entries.len() as f32)
        },
        confidence_buckets: buckets,
    };

    if json {
        return serde_json::to_string_pretty(&report).map_err(|e| AideMemoError::Serialize {
            context: "pending stats".to_string(),
            source: e,
        });
    }

    let mut out = format!(
        "Pending stats for {}: {} total\n",
        log_path.display(),
        report.total
    );
    if let Some(avg) = report.confidence_avg {
        out.push_str(&format!(
            "confidence: min {:.2}, avg {:.2}, max {:.2}\n",
            report.confidence_min.unwrap_or(0.0),
            avg,
            report.confidence_max.unwrap_or(0.0)
        ));
    }
    out.push_str("by type:\n");
    for (kind, count) in &report.by_type {
        out.push_str(&format!("  {kind}: {count}\n"));
    }
    out.push_str("confidence buckets:\n");
    for (bucket, count) in &report.confidence_buckets {
        out.push_str(&format!("  {bucket}: {count}\n"));
    }
    Ok(out)
}

fn run_pending_apply(
    from: Option<PathBuf>,
    all: bool,
    indices: Option<String>,
    approve: bool,
    store_path: &Path,
    config: Config,
    json: bool,
) -> Result<String, AideMemoError> {
    let log_path = from.unwrap_or_else(default_log_path);
    let entries = read_jsonl(&log_path)?;
    let selected = select_indices(entries.len(), all, indices.as_deref())?;

    if selected.is_empty() {
        return Ok(format!(
            "No matching pending detections in {}.\n",
            log_path.display()
        ));
    }

    let wiki = AideMemo::open(store_path, config)?;
    let (commit, discard) = if approve {
        (selected.clone(), HashSet::new())
    } else {
        (HashSet::new(), selected.clone())
    };
    let (kept, summary) = apply_review(&entries, &commit, &discard, &wiki);
    write_jsonl(&log_path, &kept)?;

    let report = PendingApplyReport {
        log_path: log_path.display().to_string(),
        selected: selected.len(),
        summary,
    };

    if json {
        return serde_json::to_string_pretty(&report).map_err(|e| AideMemoError::Serialize {
            context: "pending apply".to_string(),
            source: e,
        });
    }

    Ok(format!(
        "Selected {}. Committed {} fact(s), discarded {}, failed {}, kept {}.\nLog: {}\n",
        report.selected,
        report.summary.committed,
        report.summary.discarded,
        report.summary.failed,
        report.summary.remaining,
        log_path.display()
    ))
}

fn run_pending_review(
    from: Option<PathBuf>,
    store_path: &Path,
    config: Config,
    json: bool,
) -> Result<String, AideMemoError> {
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

    let wiki = AideMemo::open(store_path, config)?;

    let (kept, summary) = apply_review(
        &final_state.entries,
        &final_state.commit,
        &final_state.discard,
        &wiki,
    );
    write_jsonl(&log_path, &kept)?;

    if json {
        let report = PendingApplyReport {
            log_path: log_path.display().to_string(),
            selected: final_state.commit.len() + final_state.discard.len(),
            summary,
        };
        return serde_json::to_string_pretty(&report).map_err(|e| AideMemoError::Serialize {
            context: "pending review".to_string(),
            source: e,
        });
    }

    Ok(format!(
        "Committed {} fact(s), discarded {}, failed {}, kept {}.\nLog: {}\n",
        summary.committed,
        summary.discarded,
        summary.failed,
        summary.remaining,
        log_path.display()
    ))
}

fn select_indices(
    len: usize,
    all: bool,
    indices: Option<&str>,
) -> Result<HashSet<usize>, AideMemoError> {
    if all && indices.is_some() {
        return Err(AideMemoError::InvalidInput(
            "pass either --all or --indices, not both".to_string(),
        ));
    }
    if all {
        return Ok((0..len).collect());
    }
    let Some(raw) = indices else {
        return Err(AideMemoError::InvalidInput(
            "pass --all or --indices LIST".to_string(),
        ));
    };

    let mut out = HashSet::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let start = parse_one_based_index(start.trim(), len)?;
            let end = parse_one_based_index(end.trim(), len)?;
            if start > end {
                return Err(AideMemoError::InvalidInput(format!(
                    "invalid descending range: {part}"
                )));
            }
            for idx in start..=end {
                out.insert(idx);
            }
        } else {
            out.insert(parse_one_based_index(part, len)?);
        }
    }
    Ok(out)
}

fn parse_one_based_index(raw: &str, len: usize) -> Result<usize, AideMemoError> {
    let n = raw
        .parse::<usize>()
        .map_err(|e| AideMemoError::InvalidInput(format!("invalid pending index {raw:?}: {e}")))?;
    if n == 0 || n > len {
        return Err(AideMemoError::InvalidInput(format!(
            "pending index {n} out of range 1..={len}"
        )));
    }
    Ok(n - 1)
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
        let path = dir.path().join("aidememo-pending.jsonl");
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
        // We can't easily open a real AideMemo in a unit test
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
        let mut config = Config::default();
        if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
            config.store.backend = "redb".to_string();
        }
        let store = dir.path().join(if config.store.backend == "redb" {
            "wiki.redb"
        } else {
            "wiki.sqlite"
        });
        let wiki = AideMemo::open(&store, config).unwrap();

        let (kept, summary) = apply_review(&entries, &commit, &discard, &wiki);
        assert_eq!(summary.committed, 1);
        assert_eq!(summary.discarded, 1);
        assert_eq!(summary.remaining, 1);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].content, "leave me");
    }

    #[test]
    fn select_indices_supports_ranges_and_dedup() {
        let selected = select_indices(5, false, Some("1,3-5,4")).unwrap();
        assert_eq!(selected.len(), 4);
        assert!(selected.contains(&0));
        assert!(selected.contains(&2));
        assert!(selected.contains(&3));
        assert!(selected.contains(&4));
    }

    #[test]
    fn select_indices_all_selects_everything() {
        let selected = select_indices(3, true, None).unwrap();
        assert_eq!(selected, [0, 1, 2].into_iter().collect());
    }

    #[test]
    fn select_indices_rejects_bad_shapes() {
        assert!(select_indices(3, false, None).is_err());
        assert!(select_indices(3, true, Some("1")).is_err());
        assert!(select_indices(3, false, Some("0")).is_err());
        assert!(select_indices(3, false, Some("4")).is_err());
        assert!(select_indices(3, false, Some("3-1")).is_err());
    }

    #[test]
    fn pending_list_json_reports_count_and_shown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aidememo-pending.jsonl");
        write_jsonl(
            &path,
            &[
                entry("Use HNSW", "decision", 0.95),
                entry("Always lint", "convention", 0.85),
            ],
        )
        .unwrap();
        let raw = run_pending_list(Some(path), Some(1), true).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["count"], 2);
        assert_eq!(v["shown"], 1);
        assert_eq!(v["entries"][0]["index"], 1);
    }

    #[test]
    fn pending_stats_json_reports_type_and_confidence_distribution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aidememo-pending.jsonl");
        write_jsonl(
            &path,
            &[
                entry("Use HNSW", "decision", 0.95),
                entry("Always lint", "convention", 0.85),
                entry("Noise", "note", 0.20),
            ],
        )
        .unwrap();
        let raw = run_pending_stats(Some(path), true).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["total"], 3);
        assert_eq!(v["by_type"]["decision"], 1);
        assert_eq!(v["confidence_buckets"]["0.0-0.5"], 1);
        assert_eq!(v["confidence_buckets"]["0.7-0.9"], 1);
        assert_eq!(v["confidence_buckets"]["0.9-1.0"], 1);
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
        assert_eq!(
            default_log_path(),
            dir.path().join("aidememo-pending.jsonl")
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("HERMES_STATE_DIR", v),
                None => std::env::remove_var("HERMES_STATE_DIR"),
            }
        }
    }
}
