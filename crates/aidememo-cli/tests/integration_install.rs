//! Integration tests for `aidememo skill install` and `aidememo mcp-install`.
//!
//! These exercise the full binary path (build via Cargo, exec via
//! `CARGO_BIN_EXE_aidememo`) so we catch wiring breakage that the unit
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

fn aidememo_bin() -> &'static str {
    env!("CARGO_BIN_EXE_aidememo")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(aidememo_bin())
        .args(args)
        .output()
        .expect("failed to execute aidememo binary")
}

fn run_with_home(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(aidememo_bin())
        .env_remove("AIDEMEMO_STORE")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("HERMES_HOME")
        .env_remove("PI_CODING_AGENT_DIR")
        .env("HOME", home)
        .args(args)
        .output()
        .expect("failed to execute aidememo binary")
}

#[test]
fn session_resume_emits_validated_shell_exports_and_json_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("resume.sqlite");
    let store = store.to_str().unwrap();

    let created = run(&[
        "--store",
        store,
        "--json",
        "session",
        "new",
        "--source-id",
        "release-team",
        "Cross-agent release review",
    ]);
    assert!(
        created.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created_json: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let session_id = created_json["session_id"].as_str().unwrap();

    let resumed = run(&[
        "--store",
        store,
        "session",
        "resume",
        "--source-id",
        "release-team",
        session_id,
    ]);
    assert!(
        resumed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let shell = String::from_utf8_lossy(&resumed.stdout);
    assert!(shell.contains(&format!("export AIDEMEMO_SESSION_ID='{session_id}'")));
    assert!(shell.contains("export AIDEMEMO_SOURCE_ID='release-team'"));

    let resumed_json = run(&[
        "--store",
        store,
        "--json",
        "session",
        "resume",
        "--source-id",
        "release-team",
        session_id,
    ]);
    assert!(resumed_json.status.success());
    let payload: serde_json::Value = serde_json::from_slice(&resumed_json.stdout).unwrap();
    assert_eq!(payload["session_id"].as_str(), Some(session_id));
    assert_eq!(
        payload["env"]["AIDEMEMO_SOURCE_ID"].as_str(),
        Some("release-team")
    );

    let handoff = Command::new(aidememo_bin())
        .env("AIDEMEMO_SOURCE_ID", "release-team")
        .args([
            "--store",
            store,
            "--json",
            "session",
            "handoff",
            "--from",
            "codex/coding",
            "--to",
            "hermes/reviewer",
            "--done-when",
            "Focused tests pass",
            session_id,
        ])
        .output()
        .unwrap();
    assert!(
        handoff.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&handoff.stderr)
    );
    let packet: serde_json::Value = serde_json::from_slice(&handoff.stdout).unwrap();
    assert_eq!(packet["source_id"].as_str(), Some("release-team"));
    assert_eq!(packet["from_profile"].as_str(), Some("coding"));
    assert_eq!(packet["to_profile"].as_str(), Some("reviewer"));
    assert_eq!(packet["done_when"].as_str(), Some("Focused tests pass"));

    let packet_path = dir.path().join("handoff.md");
    let packet_path_str = packet_path.to_str().unwrap();
    let written = run(&[
        "--store",
        store,
        "session",
        "handoff",
        "--from",
        "codex/coding",
        "--to",
        "hermes/reviewer",
        "--done-when",
        "Focused tests pass",
        "--output",
        packet_path_str,
        session_id,
    ]);
    assert!(written.status.success());
    assert!(String::from_utf8_lossy(&written.stdout).contains("Resume: eval"));
    assert!(
        std::fs::read_to_string(packet_path)
            .unwrap()
            .contains("## Definition of Done")
    );
}

