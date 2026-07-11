//! `aidememo mcp-install` — register the AideMemo MCP server with a target agent.
//!
//! Each agent has a different surface for MCP registration:
//!
//! | target   | mechanism                                                        |
//! |----------|------------------------------------------------------------------|
//! | claude   | shells out to `claude mcp add aidememo -- aidememo --backend <resolved> mcp` |
//! | hermes   | shells out to `hermes mcp add aidememo --command aidememo --args ...`        |
//! | openclaw | shells out to `openclaw mcp set aidememo '{"command":"aidememo",...}'`       |
//! | codex    | edits `~/.codex/config.toml` to add `[mcp_servers.aidememo]`           |
//! | cursor   | edits `~/.cursor/mcp.json` to add `{"mcpServers": {"aidememo": ...}}`  |
//!
//! For shell-out targets, the agent's own CLI is the source of truth
//! for *where* the entry lands — we just invoke it. For file-edit
//! targets, we read the existing config, merge in `aidememo`, and write it
//! back atomically (stage to `.tmp`, rename).
//!
//! Use `--print` to preview the action without executing, and
//! `--force` to overwrite an existing `aidememo` entry.

use bpaf::*;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use crate::cmd::Command;
use aidememo_core::AideMemoError;

#[derive(Debug, Clone)]
pub struct McpInstallSub {
    pub target: String,
    pub force: bool,
    pub print: bool,
    pub list_targets: bool,
    pub no_verify: bool,
    pub source_id: Option<String>,
    pub actor_ids: Vec<String>,
    /// Optional Codex profile homes. Repeat to install into several isolated
    /// Codex profiles while keeping one shared AideMemo store.
    pub codex_homes: Vec<PathBuf>,
}

