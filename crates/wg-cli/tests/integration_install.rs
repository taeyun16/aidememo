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
