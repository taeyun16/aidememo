use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, RecordStatus, Revision, TenantId, TenantRecord,
};
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_local::SqliteCommandStore;
use std::{path::Path, process::Command};

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
    let store = home.path().join("embedded.sqlite");
    let store = store
        .to_str()
        .ok_or_else(|| std::io::Error::other("non-UTF-8 test store path"))?;

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
            store,
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
            store,
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
            store,
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
            store,
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
            store,
            "handoff",
            "--remote-profile",
            "codex-p1",
            "accept",
            handoff_id,
        ],
    );
    assert!(!wrong_receiver.status.success());
    assert!(String::from_utf8_lossy(&wrong_receiver.stderr).contains("not allowed"));

    let accepted = run(
        home.path(),
        &[
            "--store",
            store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-p2",
            "accept",
            handoff_id,
        ],
    );
    assert_success(&accepted);

    let result = Command::new(aidememo_bin())
        .env("HOME", home.path())
        .env("AIDEMEMO_SESSION_ID", session_id)
        .env("AIDEMEMO_SOURCE_ID", "project:aidememo")
        .env("AIDEMEMO_ACTOR_ID", "codex-p2")
        .args([
            "--store",
            store,
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

    let returned = run(
        home.path(),
        &[
            "--store",
            store,
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

    let outbox = run(
        home.path(),
        &[
            "--store",
            store,
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

    server.abort();
    Ok(())
}