pub fn mcp_install_command() -> impl Parser<Command> {
    let target = long("target")
        .help(
            "Target agent: claude, hermes, openclaw, codex, cursor, opencode \
             (or 'list'). pi is intentionally not supported — pi rejects MCP \
             upstream; use `aidememo skill install --target pi` instead.",
        )
        .argument::<String>("TARGET")
        .fallback("claude".to_string());
    let force = long("force")
        .help("Overwrite an existing `aidememo` MCP entry")
        .switch();
    let print = long("print")
        .help("Print the action that would be taken without executing")
        .switch();
    let list_targets = long("list-targets")
        .help("Print supported agents and the registration mechanism each uses")
        .switch();
    let no_verify = long("no-verify")
        .help(
            "Skip the post-install `<bin> mcp list` check. Useful when an \
             agent's CLI is slow to refresh, lives in a sandbox we can't \
             reach, or returns noisy output that breaks the heuristic.",
        )
        .switch();
    let source_id = long("source-id")
        .help(
            "Set AIDEMEMO_SOURCE_ID in the installed MCP server environment so \
             read/write tools default to this source namespace. Explicit \
             tool-call source_id values still override it.",
        )
        .argument::<String>("SOURCE_ID")
        .optional();
    let actor_ids = long("actor-id")
        .help(
            "Set AIDEMEMO_ACTOR_ID in the installed MCP server environment so \
             facts retain writer provenance independently of source_id scoping.",
        )
        .argument::<String>("ACTOR_ID")
        .many();
    let codex_homes = long("codex-home")
        .help(
            "Codex profile directory to update (the directory containing config.toml). \
             Repeat for multiple isolated Codex accounts. Defaults to CODEX_HOME, then ~/.codex.",
        )
        .argument::<PathBuf>("PATH")
        .many();

    construct!(McpInstallSub {
        target,
        force,
        print,
        list_targets,
        no_verify,
        source_id,
        actor_ids,
        codex_homes,
    })
    .map(Command::McpInstall)
    .to_options()
    .command("mcp-install")
    .help(
        "Register the AideMemo MCP server with an agent (claude / hermes / \
         openclaw / codex / cursor / opencode)",
    )
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct InstallReport {
    target: String,
    method: String,
    detail: String,
    overwrote: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor_id: Option<String>,
    storage_backend: String,
    store_path: String,
    /// Result of the post-install best-effort check that the agent
    /// actually picked up the new server. `None` when verification
    /// wasn't attempted (file-edit targets, `--print` mode, missing
    /// `<bin> mcp list` subcommand). `Some(true)` when `aidememo` appeared
    /// in the agent's MCP list, `Some(false)` when the install
    /// command exited 0 but the entry didn't show up — that's the
    /// shape we want to surface, since it usually means the agent's
    /// CLI silently rejected the entry (parser quirk, bad path, etc).
    #[serde(skip_serializing_if = "Option::is_none")]
    verified: Option<bool>,
}

#[derive(serde::Serialize)]
struct TargetEntry {
    target: &'static str,
    method: &'static str,
    detail: String,
}

const SUPPORTED: &[(&str, &str, &str)] = &[
    (
        "claude",
        "shell-out",
        "claude mcp add aidememo -- aidememo --backend <resolved> mcp",
    ),
    (
        "hermes",
        "shell-out",
        "hermes mcp add aidememo --command aidememo --args=--backend --args=<resolved> --args=mcp",
    ),
    (
        "openclaw",
        "shell-out",
        "openclaw mcp set aidememo '{\"command\":\"aidememo\",\"args\":[\"--backend\",\"<resolved>\",\"mcp\"]}'",
    ),
    (
        "codex",
        "file-edit",
        "edit ~/.codex/config.toml: add [mcp_servers.aidememo]",
    ),
    (
        "cursor",
        "file-edit",
        "edit ~/.cursor/mcp.json: add mcpServers.aidememo",
    ),
    (
        "opencode",
        "file-edit",
        "edit ~/.config/opencode/opencode.json: add mcp.aidememo",
    ),
];

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

pub fn run_mcp_install(
    sub: McpInstallSub,
    json: bool,
    storage_backend: &str,
    store_path: &Path,
) -> Result<String, AideMemoError> {
    if sub.list_targets || sub.target == "list" {
        let entries: Vec<TargetEntry> = SUPPORTED
            .iter()
            .map(|(t, m, d)| TargetEntry {
                target: t,
                method: m,
                detail: (*d).to_string(),
            })
            .collect();
        if json {
            return serde_json::to_string_pretty(&entries).map_err(|e| AideMemoError::Serialize {
                context: "mcp-install --list-targets".to_string(),
                source: e,
            });
        }
        let mut out = String::from("Supported MCP install targets:\n");
        for e in entries {
            out.push_str(&format!("  {:<10} ({}) {}\n", e.target, e.method, e.detail));
        }
        return Ok(out);
    }

    let source_id = normalise_identity_arg("--source-id", sub.source_id.as_deref())?;
    let actor_ids = sub
        .actor_ids
        .iter()
        .map(|value| normalise_identity_arg("--actor-id", Some(value)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if sub.target != "codex" && actor_ids.len() > 1 {
        return Err(AideMemoError::InvalidInput(
            "repeat --actor-id only with repeatable --codex-home profiles".to_string(),
        ));
    }
    let actor_id = actor_ids.first().map(String::as_str);
    let store_path = stable_store_path(store_path)?;
    let storage_backend = storage_backend.trim();
    let report = match sub.target.as_str() {
        "claude" | "claude-code" => install_via_cli(
            "claude",
            claude_install_args(source_id.as_deref(), actor_id, storage_backend, &store_path),
            ShellInstallOptions {
                print: sub.print,
                no_verify: sub.no_verify,
                source_id: source_id.as_deref(),
                actor_id,
                storage_backend,
                store_path: &store_path,
            },
        )?,
        "hermes" => install_via_cli(
            "hermes",
            hermes_install_args(source_id.as_deref(), actor_id, storage_backend, &store_path),
            ShellInstallOptions {
                print: sub.print,
                no_verify: sub.no_verify,
                source_id: source_id.as_deref(),
                actor_id,
                storage_backend,
                store_path: &store_path,
            },
        )?,
        "openclaw" => install_via_cli(
            "openclaw",
            openclaw_install_args(source_id.as_deref(), actor_id, storage_backend, &store_path),
            ShellInstallOptions {
                print: sub.print,
                no_verify: sub.no_verify,
                source_id: source_id.as_deref(),
                actor_id,
                storage_backend,
                store_path: &store_path,
            },
        )?,
        "codex" => install_codex(
            sub.force,
            sub.print,
            source_id.as_deref(),
            &actor_ids,
            storage_backend,
            &store_path,
            &sub.codex_homes,
        )?,
        "cursor" => install_cursor(
            sub.force,
            sub.print,
            source_id.as_deref(),
            actor_id,
            storage_backend,
            &store_path,
        )?,
        "opencode" => install_opencode(
            sub.force,
            sub.print,
            source_id.as_deref(),
            actor_id,
            storage_backend,
            &store_path,
        )?,
        "pi" => {
            return Err(AideMemoError::InvalidInput(
                "pi has no MCP support by upstream design — pi rejects MCP \
                 because the protocol's tool descriptions consume too much \
                 of the context window. Use `aidememo skill install --target pi` \
                 instead to merge aidememo's skill into ~/.config/pi/AGENTS.md."
                    .to_string(),
            ));
        }
        other => {
            return Err(AideMemoError::InvalidInput(format!(
                "unknown target `{}` — supported: {}",
                other,
                SUPPORTED
                    .iter()
                    .map(|(t, _, _)| *t)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };

    if json {
        return serde_json::to_string_pretty(&report).map_err(|e| AideMemoError::Serialize {
            context: "mcp-install".to_string(),
            source: e,
        });
    }

    let verb = if sub.print {
        "Would run"
    } else if report.overwrote {
        "Updated"
    } else {
        "Registered"
    };
    let mut out = format!(
        "{verb} aidememo MCP server for {} ({})\n  {}\n",
        report.target, report.method, report.detail
    );
    match report.verified {
        Some(true) => out.push_str("  verified: aidememo appears in the agent's MCP list ✓\n"),
        Some(false) => out.push_str(
            "  verified: ⚠ install command exited 0 but `aidememo` did not show up — \
             check the agent's config manually\n",
        ),
        None => {}
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Shell-out targets (claude / hermes / openclaw)
// ---------------------------------------------------------------------------

struct ShellInstallOptions<'a> {
    print: bool,
    no_verify: bool,
    source_id: Option<&'a str>,
    actor_id: Option<&'a str>,
    storage_backend: &'a str,
    store_path: &'a Path,
}

fn install_via_cli(
    bin: &str,
    args: Vec<String>,
    options: ShellInstallOptions<'_>,
) -> Result<InstallReport, AideMemoError> {
    let ShellInstallOptions {
        print,
        no_verify,
        source_id,
        actor_id,
        storage_backend,
        store_path,
    } = options;
    let cmdline = format!("{} {}", bin, args.join(" "));
    let target = bin.to_string();

    if print {
        return Ok(InstallReport {
            target,
            method: "shell-out".to_string(),
            detail: cmdline,
            overwrote: false,
            source_id: source_id.map(str::to_string),
            actor_id: actor_id.map(str::to_string),
            storage_backend: storage_backend.to_string(),
            store_path: store_path.display().to_string(),
            verified: None,
        });
    }

    let out = ProcessCommand::new(bin).args(&args).output().map_err(|e| {
        AideMemoError::InvalidInput(format!(
            "could not run `{}` — is the {} CLI on your PATH? (raw error: {})",
            bin, bin, e
        ))
    })?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AideMemoError::Internal(format!(
            "`{}` exited with {}: {}",
            cmdline,
            out.status,
            stderr.trim()
        )));
    }

    // Best-effort: ask the agent to list its MCP servers and check
    // that `aidememo` actually shows up. If the list subcommand doesn't
    // exist or fails for any reason, we leave `verified = None` —
    // the install already exited 0; it'd be hostile to fail the
    // command on a verification step that's only a defence in depth.
    // `--no-verify` skips this entirely for environments where the
    // list subcommand is slow, sandboxed, or noisy enough to defeat
    // the word-boundary heuristic.
    let verified = if no_verify {
        None
    } else {
        verify_registered(bin, "aidememo")
    };

    Ok(InstallReport {
        target,
        method: "shell-out".to_string(),
        detail: cmdline,
        overwrote: false,
        source_id: source_id.map(str::to_string),
        actor_id: actor_id.map(str::to_string),
        storage_backend: storage_backend.to_string(),
        store_path: store_path.display().to_string(),
        verified,
    })
}

fn normalise_identity_arg(
    flag: &str,
    value: Option<&str>,
) -> Result<Option<String>, AideMemoError> {
    let Some(value) = value.map(str::trim) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(AideMemoError::InvalidInput(format!(
            "{flag} must not be empty"
        )));
    }
    Ok(Some(value.to_string()))
}

fn stable_store_path(path: &Path) -> Result<PathBuf, AideMemoError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|e| AideMemoError::Internal(format!("resolve store path: {e}")))
}

fn agent_env(source_id: Option<&str>, actor_id: Option<&str>) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if let Some(source_id) = source_id {
        env.push(("AIDEMEMO_SOURCE_ID".to_string(), source_id.to_string()));
    }
    if let Some(actor_id) = actor_id {
        env.push(("AIDEMEMO_ACTOR_ID".to_string(), actor_id.to_string()));
    }
    env
}

fn aidememo_mcp_args(storage_backend: &str, store_path: &Path) -> Vec<String> {
    vec![
        "--backend".to_string(),
        storage_backend.to_string(),
        "--store".to_string(),
        store_path.display().to_string(),
        "mcp".to_string(),
    ]
}

fn claude_install_args(
    source_id: Option<&str>,
    actor_id: Option<&str>,
    storage_backend: &str,
    store_path: &Path,
) -> Vec<String> {
    let mut args = vec!["mcp".to_string(), "add".to_string()];
    for (key, value) in agent_env(source_id, actor_id) {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }
    args.extend(["aidememo", "--", "aidememo"].into_iter().map(String::from));
    args.extend(aidememo_mcp_args(storage_backend, store_path));
    args
}

fn hermes_install_args(
    source_id: Option<&str>,
    actor_id: Option<&str>,
    storage_backend: &str,
    store_path: &Path,
) -> Vec<String> {
    let mut args: Vec<String> = ["mcp", "add", "aidememo", "--command", "aidememo"]
        .into_iter()
        .map(String::from)
        .collect();
    for arg in aidememo_mcp_args(storage_backend, store_path) {
        args.push(format!("--args={arg}"));
    }
    for (key, value) in agent_env(source_id, actor_id) {
        args.push("--env".to_string());
        args.push(format!("{key}={value}"));
    }
    args
}

fn openclaw_install_args(
    source_id: Option<&str>,
    actor_id: Option<&str>,
    storage_backend: &str,
    store_path: &Path,
) -> Vec<String> {
    let aidememo_args = aidememo_mcp_args(storage_backend, store_path);
    let env: serde_json::Map<String, serde_json::Value> = agent_env(source_id, actor_id)
        .into_iter()
        .map(|(key, value)| (key, serde_json::Value::String(value)))
        .collect();
    let mut payload = serde_json::json!({"command": "aidememo", "args": aidememo_args});
    if !env.is_empty() {
        payload["env"] = serde_json::Value::Object(env);
    }
    vec![
        "mcp".to_string(),
        "set".to_string(),
        "aidememo".to_string(),
        payload.to_string(),
    ]
}

/// Run `<bin> mcp list` (text output) and check whether `token`
/// appears as an entry. Returns `None` if the command can't run or
/// exits non-zero — those signal "list subcommand not available",
/// not "install failed".
pub(crate) fn verify_registered(bin: &str, token: &str) -> Option<bool> {
    let out = ProcessCommand::new(bin)
        .args(["mcp", "list"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Some(stdout_contains_token(&stdout, token))
}

/// Word-boundary aware match — `stdout_contains_token("uvx-aidememo-x", "aidememo")`
/// is `false`, but `stdout_contains_token("aidememo: stdio …", "aidememo")` is
/// `true`. Pure / synchronous so it's the easy bit to unit test.
fn stdout_contains_token(stdout: &str, token: &str) -> bool {
    stdout.lines().any(|line| {
        line.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .any(|word| word == token)
    })
}

// ---------------------------------------------------------------------------
// Codex — edit the active CODEX_HOME/config.toml (or explicit profiles)
// ---------------------------------------------------------------------------

pub(crate) fn codex_config_path() -> Result<PathBuf, AideMemoError> {
    codex_config_path_for(None)
}

fn codex_config_path_for(codex_home: Option<&Path>) -> Result<PathBuf, AideMemoError> {
    let root = if let Some(path) = codex_home {
        path.to_path_buf()
    } else if let Some(path) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        PathBuf::from(path)
    } else {
        dirs::home_dir()
            .ok_or_else(|| AideMemoError::Internal("could not resolve $HOME".to_string()))?
            .join(".codex")
    };
    Ok(root.join("config.toml"))
}

fn install_codex(
    force: bool,
    print: bool,
    source_id: Option<&str>,
    actor_ids: &[String],
    storage_backend: &str,
    store_path: &Path,
    codex_homes: &[PathBuf],
) -> Result<InstallReport, AideMemoError> {
    let paths = if codex_homes.is_empty() {
        vec![codex_config_path()?]
    } else {
        codex_homes
            .iter()
            .map(|home| codex_config_path_for(Some(home)))
            .collect::<Result<Vec<_>, _>>()?
    };
    if actor_ids.len() > 1 && actor_ids.len() != paths.len() {
        return Err(AideMemoError::InvalidInput(format!(
            "received {} --actor-id values for {} Codex profiles; pass one shared actor id or one per --codex-home",
            actor_ids.len(),
            paths.len()
        )));
    }
    let actor_for = |index: usize| {
        if actor_ids.len() == 1 {
            actor_ids.first().map(String::as_str)
        } else {
            actor_ids.get(index).map(String::as_str)
        }
    };
    let report_actor_id = if actor_ids.is_empty() {
        None
    } else {
        Some(actor_ids.join(","))
    };
    let joined_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut detail = format!(
        "{} ([mcp_servers.aidememo] store={})",
        joined_paths,
        store_path.display()
    );
    if let Some(source_id) = source_id {
        detail.push_str(&format!(" env.AIDEMEMO_SOURCE_ID={source_id}"));
    }
    if let Some(actor_ids) = &report_actor_id {
        detail.push_str(&format!(" actor_ids={actor_ids}"));
    }

    if print {
        return Ok(InstallReport {
            target: "codex".to_string(),
            method: "file-edit".to_string(),
            detail,
            overwrote: false,
            source_id: source_id.map(str::to_string),
            actor_id: report_actor_id,
            storage_backend: storage_backend.to_string(),
            store_path: store_path.display().to_string(),
            verified: None,
        });
    }

    let mut overwrote = false;
    for (index, path) in paths.iter().enumerate() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AideMemoError::Internal(format!("create {}: {e}", parent.display()))
            })?;
        }

        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml::Value = if existing.trim().is_empty() {
            toml::Value::Table(toml::value::Table::new())
        } else {
            existing
                .parse::<toml::Value>()
                .map_err(|e| AideMemoError::Internal(format!("parse {}: {e}", path.display())))?
        };

        let table = doc.as_table_mut().ok_or_else(|| {
            AideMemoError::Internal(format!("{} is not a TOML table", path.display()))
        })?;
        let servers = table
            .entry("mcp_servers".to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
        let servers_table = servers.as_table_mut().ok_or_else(|| {
            AideMemoError::Internal("mcp_servers must be a TOML table".to_string())
        })?;

        let already = servers_table.contains_key("aidememo");
        if already && !force {
            return Err(AideMemoError::InvalidInput(format!(
                "[mcp_servers.aidememo] already exists in {} — pass --force to overwrite",
                path.display()
            )));
        }
        overwrote |= already;

        let mut aidememo_entry = toml::value::Table::new();
        aidememo_entry.insert(
            "command".to_string(),
            toml::Value::String("aidememo".to_string()),
        );
        aidememo_entry.insert(
            "args".to_string(),
            toml::Value::Array(
                aidememo_mcp_args(storage_backend, store_path)
                    .into_iter()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );
        let mut env = toml::value::Table::new();
        for (key, value) in agent_env(source_id, actor_for(index)) {
            env.insert(key, toml::Value::String(value));
        }
        if !env.is_empty() {
            aidememo_entry.insert("env".to_string(), toml::Value::Table(env));
        }
        servers_table.insert("aidememo".to_string(), toml::Value::Table(aidememo_entry));

        let serialized = toml::to_string_pretty(&doc)
            .map_err(|e| AideMemoError::Internal(format!("serialize codex config: {e}")))?;
        write_atomically(path, &serialized)?;
    }

    // File-edit is its own verification: we just parsed the file we
    // wrote, so we know the entry is there. Mark verified true so
    // operators get the same confidence signal as the shell-out path.
    Ok(InstallReport {
        target: "codex".to_string(),
        method: "file-edit".to_string(),
        detail,
        overwrote,
        source_id: source_id.map(str::to_string),
        actor_id: report_actor_id,
        storage_backend: storage_backend.to_string(),
        store_path: store_path.display().to_string(),
        verified: Some(true),
    })
}

// ---------------------------------------------------------------------------
// Cursor — edit ~/.cursor/mcp.json
// ---------------------------------------------------------------------------

pub(crate) fn opencode_config_path() -> Result<PathBuf, AideMemoError> {
    let home = dirs::home_dir()
        .ok_or_else(|| AideMemoError::Internal("could not resolve $HOME".to_string()))?;
    // opencode reads `~/.config/opencode/opencode.json` as the global
    // config (project configs sit at `./opencode.json`).
    Ok(home.join(".config/opencode/opencode.json"))
}

fn install_opencode(
    force: bool,
    print: bool,
    source_id: Option<&str>,
    actor_id: Option<&str>,
    storage_backend: &str,
    store_path: &Path,
) -> Result<InstallReport, AideMemoError> {
    let path = opencode_config_path()?;
    let mut detail = format!("{} (mcp.aidememo)", path.display());
    if let Some(source_id) = source_id {
        detail.push_str(&format!(" env.AIDEMEMO_SOURCE_ID={source_id}"));
    }
    if let Some(actor_id) = actor_id {
        detail.push_str(&format!(" env.AIDEMEMO_ACTOR_ID={actor_id}"));
    }

    if print {
        return Ok(InstallReport {
            target: "opencode".to_string(),
            method: "file-edit".to_string(),
            detail,
            overwrote: false,
            source_id: source_id.map(str::to_string),
            actor_id: actor_id.map(str::to_string),
            storage_backend: storage_backend.to_string(),
            store_path: store_path.display().to_string(),
            verified: None,
        });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AideMemoError::Internal(format!("create {}: {e}", parent.display())))?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: serde_json::Value = if existing.trim().is_empty() {
        // Drop the schema URL upstream embeds in fresh configs so
        // `opencode tui` / `opencode mcp list` recognize the file.
        serde_json::json!({"$schema": "https://opencode.ai/config.json"})
    } else {
        serde_json::from_str(&existing)
            .map_err(|e| AideMemoError::Internal(format!("parse {}: {e}", path.display())))?
    };

    let obj = doc.as_object_mut().ok_or_else(|| {
        AideMemoError::Internal(format!("{} is not a JSON object", path.display()))
    })?;
    // opencode keys MCP servers under `mcp` (singular), not
    // `mcpServers` like cursor. Each entry needs a `type` discriminant
    // and a single `command` array (binary + args, no separate args).
    let servers = obj
        .entry("mcp".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| AideMemoError::Internal("mcp must be a JSON object".to_string()))?;

    let already = servers_obj.contains_key("aidememo");
    if already && !force {
        return Err(AideMemoError::InvalidInput(format!(
            "mcp.aidememo already exists in {} — pass --force to overwrite",
            path.display()
        )));
    }
    let mut command = vec!["aidememo".to_string()];
    command.extend(aidememo_mcp_args(storage_backend, store_path));
    let mut aidememo_entry = serde_json::json!({
            "type": "local",
            "command": command,
            "enabled": true,
    });
    let env: serde_json::Map<String, serde_json::Value> = agent_env(source_id, actor_id)
        .into_iter()
        .map(|(key, value)| (key, serde_json::Value::String(value)))
        .collect();
    if !env.is_empty() {
        aidememo_entry["env"] = serde_json::Value::Object(env);
    }
    servers_obj.insert("aidememo".to_string(), aidememo_entry);

    let serialized = serde_json::to_string_pretty(&doc)
        .map_err(|e| AideMemoError::Internal(format!("serialize opencode config: {e}")))?;
    write_atomically(&path, &serialized)?;

    Ok(InstallReport {
        target: "opencode".to_string(),
        method: "file-edit".to_string(),
        detail,
        overwrote: already,
        source_id: source_id.map(str::to_string),
        actor_id: actor_id.map(str::to_string),
        storage_backend: storage_backend.to_string(),
        store_path: store_path.display().to_string(),
        verified: Some(true),
    })
}

pub(crate) fn cursor_config_path() -> Result<PathBuf, AideMemoError> {
    let home = dirs::home_dir()
        .ok_or_else(|| AideMemoError::Internal("could not resolve $HOME".to_string()))?;
    Ok(home.join(".cursor/mcp.json"))
}

fn install_cursor(
    force: bool,
    print: bool,
    source_id: Option<&str>,
    actor_id: Option<&str>,
    storage_backend: &str,
    store_path: &Path,
) -> Result<InstallReport, AideMemoError> {
    let path = cursor_config_path()?;
    let mut detail = format!("{} (mcpServers.aidememo)", path.display());
    if let Some(source_id) = source_id {
        detail.push_str(&format!(" env.AIDEMEMO_SOURCE_ID={source_id}"));
    }
    if let Some(actor_id) = actor_id {
        detail.push_str(&format!(" env.AIDEMEMO_ACTOR_ID={actor_id}"));
    }

    if print {
        return Ok(InstallReport {
            target: "cursor".to_string(),
            method: "file-edit".to_string(),
            detail,
            overwrote: false,
            source_id: source_id.map(str::to_string),
            actor_id: actor_id.map(str::to_string),
            storage_backend: storage_backend.to_string(),
            store_path: store_path.display().to_string(),
            verified: None,
        });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AideMemoError::Internal(format!("create {}: {e}", parent.display())))?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&existing)
            .map_err(|e| AideMemoError::Internal(format!("parse {}: {e}", path.display())))?
    };

    let obj = doc.as_object_mut().ok_or_else(|| {
        AideMemoError::Internal(format!("{} is not a JSON object", path.display()))
    })?;
    let servers = obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| AideMemoError::Internal("mcpServers must be a JSON object".to_string()))?;

    let already = servers_obj.contains_key("aidememo");
    if already && !force {
        return Err(AideMemoError::InvalidInput(format!(
            "mcpServers.aidememo already exists in {} — pass --force to overwrite",
            path.display()
        )));
    }
    let mut aidememo_entry = serde_json::json!({
        "command": "aidememo",
        "args": aidememo_mcp_args(storage_backend, store_path)
    });
    let env: serde_json::Map<String, serde_json::Value> = agent_env(source_id, actor_id)
        .into_iter()
        .map(|(key, value)| (key, serde_json::Value::String(value)))
        .collect();
    if !env.is_empty() {
        aidememo_entry["env"] = serde_json::Value::Object(env);
    }
    servers_obj.insert("aidememo".to_string(), aidememo_entry);

    let serialized = serde_json::to_string_pretty(&doc)
        .map_err(|e| AideMemoError::Internal(format!("serialize cursor config: {e}")))?;
    write_atomically(&path, &serialized)?;

    Ok(InstallReport {
        target: "cursor".to_string(),
        method: "file-edit".to_string(),
        detail,
        overwrote: already,
        source_id: source_id.map(str::to_string),
        actor_id: actor_id.map(str::to_string),
        storage_backend: storage_backend.to_string(),
        store_path: store_path.display().to_string(),
        verified: Some(true),
    })
}

