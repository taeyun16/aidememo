//! `wg mcp-install` — register the wg MCP server with a target agent.
//!
//! Each agent has a different surface for MCP registration:
//!
//! | target   | mechanism                                                        |
//! |----------|------------------------------------------------------------------|
//! | claude   | shells out to `claude mcp add wg -- wg mcp`                      |
//! | hermes   | shells out to `hermes mcp add wg --command wg --args mcp`        |
//! | openclaw | shells out to `openclaw mcp set wg '{"command":"wg",...}'`       |
//! | codex    | edits `~/.codex/config.toml` to add `[mcp_servers.wg]`           |
//! | cursor   | edits `~/.cursor/mcp.json` to add `{"mcpServers": {"wg": ...}}`  |
//!
//! For shell-out targets, the agent's own CLI is the source of truth
//! for *where* the entry lands — we just invoke it. For file-edit
//! targets, we read the existing config, merge in `wg`, and write it
//! back atomically (stage to `.tmp`, rename).
//!
//! Use `--print` to preview the action without executing, and
//! `--force` to overwrite an existing `wg` entry.

use bpaf::*;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use crate::cmd::Command;
use wg_core::WgError;

#[derive(Debug, Clone)]
pub struct McpInstallSub {
    pub target: String,
    pub force: bool,
    pub print: bool,
    pub list_targets: bool,
    pub no_verify: bool,
}

