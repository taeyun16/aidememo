//! `aidememo doctor` — friendly graph health check.
//!
//! Wraps `aidememo lint` plus a few extra invariant checks (broken refs, schema
//! mismatches) into a developer-friendly report. Also surfaces *agent
//! integration* status so operators can see at a glance which agents
//! have aidememo installed as a skill and registered as an MCP server,
//! mirroring what `aidememo skill install --list-targets` and
//! `aidememo mcp-install --list-targets` would have written.

use aidememo_core::{AideMemo, AideMemoError, Config, FactListOpts, FactRecord, FactType};
use bpaf::*;
use std::path::Path;

use crate::cmd::Command;
use crate::cmd::daemon::{self, RegistryState};
use crate::cmd::mcp_install::{
    codex_config_path, cursor_config_path, opencode_config_path, verify_registered,
};
use crate::cmd::skill::{supported_targets, target_agents_md_path, target_skills_dir};

#[derive(Debug, Clone)]
pub struct DoctorSub {
    pub json: bool,
    pub fix: bool,
    pub shell: bool,
}

pub fn doctor_command() -> impl Parser<Command> {
    let json = long("json").short('j').help("Output as JSON").switch();
    let fix = long("fix")
        .help(
            "Print the install commands that would close every gap in the \
             agent integration matrix. Doesn't run anything — copy the \
             lines you want into your shell.",
        )
        .switch();
    let shell = long("shell")
        .help(
            "With --fix, emit only the install commands (one per line, no \
             decoration) so the output can be piped into a shell. Pairs \
             with --fix; ignored otherwise.",
        )
        .switch();
    construct!(DoctorSub { json, fix, shell })
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
) -> Result<String, AideMemoError> {
    let memory = collect_memory(store_path, &config);
    let sharing = collect_sharing_status(store_path, &config);
    let wiki = AideMemo::open(store_path, config.clone())?;
    let issues = wiki.lint()?;
    let stats = wiki.stats()?;
    let superseded = wiki.fact_count_superseded()? as u64;
    let memory = memory.with_counts(stats.fact_count, superseded);
    let adaptation = collect_adaptation(&wiki, &config);
    let agents = collect_agent_integration();
    let workflow = collect_workflow_status(&wiki, &agents)?;
    let mut fixes = collect_fix_suggestions(&agents);
    fixes.extend(memory.advisories());
    fixes.extend(adaptation.advisories());
    fixes.extend(sharing.advisories());

    // `--fix --shell` short-circuits to a pipe-friendly view: just
    // the bare commands, one per line, nothing else. Designed for
    // `aidememo doctor --fix --shell | sh`. We deliberately omit a trailing
    // newline because `main.rs` adds one via `println!`; emitting our
    // own here would leave an empty final line that breaks `lines()`-
    // based downstream parsers.
    if sub.fix && sub.shell {
        return Ok(fixes
            .iter()
            .map(|f| f.command.as_str())
            .collect::<Vec<_>>()
            .join("\n"));
    }

    if sub.json || global_json {
        let payload = serde_json::json!({
            "ok": issues.is_empty(),
            "store": store_path.display().to_string(),
            "stats": stats,
            "issue_count": issues.len(),
            "issues": issues,
            "agents": agents,
            "memory": memory,
            "adaptation": adaptation,
            "sharing": sharing,
            "workflow": workflow,
            // Always emitted in JSON: tooling that consumes this
            // shouldn't have to re-derive the suggestion list.
            "fixes": fixes,
        });
        return serde_json::to_string_pretty(&payload).map_err(|e| AideMemoError::Serialize {
            context: "doctor".to_string(),
            source: e,
        });
    }

    let mut out = String::new();
    out.push_str(&format!("📋 aidememo doctor — {}\n", store_path.display()));
    let supers_suffix = if superseded > 0 {
        format!(" ({} superseded)", superseded)
    } else {
        String::new()
    };
    out.push_str(&format!(
        "  entities: {}   facts: {}{}   relations: {}\n\n",
        stats.entity_count, stats.fact_count, supers_suffix, stats.relation_count
    ));

    out.push_str(&format_memory(&memory));
    out.push_str(&format_adaptation(&adaptation));
    out.push_str(&format_sharing(&sharing));
    out.push_str(&format_workflow(&workflow));
    out.push_str(&format_agent_integration(&agents));

    if sub.fix {
        out.push_str(&format_fix_suggestions(&fixes));
    } else if !fixes.is_empty() {
        out.push_str(&format!(
            "Tip: {} doctor fix suggestion(s) available. Run `aidememo doctor --fix` for the commands.\n\n",
            fixes.len()
        ));
    }

    if issues.is_empty() {
        out.push_str("✓ Graph is healthy — no issues found.\n");
        return Ok(out);
    }

    let mut by_severity: std::collections::BTreeMap<String, Vec<&aidememo_core::LintIssue>> =
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

    out.push_str("\nTip: `aidememo lint --json` for the full issue list, or fix them with `aidememo entity delete` / `aidememo fact delete`.\n");
    Ok(out)
}

// ---------------------------------------------------------------------------
// Agent integration probe
// ---------------------------------------------------------------------------

/// Per-agent integration status. Skill check looks for the bundled
/// `SKILL.md` in the agent's standard skills directory; MCP check
/// either parses the agent's own config file (codex / cursor) or
/// shells out to `<bin> mcp list` and word-boundary-matches `aidememo`.
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
    /// `Some(true)` when aidememo is registered, `Some(false)` when not,
    /// `None` when the check couldn't run (binary not on PATH, etc).
    mcp_registered: Option<bool>,
}

const AGENTS: &[&str] = &[
    "claude", "hermes", "openclaw", "codex", "cursor", "opencode", "pi",
];