// ---------------------------------------------------------------------------
// Atomic write helper
// ---------------------------------------------------------------------------

fn write_atomically(path: &std::path::Path, contents: &str) -> Result<(), AideMemoError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)
        .map_err(|e| AideMemoError::Internal(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| AideMemoError::Internal(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_mode_for_claude_returns_command() {
        let store = Path::new("/tmp/aidememo-test.sqlite");
        let report = install_via_cli(
            "claude",
            claude_install_args(None, None, "libsqlite", store),
            ShellInstallOptions {
                print: true,
                no_verify: false,
                source_id: None,
                actor_id: None,
                storage_backend: "libsqlite",
                store_path: store,
            },
        )
        .unwrap();
        assert_eq!(report.target, "claude");
        assert!(report.detail.contains("mcp add aidememo"));
        assert!(report.detail.contains("--backend libsqlite --store"));
        assert_eq!(report.storage_backend, "libsqlite");
        assert!(!report.overwrote);
    }

    #[test]
    fn unknown_target_errors() {
        let sub = McpInstallSub {
            target: "noexist".to_string(),
            force: false,
            print: true,
            list_targets: false,
            no_verify: false,
            source_id: None,
            actor_ids: Vec::new(),
            codex_homes: Vec::new(),
        };
        let err = run_mcp_install(
            sub,
            false,
            "libsqlite",
            Path::new("/tmp/aidememo-test.sqlite"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown target"));
    }

    #[test]
    fn no_verify_skips_post_install_check() {
        // Use `true` as a stand-in for the agent CLI: it exists on
        // every Unix, exits 0, and ignores arguments — so the install
        // step "succeeds" cheaply and we can isolate the verify
        // branch. With `no_verify=true`, `verified` must be `None`
        // (we never call `verify_registered`); without it we'd fall
        // through to `true mcp list` which would also be `None` but
        // for a different reason. The contract we care about is:
        // `no_verify=true` short-circuits the call.
        let report = install_via_cli(
            "true",
            Vec::new(),
            ShellInstallOptions {
                print: false,
                no_verify: true,
                source_id: None,
                actor_id: None,
                storage_backend: "libsqlite",
                store_path: Path::new("/tmp/aidememo-test.sqlite"),
            },
        )
        .unwrap();
        assert_eq!(report.verified, None);
    }

    #[test]
    fn codex_writes_fresh_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Inline the merge logic — we can't easily redirect HOME.
        let mut doc = toml::Value::Table(toml::value::Table::new());
        let servers = doc
            .as_table_mut()
            .unwrap()
            .entry("mcp_servers".to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
        let mut aidememo = toml::value::Table::new();
        aidememo.insert("command".into(), toml::Value::String("aidememo".into()));
        aidememo.insert(
            "args".into(),
            toml::Value::Array(vec![
                toml::Value::String("--backend".into()),
                toml::Value::String("libsqlite".into()),
                toml::Value::String("mcp".into()),
            ]),
        );
        let mut env = toml::value::Table::new();
        env.insert(
            "AIDEMEMO_SOURCE_ID".into(),
            toml::Value::String("project-alpha".into()),
        );
        aidememo.insert("env".into(), toml::Value::Table(env));
        servers
            .as_table_mut()
            .unwrap()
            .insert("aidememo".into(), toml::Value::Table(aidememo));

        let s = toml::to_string_pretty(&doc).unwrap();
        std::fs::write(&path, &s).unwrap();
        let parsed: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            parsed["mcp_servers"]["aidememo"]["command"].as_str(),
            Some("aidememo")
        );
        assert_eq!(
            parsed["mcp_servers"]["aidememo"]["args"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(toml::Value::as_str)
                .collect::<Vec<_>>(),
            vec!["--backend", "libsqlite", "mcp"]
        );
        assert_eq!(
            parsed["mcp_servers"]["aidememo"]["env"]["AIDEMEMO_SOURCE_ID"].as_str(),
            Some("project-alpha")
        );
    }

    #[test]
    fn codex_installs_same_store_into_multiple_profile_homes() {
        let dir = tempfile::tempdir().unwrap();
        let profile_a = dir.path().join("codex-a");
        let profile_b = dir.path().join("codex-b");
        let store = dir.path().join("shared.sqlite");
        let report = install_codex(
            false,
            false,
            Some("project:aidememo"),
            &["codex:account-a".to_string(), "codex:account-b".to_string()],
            "libsqlite",
            &store,
            &[profile_a.clone(), profile_b.clone()],
        )
        .unwrap();
        assert!(report.detail.contains("codex-a/config.toml"));
        assert!(report.detail.contains("codex-b/config.toml"));
        for (profile, actor_id) in [
            (profile_a, "codex:account-a"),
            (profile_b, "codex:account-b"),
        ] {
            let body = std::fs::read_to_string(profile.join("config.toml")).unwrap();
            let parsed: toml::Value = body.parse().unwrap();
            let entry = &parsed["mcp_servers"]["aidememo"];
            let args = entry["args"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(toml::Value::as_str)
                .collect::<Vec<_>>();
            assert_eq!(
                args,
                vec![
                    "--backend",
                    "libsqlite",
                    "--store",
                    store.to_str().unwrap(),
                    "mcp"
                ]
            );
            assert_eq!(
                entry["env"]["AIDEMEMO_SOURCE_ID"].as_str(),
                Some("project:aidememo")
            );
            assert_eq!(entry["env"]["AIDEMEMO_ACTOR_ID"].as_str(), Some(actor_id));
        }
    }

    #[test]
    fn stdout_contains_token_matches_word_boundary() {
        // Whole-line entry — the common claude/openclaw `mcp list` form.
        assert!(stdout_contains_token(
            "aidememo: stdio command=aidememo args=[mcp]\n",
            "aidememo"
        ));
        // Two-column form — `name<space><type>` (some hermes versions).
        assert!(stdout_contains_token(
            "aidememo            stdio\nctx7  http\n",
            "aidememo"
        ));
        // Token nestled in punctuation: `[aidememo]` should match.
        assert!(stdout_contains_token(
            "servers: [context7, aidememo, docs]\n",
            "aidememo"
        ));
    }

    #[test]
    fn stdout_contains_token_rejects_substring() {
        // `uvx-aidememo` shares the substring but is not a separate entry.
        assert!(!stdout_contains_token(
            "uvx-aidememo-bridge: stdio\n",
            "aidememo"
        ));
        // Empty stdout never matches.
        assert!(!stdout_contains_token("", "aidememo"));
        // Token with hyphens — exact match only.
        assert!(stdout_contains_token(
            "hermes-aidememo: stdio\n",
            "hermes-aidememo"
        ));
        assert!(!stdout_contains_token(
            "hermes-aidememo-x: stdio\n",
            "hermes-aidememo"
        ));
    }

    #[test]
    fn cursor_writes_fresh_config() {
        let mut doc = serde_json::json!({});
        let obj = doc.as_object_mut().unwrap();
        obj.insert("mcpServers".into(), serde_json::json!({}));
        obj["mcpServers"].as_object_mut().unwrap().insert(
            "aidememo".into(),
            serde_json::json!({
                "command": "aidememo",
                "args": ["--backend", "libsqlite", "mcp"],
                "env": {"AIDEMEMO_SOURCE_ID": "project-alpha"}
            }),
        );
        assert_eq!(doc["mcpServers"]["aidememo"]["command"], "aidememo");
        assert_eq!(doc["mcpServers"]["aidememo"]["args"][0], "--backend");
        assert_eq!(doc["mcpServers"]["aidememo"]["args"][1], "libsqlite");
        assert_eq!(doc["mcpServers"]["aidememo"]["args"][2], "mcp");
        assert_eq!(
            doc["mcpServers"]["aidememo"]["env"]["AIDEMEMO_SOURCE_ID"],
            "project-alpha"
        );
    }

    #[test]
    fn opencode_target_in_supported_list() {
        let names: Vec<&str> = SUPPORTED.iter().map(|(t, _, _)| *t).collect();
        assert!(names.contains(&"opencode"));
        // pi must NOT be in the MCP install matrix — it rejects MCP
        // upstream and is skill-only.
        assert!(!names.contains(&"pi"));
    }

    #[test]
    fn opencode_install_print_describes_the_path() {
        let report = install_opencode(
            false,
            /*print*/ true,
            Some("project-alpha"),
            Some("codex:account-a"),
            "libsqlite",
            Path::new("/tmp/aidememo-test.sqlite"),
        )
        .unwrap();
        assert_eq!(report.target, "opencode");
        assert_eq!(report.method, "file-edit");
        assert!(report.detail.contains("opencode.json"));
        assert!(report.detail.contains("mcp.aidememo"));
        assert_eq!(report.source_id.as_deref(), Some("project-alpha"));
        assert_eq!(report.storage_backend, "libsqlite");
    }

    #[test]
    fn shell_install_args_include_source_id_env() {
        let store = Path::new("/tmp/aidememo-test.sqlite");
        assert_eq!(
            claude_install_args(
                Some("project-alpha"),
                Some("codex:account-a"),
                "libsqlite",
                store,
            ),
            vec![
                "mcp",
                "add",
                "-e",
                "AIDEMEMO_SOURCE_ID=project-alpha",
                "-e",
                "AIDEMEMO_ACTOR_ID=codex:account-a",
                "aidememo",
                "--",
                "aidememo",
                "--backend",
                "libsqlite",
                "--store",
                "/tmp/aidememo-test.sqlite",
                "mcp"
            ]
        );
        assert!(
            hermes_install_args(Some("project-alpha"), None, "libsqlite", store)
                .contains(&"AIDEMEMO_SOURCE_ID=project-alpha".to_string())
        );
        assert!(
            hermes_install_args(Some("project-alpha"), None, "libsqlite", store)
                .contains(&"--args=--backend".to_string())
        );
        let openclaw_payload =
            &openclaw_install_args(Some("project-alpha"), None, "libsqlite", store)[3];
        assert!(openclaw_payload.contains("AIDEMEMO_SOURCE_ID"));
        assert!(openclaw_payload.contains("--backend"));
        assert!(openclaw_payload.contains("libsqlite"));
    }
}
