//! `wg init` — initialize a new wiki graph.
//!
//! Creates the store, runs the first ingest, and prints a quick-start example.

use bpaf::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::cmd::{Command, McpInstallSub, SkillSub};
use wg_core::{Config, IngestStats, WgError, WikiGraph};

#[derive(Debug, Clone)]
pub struct InitSub {
    /// Skip the initial ingest and only create the store.
    pub no_ingest: bool,
    /// Optional agent target for one-shot onboarding.
    pub agent: Option<String>,
    /// Overwrite existing agent skill/MCP config when --agent is set.
    pub agent_force: bool,
    /// Wiki root directory path.
    pub wiki_root: PathBuf,
}

pub fn init_command() -> impl Parser<Command> {
    let no_ingest = long("no-ingest")
        .short('n')
        .help("Skip the initial ingest (create store only)")
        .switch();
    let agent = long("agent")
        .help(
            "Also install wg into an agent target after init. Supported \
             targets follow `wg mcp-install --list-targets` and \
             `wg skill install --list-targets`; unsupported surfaces are \
             reported as skipped.",
        )
        .argument::<String>("TARGET")
        .optional();
    let agent_force = long("agent-force")
        .help("Overwrite existing agent skill/MCP config during --agent setup")
        .switch();

    let wiki_root = positional::<PathBuf>("WIKI_ROOT").help("Path to the wiki root directory");

    construct!(InitSub {
        no_ingest,
        agent,
        agent_force,
        wiki_root,
    })
    .map(Command::Init)
    .to_options()
    .command("init")
    .help("Initialize a wiki graph store and optionally ingest the wiki")
}

/// Run `wg init` — create config/store and optionally ingest.
pub fn run_init(
    wiki_root: PathBuf,
    no_ingest: bool,
    agent: Option<String>,
    agent_force: bool,
    store_path: &Path,
    config: Config,
    json: bool,
) -> Result<String, WgError> {
    let started = Instant::now();
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

    let mut steps = Vec::new();

    // 2. Ensure the store directory exists
    let step_t0 = Instant::now();
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WgError::Internal(format!("failed to create store dir: {}", e)))?;
    }
    steps.push(InitStep {
        name: "store_dir".to_string(),
        status: "ok".to_string(),
        detail: store_path.display().to_string(),
        elapsed_ms: step_t0.elapsed().as_millis() as u64,
    });

    // 3. Open (or create) the store
    let step_t0 = Instant::now();
    let wiki = WikiGraph::open(store_path, config.clone())?;
    steps.push(InitStep {
        name: "store_open".to_string(),
        status: "ok".to_string(),
        detail: store_path.display().to_string(),
        elapsed_ms: step_t0.elapsed().as_millis() as u64,
    });

    // 4. Ingest if not skipped
    let stats = if no_ingest {
        steps.push(InitStep {
            name: "ingest".to_string(),
            status: "skipped".to_string(),
            detail: "--no-ingest".to_string(),
            elapsed_ms: 0,
        });
        None
    } else {
        let step_t0 = Instant::now();
        let s = wiki.ingest(&wiki_root, false)?;
        steps.push(InitStep {
            name: "ingest".to_string(),
            status: "ok".to_string(),
            detail: format!(
                "{} files, +{} entities, +{} facts, +{} relations",
                s.files_scanned, s.entities_added, s.facts_added, s.relations_added
            ),
            elapsed_ms: step_t0.elapsed().as_millis() as u64,
        });
        Some(s)
    };

    if let Some(target) = agent.as_deref() {
        steps.extend(run_agent_setup(target, agent_force));
    }

    let report = InitReport {
        store_path: store_path.display().to_string(),
        wiki_root: wiki_root.display().to_string(),
        no_ingest,
        agent,
        ingest: stats.clone(),
        steps,
        elapsed_ms: started.elapsed().as_millis() as u64,
    };

    if json {
        return serde_json::to_string_pretty(&report).map_err(|e| WgError::Serialize {
            context: "init".to_string(),
            source: e,
        });
    }

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
    if report.agent.is_some() {
        out.push_str("\nAgent setup:\n");
        for step in &report.steps {
            if step.name.starts_with("agent_") {
                out.push_str(&format!(
                    "  {}: {} ({} ms) — {}\n",
                    step.name, step.status, step.elapsed_ms, step.detail
                ));
            }
        }
    }

    Ok(out)
}

#[derive(Debug, serde::Serialize)]
struct InitStep {
    name: String,
    status: String,
    detail: String,
    elapsed_ms: u64,
}

#[derive(Debug, serde::Serialize)]
struct InitReport {
    store_path: String,
    wiki_root: String,
    no_ingest: bool,
    agent: Option<String>,
    ingest: Option<IngestStats>,
    steps: Vec<InitStep>,
    elapsed_ms: u64,
}

fn run_agent_setup(target: &str, force: bool) -> Vec<InitStep> {
    let mut steps = Vec::new();

    let skill_supported = crate::cmd::skill::target_skills_dir(target).is_some()
        || crate::cmd::skill::target_agents_md_path(target).is_some();
    if skill_supported {
        let t0 = Instant::now();
        let result = crate::cmd::skill::run_skill(
            SkillSub::Install {
                target: target.to_string(),
                dest: None,
                force,
                list_targets: false,
            },
            false,
        );
        steps.push(step_from_result("agent_skill_install", result, t0));
    } else {
        steps.push(InitStep {
            name: "agent_skill_install".to_string(),
            status: "skipped".to_string(),
            detail: format!("{target} has no bundled skill install target"),
            elapsed_ms: 0,
        });
    }

    if target == "pi" {
        steps.push(InitStep {
            name: "agent_mcp_install".to_string(),
            status: "skipped".to_string(),
            detail: "pi does not support MCP; skill install is the integration path".to_string(),
            elapsed_ms: 0,
        });
    } else {
        let t0 = Instant::now();
        let result = crate::cmd::mcp_install::run_mcp_install(
            McpInstallSub {
                target: target.to_string(),
                force,
                print: false,
                list_targets: false,
                no_verify: true,
            },
            false,
        );
        steps.push(step_from_result("agent_mcp_install", result, t0));
    }

    steps
}

fn step_from_result(name: &str, result: Result<String, WgError>, started: Instant) -> InitStep {
    match result {
        Ok(detail) => InitStep {
            name: name.to_string(),
            status: "ok".to_string(),
            detail: first_line(&detail),
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
        Err(e) => InitStep {
            name: name.to_string(),
            status: "error".to_string(),
            detail: e.to_string(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
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