fn collect_agent_integration() -> Vec<AgentStatus> {
    AGENTS
        .iter()
        .map(|target| {
            let (skill_path, skill_installed) = skill_status_for(target);
            AgentStatus {
                target,
                skill_path,
                skill_installed,
                mcp_detail: mcp_detail_for(target),
                mcp_registered: check_mcp(target),
            }
        })
        .collect()
}

/// Resolve the skill destination for an agent and probe whether
/// aidememo's bundled skill is already there. Two shapes:
/// * directory targets (claude / hermes / openclaw / agents) — the
///   skill lives at `<dir>/SKILL.md`;
/// * agents.md targets (opencode / pi) — the skill is a section
///   inside a single `AGENTS.md` file, so we probe for our marker
///   instead of an `exists()` on the whole file.
fn skill_status_for(target: &str) -> (Option<String>, Option<bool>) {
    if !supported_targets().contains(&target) {
        return (None, None);
    }
    if let Some(dir) = target_skills_dir(target) {
        let installed = dir.join("SKILL.md").exists();
        return (Some(dir.display().to_string()), Some(installed));
    }
    if let Some(file) = target_agents_md_path(target) {
        let installed = std::fs::read_to_string(&file)
            .ok()
            .map(|c| c.contains("<!-- BEGIN aidememo-skill -->"))
            .unwrap_or(false);
        return (Some(file.display().to_string()), Some(installed));
    }
    (None, None)
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
        "opencode" => opencode_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~/.config/opencode/opencode.json".to_string()),
        // pi rejects MCP upstream — record the fact in the matrix
        // instead of leaving it as "(unknown)".
        "pi" => "(no MCP — pi only consumes skills)".to_string(),
        _ => "(unknown)".to_string(),
    }
}

fn check_mcp(target: &str) -> Option<bool> {
    match target {
        "claude" | "hermes" | "openclaw" => verify_registered(target, "aidememo"),
        "codex" => check_codex_config(),
        "cursor" => check_cursor_config(),
        "opencode" => check_opencode_config(),
        // pi has no MCP; report `None` (n/a) rather than false.
        _ => None,
    }
}

fn check_opencode_config() -> Option<bool> {
    let path = opencode_config_path().ok()?;
    if !path.exists() {
        return Some(false);
    }
    let body = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    Some(
        parsed
            .get("mcp")
            .and_then(|s| s.as_object())
            .map(|s| s.contains_key("aidememo"))
            .unwrap_or(false),
    )
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
            .map(|s| s.contains_key("aidememo"))
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
            .map(|s| s.contains_key("aidememo"))
            .unwrap_or(false),
    )
}

/// One actionable fix the user can run to close a gap in the agent
/// integration matrix. The shell command is a copy-pasteable string —
/// emitting structured fields too lets `aidememo doctor --json` callers
/// drive their own UI without re-deriving the command from the kind.
#[derive(Debug, serde::Serialize)]
pub(crate) struct FixSuggestion {
    target: &'static str,
    /// `"skill"` or `"mcp"` — narrows what the user is being asked
    /// to install.
    kind: &'static str,
    command: String,
    /// One-line rationale. Stays in JSON so consumers can show it
    /// next to the command without re-deriving from `kind`.
    reason: String,
}

fn collect_fix_suggestions(agents: &[AgentStatus]) -> Vec<FixSuggestion> {
    let mut out = Vec::new();
    for a in agents {
        // Skill gap: target supports skills (skill_path is Some) but
        // SKILL.md isn't there yet.
        if matches!(a.skill_installed, Some(false)) {
            out.push(FixSuggestion {
                target: a.target,
                kind: "skill",
                command: format!("aidememo skill install --target {}", a.target),
                reason: format!(
                    "no SKILL.md at {}",
                    a.skill_path.as_deref().unwrap_or("<unknown>")
                ),
            });
        }
        // MCP gap: registration confirmed missing. We *don't* suggest
        // a fix when the check returned None — that usually means the
        // agent's CLI isn't installed, in which case `aidememo mcp-install`
        // would just fail. Surfacing that as a fix would be misleading.
        if matches!(a.mcp_registered, Some(false)) {
            out.push(FixSuggestion {
                target: a.target,
                kind: "mcp",
                command: format!("aidememo mcp-install --target {}", a.target),
                reason: format!("aidememo not registered ({})", a.mcp_detail),
            });
        }
    }
    out
}

fn format_fix_suggestions(fixes: &[FixSuggestion]) -> String {
    if fixes.is_empty() {
        return "✓ No doctor fix suggestions.\n\n".to_string();
    }
    let mut out = format!("Suggested fixes ({}):\n", fixes.len());
    for f in fixes {
        out.push_str(&format!(
            "  $ {:<40}  # {}: {}\n",
            f.command, f.target, f.reason
        ));
    }
    out.push_str("  (none of these run automatically — copy what you want)\n\n");
    out
}

// ---------------------------------------------------------------------------
// Workflow readiness
// ---------------------------------------------------------------------------

const WORKFLOW_RECENT_DAYS: u64 = 30;

