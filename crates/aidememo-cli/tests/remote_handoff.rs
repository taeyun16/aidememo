use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, RecordStatus, Revision, TenantId, TenantRecord,
};
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_local::SqliteCommandStore;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

const P1_TOKEN: &str = "codex-p1-token-0123456789";
const P2_TOKEN: &str = "codex-p2-token-0123456789";

fn aidememo_bin() -> &'static str {
    env!("CARGO_BIN_EXE_aidememo")
}

fn run(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(aidememo_bin())
        .env("HOME", home)
        .env_remove("AIDEMEMO_REMOTE_PROFILE")
        .env_remove("AIDEMEMO_ACTOR_ID")
        .env_remove("AIDEMEMO_SESSION_ID")
        .env_remove("AIDEMEMO_SOURCE_ID")
        .args(args)
        .output()
        .expect("failed to execute aidememo binary")
}

fn provision(
    store: &mut SqliteCommandStore,
    tenant: &TenantRecord,
    project: &ProjectRecord,
    actor_id: &str,
    token: &str,
    timestamp: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let actor = ActorRecord {
        tenant_id: tenant.tenant_id.clone(),
        actor_id: ActorId::try_from(actor_id)?,
        display_name: actor_id.to_owned(),
        kind: ActorKind::Agent,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let membership = ProjectMembership {
        tenant_id: tenant.tenant_id.clone(),
        project_id: project.project_id.clone(),
        actor_id: actor.actor_id.clone(),
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    store.provision_actor(&actor, &membership, &bearer_token_digest(token)?, timestamp)?;
    Ok(())
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn mcp_tool(
    home: &Path,
    codex_home: &Path,
    name: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let config: toml::Value = std::fs::read_to_string(codex_home.join("config.toml"))?.parse()?;
    let entry = &config["mcp_servers"]["aidememo"];
    let args = entry["args"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("installed MCP args missing"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("installed MCP arg is not a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let env = entry["env"]
        .as_table()
        .ok_or_else(|| std::io::Error::other("installed MCP env missing"))?;
    let mut command = Command::new(aidememo_bin());
    command
        .env("HOME", home)
        .env_remove("AIDEMEMO_REMOTE_PROFILE")
        .env_remove("AIDEMEMO_ACTOR_ID")
        .env_remove("AIDEMEMO_SESSION_ID")
        .env_remove("AIDEMEMO_SOURCE_ID")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(
            key,
            value
                .as_str()
                .ok_or_else(|| std::io::Error::other("installed MCP env is not a string"))?,
        );
    }
    let mut child = command.spawn()?;
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    });
    writeln!(
        child
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("MCP stdin missing"))?,
        "{request}"
    )?;
    drop(child.stdin.take());
    let output = child.wait_with_output()?;
    assert_success(&output);
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if response["result"]["isError"].as_bool() == Some(true) {
        return Err(std::io::Error::other(format!(
            "MCP tool failed: {}",
            response["result"]["content"][0]["text"]
        ))
        .into());
    }
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("MCP tool text missing"))?;
    serde_json::from_str(text).map_err(Into::into)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_named_codex_profiles_complete_a_remote_handoff_round_trip()
-> Result<(), Box<dyn std::error::Error>> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_cli_remote")?,
        display_name: "CLI remote tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_cli_remote")?,
        display_name: "CLI remote project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_cli_remote")?,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let mut command_store = SqliteCommandStore::open_in_memory()?;
    command_store.bootstrap_project(&tenant, &project)?;
    provision(
        &mut command_store,
        &tenant,
        &project,
        "codex-p1",
        P1_TOKEN,
        timestamp,
    )?;
    provision(
        &mut command_store,
        &tenant,
        &project,
        "codex-p2",
        P2_TOKEN,
        timestamp,
    )?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        axum::serve(listener, router(ServerState::new(command_store))).await
    });
    let url = format!("http://{address}");

    let home = tempfile::tempdir()?;
    let p1_store = home.path().join("embedded-p1.sqlite");
    let p1_store = p1_store
        .to_str()
        .ok_or_else(|| std::io::Error::other("non-UTF-8 p1 test store path"))?;
    let p2_store = home.path().join("embedded-p2.sqlite");
    let p2_store = p2_store
        .to_str()
        .ok_or_else(|| std::io::Error::other("non-UTF-8 p2 test store path"))?;

    for (profile, token) in [("codex-p1", P1_TOKEN), ("codex-p2", P2_TOKEN)] {
        let login = run(
            home.path(),
            &[
                "auth",
                "login",
                "--profile",
                profile,
                "--project-id",
                "project_cli_remote",
                "--token",
                token,
                &url,
            ],
        );
        assert_success(&login);
    }

    let auth_list = run(home.path(), &["auth", "list"]);
    assert_success(&auth_list);
    let auth_list = String::from_utf8(auth_list.stdout)?;
    assert!(auth_list.contains("profile=codex-p1"));
    assert!(auth_list.contains("profile=codex-p2"));
    assert!(!auth_list.contains(P1_TOKEN));
    assert!(!auth_list.contains(P2_TOKEN));

    let created = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "--json",
            "session",
            "new",
            "--source-id",
            "project:aidememo",
            "Two remote Codex accounts",
        ],
    );
    assert_success(&created);
    let created: serde_json::Value = serde_json::from_slice(&created.stdout)?;
    let session_id = created["session_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("session id missing"))?;

    let sent = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p1",
            "send",
            "--source-id",
            "project:aidememo",
            "--focus",
            "Review the remote profile boundary",
            "codex-p2",
            session_id,
        ],
    );
    assert_success(&sent);
    let sent: serde_json::Value = serde_json::from_slice(&sent.stdout)?;
    assert_eq!(sent["actor_id"], "codex-p1");
    let handoff_id = sent["handoff_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("handoff id missing"))?;

    let inbox = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "inbox",
            "--source-id",
            "project:aidememo",
        ],
    );
    assert_success(&inbox);
    let inbox: serde_json::Value = serde_json::from_slice(&inbox.stdout)?;
    assert_eq!(inbox["assignments"][0]["record"]["handoff_id"], handoff_id);
    assert_eq!(inbox["assignments"][0]["record"]["to_actor"], "codex-p2");

    let actor_override = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "handoff",
            "--remote-profile",
            "codex-p2",
            "inbox",
            "--actor-id",
            "codex-p1",
        ],
    );
    assert!(!actor_override.status.success());
    assert!(String::from_utf8_lossy(&actor_override.stderr).contains("cannot override"));

    let wrong_receiver = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "handoff",
            "--remote-profile",
            "codex-p1",
            "accept",
            handoff_id,
        ],
    );
    assert!(!wrong_receiver.status.success());
    assert!(String::from_utf8_lossy(&wrong_receiver.stderr).contains("not addressed"));

    let accepted = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "accept",
            handoff_id,
        ],
    );
    assert_success(&accepted);
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout)?;
    assert_eq!(accepted["session_id"], session_id);
    assert_eq!(accepted["actor_id"], "codex-p2");
    assert!(accepted["context_id"].as_str().is_some());
    assert_eq!(accepted["recovered"], false);
    let accepted_retry = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "accept",
            handoff_id,
        ],
    );
    assert_success(&accepted_retry);
    let accepted_retry: serde_json::Value = serde_json::from_slice(&accepted_retry.stdout)?;
    assert_eq!(accepted_retry["recovered"], true);
    assert_eq!(accepted_retry["claim_id"], accepted["claim_id"]);
    assert_eq!(accepted_retry["command_id"], accepted["command_id"]);
    assert_eq!(
        accepted_retry["local_context_fact_id"],
        accepted["local_context_fact_id"]
    );

    let resumed = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "session",
            "resume",
            "--source-id",
            "project:aidememo",
            session_id,
        ],
    );
    assert_success(&resumed);
    let resumed: serde_json::Value = serde_json::from_slice(&resumed.stdout)?;
    assert_eq!(resumed["session_id"], session_id);

    let result = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .env("AIDEMEMO_SESSION_ID", session_id)
        .env("AIDEMEMO_SOURCE_ID", "project:aidememo")
        .env("AIDEMEMO_ACTOR_ID", "codex-p2")
        .args([
            "--store",
            p2_store,
            "--json",
            "fact",
            "add",
            "Remote review completed",
            "--entities",
            "RemoteReview",
            "--type",
            "note",
            "--source-id",
            "project:aidememo",
            "--actor-id",
            "codex-p2",
        ])
        .output()?;
    assert_success(&result);
    let result: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    let result_fact_id = result["id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("result fact id missing"))?;

    let failed_return = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "return",
            "--outcome",
            "failed",
            "--result-fact-id",
            result_fact_id,
            handoff_id,
        ],
    );
    assert_success(&failed_return);
    let failed_return: serde_json::Value = serde_json::from_slice(&failed_return.stdout)?;
    assert_eq!(failed_return["recovered"], false);

    let failed_return_retry = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "return",
            "--outcome",
            "failed",
            "--result-fact-id",
            result_fact_id,
            handoff_id,
        ],
    );
    assert_success(&failed_return_retry);
    let failed_return_retry: serde_json::Value =
        serde_json::from_slice(&failed_return_retry.stdout)?;
    assert_eq!(failed_return_retry["recovered"], true);
    assert_eq!(
        failed_return_retry["command_id"],
        failed_return["command_id"]
    );

    let retried_accept = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "accept",
            handoff_id,
        ],
    );
    assert_success(&retried_accept);
    let retried_accept: serde_json::Value = serde_json::from_slice(&retried_accept.stdout)?;
    assert_eq!(retried_accept["recovered"], false);
    assert_ne!(retried_accept["claim_id"], accepted["claim_id"]);
    assert_ne!(retried_accept["command_id"], accepted["command_id"]);

    let returned = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "return",
            "--outcome",
            "succeeded",
            "--result-fact-id",
            result_fact_id,
            handoff_id,
        ],
    );
    assert_success(&returned);
    let returned: serde_json::Value = serde_json::from_slice(&returned.stdout)?;
    assert_eq!(returned["recovered"], false);

    let returned_retry = run(
        home.path(),
        &[
            "--store",
            p2_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "return",
            "--outcome",
            "succeeded",
            "--result-fact-id",
            result_fact_id,
            handoff_id,
        ],
    );
    assert_success(&returned_retry);
    let returned_retry: serde_json::Value = serde_json::from_slice(&returned_retry.stdout)?;
    assert_eq!(returned_retry["recovered"], true);
    assert_eq!(returned_retry["command_id"], returned["command_id"]);

    let outbox = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p1",
            "outbox",
        ],
    );
    assert_success(&outbox);
    let outbox: serde_json::Value = serde_json::from_slice(&outbox.stdout)?;
    assert_eq!(outbox["assignments"][0]["record"]["status"], "completed");
    assert_eq!(
        outbox["assignments"][0]["record"]["result_fact_id"],
        result_fact_id
    );

    let p1_home = home.path().join("codex-p1-home");
    let p2_home = home.path().join("codex-p2-home");
    let overridden_install = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "mcp-install",
            "--target",
            "codex",
            "--codex-home",
            p1_home
                .to_str()
                .ok_or_else(|| std::io::Error::other("non-UTF-8 Codex home"))?,
            "--remote-profile",
            "codex-p1",
            "--actor-id",
            "codex-p2",
        ],
    );
    assert!(!overridden_install.status.success());
    assert!(
        String::from_utf8_lossy(&overridden_install.stderr)
            .contains("cannot be combined with --actor-id")
    );
    for (profile, codex_home, profile_store) in [
        ("codex-p1", &p1_home, p1_store),
        ("codex-p2", &p2_home, p2_store),
    ] {
        let installed = run(
            home.path(),
            &[
                "--store",
                profile_store,
                "mcp-install",
                "--target",
                "codex",
                "--codex-home",
                codex_home
                    .to_str()
                    .ok_or_else(|| std::io::Error::other("non-UTF-8 Codex home"))?,
                "--source-id",
                "project:aidememo",
                "--remote-profile",
                profile,
            ],
        );
        assert_success(&installed);
        let config: toml::Value =
            std::fs::read_to_string(codex_home.join("config.toml"))?.parse()?;
        let entry = &config["mcp_servers"]["aidememo"];
        assert_eq!(entry["env"]["AIDEMEMO_ACTOR_ID"].as_str(), Some(profile));
        assert!(
            entry["args"]
                .as_array()
                .is_some_and(|args| args.windows(2).any(|pair| {
                    pair[0].as_str() == Some("--remote-profile")
                        && pair[1].as_str() == Some(profile)
                }))
        );
    }

    let mcp_session = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "--json",
            "session",
            "new",
            "--source-id",
            "project:aidememo",
            "Remote MCP profile round trip",
        ],
    );
    assert_success(&mcp_session);
    let mcp_session: serde_json::Value = serde_json::from_slice(&mcp_session.stdout)?;
    let mcp_session_id = mcp_session["session_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("MCP session id missing"))?;

    let mcp_sent = mcp_tool(
        home.path(),
        &p1_home,
        "aidememo_handoff",
        serde_json::json!({
            "dispatch": true,
            "session_id": mcp_session_id,
            "source_id": "project:aidememo",
            "to_actor": "codex-p2",
            "focus": "Verify installed remote MCP routing",
        }),
    )?;
    assert_eq!(mcp_sent["actor_id"], "codex-p1");
    assert_eq!(mcp_sent["dispatched"], true);
    let mcp_handoff_id = mcp_sent["handoff_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("MCP handoff id missing"))?;

    let mcp_inbox = mcp_tool(
        home.path(),
        &p2_home,
        "aidememo_handoff_inbox",
        serde_json::json!({"action": "list", "source_id": "project:aidememo"}),
    )?;
    assert_eq!(
        mcp_inbox["assignments"][0]["record"]["handoff_id"],
        mcp_handoff_id
    );

    let mcp_accepted = mcp_tool(
        home.path(),
        &p2_home,
        "aidememo_handoff_inbox",
        serde_json::json!({"action": "accept", "handoff_id": mcp_handoff_id}),
    )?;
    assert_eq!(mcp_accepted["remote_profile"], "codex-p2");

    let mcp_fact = mcp_tool(
        home.path(),
        &p2_home,
        "aidememo_fact_add",
        serde_json::json!({
            "content": "Installed remote MCP round trip passed",
            "entities": ["RemoteMcpReview"],
            "fact_type": "note",
            "source_id": "project:aidememo",
            "session_id": mcp_session_id,
        }),
    )?;
    assert_eq!(mcp_fact["actor_id"], "codex-p2");
    let mcp_fact_id = mcp_fact["id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("MCP result fact id missing"))?;

    let mcp_returned = mcp_tool(
        home.path(),
        &p2_home,
        "aidememo_handoff_inbox",
        serde_json::json!({
            "action": "return",
            "handoff_id": mcp_handoff_id,
            "outcome": "succeeded",
            "result_fact_id": mcp_fact_id,
        }),
    )?;
    assert_eq!(mcp_returned["outcome"], "succeeded");

    let mcp_outbox = mcp_tool(
        home.path(),
        &p1_home,
        "aidememo_handoff_inbox",
        serde_json::json!({"action": "outbox"}),
    )?;
    assert_eq!(
        mcp_outbox["assignments"][0]["record"]["result_fact_id"],
        mcp_fact_id
    );

    let replica_pull = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "--json",
            "replica",
            "pull",
            "--remote-profile",
            "codex-p1",
            "--limit",
            "2",
        ],
    );
    assert_success(&replica_pull);
    let replica_pull: serde_json::Value = serde_json::from_slice(&replica_pull.stdout)?;
    assert_eq!(replica_pull["report"]["bootstrapped"], true);
    assert_eq!(replica_pull["report"]["changes"], 0);
    assert!(
        replica_pull["report"]["after_seq"]
            .as_u64()
            .is_some_and(|after_seq| after_seq > 0)
    );
    assert!(
        replica_pull["report"]["resource_count"]
            .as_u64()
            .is_some_and(|resources| resources > 0)
    );

    let replica_status = run(
        home.path(),
        &["--store", p1_store, "--json", "replica", "status"],
    );
    assert_success(&replica_status);
    let replica_status: serde_json::Value = serde_json::from_slice(&replica_status.stdout)?;
    assert_eq!(replica_status["status"]["initialized"], true);
    assert_eq!(
        replica_status["status"]["scope"]["project_id"],
        "project_cli_remote"
    );
    assert_eq!(replica_status["status"]["actor_id"], "codex-p1");

    let actor_reuse = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "replica",
            "pull",
            "--remote-profile",
            "codex-p2",
        ],
    );
    assert!(!actor_reuse.status.success());
    assert!(String::from_utf8_lossy(&actor_reuse.stderr).contains("replica actor mismatch"));

    server.abort();
    let _ = server.await;

    let offline_handoff = run(
        home.path(),
        &[
            "--store",
            p1_store,
            "--json",
            "replica",
            "get",
            "handoff",
            mcp_handoff_id,
        ],
    );
    assert_success(&offline_handoff);
    let offline_handoff: serde_json::Value = serde_json::from_slice(&offline_handoff.stdout)?;
    assert_eq!(offline_handoff["state"]["state"], "present");
    assert_eq!(offline_handoff["state"]["body"]["status"], "completed");

    let reset_without_force = run(home.path(), &["--store", p1_store, "replica", "reset"]);
    assert!(!reset_without_force.status.success());
    assert!(String::from_utf8_lossy(&reset_without_force.stderr).contains("pass --force"));

    let reset = run(
        home.path(),
        &["--store", p1_store, "--json", "replica", "reset", "--force"],
    );
    assert_success(&reset);
    let reset: serde_json::Value = serde_json::from_slice(&reset.stdout)?;
    assert_eq!(reset["reset"], true);
    Ok(())
}
