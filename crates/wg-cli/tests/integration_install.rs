//! Integration tests for `wg skill install` and `wg mcp-install`.
//!
//! These exercise the full binary path (build via Cargo, exec via
//! `CARGO_BIN_EXE_wg`) so we catch wiring breakage that the unit
//! tests in `cmd/skill.rs` and `cmd/mcp_install.rs` can't see —
//! argparse drift, frontmatter inclusion, atomic-write failures,
//! and the way we merge into an existing config.
//!
//! Each test uses an isolated `tempfile::tempdir()` for either the
//! `--dest` override (skill install) or `$HOME` (mcp-install
//! file-edit targets), so they never touch the developer's real
//! `~/.claude`, `~/.codex`, or `~/.cursor` directories.

use std::path::Path;
use std::process::Command;

fn wg_bin() -> &'static str {
    env!("CARGO_BIN_EXE_wg")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(wg_bin())
        .args(args)
        .output()
        .expect("failed to execute wg binary")
}

fn run_with_home(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(wg_bin())
        .env_remove("WG_STORE")
        .env("HOME", home)
        .args(args)
        .output()
        .expect("failed to execute wg binary")
}

// ---------------------------------------------------------------------------
// `wg skill install`
// ---------------------------------------------------------------------------

#[test]
fn skill_install_list_targets_lists_all_supported() {
    let out = run(&["skill", "install", "--list-targets"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for target in ["claude", "hermes", "openclaw", "agents"] {
        assert!(
            stdout.contains(target),
            "expected target `{}` in --list-targets output:\n{}",
            target,
            stdout
        );
    }
}

#[test]
fn skill_install_writes_skill_and_reference() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("hermes-skill");

    let out = run(&[
        "skill",
        "install",
        "--target",
        "hermes",
        "--dest",
        dest.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let skill = std::fs::read_to_string(dest.join("SKILL.md")).unwrap();
    assert!(skill.starts_with("---"), "SKILL.md missing frontmatter");
    assert!(
        skill.contains("name: wg"),
        "name field missing in installed SKILL.md"
    );

    let reference = std::fs::read_to_string(dest.join("REFERENCE.md")).unwrap();
    assert!(!reference.is_empty(), "REFERENCE.md should not be empty");
}

#[test]
fn skill_install_refuses_existing_dest_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("existing-skill");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("SKILL.md"), "old content").unwrap();

    let out = run(&[
        "skill",
        "install",
        "--target",
        "hermes",
        "--dest",
        dest.to_str().unwrap(),
    ]);
    assert!(!out.status.success(), "should refuse without --force");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists"),
        "expected 'already exists' error, got: {}",
        stderr
    );

    // SKILL.md must not have been overwritten.
    assert_eq!(
        std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
        "old content"
    );
}

#[test]
fn skill_install_overwrites_with_force() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("force-skill");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("SKILL.md"), "old content").unwrap();

    let out = run(&[
        "skill",
        "install",
        "--target",
        "hermes",
        "--dest",
        dest.to_str().unwrap(),
        "--force",
    ]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let skill = std::fs::read_to_string(dest.join("SKILL.md")).unwrap();
    assert!(
        skill.contains("name: wg"),
        "should now have the bundled SKILL.md"
    );
    assert!(
        !skill.contains("old content"),
        "old content should have been replaced"
    );
}