pub fn mcp_install_command() -> impl Parser<Command> {
    let target = long("target")
        .help(
            "Target agent: claude, hermes, openclaw, codex, cursor, opencode \
             (or 'list'). pi is intentionally not supported — pi rejects MCP \
             upstream; use `wg skill install --target pi` instead.",
        )
        .argument::<String>("TARGET")
        .fallback("claude".to_string());
    let force = long("force")
        .help("Overwrite an existing `wg` MCP entry")
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

    construct!(McpInstallSub {
        target,
        force,
        print,
        list_targets,
        no_verify,
    })
    .map(Command::McpInstall)
    .to_options()
    .command("mcp-install")
    .help(
        "Register the wg MCP server with an agent (claude / hermes / \
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
    /// Result of the post-install best-effort check that the agent
    /// actually picked up the new server. `None` when verification
    /// wasn't attempted (file-edit targets, `--print` mode, missing
    /// `<bin> mcp list` subcommand). `Some(true)` when `wg` appeared
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
    ("claude", "shell-out", "claude mcp add wg -- wg mcp"),
    (
        "hermes",
        "shell-out",
        "hermes mcp add wg --command wg --args mcp",
    ),
    (
        "openclaw",
        "shell-out",
        "openclaw mcp set wg '{\"command\":\"wg\",\"args\":[\"mcp\"]}'",
    ),
    (
        "codex",
        "file-edit",
        "edit ~/.codex/config.toml: add [mcp_servers.wg]",
    ),
    (
        "cursor",
        "file-edit",
        "edit ~/.cursor/mcp.json: add mcpServers.wg",
    ),
    (
        "opencode",
        "file-edit",
        "edit ~/.config/opencode/opencode.json: add mcp.wg",
    ),
];

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

pub fn run_mcp_install(sub: McpInstallSub, json: bool) -> Result<String, WgError> {
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
            return serde_json::to_string_pretty(&entries).map_err(|e| WgError::Serialize {
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

    let report = match sub.target.as_str() {
        "claude" | "claude-code" => install_via_cli(
            "claude",
            &["mcp", "add", "wg", "--", "wg", "mcp"],
            sub.force,
            sub.print,
            sub.no_verify,
        )?,
        "hermes" => install_via_cli(
            "hermes",
            &["mcp", "add", "wg", "--command", "wg", "--args", "mcp"],
            sub.force,
            sub.print,
            sub.no_verify,
        )?,
        "openclaw" => install_via_cli(
            "openclaw",
            &["mcp", "set", "wg", r#"{"command":"wg","args":["mcp"]}"#],
            sub.force,
            sub.print,
            sub.no_verify,
        )?,
        "codex" => install_codex(sub.force, sub.print)?,
        "cursor" => install_cursor(sub.force, sub.print)?,
        "opencode" => install_opencode(sub.force, sub.print)?,
        "pi" => {
            return Err(WgError::InvalidInput(
                "pi has no MCP support by upstream design — pi rejects MCP \
                 because the protocol's tool descriptions consume too much \
                 of the context window. Use `wg skill install --target pi` \
                 instead to merge wg's skill into ~/.config/pi/AGENTS.md."
                    .to_string(),
            ));
        }
        other => {
            return Err(WgError::InvalidInput(format!(
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
        return serde_json::to_string_pretty(&report).map_err(|e| WgError::Serialize {
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
        "{verb} wg MCP server for {} ({})\n  {}\n",
        report.target, report.method, report.detail
    );
    match report.verified {
        Some(true) => out.push_str("  verified: wg appears in the agent's MCP list ✓\n"),
        Some(false) => out.push_str(
            "  verified: ⚠ install command exited 0 but `wg` did not show up — \
             check the agent's config manually\n",
        ),
        None => {}
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Shell-out targets (claude / hermes / openclaw)
// ---------------------------------------------------------------------------

fn install_via_cli(
    bin: &str,
    args: &[&str],
    _force: bool,
    print: bool,
    no_verify: bool,
) -> Result<InstallReport, WgError> {
    let cmdline = format!("{} {}", bin, args.join(" "));
    let target = bin.to_string();

    if print {
        return Ok(InstallReport {
            target,
            method: "shell-out".to_string(),
            detail: cmdline,
            overwrote: false,
            verified: None,
        });
    }

    let out = ProcessCommand::new(bin).args(args).output().map_err(|e| {
        WgError::InvalidInput(format!(
            "could not run `{}` — is the {} CLI on your PATH? (raw error: {})",
            bin, bin, e
        ))
    })?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(WgError::Internal(format!(
            "`{}` exited with {}: {}",
            cmdline,
            out.status,
            stderr.trim()
        )));
    }

    // Best-effort: ask the agent to list its MCP servers and check
    // that `wg` actually shows up. If the list subcommand doesn't
    // exist or fails for any reason, we leave `verified = None` —
    // the install already exited 0; it'd be hostile to fail the
    // command on a verification step that's only a defence in depth.
    // `--no-verify` skips this entirely for environments where the
    // list subcommand is slow, sandboxed, or noisy enough to defeat
    // the word-boundary heuristic.
    let verified = if no_verify {
        None
    } else {
        verify_registered(bin, "wg")
    };

    Ok(InstallReport {
        target,
        method: "shell-out".to_string(),
        detail: cmdline,
        overwrote: false,
        verified,
    })
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

/// Word-boundary aware match — `stdout_contains_token("uvx-wg-x", "wg")`
/// is `false`, but `stdout_contains_token("wg: stdio …", "wg")` is
/// `true`. Pure / synchronous so it's the easy bit to unit test.
fn stdout_contains_token(stdout: &str, token: &str) -> bool {
    stdout.lines().any(|line| {
        line.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .any(|word| word == token)
    })
}

// ---------------------------------------------------------------------------
// Codex — edit ~/.codex/config.toml
// ---------------------------------------------------------------------------

pub(crate) fn codex_config_path() -> Result<PathBuf, WgError> {
    let home =
        dirs::home_dir().ok_or_else(|| WgError::Internal("could not resolve $HOME".to_string()))?;
    Ok(home.join(".codex/config.toml"))
}

fn install_codex(force: bool, print: bool) -> Result<InstallReport, WgError> {
    let path = codex_config_path()?;
    let detail = format!(
        "{} ([mcp_servers.wg] command=wg args=[\"mcp\"])",
        path.display()
    );

    if print {
        return Ok(InstallReport {
            target: "codex".to_string(),
            method: "file-edit".to_string(),
            detail,
            overwrote: false,
            verified: None,
        });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WgError::Internal(format!("create {}: {e}", parent.display())))?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml::Value = if existing.trim().is_empty() {
        toml::Value::Table(toml::value::Table::new())
    } else {
        existing
            .parse::<toml::Value>()
            .map_err(|e| WgError::Internal(format!("parse {}: {e}", path.display())))?
    };

    let table = doc
        .as_table_mut()
        .ok_or_else(|| WgError::Internal(format!("{} is not a TOML table", path.display())))?;
    let servers = table
        .entry("mcp_servers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let servers_table = servers
        .as_table_mut()
        .ok_or_else(|| WgError::Internal("mcp_servers must be a TOML table".to_string()))?;

    let already = servers_table.contains_key("wg");
    if already && !force {
        return Err(WgError::InvalidInput(format!(
            "[mcp_servers.wg] already exists in {} — pass --force to overwrite",
            path.display()
        )));
    }

    let mut wg_entry = toml::value::Table::new();
    wg_entry.insert("command".to_string(), toml::Value::String("wg".to_string()));
    wg_entry.insert(
        "args".to_string(),
        toml::Value::Array(vec![toml::Value::String("mcp".to_string())]),
    );
    servers_table.insert("wg".to_string(), toml::Value::Table(wg_entry));

    let serialized = toml::to_string_pretty(&doc)
        .map_err(|e| WgError::Internal(format!("serialize codex config: {e}")))?;
    write_atomically(&path, &serialized)?;

    // File-edit is its own verification: we just parsed the file we
    // wrote, so we know the entry is there. Mark verified true so
    // operators get the same confidence signal as the shell-out path.
    Ok(InstallReport {
        target: "codex".to_string(),
        method: "file-edit".to_string(),
        detail,
        overwrote: already,
        verified: Some(true),
    })
}

// ---------------------------------------------------------------------------
// Cursor — edit ~/.cursor/mcp.json
// ---------------------------------------------------------------------------

pub(crate) fn opencode_config_path() -> Result<PathBuf, WgError> {
    let home =
        dirs::home_dir().ok_or_else(|| WgError::Internal("could not resolve $HOME".to_string()))?;
    // opencode reads `~/.config/opencode/opencode.json` as the global
    // config (project configs sit at `./opencode.json`).
    Ok(home.join(".config/opencode/opencode.json"))
}

fn install_opencode(force: bool, print: bool) -> Result<InstallReport, WgError> {
    let path = opencode_config_path()?;
    let detail = format!("{} (mcp.wg)", path.display());

    if print {
        return Ok(InstallReport {
            target: "opencode".to_string(),
            method: "file-edit".to_string(),
            detail,
            overwrote: false,
            verified: None,
        });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WgError::Internal(format!("create {}: {e}", parent.display())))?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: serde_json::Value = if existing.trim().is_empty() {
        // Drop the schema URL upstream embeds in fresh configs so
        // `opencode tui` / `opencode mcp list` recognize the file.
        serde_json::json!({"$schema": "https://opencode.ai/config.json"})
    } else {
        serde_json::from_str(&existing)
            .map_err(|e| WgError::Internal(format!("parse {}: {e}", path.display())))?
    };

    let obj = doc
        .as_object_mut()
        .ok_or_else(|| WgError::Internal(format!("{} is not a JSON object", path.display())))?;
    // opencode keys MCP servers under `mcp` (singular), not
    // `mcpServers` like cursor. Each entry needs a `type` discriminant
    // and a single `command` array (binary + args, no separate args).
    let servers = obj
        .entry("mcp".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| WgError::Internal("mcp must be a JSON object".to_string()))?;

    let already = servers_obj.contains_key("wg");
    if already && !force {
        return Err(WgError::InvalidInput(format!(
            "mcp.wg already exists in {} — pass --force to overwrite",
            path.display()
        )));
    }
    servers_obj.insert(
        "wg".to_string(),
        serde_json::json!({
            "type": "local",
            "command": ["wg", "mcp"],
            "enabled": true,
        }),
    );

    let serialized = serde_json::to_string_pretty(&doc)
        .map_err(|e| WgError::Internal(format!("serialize opencode config: {e}")))?;
    write_atomically(&path, &serialized)?;

    Ok(InstallReport {
        target: "opencode".to_string(),
        method: "file-edit".to_string(),
        detail,
        overwrote: already,
        verified: Some(true),
    })
}

pub(crate) fn cursor_config_path() -> Result<PathBuf, WgError> {
    let home =
        dirs::home_dir().ok_or_else(|| WgError::Internal("could not resolve $HOME".to_string()))?;
    Ok(home.join(".cursor/mcp.json"))
}

fn install_cursor(force: bool, print: bool) -> Result<InstallReport, WgError> {
    let path = cursor_config_path()?;
    let detail = format!("{} (mcpServers.wg)", path.display());

    if print {
        return Ok(InstallReport {
            target: "cursor".to_string(),
            method: "file-edit".to_string(),
            detail,
            overwrote: false,
            verified: None,
        });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WgError::Internal(format!("create {}: {e}", parent.display())))?;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&existing)
            .map_err(|e| WgError::Internal(format!("parse {}: {e}", path.display())))?
    };

    let obj = doc
        .as_object_mut()
        .ok_or_else(|| WgError::Internal(format!("{} is not a JSON object", path.display())))?;
    let servers = obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| WgError::Internal("mcpServers must be a JSON object".to_string()))?;

    let already = servers_obj.contains_key("wg");
    if already && !force {
        return Err(WgError::InvalidInput(format!(
            "mcpServers.wg already exists in {} — pass --force to overwrite",
            path.display()
        )));
    }
    servers_obj.insert(
        "wg".to_string(),
        serde_json::json!({"command": "wg", "args": ["mcp"]}),
    );

    let serialized = serde_json::to_string_pretty(&doc)
        .map_err(|e| WgError::Internal(format!("serialize cursor config: {e}")))?;
    write_atomically(&path, &serialized)?;

    Ok(InstallReport {
        target: "cursor".to_string(),
        method: "file-edit".to_string(),
        detail,
        overwrote: already,
        verified: Some(true),
    })
}

// ---------------------------------------------------------------------------
// Atomic write helper
// ---------------------------------------------------------------------------

fn write_atomically(path: &std::path::Path, contents: &str) -> Result<(), WgError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)
        .map_err(|e| WgError::Internal(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| WgError::Internal(format!("rename {}: {e}", path.display())))?;
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
        let report = install_via_cli(
            "claude",
            &["mcp", "add", "wg", "--", "wg", "mcp"],
            false,
            true,
            false,
        )
        .unwrap();
        assert_eq!(report.target, "claude");
        assert!(report.detail.contains("mcp add wg"));
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
        };
        let err = run_mcp_install(sub, false).unwrap_err();
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
        let report = install_via_cli("true", &[], false, false, true).unwrap();
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
        let mut wg = toml::value::Table::new();
        wg.insert("command".into(), toml::Value::String("wg".into()));
        wg.insert(
            "args".into(),
            toml::Value::Array(vec![toml::Value::String("mcp".into())]),
        );
        servers
            .as_table_mut()
            .unwrap()
            .insert("wg".into(), toml::Value::Table(wg));

        let s = toml::to_string_pretty(&doc).unwrap();
        std::fs::write(&path, &s).unwrap();
        let parsed: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(parsed["mcp_servers"]["wg"]["command"].as_str(), Some("wg"));
    }

    #[test]
    fn stdout_contains_token_matches_word_boundary() {
        // Whole-line entry — the common claude/openclaw `mcp list` form.
        assert!(stdout_contains_token(
            "wg: stdio command=wg args=[mcp]\n",
            "wg"
        ));
        // Two-column form — `name<space><type>` (some hermes versions).
        assert!(stdout_contains_token(
            "wg            stdio\nctx7  http\n",
            "wg"
        ));
        // Token nestled in punctuation: `[wg]` should match.
        assert!(stdout_contains_token(
            "servers: [context7, wg, docs]\n",
            "wg"
        ));
    }

    #[test]
    fn stdout_contains_token_rejects_substring() {
        // `uvx-wg` shares the substring but is not a separate entry.
        assert!(!stdout_contains_token("uvx-wg-bridge: stdio\n", "wg"));
        // Empty stdout never matches.
        assert!(!stdout_contains_token("", "wg"));
        // Token with hyphens — exact match only.
        assert!(stdout_contains_token("hermes-wg: stdio\n", "hermes-wg"));
        assert!(!stdout_contains_token("hermes-wg-x: stdio\n", "hermes-wg"));
    }

    #[test]
    fn cursor_writes_fresh_config() {
        let mut doc = serde_json::json!({});
        let obj = doc.as_object_mut().unwrap();
        obj.insert("mcpServers".into(), serde_json::json!({}));
        obj["mcpServers"].as_object_mut().unwrap().insert(
            "wg".into(),
            serde_json::json!({"command": "wg", "args": ["mcp"]}),
        );
        assert_eq!(doc["mcpServers"]["wg"]["command"], "wg");
        assert_eq!(doc["mcpServers"]["wg"]["args"][0], "mcp");
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
        let report = install_opencode(false, /*print*/ true).unwrap();
        assert_eq!(report.target, "opencode");
        assert_eq!(report.method, "file-edit");
        assert!(report.detail.contains("opencode.json"));
        assert!(report.detail.contains("mcp.wg"));
    }
}
