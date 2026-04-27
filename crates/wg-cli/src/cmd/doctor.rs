//! `wg doctor` — friendly graph health check.
//!
//! Wraps `wg lint` plus a few extra invariant checks (broken refs, schema
//! mismatches) into a developer-friendly report. Also surfaces *agent
//! integration* status so operators can see at a glance which agents
//! have wg installed as a skill and registered as an MCP server,
//! mirroring what `wg skill install --list-targets` and
//! `wg mcp-install --list-targets` would have written.

use bpaf::*;
use std::path::{Path, PathBuf};
use wg_core::{Config, WgError, WikiGraph};

use crate::cmd::Command;
use crate::cmd::mcp_install::{codex_config_path, cursor_config_path, verify_registered};
use crate::cmd::skill::{supported_targets, target_skills_dir};

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
    let agents = collect_agent_integration();

    if sub.json || global_json {
        let payload = serde_json::json!({
            "ok": issues.is_empty(),
            "store": store_path.display().to_string(),
            "stats": stats,
            "issue_count": issues.len(),
            "issues": issues,
            "agents": agents,
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

    out.push_str(&format_agent_integration(&agents));

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

// ---------------------------------------------------------------------------
// Agent integration probe
// ---------------------------------------------------------------------------

/// Per-agent integration status. Skill check looks for the bundled
/// `SKILL.md` in the agent's standard skills directory; MCP check
/// either parses the agent's own config file (codex / cursor) or
/// shells out to `<bin> mcp list` and word-boundary-matches `wg`.
#[derive(Debug, serde::Serialize)]
pub(crate) struct AgentStatus {
    target: &'static str,
    /// Where SKILL.md is expected to land. `None` for agents that
    /// don't consume agentskills.io files (codex / cursor).
    skill_path: Option<String>,
    /// Whether SKILL.md was found at `skill_path`. `None` mirrors
    /// `skill_path = None`.
    skill_installed: Option<bool>,
    /// Where the MCP entry would live (file path or `<bin> mcp list`).
    mcp_detail: String,
    /// `Some(true)` when wg is registered, `Some(false)` when not,
    /// `None` when the check couldn't run (binary not on PATH, etc).
    mcp_registered: Option<bool>,
}

const AGENTS: &[&str] = &["claude", "hermes", "openclaw", "codex", "cursor"];

fn collect_agent_integration() -> Vec<AgentStatus> {
    AGENTS
        .iter()
        .map(|target| AgentStatus {
            target,
            skill_path: skill_path_for(target).map(|p| p.display().to_string()),
            skill_installed: skill_path_for(target).map(|p| p.join("SKILL.md").exists()),
            mcp_detail: mcp_detail_for(target),
            mcp_registered: check_mcp(target),
        })
        .collect()
}

fn skill_path_for(target: &str) -> Option<PathBuf> {
    if !supported_targets().contains(&target) {
        return None;
    }
    target_skills_dir(target)
}

fn mcp_detail_for(target: &str) -> String {
    match target {
        "claude" => "via `claude mcp list`".to_string(),
        "hermes" => "via `hermes mcp list`".to_string(),
        "openclaw" => "via `openclaw mcp list`".to_string(),
        "codex" => codex_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~/.codex/config.toml".to_string()),
        "cursor" => cursor_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~/.cursor/mcp.json".to_string()),
        _ => "(unknown)".to_string(),
    }
}

fn check_mcp(target: &str) -> Option<bool> {
    match target {
        "claude" | "hermes" | "openclaw" => verify_registered(target, "wg"),
        "codex" => check_codex_config(),
        "cursor" => check_cursor_config(),
        _ => None,
    }
}

fn check_codex_config() -> Option<bool> {
    let path = codex_config_path().ok()?;
    if !path.exists() {
        return Some(false);
    }
    let body = std::fs::read_to_string(&path).ok()?;
    let parsed: toml::Value = body.parse().ok()?;
    Some(
        parsed
            .as_table()
            .and_then(|t| t.get("mcp_servers"))
            .and_then(|s| s.as_table())
            .map(|s| s.contains_key("wg"))
            .unwrap_or(false),
    )
}

fn check_cursor_config() -> Option<bool> {
    let path = cursor_config_path().ok()?;
    if !path.exists() {
        return Some(false);
    }
    let body = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    Some(
        parsed
            .get("mcpServers")
            .and_then(|s| s.as_object())
            .map(|s| s.contains_key("wg"))
            .unwrap_or(false),
    )
}

fn format_agent_integration(agents: &[AgentStatus]) -> String {
    let mut out = String::from("Agent integration:\n");
    for a in agents {
        let skill_marker = match a.skill_installed {
            Some(true) => "✓".to_string(),
            Some(false) => "—".to_string(),
            None => " ".to_string(),
        };
        let mcp_marker = match a.mcp_registered {
            Some(true) => "✓".to_string(),
            Some(false) => "—".to_string(),
            None => "?".to_string(), // CLI not reachable
        };
        let skill_label = a.skill_path.as_deref().unwrap_or("(no skill format)");
        out.push_str(&format!(
            "  {:<9}  skill {} {:<40}  mcp {} {}\n",
            a.target, skill_marker, skill_label, mcp_marker, a.mcp_detail
        ));
    }
    out.push_str(
        "    legend:  ✓ installed   — not installed   ? cli unavailable / could not check\n\n",
    );
    out
}