#[test]
fn account_handoff_cli_dispatch_accept_return_outbox_is_json_stable() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("account-handoff.sqlite");
    let store = store.to_str().unwrap();
    let connected = Command::new(aidememo_bin())
        .env("HOME", dir.path())
        .args([
            "agent",
            "add",
            "codex-two",
            "--type",
            "codex",
            "--home",
            dir.path().to_str().unwrap(),
            "--workspace",
            dir.path().to_str().unwrap(),
            "--source-id",
            "team-a",
        ])
        .output()
        .unwrap();
    assert!(
        connected.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&connected.stderr)
    );
    let created = run(&[
        "--store",
        store,
        "--json",
        "session",
        "new",
        "--source-id",
        "team-a",
        "Two Codex accounts review one patch",
    ]);
    let created_json: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let session_id = created_json["session_id"].as_str().unwrap();

    let dispatched = Command::new(aidememo_bin())
        .env("HOME", dir.path())
        .env("AIDEMEMO_ACTOR_ID", "codex-one")
        .env("HERMES_KANBAN_TASK", "task-42")
        .env("HERMES_KANBAN_BOARD", "board-a")
        .args([
            "--store",
            store,
            "--json",
            "handoff",
            "send",
            "codex-two",
            "--focus",
            "Review the patch",
            "--done-when",
            "Focused tests pass",
            session_id,
        ])
        .output()
        .unwrap();
    assert!(
        dispatched.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&dispatched.stderr)
    );
    // Regression: human-readable Assignment text must never trail JSON output.
    let dispatched_json: serde_json::Value = serde_json::from_slice(&dispatched.stdout).unwrap();
    let handoff_id = dispatched_json["handoff_id"].as_str().unwrap();
    assert_eq!(dispatched_json["from_actor"].as_str(), Some("codex-one"));
    assert_eq!(dispatched_json["to_actor"].as_str(), Some("codex-two"));
    assert_eq!(dispatched_json["to_agent"].as_str(), Some("codex"));
    assert_eq!(dispatched_json["source_id"].as_str(), Some("team-a"));
    assert_eq!(
        dispatched_json["upstream_system"].as_str(),
        Some("hermes_kanban")
    );
    assert_eq!(
        dispatched_json["upstream_task_id"].as_str(),
        Some("task-42")
    );

    let inbox = Command::new(aidememo_bin())
        .env("AIDEMEMO_ACTOR_ID", "codex-two")
        .env("AIDEMEMO_SOURCE_ID", "team-a")
        .args(["--store", store, "--json", "handoff", "inbox"])
        .output()
        .unwrap();
    let inbox_json: serde_json::Value = serde_json::from_slice(&inbox.stdout).unwrap();
    assert_eq!(inbox_json["assignments"][0]["handoff_id"], handoff_id);

    let accepted = Command::new(aidememo_bin())
        .env("AIDEMEMO_ACTOR_ID", "codex-two")
        .args(["--store", store, "--json", "handoff", "accept", handoff_id])
        .output()
        .unwrap();
    let accepted_json: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted_json["assignment"]["status"], "accepted");
    assert_eq!(
        accepted_json["resume"]["env"]["AIDEMEMO_ACTOR_ID"],
        "codex-two"
    );
    assert_eq!(
        accepted_json["resume"]["env"]["AIDEMEMO_SESSION_ID"],
        session_id
    );

    let heartbeat = Command::new(aidememo_bin())
        .env("AIDEMEMO_ACTOR_ID", "codex-two")
        .args([
            "--store",
            store,
            "--json",
            "handoff",
            "heartbeat",
            handoff_id,
        ])
        .output()
        .unwrap();
    let heartbeat_json: serde_json::Value = serde_json::from_slice(&heartbeat.stdout).unwrap();
    assert_eq!(heartbeat_json["heartbeat_count"], 1);

    let board = Command::new(aidememo_bin())
        .args([
            "--store",
            store,
            "--json",
            "handoff",
            "board",
            "--actor-id",
            "codex-two",
            "--stale-after",
            "1h",
        ])
        .output()
        .unwrap();
    let board_json: serde_json::Value = serde_json::from_slice(&board.stdout).unwrap();
    assert_eq!(board_json["lanes"]["in_progress"], 1);
    assert_eq!(
        board_json["assignments"][0]["lifecycle_owner"],
        "hermes_kanban"
    );

    let result_fact = Command::new(aidememo_bin())
        .env("AIDEMEMO_SESSION_ID", session_id)
        .args([
            "--store",
            store,
            "--json",
            "fact",
            "add",
            "Focused tests pass",
            "--entities",
            "Release",
            "--type",
            "note",
        ])
        .output()
        .unwrap();
    assert!(
        result_fact.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result_fact.stderr)
    );
    let result_json: serde_json::Value = serde_json::from_slice(&result_fact.stdout).unwrap();
    let result_fact_id = result_json["id"].as_str().unwrap();

    let completed = Command::new(aidememo_bin())
        .env("AIDEMEMO_ACTOR_ID", "codex-two")
        .args([
            "--store",
            store,
            "--json",
            "handoff",
            "return",
            "--outcome",
            "succeeded",
            "--result-fact-id",
            result_fact_id,
            handoff_id,
        ])
        .output()
        .unwrap();
    let completed_json: serde_json::Value = serde_json::from_slice(&completed.stdout).unwrap();
    assert_eq!(completed_json["status"], "completed");
    assert_eq!(completed_json["result_fact_id"], result_fact_id);

    let outbox = Command::new(aidememo_bin())
        .env("AIDEMEMO_ACTOR_ID", "codex-one")
        .args(["--store", store, "--json", "handoff", "outbox"])
        .output()
        .unwrap();
    let outbox_json: serde_json::Value = serde_json::from_slice(&outbox.stdout).unwrap();
    assert_eq!(
        outbox_json["assignments"][0]["result_fact_id"],
        result_fact_id
    );

    let shown = Command::new(aidememo_bin())
        .args(["--store", store, "--json", "handoff", "show", handoff_id])
        .output()
        .unwrap();
    let shown_json: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown_json["outcome"], "succeeded");

    let status = Command::new(aidememo_bin())
        .env("AIDEMEMO_ACTOR_ID", "codex-one")
        .args(["--store", store, "--json", "handoff", "status", handoff_id])
        .output()
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["outcome"], "succeeded");
}