#[derive(Debug, serde::Serialize)]
pub(crate) struct WorkflowReport {
    pub ready: bool,
    pub mcp_ready: bool,
    pub recent_window_days: u64,
    pub recent_ticket_count: usize,
    pub recent_tickets: Vec<WorkflowTicketSummary>,
    pub hints: Vec<WorkflowHint>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct WorkflowTicketSummary {
    pub id: String,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub timestamp_ms: u64,
    pub preview: String,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct WorkflowHint {
    pub code: &'static str,
    pub severity: &'static str,
    pub message: String,
    pub action: String,
}

fn collect_workflow_status(
    wiki: &AideMemo,
    agents: &[AgentStatus],
) -> Result<WorkflowReport, AideMemoError> {
    let now = aidememo_core::time::current_epoch_ms();
    let since = now.saturating_sub(WORKFLOW_RECENT_DAYS * 24 * 60 * 60 * 1_000);
    let mut tickets: Vec<FactRecord> = wiki
        .fact_list(FactListOpts {
            fact_type: Some(FactType::Question),
            since: Some(since),
            current_only: true,
            ..Default::default()
        })?
        .into_iter()
        .filter(is_workflow_ticket)
        .collect();

    tickets.sort_by_key(|f| std::cmp::Reverse(f.observed_at.unwrap_or(f.created_at)));
    let recent_ticket_count = tickets.len();
    let recent_tickets = tickets
        .iter()
        .take(5)
        .map(summarize_workflow_ticket)
        .collect();
    let suggested_source_id = tickets.iter().find_map(|f| {
        f.source_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    });

    let mcp_ready = agents
        .iter()
        .any(|a| matches!(a.mcp_registered, Some(true)));
    let skill_ready = agents
        .iter()
        .any(|a| matches!(a.skill_installed, Some(true)));

    let mut hints = Vec::new();
    if !mcp_ready {
        hints.push(WorkflowHint {
            code: "workflow_no_mcp_agent",
            severity: "error",
            message: "no checked agent has aidememo registered as an MCP server".to_string(),
            action: mcp_install_action(suggested_source_id),
        });
    }
    if !skill_ready {
        hints.push(WorkflowHint {
            code: "workflow_no_skill_prompt",
            severity: "warn",
            message: "no checked agent has the AideMemo workflow skill/prompt installed"
                .to_string(),
            action: "aidememo skill install --target claude".to_string(),
        });
    }
    if recent_ticket_count == 0 {
        hints.push(WorkflowHint {
            code: "workflow_no_recent_tickets",
            severity: "info",
            message: format!(
                "no workflow-start ticket facts found in the last {WORKFLOW_RECENT_DAYS} days"
            ),
            action: "aidememo workflow start \"Fix sparse ticket\" --source github:org/repo#123"
                .to_string(),
        });
    }

    Ok(WorkflowReport {
        ready: mcp_ready,
        mcp_ready,
        recent_window_days: WORKFLOW_RECENT_DAYS,
        recent_ticket_count,
        recent_tickets,
        hints,
    })
}

fn is_workflow_ticket(fact: &FactRecord) -> bool {
    fact.tags.iter().any(|t| t == "workflow-start") && fact.tags.iter().any(|t| t == "ticket")
}

fn summarize_workflow_ticket(fact: &FactRecord) -> WorkflowTicketSummary {
    WorkflowTicketSummary {
        id: fact.id.to_string(),
        source: fact.source.clone(),
        source_id: fact.source_id.clone(),
        timestamp_ms: fact.observed_at.unwrap_or(fact.created_at),
        preview: preview(&fact.content, 96),
    }
}

fn mcp_install_action(source_id: Option<&str>) -> String {
    match source_id {
        Some(source_id) => format!(
            "aidememo mcp-install --target codex --source-id {}",
            shell_arg(source_id)
        ),
        None => "aidememo mcp-install --target codex".to_string(),
    }
}

fn shell_arg(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn preview(s: &str, max_chars: usize) -> String {
    let normalized = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for ch in normalized.chars().take(max_chars) {
        out.push(ch);
    }
    if normalized.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn format_workflow(workflow: &WorkflowReport) -> String {
    let mut out = String::from("Workflow readiness:\n");
    let ready = if workflow.ready { "yes" } else { "no" };
    let mcp = if workflow.mcp_ready { "yes" } else { "no" };
    out.push_str(&format!(
        "  ready: {ready}   mcp: {mcp}   recent tickets ({}d): {}\n",
        workflow.recent_window_days, workflow.recent_ticket_count
    ));
    for ticket in &workflow.recent_tickets {
        let source = ticket.source.as_deref().unwrap_or("-");
        let source_id = ticket.source_id.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "    {}  source={}  source_id={}  {}\n",
            ticket.id, source, source_id, ticket.preview
        ));
    }
    if workflow.hints.is_empty() {
        out.push_str("  ✓ workflow setup looks ready\n\n");
    } else {
        out.push_str("  hints\n");
        for hint in &workflow.hints {
            out.push_str(&format!(
                "    [{}] {} — {}\n      action: {}\n",
                hint.code, hint.severity, hint.message, hint.action
            ));
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Shared-store ergonomics
// ---------------------------------------------------------------------------

const SERVERLESS_RECOMMENDED_WRITERS: u8 = 4;
const HIGH_CONCURRENCY_WRITERS: u8 = 8;
const RECOMMENDED_LOCK_RETRY_MS: u64 = 5_000;

#[derive(Debug, serde::Serialize)]
pub(crate) struct SharingReport {
    pub lock_retry_ms: u64,
    pub serverless_recommended_writers: u8,
    pub high_concurrency_writers: u8,
    pub daemon: DaemonStatus,
    pub recommended_mode: &'static str,
    pub hints: Vec<SharingHint>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct DaemonStatus {
    pub state: &'static str,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub store: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct SharingHint {
    pub code: &'static str,
    pub severity: &'static str,
    pub message: String,
    pub action: String,
}

pub(crate) fn collect_sharing_status(store_path: &Path, config: &Config) -> SharingReport {
    let daemon = match daemon::registry_state(store_path) {
        RegistryState::Healthy(reg) => DaemonStatus {
            state: "healthy",
            port: Some(reg.port),
            pid: Some(reg.pid),
            store: Some(reg.store.display().to_string()),
        },
        RegistryState::StaleRegistry => DaemonStatus {
            state: "stale_registry",
            port: None,
            pid: None,
            store: None,
        },
        RegistryState::None => DaemonStatus {
            state: "none",
            port: None,
            pid: None,
            store: None,
        },
    };

    let mut hints = Vec::new();
    if config.store.lock_retry_ms == 0 && daemon.state != "healthy" {
        hints.push(SharingHint {
            code: "sharing_retry_disabled",
            severity: "info",
            message: format!(
                "serverless shared writes fail fast without lock retry; measured smooth path uses {RECOMMENDED_LOCK_RETRY_MS} ms"
            ),
            action: format!("aidememo config set store.lock_retry_ms {RECOMMENDED_LOCK_RETRY_MS}"),
        });
    }
    if daemon.state == "stale_registry" {
        hints.push(SharingHint {
            code: "sharing_stale_daemon_registry",
            severity: "warn",
            message: "daemon registry exists but the daemon is not healthy for this store"
                .to_string(),
            action: "aidememo daemon status".to_string(),
        });
    }
    if daemon.state != "healthy" {
        hints.push(SharingHint {
            code: "sharing_daemon_for_high_concurrency",
            severity: "info",
            message: format!(
                "use a shared daemon when more than {SERVERLESS_RECOMMENDED_WRITERS} agents write to the same store concurrently"
            ),
            action: "aidememo daemon start".to_string(),
        });
    }
    if let Some(hint) = backend_path_hint(store_path, &config.store.backend) {
        hints.push(hint);
    }

    let recommended_mode = if daemon.state == "healthy" {
        "daemon"
    } else if config.store.lock_retry_ms >= RECOMMENDED_LOCK_RETRY_MS {
        "serverless_retry"
    } else {
        "serverless_fail_fast"
    };

    SharingReport {
        lock_retry_ms: config.store.lock_retry_ms,
        serverless_recommended_writers: SERVERLESS_RECOMMENDED_WRITERS,
        high_concurrency_writers: HIGH_CONCURRENCY_WRITERS,
        daemon,
        recommended_mode,
        hints,
    }
}

impl SharingReport {
    fn advisories(&self) -> Vec<FixSuggestion> {
        let mut out = Vec::new();
        if self.lock_retry_ms == 0 && self.daemon.state != "healthy" {
            out.push(FixSuggestion {
                target: "store",
                kind: "sharing",
                command: format!("aidememo config set store.lock_retry_ms {RECOMMENDED_LOCK_RETRY_MS}"),
                reason: format!(
                    "smooth serverless sharing up to {SERVERLESS_RECOMMENDED_WRITERS} concurrent writers"
                ),
            });
        }
        out
    }
}

fn backend_path_hint(store_path: &Path, backend: &str) -> Option<SharingHint> {
    let ext = store_path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)?;
    let backend = backend.to_ascii_lowercase();
    let suggested = match backend.as_str() {
        "redb" if ext == "sqlite" => Some((
            "redb",
            "redb",
            "aidememo config set store.path ./_meta/wiki.redb",
        )),
        "sqlite" | "libsqlite" if ext == "redb" => Some((
            "sqlite",
            "SQLite",
            "aidememo config set store.path ./_meta/wiki.sqlite",
        )),
        _ => None,
    }?;

    Some(SharingHint {
        code: "storage_backend_path_extension_mismatch",
        severity: "warn",
        message: format!(
            "store.backend is {} but store path ends with .{}; this works, but makes the persistence layer ambiguous",
            backend, ext
        ),
        action: format!(
            "rename or export/import the store to a .{} path for {} stores, then run `{}`",
            suggested.0, suggested.1, suggested.2
        ),
    })
}

fn format_sharing(sharing: &SharingReport) -> String {
    let mut out = String::from("Shared-store ergonomics:\n");
    out.push_str(&format!(
        "  mode: {}   lock_retry_ms: {}   serverless writers: <= {}\n",
        sharing.recommended_mode, sharing.lock_retry_ms, sharing.serverless_recommended_writers
    ));
    match (
        sharing.daemon.state,
        sharing.daemon.port,
        sharing.daemon.pid,
    ) {
        ("healthy", Some(port), Some(pid)) => {
            out.push_str(&format!("  daemon: healthy (pid={pid}, port={port})\n"));
        }
        ("stale_registry", _, _) => out.push_str("  daemon: stale registry\n"),
        _ => out.push_str("  daemon: not running\n"),
    }
    if sharing.hints.is_empty() {
        out.push_str("  ✓ sharing setup looks ready\n\n");
    } else {
        out.push_str("  hints\n");
        for hint in &sharing.hints {
            out.push_str(&format!(
                "    [{}] {} — {}\n      action: {}\n",
                hint.code, hint.severity, hint.message, hint.action
            ));
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Feedback + adaptation
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub(crate) struct AdaptationReport {
    pub use_adapter: bool,
    pub feedback_count: usize,
    pub has_adapter: bool,
    pub generation: u32,
    pub ready: bool,
    pub error: Option<String>,
}

fn collect_adaptation(wiki: &AideMemo, config: &Config) -> AdaptationReport {
    match wiki.adapt_status() {
        Ok(status) => AdaptationReport {
            use_adapter: config.search.use_adapter,
            feedback_count: status.feedback_count,
            has_adapter: status.has_adapter,
            generation: status.generation,
            ready: status.ready,
            error: None,
        },
        Err(e) => AdaptationReport {
            use_adapter: config.search.use_adapter,
            feedback_count: 0,
            has_adapter: false,
            generation: 0,
            ready: false,
            error: Some(e.to_string()),
        },
    }
}

impl AdaptationReport {
    fn advisories(&self) -> Vec<FixSuggestion> {
        let mut out = Vec::new();
        if self.error.is_none() && self.feedback_count > 0 && !self.has_adapter {
            out.push(FixSuggestion {
                target: "aidememo",
                kind: "adapt",
                command: "aidememo adapt train".to_string(),
                reason: format!(
                    "{} feedback item(s) recorded but no trained domain adapter exists",
                    self.feedback_count
                ),
            });
        }
        if self.error.is_none() && self.has_adapter && !self.use_adapter {
            out.push(FixSuggestion {
                target: "aidememo",
                kind: "adapt",
                command: "aidememo config set search.use_adapter true".to_string(),
                reason: "domain adapter exists but search.use_adapter is disabled".to_string(),
            });
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Memory + disk footprint
// ---------------------------------------------------------------------------

/// Snapshot of the wiki's on-disk artifacts and an estimate of how
/// much RAM a fully-loaded AideMemo uses. Numbers come from
/// `std::fs::metadata` for disk and a count × per-entry-size table
/// for RAM (see `MEMORY_PER_FACT_*` and `model_load_bytes`). Estimates,
/// not measurements — we don't read RSS to keep the doctor portable.
#[derive(Debug, serde::Serialize)]
pub(crate) struct MemoryReport {
    pub disk: Vec<MemoryEntry>,
    pub ram_estimate: Vec<MemoryEntry>,
    pub ram_total_bytes: u64,
    /// Mirrors the model name from config so the JSON consumer can
    /// surface "you're loading X" without re-reading the config.
    pub model_name: String,
    /// `Some(true)` when `search.semantic_index = "hnsw"` and the
    /// sidecar is present, `Some(false)` when configured but missing,
    /// `None` when the user opted out of HNSW.
    pub hnsw_sidecar_present: Option<bool>,
    /// Facts with `superseded_at` set. Filled in by `with_counts`.
    /// Drives the "stale HNSW after consolidate" advisory: when this
    /// is > 0 and the sidecar is present, the operator likely
    /// forgot to re-run `aidememo vector-rebuild --current-only`.
    pub superseded_facts: u64,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct MemoryEntry {
    pub name: String,
    pub bytes: u64,
    /// Free-form one-line annotation (path on disk, "N facts × X B",
    /// model id, etc).
    pub detail: String,
}

/// Per-fact RAM cost of the BM25 inverted index, drawn from PLAN.md
/// §9.1's "1만 fact 기준 ~5 MB" estimate.
const MEMORY_PER_FACT_BM25: u64 = 500;

/// Per-fact cost of a quantized fact embedding sitting in
/// `fact_embed_cache`: 256 i8 dims + serde overhead.
const MEMORY_PER_FACT_EMBED_CACHE: u64 = 272;

/// Per-fact cost of the in-memory HNSW (graph node + the f32 vector
/// it points at). PLAN.md §9.1 budgets ~1 KB/fact at 256 dims.
const MEMORY_PER_FACT_HNSW: u64 = 1024;

impl MemoryReport {
    /// `collect_memory` runs *before* `AideMemo::open` so it doesn't
    /// have a fact_count yet — its `ram_estimate` is seeded with just
    /// the model load row. `with_counts` then prepends the
    /// per-fact-scaled rows (bm25, embed cache, hnsw runtime) so the
    /// final order in the report is bm25 → embed → hnsw → model load.
    fn with_counts(mut self, fact_count: u64, superseded_count: u64) -> Self {
        self.superseded_facts = superseded_count;
        let bm25 = MemoryEntry {
            name: "bm25 index".into(),
            bytes: fact_count * MEMORY_PER_FACT_BM25,
            detail: format!("{} facts × ~{} B", fact_count, MEMORY_PER_FACT_BM25),
        };
        let embed = MemoryEntry {
            name: "fact embed cache".into(),
            bytes: fact_count * MEMORY_PER_FACT_EMBED_CACHE,
            detail: format!(
                "{} facts × ~{} B (i8)",
                fact_count, MEMORY_PER_FACT_EMBED_CACHE
            ),
        };
        let hnsw = MemoryEntry {
            name: "hnsw runtime".into(),
            bytes: match self.hnsw_sidecar_present {
                Some(true) => fact_count * MEMORY_PER_FACT_HNSW,
                _ => 0,
            },
            detail: match self.hnsw_sidecar_present {
                Some(true) => format!("{} facts × ~{} B", fact_count, MEMORY_PER_FACT_HNSW),
                Some(false) => {
                    "configured but sidecar missing — run `aidememo vector-rebuild`".into()
                }
                None => "n/a (semantic_index != hnsw)".into(),
            },
        };

        // collect_memory left exactly one entry in `ram_estimate`: the
        // model load row. Prepend the per-fact rows.
        let model = self.ram_estimate.pop().unwrap_or_else(|| MemoryEntry {
            name: "model load".into(),
            bytes: 0,
            detail: "unknown".into(),
        });
        self.ram_estimate = vec![bm25, embed, hnsw, model];
        self.ram_total_bytes = self.ram_estimate.iter().map(|e| e.bytes).sum();
        self
    }

    /// Surface advisories for the fix-suggestions block when the
    /// memory picture suggests an obvious tweak (`aidememo vector-rebuild`
    /// for a missing sidecar).
    fn advisories(&self) -> Vec<FixSuggestion> {
        let mut out = Vec::new();
        if self.hnsw_sidecar_present == Some(false) {
            out.push(FixSuggestion {
                target: "aidememo",
                kind: "memory",
                command: "aidememo vector-rebuild".to_string(),
                reason: "search.semantic_index = hnsw but sidecar is missing".to_string(),
            });
        }
        if self.hnsw_sidecar_present == Some(true) && self.superseded_facts > 0 {
            out.push(FixSuggestion {
                target: "aidememo",
                kind: "memory",
                command: "aidememo vector-rebuild --current-only".to_string(),
                reason: format!(
                    "{} superseded fact(s) still in the HNSW sidecar — \
                     `consolidate` left them indexed",
                    self.superseded_facts
                ),
            });
        }
        out
    }
}

impl Clone for MemoryEntry {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            bytes: self.bytes,
            detail: self.detail.clone(),
        }
    }
}

fn collect_memory(store_path: &Path, config: &Config) -> MemoryReport {
    let mut disk: Vec<MemoryEntry> = Vec::new();
    if let Ok(meta) = std::fs::metadata(store_path) {
        disk.push(MemoryEntry {
            name: "store".into(),
            bytes: meta.len(),
            detail: store_path.display().to_string(),
        });
    }
    let hnsw_path = store_path.with_extension("hnsw.bin");
    let hnsw_present = hnsw_path.exists();
    if hnsw_present {
        if let Ok(meta) = std::fs::metadata(&hnsw_path) {
            disk.push(MemoryEntry {
                name: "hnsw sidecar".into(),
                bytes: meta.len(),
                detail: hnsw_path.display().to_string(),
            });
        }
    }

    let model_bytes = model_load_bytes(&config.model.name);
    let model_detail = match model_bytes {
        Some(_) => config.model.name.clone(),
        None => format!("{} (unknown size)", config.model.name),
    };

    let hnsw_sidecar_present = if config.search.semantic_index == "hnsw" {
        Some(hnsw_present)
    } else {
        None
    };

    // Stash just the model entry in `ram_estimate` so `with_counts`
    // can read it back; counts-dependent rows get added then.
    let ram_estimate = vec![MemoryEntry {
        name: "model load".into(),
        bytes: model_bytes.unwrap_or(0),
        detail: model_detail,
    }];

    MemoryReport {
        disk,
        ram_estimate,
        ram_total_bytes: 0, // recomputed in `with_counts`
        model_name: config.model.name.clone(),
        hnsw_sidecar_present,
        superseded_facts: 0, // populated in `with_counts`
    }
}

/// Best-effort RAM footprint for a Model2Vec model by name, mapping
/// the well-known potion lineup. Returns `None` for unrecognized
/// model names so doctor can print "unknown" instead of guessing.
fn model_load_bytes(name: &str) -> Option<u64> {
    let lower = name.to_lowercase();
    let mb = match () {
        _ if lower.contains("multilingual-32m") => 32,
        _ if lower.contains("multilingual-128m") || lower.contains("base-128m") => 128,
        _ if lower.contains("base-32m") => 32,
        _ if lower.contains("base-8m") => 8,
        _ if lower.contains("base-4m") || lower.contains("base-2m") => 4,
        _ => return None,
    };
    Some(mb * 1024 * 1024)
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_memory(memory: &MemoryReport) -> String {
    let mut out = String::from("Memory:\n");

    if !memory.disk.is_empty() {
        out.push_str("  disk\n");
        for entry in &memory.disk {
            out.push_str(&format!(
                "    {:<18}  {:>9}   ({})\n",
                entry.name,
                format_bytes(entry.bytes),
                entry.detail
            ));
        }
    }

    if !memory.ram_estimate.is_empty() {
        out.push_str("  ram (estimate)\n");
        for entry in &memory.ram_estimate {
            out.push_str(&format!(
                "    {:<18}  ~{:>8}   ({})\n",
                entry.name,
                format_bytes(entry.bytes),
                entry.detail
            ));
        }
        out.push_str(&format!(
            "    {:<18}  ~{:>8}\n",
            "total",
            format_bytes(memory.ram_total_bytes)
        ));
    }
    out.push('\n');
    out
}

fn format_adaptation(adaptation: &AdaptationReport) -> String {
    let mut out = String::from("Feedback adaptation:\n");
    if let Some(error) = &adaptation.error {
        out.push_str(&format!("  unavailable: {error}\n\n"));
        return out;
    }
    let status = if adaptation.ready {
        "ready"
    } else if adaptation.has_adapter {
        "trained"
    } else {
        "not trained"
    };
    out.push_str(&format!(
        "  feedback: {}   adapter: {}   generation: {}   search.use_adapter: {}\n\n",
        adaptation.feedback_count, status, adaptation.generation, adaptation.use_adapter
    ));
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use aidememo_core::{EntityInput, EntityType, FactInput};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn config_with_model(name: &str, quantize: bool, semantic_index: &str) -> Config {
        let mut c = Config::default();
        c.model.name = name.to_string();
        c.model.quantize = quantize;
        c.search.semantic_index = semantic_index.to_string();
        c
    }

    fn open_temp_wiki() -> (TempDir, AideMemo) {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
            config.store.backend = "redb".to_string();
        }
        let store_path = dir.path().join(if config.store.backend == "redb" {
            "wiki.redb"
        } else {
            "wiki.sqlite"
        });
        let wiki = AideMemo::open(&store_path, config).unwrap();
        (dir, wiki)
    }

    #[test]
    fn model_load_bytes_known_models() {
        assert_eq!(
            model_load_bytes("minishlab/potion-base-4M"),
            Some(4 * 1024 * 1024)
        );
        assert_eq!(
            model_load_bytes("minishlab/potion-base-8M"),
            Some(8 * 1024 * 1024)
        );
        assert_eq!(
            model_load_bytes("minishlab/potion-multilingual-32M"),
            Some(32 * 1024 * 1024)
        );
        assert_eq!(
            model_load_bytes("minishlab/potion-multilingual-128M"),
            Some(128 * 1024 * 1024)
        );
    }

    #[test]
    fn model_load_bytes_unknown_returns_none() {
        assert_eq!(model_load_bytes("openai/text-embedding-3-small"), None);
        assert_eq!(model_load_bytes(""), None);
    }

    #[test]
    fn collect_memory_advises_rebuild_for_missing_hnsw_sidecar() {
        let dir = TempDir::new().unwrap();
        let store_path: PathBuf = dir.path().join("wiki.sqlite");
        std::fs::write(&store_path, vec![0u8; 64]).unwrap();
        let cfg = config_with_model("minishlab/potion-multilingual-128M", false, "hnsw");
        let mem = collect_memory(&store_path, &cfg).with_counts(0, 0);
        let advisories = mem.advisories();
        assert!(
            advisories
                .iter()
                .any(|a| a.command == "aidememo vector-rebuild"),
            "expected vector-rebuild advisory when sidecar is absent"
        );
    }

    #[test]
    fn collect_memory_no_advisory_for_small_model() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("wiki.sqlite");
        std::fs::write(&store_path, vec![0u8; 64]).unwrap();
        let cfg = config_with_model("minishlab/potion-base-8M", false, "bm25");
        let mem = collect_memory(&store_path, &cfg).with_counts(0, 0);
        assert!(
            mem.advisories().is_empty(),
            "expected no advisories for an 8M model on bm25, got {:?}",
            mem.advisories()
        );
    }

    #[test]
    fn adaptation_advises_train_when_feedback_exists_without_adapter() {
        let report = AdaptationReport {
            use_adapter: true,
            feedback_count: 3,
            has_adapter: false,
            generation: 0,
            ready: false,
            error: None,
        };
        let advisories = report.advisories();
        assert!(
            advisories
                .iter()
                .any(|a| a.command == "aidememo adapt train" && a.reason.contains("3 feedback")),
            "expected adapt train advisory, got {:?}",
            advisories
        );
    }

    #[test]
    fn adaptation_advises_enable_when_adapter_is_disabled() {
        let report = AdaptationReport {
            use_adapter: false,
            feedback_count: 3,
            has_adapter: true,
            generation: 1,
            ready: true,
            error: None,
        };
        let advisories = report.advisories();
        assert!(
            advisories
                .iter()
                .any(|a| a.command == "aidememo config set search.use_adapter true"),
            "expected search.use_adapter advisory, got {:?}",
            advisories
        );
    }

    #[test]
    fn workflow_status_counts_recent_ticket_facts() {
        let (_dir, wiki) = open_temp_wiki();
        let session_id = wiki
            .entity_add(EntityInput {
                name: "session-test".to_string(),
                entity_type: Some(EntityType::parse("session")),
                ..Default::default()
            })
            .unwrap();

        wiki.add_fact(FactInput {
            content: "Workflow ticket: Fix sparse Redis timeout\n\nBody text".to_string(),
            fact_type: Some(FactType::Question),
            entity_ids: Some(vec![session_id]),
            tags: Some(vec!["workflow-start".into(), "ticket".into()]),
            source: Some("github:org/repo#123".to_string()),
            source_id: Some("alpha".to_string()),
            source_confidence: Some(1.0),
            observed_at: None,
        })
        .unwrap();
        wiki.add_fact(FactInput {
            content: "Ordinary question that is not a workflow ticket".to_string(),
            fact_type: Some(FactType::Question),
            entity_ids: Some(vec![session_id]),
            tags: Some(vec!["question".into()]),
            source: None,
            source_id: None,
            source_confidence: Some(1.0),
            observed_at: None,
        })
        .unwrap();

        let agents = vec![AgentStatus {
            target: "codex",
            skill_path: None,
            skill_installed: Some(true),
            mcp_detail: "test".to_string(),
            mcp_registered: Some(true),
        }];
        let report = collect_workflow_status(&wiki, &agents).unwrap();

        assert!(report.ready);
        assert!(report.mcp_ready);
        assert_eq!(report.recent_ticket_count, 1);
        assert_eq!(report.recent_tickets[0].source_id.as_deref(), Some("alpha"));
        assert!(report.recent_tickets[0].preview.contains("Workflow ticket"));
        assert!(
            !report
                .hints
                .iter()
                .any(|h| h.code == "workflow_no_mcp_agent"),
            "mcp-ready agent must not produce no-mcp hint"
        );
        assert!(
            !report
                .hints
                .iter()
                .any(|h| h.code == "workflow_no_recent_tickets"),
            "workflow ticket exists, got hints {:?}",
            report.hints
        );
    }

    #[test]
    fn workflow_status_recommends_source_id_for_mcp_install_when_ticket_is_scoped() {
        let (_dir, wiki) = open_temp_wiki();
        let session_id = wiki
            .entity_add(EntityInput {
                name: "session-scoped".to_string(),
                entity_type: Some(EntityType::parse("session")),
                ..Default::default()
            })
            .unwrap();

        wiki.add_fact(FactInput {
            content: "Workflow ticket: Add Redis retry policy".to_string(),
            fact_type: Some(FactType::Question),
            entity_ids: Some(vec![session_id]),
            tags: Some(vec!["workflow-start".into(), "ticket".into()]),
            source: Some("github:org/repo#124".to_string()),
            source_id: Some("team-alpha".to_string()),
            source_confidence: Some(1.0),
            observed_at: None,
        })
        .unwrap();

        let agents = vec![AgentStatus {
            target: "codex",
            skill_path: None,
            skill_installed: Some(true),
            mcp_detail: "test".to_string(),
            mcp_registered: Some(false),
        }];

        let report = collect_workflow_status(&wiki, &agents).unwrap();
        let hint = report
            .hints
            .iter()
            .find(|h| h.code == "workflow_no_mcp_agent")
            .expect("missing no-mcp workflow hint");

        assert_eq!(
            hint.action,
            "aidememo mcp-install --target codex --source-id team-alpha"
        );
        assert!(
            !report
                .hints
                .iter()
                .any(|h| h.code == "workflow_no_recent_tickets"),
            "scoped ticket exists, got hints {:?}",
            report.hints
        );
    }

    #[test]
    fn mcp_install_action_quotes_shell_sensitive_source_id() {
        assert_eq!(
            mcp_install_action(Some("team alpha")),
            "aidememo mcp-install --target codex --source-id 'team alpha'"
        );
        assert_eq!(
            mcp_install_action(Some("team-alpha")),
            "aidememo mcp-install --target codex --source-id team-alpha"
        );
    }

    #[test]
    fn workflow_status_surfaces_actionable_setup_hints() {
        let (_dir, wiki) = open_temp_wiki();
        let agents = vec![AgentStatus {
            target: "codex",
            skill_path: None,
            skill_installed: Some(false),
            mcp_detail: "test".to_string(),
            mcp_registered: Some(false),
        }];

        let report = collect_workflow_status(&wiki, &agents).unwrap();

        assert!(!report.ready);
        assert_eq!(report.recent_ticket_count, 0);
        for code in [
            "workflow_no_mcp_agent",
            "workflow_no_skill_prompt",
            "workflow_no_recent_tickets",
        ] {
            let hint = report.hints.iter().find(|h| h.code == code);
            assert!(hint.is_some(), "missing {code} in {:?}", report.hints);
            assert!(
                !hint.unwrap().action.trim().is_empty(),
                "hint {code} must have a concrete action"
            );
        }
    }

    #[test]
    fn sharing_report_advises_retry_when_serverless_fail_fast() {
        let report = SharingReport {
            lock_retry_ms: 0,
            serverless_recommended_writers: SERVERLESS_RECOMMENDED_WRITERS,
            high_concurrency_writers: HIGH_CONCURRENCY_WRITERS,
            daemon: DaemonStatus {
                state: "none",
                port: None,
                pid: None,
                store: None,
            },
            recommended_mode: "serverless_fail_fast",
            hints: Vec::new(),
        };

        let advisories = report.advisories();
        assert!(
            advisories
                .iter()
                .any(|a| a.command == "aidememo config set store.lock_retry_ms 5000"),
            "expected lock retry advisory, got {:?}",
            advisories
        );
    }

    #[test]
    fn sharing_report_suppresses_retry_advisory_when_daemon_is_healthy() {
        let report = SharingReport {
            lock_retry_ms: 0,
            serverless_recommended_writers: SERVERLESS_RECOMMENDED_WRITERS,
            high_concurrency_writers: HIGH_CONCURRENCY_WRITERS,
            daemon: DaemonStatus {
                state: "healthy",
                port: Some(3000),
                pid: Some(123),
                store: Some("/tmp/wiki.sqlite".to_string()),
            },
            recommended_mode: "daemon",
            hints: Vec::new(),
        };

        assert!(
            report.advisories().is_empty(),
            "healthy daemon should be the smooth sharing path"
        );
    }

    #[test]
    fn backend_path_hint_warns_when_redb_uses_sqlite_extension() {
        let hint =
            backend_path_hint(Path::new("./_meta/wiki.sqlite"), "redb").expect("redb path hint");
        assert_eq!(hint.code, "storage_backend_path_extension_mismatch");
        assert_eq!(hint.severity, "warn");
        assert!(hint.message.contains("store.backend is redb"));
        assert!(hint.message.contains(".sqlite"));
        assert!(hint.action.contains("./_meta/wiki.redb"));
    }

    #[test]
    fn backend_path_hint_warns_when_sqlite_uses_redb_extension() {
        let hint = backend_path_hint(Path::new("./_meta/wiki.redb"), "libsqlite")
            .expect("sqlite path hint");
        assert_eq!(hint.code, "storage_backend_path_extension_mismatch");
        assert!(hint.message.contains("store.backend is libsqlite"));
        assert!(hint.message.contains(".redb"));
        assert!(hint.action.contains("./_meta/wiki.sqlite"));
    }

    #[test]
    fn stale_hnsw_advisory_fires_when_superseded_facts_present() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("wiki.sqlite");
        std::fs::write(&store_path, vec![0u8; 64]).unwrap();
        // Sidecar present + 7 superseded facts → advise --current-only.
        std::fs::write(dir.path().join("wiki.hnsw.bin"), vec![0u8; 16]).unwrap();
        let cfg = config_with_model("minishlab/potion-base-8M", false, "hnsw");
        let mem = collect_memory(&store_path, &cfg).with_counts(100, 7);
        let advisories = mem.advisories();
        assert!(
            advisories
                .iter()
                .any(|a| a.command == "aidememo vector-rebuild --current-only"
                    && a.reason.contains("7 superseded")),
            "expected stale-HNSW advisory, got {:?}",
            advisories
        );
    }

    #[test]
    fn stale_hnsw_advisory_silent_without_superseded_facts() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("wiki.sqlite");
        std::fs::write(&store_path, vec![0u8; 64]).unwrap();
        std::fs::write(dir.path().join("wiki.hnsw.bin"), vec![0u8; 16]).unwrap();
        let cfg = config_with_model("minishlab/potion-base-8M", false, "hnsw");
        let mem = collect_memory(&store_path, &cfg).with_counts(100, 0);
        assert!(
            !mem.advisories()
                .iter()
                .any(|a| a.command.contains("--current-only")),
            "no superseded facts → must not advise --current-only rebuild"
        );
    }

    #[test]
    fn stale_hnsw_advisory_silent_without_sidecar() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("wiki.sqlite");
        std::fs::write(&store_path, vec![0u8; 64]).unwrap();
        // No sidecar file present even though hnsw is configured.
        let cfg = config_with_model("minishlab/potion-base-8M", false, "hnsw");
        let mem = collect_memory(&store_path, &cfg).with_counts(100, 7);
        assert!(
            !mem.advisories()
                .iter()
                .any(|a| a.command.contains("--current-only")),
            "stale advisory targets HNSW, but no sidecar exists — \
             the missing-sidecar advisory should fire instead"
        );
    }

    #[test]
    fn collect_memory_with_counts_sums_per_fact_estimates() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("wiki.sqlite");
        std::fs::write(&store_path, vec![0u8; 64]).unwrap();
        // hnsw sidecar present so the runtime row contributes.
        std::fs::write(dir.path().join("wiki.hnsw.bin"), vec![0u8; 16]).unwrap();
        let cfg = config_with_model("minishlab/potion-base-8M", false, "hnsw");
        let mem = collect_memory(&store_path, &cfg).with_counts(1_000, 0);

        // bm25 + embed + hnsw + model_load = sum
        let want_bm25 = 1_000 * MEMORY_PER_FACT_BM25;
        let want_embed = 1_000 * MEMORY_PER_FACT_EMBED_CACHE;
        let want_hnsw = 1_000 * MEMORY_PER_FACT_HNSW;
        let want_model = 8 * 1024 * 1024;
        assert_eq!(
            mem.ram_total_bytes,
            want_bm25 + want_embed + want_hnsw + want_model
        );
        // model row order is last.
        assert_eq!(mem.ram_estimate.last().unwrap().name, "model load");
    }
}
