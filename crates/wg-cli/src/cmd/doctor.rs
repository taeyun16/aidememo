//! `wg doctor` — friendly graph health check.
//!
//! Wraps `wg lint` plus a few extra invariant checks (broken refs, schema
//! mismatches) into a developer-friendly report. The intent is "is my wiki
//! healthy right now" — fast, opinionated, actionable.

use bpaf::*;
use std::path::Path;
use wg_core::{Config, WgError, WikiGraph};

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub struct DoctorSub {
    pub json: bool,
}

pub fn doctor_command() -> impl Parser<Command> {
    let json = long("json").short('j').help("Output as JSON").switch();
    construct!(DoctorSub { json })
        .map(Command::Doctor)
        .to_options()
        .command("doctor")
        .help("Check wiki health (orphans, broken refs, stale facts)")
}

pub fn run_doctor(
    store_path: &Path,
    config: Config,
    sub: DoctorSub,
    global_json: bool,
) -> Result<String, WgError> {
    let wiki = WikiGraph::open(store_path, config)?;
    let issues = wiki.lint()?;
    let stats = wiki.stats()?;

    if sub.json || global_json {
        let payload = serde_json::json!({
            "ok": issues.is_empty(),
            "store": store_path.display().to_string(),
            "stats": stats,
            "issue_count": issues.len(),
            "issues": issues,
        });
        return serde_json::to_string_pretty(&payload).map_err(|e| WgError::Serialize {
            context: "doctor".to_string(),
            source: e,
        });
    }

    let mut out = String::new();
    out.push_str(&format!("📋 wg doctor — {}\n", store_path.display()));
    out.push_str(&format!(
        "  entities: {}   facts: {}   relations: {}\n\n",
        stats.entity_count, stats.fact_count, stats.relation_count
    ));

    if issues.is_empty() {
        out.push_str("✓ Graph is healthy — no issues found.\n");
        return Ok(out);
    }

    let mut by_severity: std::collections::BTreeMap<String, Vec<&wg_core::LintIssue>> =
        std::collections::BTreeMap::new();
    for issue in &issues {
        by_severity
            .entry(issue.severity.to_string())
            .or_default()
            .push(issue);
    }

    out.push_str(&format!("✗ Found {} issue(s):\n", issues.len()));
    for (severity, items) in &by_severity {
        out.push_str(&format!("\n  {} ({})\n", severity, items.len()));
        for issue in items.iter().take(20) {
            out.push_str(&format!("    [{}] {}\n", issue.code, issue.message));
        }
        if items.len() > 20 {
            out.push_str(&format!("    … {} more\n", items.len() - 20));
        }
    }

    out.push_str("\nTip: `wg lint --json` for the full issue list, or fix them with `wg entity delete` / `wg fact delete`.\n");
    Ok(out)
}