#[test]
fn installation_profile_crud_stores_runtime_metadata_without_credentials() {
    let home = tempfile::tempdir().unwrap();
    let codex_home = home.path().join("codex-two");
    let workspace = home.path().join("workspace");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let added = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .args([
            "--json",
            "installation",
            "add",
            "codex-two",
            "--agent",
            "codex",
            "--config-home",
            codex_home.to_str().unwrap(),
            "--workspace",
            workspace.to_str().unwrap(),
            "--source-id",
            "team-a",
            "--pass-env",
            "RELEASE_CHANNEL",
        ])
        .output()
        .unwrap();
    assert!(
        added.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&added.stderr)
    );
    let added_json: serde_json::Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(added_json["alias"], "codex-two");
    assert_eq!(added_json["installation"]["env_policy"], "core");
    assert_eq!(added_json["installation"]["pass_env"][0], "RELEASE_CHANNEL");

    let config = std::fs::read_to_string(home.path().join(".aidememo/config.toml")).unwrap();
    assert!(config.contains("[installations.codex-two]"));
    let config_value: toml::Value = toml::from_str(&config).unwrap();
    let profile = config_value["installations"]["codex-two"]
        .as_table()
        .unwrap();
    assert!(!profile.contains_key("token"));
    assert!(!profile.contains_key("secret"));
    assert!(!profile.contains_key("password"));

    let shown = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .args(["--json", "installation", "show", "codex-two"])
        .output()
        .unwrap();
    let shown_json: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(
        shown_json["installation"]["config_home"],
        codex_home.to_str().unwrap()
    );

    let removed = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .args(["--json", "installation", "remove", "codex-two"])
        .output()
        .unwrap();
    assert!(removed.status.success());
}

