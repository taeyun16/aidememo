//! `wg doctor` — friendly graph health check.
//!
//! Wraps `wg lint` plus a few extra invariant checks (broken refs, schema
//! mismatches) into a developer-friendly report. Also surfaces *agent
//! integration* status so operators can see at a glance which agents
//! have wg installed as a skill and registered as an MCP server,
//! mirroring what `wg skill install --list-targets` and
//! `wg mcp-install --list-targets` would have written.

use bpaf::*;
use std::path::Path;
use wg_core::{Config, WgError, WikiGraph};

use crate::cmd::Command;
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
) -> Result<String, WgError> {
    let memory = collect_memory(store_path, &config);
    let wiki = WikiGraph::open(store_path, config)?;
    let issues = wiki.lint()?;
    let stats = wiki.stats()?;
    let superseded = wiki.fact_count_superseded()? as u64;
    let memory = memory.with_counts(stats.fact_count, superseded);
    let agents = collect_agent_integration();
    let mut fixes = collect_fix_suggestions(&agents);
    fixes.extend(memory.advisories());

    // `--fix --shell` short-circuits to a pipe-friendly view: just
    // the bare commands, one per line, nothing else. Designed for
    // `wg doctor --fix --shell | sh`. We deliberately omit a trailing
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
            // Always emitted in JSON: tooling that consumes this
            // shouldn't have to re-derive the suggestion list.
            "fixes": fixes,
        });
        return serde_json::to_string_pretty(&payload).map_err(|e| WgError::Serialize {
            context: "doctor".to_string(),
            source: e,
        });
    }

    let mut out = String::new();
    out.push_str(&format!("📋 wg doctor — {}\n", store_path.display()));
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
    out.push_str(&format_agent_integration(&agents));

    if sub.fix {
        out.push_str(&format_fix_suggestions(&fixes));
    } else if !fixes.is_empty() {
        out.push_str(&format!(
            "Tip: {} agent integration gap(s) detected. Run `wg doctor --fix` for the install commands.\n\n",
            fixes.len()
        ));
    }

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
/// wg's bundled skill is already there. Two shapes:
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
            .map(|c| c.contains("<!-- BEGIN wg-skill -->"))
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
        "claude" | "hermes" | "openclaw" => verify_registered(target, "wg"),
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
            .map(|s| s.contains_key("wg"))
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

/// One actionable fix the user can run to close a gap in the agent
/// integration matrix. The shell command is a copy-pasteable string —
/// emitting structured fields too lets `wg doctor --json` callers
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
                command: format!("wg skill install --target {}", a.target),
                reason: format!(
                    "no SKILL.md at {}",
                    a.skill_path.as_deref().unwrap_or("<unknown>")
                ),
            });
        }
        // MCP gap: registration confirmed missing. We *don't* suggest
        // a fix when the check returned None — that usually means the
        // agent's CLI isn't installed, in which case `wg mcp-install`
        // would just fail. Surfacing that as a fix would be misleading.
        if matches!(a.mcp_registered, Some(false)) {
            out.push(FixSuggestion {
                target: a.target,
                kind: "mcp",
                command: format!("wg mcp-install --target {}", a.target),
                reason: format!("wg not registered ({})", a.mcp_detail),
            });
        }
    }
    out
}