#[test]
fn skill_install_unknown_target_errors() {
    let out = run(&["skill", "install", "--target", "nope"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown target"),
        "unexpected stderr: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// `wg mcp-install` — file-edit targets (codex, cursor)
// ---------------------------------------------------------------------------

#[test]
fn mcp_install_codex_writes_fresh_config() {
    let home = tempfile::tempdir().unwrap();
    let out = run_with_home(home.path(), &["mcp-install", "--target", "codex"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg_path = home.path().join(".codex/config.toml");
    let body = std::fs::read_to_string(&cfg_path).unwrap();
    let parsed: toml::Value = body.parse().unwrap();
    let wg = &parsed["mcp_servers"]["wg"];
    assert_eq!(wg["command"].as_str(), Some("wg"));
    assert_eq!(wg["args"].as_array().unwrap()[0].as_str(), Some("mcp"));
}

#[test]
fn mcp_install_codex_preserves_existing_entries() {
    // The user already has another MCP server configured. Our merge
    // must add wg without disturbing it.
    let home = tempfile::tempdir().unwrap();
    let cfg_path = home.path().join(".codex/config.toml");
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg_path,
        "[mcp_servers.context7]\ncommand = \"uvx\"\nargs = [\"context7-mcp\"]\n",
    )
    .unwrap();

    let out = run_with_home(home.path(), &["mcp-install", "--target", "codex"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: toml::Value = std::fs::read_to_string(&cfg_path).unwrap().parse().unwrap();
    assert_eq!(
        parsed["mcp_servers"]["context7"]["command"].as_str(),
        Some("uvx")
    );
    assert_eq!(parsed["mcp_servers"]["wg"]["command"].as_str(), Some("wg"));
}

#[test]
fn mcp_install_codex_refuses_overwrite_without_force() {
    let home = tempfile::tempdir().unwrap();

    let first = run_with_home(home.path(), &["mcp-install", "--target", "codex"]);
    assert!(first.status.success());

    let second = run_with_home(home.path(), &["mcp-install", "--target", "codex"]);
    assert!(
        !second.status.success(),
        "second install should require --force"
    );

    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("already exists"),
        "expected 'already exists' error, got: {}",
        stderr
    );
}

#[test]
fn mcp_install_codex_force_overwrites() {
    let home = tempfile::tempdir().unwrap();

    let first = run_with_home(home.path(), &["mcp-install", "--target", "codex"]);
    assert!(first.status.success());

    let second = run_with_home(
        home.path(),
        &["mcp-install", "--target", "codex", "--force"],
    );
    assert!(second.status.success());

    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("Updated"),
        "expected 'Updated' verb, got: {}",
        stdout
    );
}

#[test]
fn mcp_install_cursor_writes_fresh_config() {
    let home = tempfile::tempdir().unwrap();
    let out = run_with_home(home.path(), &["mcp-install", "--target", "cursor"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg_path = home.path().join(".cursor/mcp.json");
    let body = std::fs::read_to_string(&cfg_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["mcpServers"]["wg"]["command"], "wg");
    assert_eq!(parsed["mcpServers"]["wg"]["args"][0], "mcp");
}

#[test]
fn mcp_install_cursor_preserves_user_keys() {
    let home = tempfile::tempdir().unwrap();
    let cfg_path = home.path().join(".cursor/mcp.json");
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg_path,
        r#"{"globalIgnore": [".env"], "mcpServers": {"context7": {"command": "uvx"}}}"#,
    )
    .unwrap();

    let out = run_with_home(home.path(), &["mcp-install", "--target", "cursor"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(parsed["globalIgnore"][0], ".env");
    assert_eq!(parsed["mcpServers"]["context7"]["command"], "uvx");
    assert_eq!(parsed["mcpServers"]["wg"]["command"], "wg");
}

#[test]
fn mcp_install_print_mode_does_not_touch_filesystem() {
    let home = tempfile::tempdir().unwrap();
    let out = run_with_home(
        home.path(),
        &["mcp-install", "--target", "cursor", "--print"],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // No file should have been written.
    assert!(
        !home.path().join(".cursor/mcp.json").exists(),
        "--print mode must not touch the filesystem"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Would run"), "stdout: {}", stdout);
}

#[test]
fn mcp_install_list_targets_lists_all_supported() {
    let out = run(&["mcp-install", "--list-targets"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for target in ["claude", "hermes", "openclaw", "codex", "cursor"] {
        assert!(
            stdout.contains(target),
            "missing target `{}` in:\n{}",
            target,
            stdout
        );
    }
}

#[test]
fn mcp_install_file_edit_reports_verified() {
    // File-edit targets verify themselves trivially — they re-parse
    // the file they just wrote — so the human output should reassure
    // operators with the verified ✓ line.
    let home = tempfile::tempdir().unwrap();
    let out = run_with_home(home.path(), &["mcp-install", "--target", "codex"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("verified:"),
        "expected verification line in stdout, got: {}",
        stdout
    );
}

#[test]
fn mcp_install_print_mode_does_not_show_verified() {
    // `--print` is dry-run: nothing was actually written, so we must
    // not lie about having verified anything.
    let home = tempfile::tempdir().unwrap();
    let out = run_with_home(
        home.path(),
        &["mcp-install", "--target", "codex", "--print"],
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("verified:"),
        "print mode should not claim verification: {}",
        stdout
    );
}

// ---------------------------------------------------------------------------
// `wg doctor` — agent integration matrix
// ---------------------------------------------------------------------------

/// Run `wg doctor` against an isolated `$HOME` and `$PATH` so that
/// shell-out checks (claude / hermes / openclaw) never reach the
/// developer's real agents — keeps the test deterministic across
/// environments. Returns the parsed JSON payload.
fn doctor_json(home: &Path, store_path: &Path) -> serde_json::Value {
    let out = Command::new(wg_bin())
        .env_remove("WG_STORE")
        .env("HOME", home)
        .env("PATH", "/nonexistent")
        .args(["--json", "--store", store_path.to_str().unwrap(), "doctor"])
        .output()
        .expect("failed to execute wg binary");
    assert!(
        out.status.success(),
        "doctor exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("doctor --json should emit valid JSON")
}

#[test]
fn doctor_reports_agent_matrix_with_no_installs() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.redb");
    let payload = doctor_json(home.path(), &store);

    let agents = payload["agents"].as_array().unwrap();
    let names: Vec<&str> = agents
        .iter()
        .map(|a| a["target"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["claude", "hermes", "openclaw", "codex", "cursor"]
    );

    // With $PATH stripped, every shell-out target reports `null`
    // (couldn't run `<bin> mcp list`).
    for shell_out in ["claude", "hermes", "openclaw"] {
        let entry = agents
            .iter()
            .find(|a| a["target"] == shell_out)
            .unwrap_or_else(|| panic!("missing {}", shell_out));
        assert_eq!(
            entry["mcp_registered"],
            serde_json::Value::Null,
            "{}: expected null mcp_registered when PATH is stripped",
            shell_out
        );
    }

    // File-edit targets without any config should be Some(false).
    for file_edit in ["codex", "cursor"] {
        let entry = agents
            .iter()
            .find(|a| a["target"] == file_edit)
            .unwrap_or_else(|| panic!("missing {}", file_edit));
        assert_eq!(
            entry["mcp_registered"],
            serde_json::Value::Bool(false),
            "{}: expected false mcp_registered with empty home",
            file_edit
        );
    }
}

#[test]
fn doctor_detects_codex_and_cursor_after_mcp_install() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.redb");

    // Pre-install both file-edit targets.
    let codex = run_with_home(home.path(), &["mcp-install", "--target", "codex"]);
    assert!(codex.status.success());
    let cursor = run_with_home(home.path(), &["mcp-install", "--target", "cursor"]);
    assert!(cursor.status.success());

    let payload = doctor_json(home.path(), &store);
    let agents = payload["agents"].as_array().unwrap();
    let codex_entry = agents.iter().find(|a| a["target"] == "codex").unwrap();
    assert_eq!(codex_entry["mcp_registered"], true);
    let cursor_entry = agents.iter().find(|a| a["target"] == "cursor").unwrap();
    assert_eq!(cursor_entry["mcp_registered"], true);
}

#[test]
fn doctor_detects_skill_installation() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.redb");
    let dest = home.path().join(".hermes/skills/wg");

    // Use --dest so the test isn't sensitive to where dirs::home_dir
    // resolves on the CI runner.
    let installed = Command::new(wg_bin())
        .env("HOME", home.path())
        .args([
            "skill",
            "install",
            "--target",
            "hermes",
            "--dest",
            dest.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run wg skill install");
    assert!(installed.status.success());

    let payload = doctor_json(home.path(), &store);
    let agents = payload["agents"].as_array().unwrap();
    let hermes = agents.iter().find(|a| a["target"] == "hermes").unwrap();
    assert_eq!(
        hermes["skill_installed"],
        true,
        "hermes skill should be reported installed at {}",
        dest.display()
    );
    let claude = agents.iter().find(|a| a["target"] == "claude").unwrap();
    assert_eq!(
        claude["skill_installed"], false,
        "claude should still report skill not installed"
    );
}

#[test]
fn doctor_fix_lists_install_commands_for_gaps() {
    // Empty home + nuked PATH means: no skills installed anywhere,
    // shell-out targets unverifiable (mcp_registered = None), and
    // file-edit targets confirmed missing (Some(false)). Only the
    // confirmed-missing ones should produce suggestions.
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.redb");

    let out = Command::new(wg_bin())
        .env_remove("WG_STORE")
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .args(["--store", store.to_str().unwrap(), "doctor", "--fix"])
        .output()
        .expect("doctor --fix");
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Suggested fixes"),
        "missing fixes section: {}",
        stdout
    );
    // Skill fixes are always confirmable (file-existence check).
    assert!(stdout.contains("wg skill install --target claude"));
    assert!(stdout.contains("wg skill install --target hermes"));
    assert!(stdout.contains("wg skill install --target openclaw"));
    // codex / cursor have no skill format — no suggestion for them.
    assert!(!stdout.contains("wg skill install --target codex"));
    assert!(!stdout.contains("wg skill install --target cursor"));
    // MCP fixes only for confirmed-missing (file-edit), not for
    // unverifiable shell-out targets.
    assert!(stdout.contains("wg mcp-install --target codex"));
    assert!(stdout.contains("wg mcp-install --target cursor"));
    assert!(!stdout.contains("wg mcp-install --target openclaw"));
}

#[test]
fn doctor_without_fix_emits_tip_when_gaps_present() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.redb");
    let out = Command::new(wg_bin())
        .env_remove("WG_STORE")
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .args(["--store", store.to_str().unwrap(), "doctor"])
        .output()
        .expect("doctor");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Run `wg doctor --fix`"),
        "missing tip: {}",
        stdout
    );
    // ...but no fixes block when --fix isn't requested.
    assert!(!stdout.contains("Suggested fixes"));
}

#[test]
fn doctor_json_includes_fixes_array() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.redb");
    let payload = doctor_json(home.path(), &store);
    let fixes = payload["fixes"].as_array().expect("fixes must be an array");
    let kinds: Vec<&str> = fixes.iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert!(
        kinds.contains(&"skill"),
        "fixes should include skill suggestions"
    );
    assert!(
        kinds.contains(&"mcp"),
        "fixes should include mcp suggestions"
    );
    let commands: Vec<&str> = fixes
        .iter()
        .map(|f| f["command"].as_str().unwrap())
        .collect();
    assert!(commands.iter().any(|c| c.contains("wg skill install")));
    assert!(commands.iter().any(|c| c.contains("wg mcp-install")));
}

#[test]
fn doctor_fix_shell_emits_only_commands_one_per_line() {
    // `wg doctor --fix --shell | sh` is the documented quick-fix
    // pipeline, so the contract is: every non-empty line is a
    // valid command, no decoration, no headers, no comments. We
    // exercise the same isolated home as the gap-suggestion test
    // so the expected command set is stable.
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.redb");

    let out = Command::new(wg_bin())
        .env_remove("WG_STORE")
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .args([
            "--store",
            store.to_str().unwrap(),
            "doctor",
            "--fix",
            "--shell",
        ])
        .output()
        .expect("doctor --fix --shell");
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(!lines.is_empty(), "expected at least one fix line");
    for line in &lines {
        // Every line should look like a wg subcommand invocation —
        // no shell-prompt prefixes, no comments, no blanks.
        assert!(
            line.starts_with("wg skill install ") || line.starts_with("wg mcp-install "),
            "unexpected line in --shell output: {:?}",
            line
        );
        assert!(
            !line.contains('#'),
            "comment leaked into --shell output: {:?}",
            line
        );
    }

    // Sanity: same set of commands as `--fix` (without --shell)
    // would suggest, just stripped of decoration.
    let plain = Command::new(wg_bin())
        .env_remove("WG_STORE")
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .args(["--store", store.to_str().unwrap(), "doctor", "--fix"])
        .output()
        .expect("doctor --fix");
    let plain_stdout = String::from_utf8_lossy(&plain.stdout);
    for line in &lines {
        assert!(
            plain_stdout.contains(line),
            "command `{}` from --shell missing in --fix human output",
            line
        );
    }
}

#[test]
fn doctor_human_output_includes_agent_section() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.redb");
    let out = Command::new(wg_bin())
        .env_remove("WG_STORE")
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .args(["--store", store.to_str().unwrap(), "doctor"])
        .output()
        .expect("doctor");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Agent integration:"));
    for target in ["claude", "hermes", "openclaw", "codex", "cursor"] {
        assert!(
            stdout.contains(target),
            "missing {} row in doctor output: {}",
            target,
            stdout
        );
    }
    assert!(
        stdout.contains("legend:"),
        "missing legend line: {}",
        stdout
    );
}