#[test]
fn mcp_install_codex_honors_codex_home() {
    let home = tempfile::tempdir().unwrap();
    let codex_home = home.path().join("isolated-codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    let out = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .env("CODEX_HOME", &codex_home)
        .args([
            "mcp-install",
            "--target",
            "codex",
            "--source-id",
            "team-a",
            "--actor-id",
            "codex-two",
            "--no-verify",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let config = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
    assert!(config.contains("AIDEMEMO_ACTOR_ID = \"codex-two\""));
    assert!(!home.path().join(".codex/config.toml").exists());
}

// ---------------------------------------------------------------------------
// `aidememo skill install`
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
    for target in ["claude", "hermes", "openclaw", "agents", "pi"] {
        assert!(
            stdout.contains(target),
            "expected target `{}` in --list-targets output:\n{}",
            target,
            stdout
        );
    }
}

#[test]
fn skill_install_hermes_honors_hermes_home() {
    let home = tempfile::tempdir().unwrap();
    let hermes_home = home.path().join("isolated-hermes");
    let out = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .env("HERMES_HOME", &hermes_home)
        .args(["skill", "install", "--target", "hermes"])
        .output()
        .expect("failed to install Hermes skill");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(hermes_home.join("skills/aidememo/SKILL.md").is_file());
    assert!(
        !home
            .path()
            .join(".hermes/skills/aidememo/SKILL.md")
            .exists()
    );
}

#[test]
fn skill_install_claude_honors_claude_config_dir() {
    let home = tempfile::tempdir().unwrap();
    let claude_config_dir = home.path().join("isolated-claude");
    let out = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", &claude_config_dir)
        .args(["skill", "install", "--target", "claude"])
        .output()
        .expect("failed to install Claude skill");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(claude_config_dir.join("skills/aidememo/SKILL.md").is_file());
    assert!(
        !home
            .path()
            .join(".claude/skills/aidememo/SKILL.md")
            .exists()
    );
}

#[test]
fn skill_install_pi_uses_native_agent_skills_directory() {
    let home = tempfile::tempdir().unwrap();
    let pi_agent_dir = home.path().join("isolated-pi-agent");
    let out = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .env("PI_CODING_AGENT_DIR", &pi_agent_dir)
        .args(["skill", "install", "--target", "pi"])
        .output()
        .expect("failed to install pi skill");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(pi_agent_dir.join("skills/aidememo/SKILL.md").is_file());
    assert!(pi_agent_dir.join("skills/aidememo/REFERENCE.md").is_file());
    assert!(!home.path().join(".config/pi/AGENTS.md").exists());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Ready for pi"));
    assert!(!stdout.contains("mcp-install --target pi"));
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
        skill.contains("name: aidememo"),
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
        skill.contains("name: aidememo"),
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
// `aidememo mcp-install` — file-edit targets (codex, cursor)
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
    let aidememo = &parsed["mcp_servers"]["aidememo"];
    assert_eq!(aidememo["command"].as_str(), Some("aidememo"));
    let args = aidememo["args"].as_array().unwrap();
    assert_eq!(args.len(), 5);
    assert_eq!(args[0].as_str(), Some("--backend"));
    assert_eq!(args[1].as_str(), Some("sqlite"));
    assert_eq!(args[2].as_str(), Some("--store"));
    assert!(
        args[3]
            .as_str()
            .is_some_and(|path| path.ends_with("/_meta/wiki.sqlite"))
    );
    assert_eq!(args[4].as_str(), Some("mcp"));
}

#[test]
fn mcp_install_codex_multi_profile_pins_shared_store_and_distinct_actors() {
    let home = tempfile::tempdir().unwrap();
    let profile_a = home.path().join("codex-a");
    let profile_b = home.path().join("codex-b");
    let store = home.path().join("shared.sqlite");
    let out = run_with_home(
        home.path(),
        &[
            "--store",
            store.to_str().unwrap(),
            "mcp-install",
            "--target",
            "codex",
            "--codex-home",
            profile_a.to_str().unwrap(),
            "--actor-id",
            "codex:account-a",
            "--codex-home",
            profile_b.to_str().unwrap(),
            "--actor-id",
            "codex:account-b",
            "--source-id",
            "project:aidememo",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    for (profile, actor_id) in [
        (profile_a, "codex:account-a"),
        (profile_b, "codex:account-b"),
    ] {
        let parsed: toml::Value = std::fs::read_to_string(profile.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let entry = &parsed["mcp_servers"]["aidememo"];
        let args = entry["args"].as_array().unwrap();
        assert_eq!(args[3].as_str(), store.to_str());
        assert_eq!(
            entry["env"]["AIDEMEMO_SOURCE_ID"].as_str(),
            Some("project:aidememo")
        );
        assert_eq!(entry["env"]["AIDEMEMO_ACTOR_ID"].as_str(), Some(actor_id));
    }
}

#[test]
fn mcp_install_codex_preserves_existing_entries() {
    // The user already has another MCP server configured. Our merge
    // must add aidememo without disturbing it.
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
    assert_eq!(
        parsed["mcp_servers"]["aidememo"]["command"].as_str(),
        Some("aidememo")
    );
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
fn init_agent_codex_initializes_store_and_mcp_config() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let wiki = root.path().join("wiki");
    std::fs::create_dir_all(&wiki).unwrap();
    std::fs::write(wiki.join("redis.md"), "# Redis\n\nRedis note.\n").unwrap();
    let store = root.path().join("wiki.sqlite");

    let out = run_with_home(
        home.path(),
        &[
            "--store",
            store.to_str().unwrap(),
            "--json",
            "init",
            "--agent",
            "codex",
            "--no-ingest",
            wiki.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["agent"], "codex");
    assert_eq!(report["no_ingest"], true);
    let steps = report["steps"].as_array().unwrap();
    assert!(
        steps
            .iter()
            .any(|s| { s["name"] == "agent_skill_install" && s["status"] == "skipped" })
    );
    assert!(
        steps
            .iter()
            .any(|s| { s["name"] == "agent_mcp_install" && s["status"] == "ok" })
    );

    let cfg_path = home.path().join(".codex/config.toml");
    let parsed: toml::Value = std::fs::read_to_string(&cfg_path).unwrap().parse().unwrap();
    assert_eq!(
        parsed["mcp_servers"]["aidememo"]["command"].as_str(),
        Some("aidememo")
    );
    assert!(store.exists(), "init should create the store");
}

#[test]
fn overview_honors_global_json_flag() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("wiki.sqlite");

    let add = run_with_home(
        home.path(),
        &[
            "--store",
            store.to_str().unwrap(),
            "fact",
            "add",
            "Overview JSON smoke fact",
            "--entities",
            "Overview",
        ],
    );
    assert!(
        add.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let out = run_with_home(
        home.path(),
        &[
            "--json",
            "--store",
            store.to_str().unwrap(),
            "overview",
            "--recent-days",
            "365",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("overview --json should emit valid JSON");
    assert_eq!(payload["stats"]["fact_count"], 1);
    assert_eq!(payload["current_fact_count"], 1);
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
    assert_eq!(parsed["mcpServers"]["aidememo"]["command"], "aidememo");
    let args = parsed["mcpServers"]["aidememo"]["args"].as_array().unwrap();
    assert_eq!(args.len(), 5);
    assert_eq!(args[0], "--backend");
    assert_eq!(args[1], "sqlite");
    assert_eq!(args[2], "--store");
    assert!(
        args[3]
            .as_str()
            .is_some_and(|path| path.ends_with("/_meta/wiki.sqlite"))
    );
    assert_eq!(args[4], "mcp");
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
    assert_eq!(parsed["mcpServers"]["aidememo"]["command"], "aidememo");
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
// `aidememo doctor` — agent integration matrix
// ---------------------------------------------------------------------------

/// Run `aidememo doctor` against an isolated `$HOME` and `$PATH` so that
/// shell-out checks (claude / hermes / openclaw) never reach the
/// developer's real agents — keeps the test deterministic across
/// environments. Returns the parsed JSON payload.
fn doctor_json(home: &Path, store_path: &Path) -> serde_json::Value {
    let out = Command::new(aidememo_bin())
        .env_remove("AIDEMEMO_STORE")
        .env_remove("CODEX_HOME")
        .env("HOME", home)
        .env("PATH", "/nonexistent")
        .args(["--json", "--store", store_path.to_str().unwrap(), "doctor"])
        .output()
        .expect("failed to execute aidememo binary");
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
    let store = home.path().join("wiki.sqlite");
    let payload = doctor_json(home.path(), &store);

    let agents = payload["agents"].as_array().unwrap();
    let names: Vec<&str> = agents
        .iter()
        .map(|a| a["target"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "claude", "hermes", "openclaw", "codex", "cursor", "opencode", "pi"
        ]
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
    let store = home.path().join("wiki.sqlite");

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
    let store = home.path().join("wiki.sqlite");
    let dest = home.path().join(".hermes/skills/aidememo");

    // Use --dest so the test isn't sensitive to where dirs::home_dir
    // resolves on the CI runner.
    let installed = Command::new(aidememo_bin())
        .env_remove("CODEX_HOME")
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
        .expect("failed to run aidememo skill install");
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
    let store = home.path().join("wiki.sqlite");

    let out = Command::new(aidememo_bin())
        .env_remove("AIDEMEMO_STORE")
        .env_remove("CODEX_HOME")
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
    assert!(stdout.contains("aidememo skill install --target claude"));
    assert!(stdout.contains("aidememo skill install --target hermes"));
    assert!(stdout.contains("aidememo skill install --target openclaw"));
    // codex / cursor have no skill format — no suggestion for them.
    assert!(!stdout.contains("aidememo skill install --target codex"));
    assert!(!stdout.contains("aidememo skill install --target cursor"));
    // MCP fixes only for confirmed-missing (file-edit), not for
    // unverifiable shell-out targets.
    assert!(stdout.contains("aidememo --backend sqlite mcp-install --target codex"));
    assert!(stdout.contains("aidememo --backend sqlite mcp-install --target cursor"));
    assert!(!stdout.contains("aidememo --backend sqlite mcp-install --target openclaw"));
}

#[test]
fn doctor_without_fix_emits_tip_when_gaps_present() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.sqlite");
    let out = Command::new(aidememo_bin())
        .env_remove("AIDEMEMO_STORE")
        .env_remove("CODEX_HOME")
        .env("HOME", home.path())
        .env("PATH", "/nonexistent")
        .args(["--store", store.to_str().unwrap(), "doctor"])
        .output()
        .expect("doctor");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Run `aidememo doctor --fix`"),
        "missing tip: {}",
        stdout
    );
    // ...but no fixes block when --fix isn't requested.
    assert!(!stdout.contains("Suggested fixes"));
}

#[test]
fn doctor_json_includes_fixes_array() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.sqlite");
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
    assert!(
        kinds.contains(&"sharing"),
        "fixes should include sharing suggestions"
    );
    let commands: Vec<&str> = fixes
        .iter()
        .map(|f| f["command"].as_str().unwrap())
        .collect();
    assert!(
        commands
            .iter()
            .any(|c| c.contains("aidememo skill install"))
    );
    assert!(
        commands
            .iter()
            .any(|c| c.contains("aidememo --backend sqlite mcp-install"))
    );
    assert!(
        commands
            .iter()
            .any(|c| c.contains("aidememo config set store.lock_retry_ms 5000"))
    );
}

#[test]
fn doctor_json_includes_workflow_readiness_hints() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.sqlite");
    let payload = doctor_json(home.path(), &store);
    let workflow = &payload["workflow"];

    assert_eq!(workflow["ready"], false);
    assert_eq!(workflow["recent_ticket_count"], 0);
    let hints = workflow["hints"]
        .as_array()
        .expect("hints must be an array");
    for code in ["workflow_no_mcp_agent", "workflow_no_recent_tickets"] {
        let hint = hints.iter().find(|h| h["code"] == code);
        assert!(hint.is_some(), "missing workflow hint {code}: {hints:?}");
        assert!(
            hint.unwrap()["action"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "workflow hint {code} needs an actionable command"
        );
    }
}

#[test]
fn doctor_json_includes_shared_store_guidance() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.sqlite");
    let payload = doctor_json(home.path(), &store);
    let sharing = &payload["sharing"];

    assert_eq!(sharing["lock_retry_ms"], 0);
    assert_eq!(sharing["serverless_recommended_writers"], 4);
    assert_eq!(sharing["high_concurrency_writers"], 8);
    assert_eq!(sharing["daemon"]["state"], "none");
    assert_eq!(sharing["recommended_mode"], "serverless_fail_fast");
    let hints = sharing["hints"]
        .as_array()
        .expect("sharing hints must be an array");
    for code in [
        "sharing_retry_disabled",
        "sharing_daemon_for_high_concurrency",
    ] {
        let hint = hints.iter().find(|h| h["code"] == code);
        assert!(hint.is_some(), "missing sharing hint {code}: {hints:?}");
        assert!(
            hint.unwrap()["action"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "sharing hint {code} needs an actionable command"
        );
    }
}

#[test]
fn doctor_fix_shell_emits_only_commands_one_per_line() {
    // `aidememo doctor --fix --shell | sh` is the documented quick-fix
    // pipeline, so the contract is: every non-empty line is a
    // valid command, no decoration, no headers, no comments. We
    // exercise the same isolated home as the gap-suggestion test
    // so the expected command set is stable.
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("wiki.sqlite");

    let out = Command::new(aidememo_bin())
        .env_remove("AIDEMEMO_STORE")
        .env_remove("CODEX_HOME")
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
        // Every line should look like a `aidememo <subcommand>` invocation
        // — no shell-prompt prefixes, no comments, no blanks. We
        // allow any `aidememo ...` because doctor's fix list now also
        // emits config / vector-rebuild advisories alongside the
        // skill / mcp-install ones.
        assert!(
            line.starts_with("aidememo "),
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
    let plain = Command::new(aidememo_bin())
        .env_remove("AIDEMEMO_STORE")
        .env_remove("CODEX_HOME")
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
    let store = home.path().join("wiki.sqlite");
    let out = Command::new(aidememo_bin())
        .env_remove("AIDEMEMO_STORE")
        .env_remove("CODEX_HOME")
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