fn format_fix_suggestions(fixes: &[FixSuggestion]) -> String {
    if fixes.is_empty() {
        return "✓ No gaps to fix — every reachable agent has wg installed.\n\n".to_string();
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
// Memory + disk footprint
// ---------------------------------------------------------------------------

/// Snapshot of the wiki's on-disk artifacts and an estimate of how
/// much RAM a fully-loaded WikiGraph uses. Numbers come from
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
    /// Whether `model.quantize = true` was set; surfaces in advisories.
    pub model_quantize: bool,
    /// `Some(true)` when `search.semantic_index = "hnsw"` and the
    /// sidecar is present, `Some(false)` when configured but missing,
    /// `None` when the user opted out of HNSW.
    pub hnsw_sidecar_present: Option<bool>,
    /// Facts with `superseded_at` set. Filled in by `with_counts`.
    /// Drives the "stale HNSW after consolidate" advisory: when this
    /// is > 0 and the sidecar is present, the operator likely
    /// forgot to re-run `wg vector-rebuild --current-only`.
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
    /// `collect_memory` runs *before* `WikiGraph::open` so it doesn't
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
                Some(false) => "configured but sidecar missing — run `wg vector-rebuild`".into(),
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
    /// memory picture suggests an obvious tweak (`model.quantize` for
    /// large models, `wg vector-rebuild` for missing sidecar).
    fn advisories(&self) -> Vec<FixSuggestion> {
        let mut out = Vec::new();
        if let Some(model_entry) = self.ram_estimate.iter().find(|e| e.name == "model load") {
            if model_entry.bytes >= 100 * 1024 * 1024 && !self.model_quantize {
                out.push(FixSuggestion {
                    target: "wg",
                    kind: "memory",
                    command: "wg config set model.quantize true".to_string(),
                    reason: format!(
                        "{} loads {:.0} MB into RAM; int8 quantize cuts that ~3.9×",
                        self.model_name,
                        model_entry.bytes as f64 / (1024.0 * 1024.0)
                    ),
                });
            }
        }
        if self.hnsw_sidecar_present == Some(false) {
            out.push(FixSuggestion {
                target: "wg",
                kind: "memory",
                command: "wg vector-rebuild".to_string(),
                reason: "search.semantic_index = hnsw but sidecar is missing".to_string(),
            });
        }
        if self.hnsw_sidecar_present == Some(true) && self.superseded_facts > 0 {
            out.push(FixSuggestion {
                target: "wg",
                kind: "memory",
                command: "wg vector-rebuild --current-only".to_string(),
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
            name: "redb store".into(),
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

    let model_bytes_unquantized = model_load_bytes(&config.model.name);
    let model_bytes = if config.model.quantize {
        // `quantize` field doc cites ~3.9× shrink (489 MB → 124 MB on
        // 128M); apply the same factor here for the estimate.
        model_bytes_unquantized.map(|b| (b as f64 / 3.9) as u64)
    } else {
        model_bytes_unquantized
    };
    let model_detail = match (model_bytes_unquantized, config.model.quantize) {
        (Some(_), true) => format!("{} (quantized)", config.model.name),
        (Some(_), false) => config.model.name.clone(),
        (None, _) => format!("{} (unknown size)", config.model.name),
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
        model_quantize: config.model.quantize,
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
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn config_with_model(name: &str, quantize: bool, semantic_index: &str) -> Config {
        let mut c = Config::default();
        c.model.name = name.to_string();
        c.model.quantize = quantize;
        c.search.semantic_index = semantic_index.to_string();
        c
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
    fn collect_memory_quantize_advisory_fires_on_large_model() {
        let dir = TempDir::new().unwrap();
        let store_path: PathBuf = dir.path().join("wiki.redb");
        std::fs::write(&store_path, vec![0u8; 64]).unwrap();
        let cfg = config_with_model("minishlab/potion-multilingual-128M", false, "hnsw");
        let mem = collect_memory(&store_path, &cfg).with_counts(0, 0);
        let advisories = mem.advisories();
        assert!(
            advisories
                .iter()
                .any(|a| a.command.contains("model.quantize true")),
            "expected quantize advisory, got {:?}",
            advisories
        );
        // hnsw sidecar missing → also advises rebuild.
        assert!(
            advisories.iter().any(|a| a.command == "wg vector-rebuild"),
            "expected vector-rebuild advisory when sidecar is absent"
        );
    }

    #[test]
    fn collect_memory_no_advisory_for_small_model() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("wiki.redb");
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
    fn stale_hnsw_advisory_fires_when_superseded_facts_present() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("wiki.redb");
        std::fs::write(&store_path, vec![0u8; 64]).unwrap();
        // Sidecar present + 7 superseded facts → advise --current-only.
        std::fs::write(dir.path().join("wiki.hnsw.bin"), vec![0u8; 16]).unwrap();
        let cfg = config_with_model("minishlab/potion-base-8M", false, "hnsw");
        let mem = collect_memory(&store_path, &cfg).with_counts(100, 7);
        let advisories = mem.advisories();
        assert!(
            advisories
                .iter()
                .any(|a| a.command == "wg vector-rebuild --current-only"
                    && a.reason.contains("7 superseded")),
            "expected stale-HNSW advisory, got {:?}",
            advisories
        );
    }

    #[test]
    fn stale_hnsw_advisory_silent_without_superseded_facts() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("wiki.redb");
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
        let store_path = dir.path().join("wiki.redb");
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
        let store_path = dir.path().join("wiki.redb");
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
