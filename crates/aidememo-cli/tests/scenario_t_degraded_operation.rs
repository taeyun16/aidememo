use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, ClaimId, FactId, FactRecord, HandoffId, HandoffOutcome,
    HandoffRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId, ProjectMembership,
    ProjectRecord, RecordStatus, Revision, SessionId, SessionRecord, SourceId, TenantId,
    TenantRecord,
};
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_local::SqliteCommandStore;
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::Path,
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const PROJECT: &str = "project_phase1_t";
const SOURCE: &str = "project:phase1-t";
const A_TOKEN: &str = "phase1-codex-a-token-0123456789";
const B_TOKEN: &str = "phase1-codex-b-token-0123456789";
const H_TOKEN: &str = "phase1-hermes-token-0123456789";
const OUTSIDER_TOKEN: &str = "phase1-outsider-token-0123456789";

fn aidememo_bin() -> &'static str {
    env!("CARGO_BIN_EXE_aidememo")
}

fn run(home: &Path, args: &[&str]) -> Output {
    run_with_env(home, &[], args)
}

fn run_with_env(home: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = Command::new(aidememo_bin());
    command
        .env("HOME", home)
        .env_remove("AIDEMEMO_REMOTE_PROFILE")
        .env_remove("AIDEMEMO_ACTOR_ID")
        .env_remove("AIDEMEMO_SESSION_ID")
        .env_remove("AIDEMEMO_SOURCE_ID")
        .args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("failed to execute aidememo binary")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn output_json(output: Output) -> Result<Value, Box<dyn std::error::Error>> {
    assert_success(&output);
    serde_json::from_slice(&output.stdout).map_err(Into::into)
}

fn provision(
    store: &mut SqliteCommandStore,
    tenant: &TenantRecord,
    project: &ProjectRecord,
    actor_id: &str,
    role: MembershipRole,
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
        role,
        status: MembershipStatus::Active,
    };
    store.provision_actor(&actor, &membership, &bearer_token_digest(token)?, timestamp)?;
    Ok(())
}

#[derive(Clone, Default)]
struct FaultSwitches {
    offline: Arc<AtomicBool>,
    drop_next_handoff_success: Arc<AtomicBool>,
}

async fn fault_middleware(
    State(faults): State<FaultSwitches>,
    request: Request,
    next: Next,
) -> Response {
    if faults.offline.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, "injected server outage").into_response();
    }
    let discard_success = request.method() == Method::POST
        && request.uri().path().ends_with("/handoffs")
        && faults
            .drop_next_handoff_success
            .swap(false, Ordering::SeqCst);
    let response = next.run(request).await;
    if discard_success && response.status().is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            "injected lost upstream success response",
        )
            .into_response();
    }
    response
}

fn http_json(
    method: &str,
    base: &str,
    token: &str,
    path: &str,
    body: Option<Value>,
) -> Result<(u16, Value), Box<dyn std::error::Error>> {
    let url = format!("{base}{path}");
    let mut request = ureq::request(method, &url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json");
    let result = if let Some(body) = body {
        request = request.set("Content-Type", "application/json");
        request.send_json(body)
    } else {
        request.call()
    };
    match result {
        Ok(response) => {
            let status = response.status();
            let value = response.into_json::<Value>().unwrap_or(Value::Null);
            Ok((status, value))
        }
        Err(ureq::Error::Status(status, response)) => {
            let value = response.into_json::<Value>().unwrap_or(Value::Null);
            Ok((status, value))
        }
        Err(error) => Err(error.into()),
    }
}

fn count_handoff(page: &Value, handoff_id: &str) -> usize {
    page["assignments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["record"]["handoff_id"].as_str() == Some(handoff_id))
        .count()
}

fn assert_stale_claim_fencing() -> Result<(), Box<dyn std::error::Error>> {
    let sender = ActorId::try_from("scenario-t-sender")?;
    let receiver = ActorId::try_from("scenario-t-receiver")?;
    let session = SessionRecord::new(
        SessionId::try_from("scenario-t-stale-session")?,
        Some(SourceId::try_from(SOURCE)?),
        "Scenario T stale claim".to_owned(),
        sender.clone(),
    )?;
    let mut handoff = HandoffRecord::new(
        HandoffId::try_from("scenario-t-stale-handoff")?,
        &session,
        sender,
        receiver.clone(),
        None,
        None,
    )?;
    let old_claim = ClaimId::try_from("scenario-t-old-claim")?;
    let new_claim = ClaimId::try_from("scenario-t-new-claim")?;
    handoff.accept_with_lease(&receiver, old_claim.clone(), 1_000, 10)?;
    handoff.accept_with_lease(&receiver, new_claim.clone(), 1_011, 10)?;
    let fact = FactRecord::new(
        FactId::try_from("scenario-t-stale-result")?,
        session.session_id,
        session.source_id,
        receiver.clone(),
        "new worker evidence".to_owned(),
    )?;
    assert!(
        handoff
            .return_result_at(
                &receiver,
                &old_claim,
                &fact,
                HandoffOutcome::Succeeded,
                1_012
            )
            .is_err(),
        "expired old worker returned evidence after a newer claim"
    );
    handoff.return_result_at(
        &receiver,
        &new_claim,
        &fact,
        HandoffOutcome::Succeeded,
        1_012,
    )?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_t_degraded_remote_ssot_closes_phase1_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_phase1_t")?,
        display_name: "Phase 1 Scenario T".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from(PROJECT)?,
        display_name: "Phase 1 Scenario T".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_phase1_t")?,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let mut command_store = SqliteCommandStore::open_in_memory()?;
    command_store.bootstrap_project(&tenant, &project)?;
    for (actor, role, token) in [
        ("codex-a", MembershipRole::Writer, A_TOKEN),
        ("codex-b", MembershipRole::Writer, B_TOKEN),
        ("hermes", MembershipRole::Writer, H_TOKEN),
        ("outsider", MembershipRole::Reader, OUTSIDER_TOKEN),
    ] {
        provision(
            &mut command_store,
            &tenant,
            &project,
            actor,
            role,
            token,
            timestamp,
        )?;
    }

    let faults = FaultSwitches::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let app = router(ServerState::new(command_store)).layer(middleware::from_fn_with_state(
        faults.clone(),
        fault_middleware,
    ));
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let url = format!("http://{address}");

    let home = tempfile::tempdir()?;
    let a_store = home.path().join("codex-a.sqlite");
    let b_store = home.path().join("codex-b.sqlite");
    let h_store = home.path().join("hermes.sqlite");
    let b_replica = home.path().join("codex-b.replica.sqlite");
    let a_store = a_store.to_string_lossy().into_owned();
    let b_store = b_store.to_string_lossy().into_owned();
    let h_store = h_store.to_string_lossy().into_owned();
    let b_replica = b_replica.to_string_lossy().into_owned();

    for (profile, token) in [
        ("codex-a", A_TOKEN),
        ("codex-b", B_TOKEN),
        ("hermes", H_TOKEN),
        ("outsider", OUTSIDER_TOKEN),
    ] {
        assert_success(&run(
            home.path(),
            &[
                "auth",
                "login",
                "--profile",
                profile,
                "--project-id",
                PROJECT,
                "--token",
                token,
                &url,
            ],
        ));
    }

    let created = output_json(run(
        home.path(),
        &[
            "--store",
            &a_store,
            "--json",
            "session",
            "new",
            "--source-id",
            SOURCE,
            "Scenario T degraded remote SSOT",
        ],
    ))?;
    let session_id = created["session_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("session_id missing"))?
        .to_owned();

    faults
        .drop_next_handoff_success
        .store(true, Ordering::SeqCst);
    let ambiguous_send = output_json(run(
        home.path(),
        &[
            "--store",
            &a_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-a",
            "send",
            "--source-id",
            SOURCE,
            "--focus",
            "Codex B validates the degraded-operation boundary",
            "codex-b",
            &session_id,
        ],
    ))?;
    assert_eq!(ambiguous_send["queued"], true);
    assert_eq!(ambiguous_send["dispatched"], false);
    let first_handoff_id = ambiguous_send["handoff_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("first handoff id missing"))?
        .to_owned();

    let (status, before_replay) = http_json(
        "GET",
        &url,
        A_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs/{first_handoff_id}"),
        None,
    )?;
    assert_eq!(status, 200, "server did not commit before response loss");
    let first_revision = before_replay["revision"].clone();

    let queued_after_loss = output_json(run(
        home.path(),
        &[
            "--store",
            &a_store,
            "--json",
            "replica",
            "outbox",
            "--remote-profile",
            "codex-a",
        ],
    ))?;
    assert_eq!(queued_after_loss["count"], 1);

    let replay = output_json(run(
        home.path(),
        &[
            "--store",
            &a_store,
            "--json",
            "replica",
            "publish",
            "--remote-profile",
            "codex-a",
        ],
    ))?;
    assert_eq!(replay["published"], 1);
    assert_eq!(replay["failed"], 0);
    assert_eq!(replay["conflicts"], 0);

    let (_, after_replay) = http_json(
        "GET",
        &url,
        A_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs/{first_handoff_id}"),
        None,
    )?;
    assert_eq!(after_replay["revision"], first_revision);
    assert_eq!(after_replay["record"], before_replay["record"]);
    let (_, a_outbox) = http_json(
        "GET",
        &url,
        A_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs?box=outbox&include_completed=true&limit=100"),
        None,
    )?;
    assert_eq!(count_handoff(&a_outbox, &first_handoff_id), 1);

    let pull = output_json(run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "replica",
            "pull",
            "--remote-profile",
            "codex-b",
            "--replica-path",
            &b_replica,
        ],
    ))?;
    assert!(pull["report"]["after_seq"].as_u64().unwrap_or_default() > 0);

    let accepted_b = output_json(run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-b",
            "accept",
            &first_handoff_id,
        ],
    ))?;
    assert_eq!(accepted_b["actor_id"], "codex-b");

    assert_success(&run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "session",
            "resume",
            "--source-id",
            SOURCE,
            &session_id,
        ],
    ));
    let b_fact = output_json(run_with_env(
        home.path(),
        &[
            ("AIDEMEMO_SESSION_ID", &session_id),
            ("AIDEMEMO_SOURCE_ID", SOURCE),
            ("AIDEMEMO_ACTOR_ID", "codex-b"),
        ],
        &[
            "--store",
            &b_store,
            "--json",
            "fact",
            "add",
            "Codex B validated the first remote handoff",
            "--type",
            "note",
            "--entities",
            "ScenarioT",
            "--source-id",
            SOURCE,
            "--actor-id",
            "codex-b",
        ],
    ))?;
    let b_fact_id = b_fact["id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("codex-b result fact missing"))?
        .to_owned();
    let returned_b = output_json(run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-b",
            "return",
            "--outcome",
            "succeeded",
            "--result-fact-id",
            &b_fact_id,
            &first_handoff_id,
        ],
    ))?;
    assert_eq!(returned_b["outcome"], "succeeded");

    faults.offline.store(true, Ordering::SeqCst);
    let cached_session = output_json(run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "replica",
            "get",
            "--replica-path",
            &b_replica,
            "session",
            &session_id,
        ],
    ))?;
    assert_eq!(cached_session["state"]["body"]["session_id"], session_id);

    let offline_send = output_json(run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "handoff",
            "--remote-profile",
            "codex-b",
            "send",
            "--source-id",
            SOURCE,
            "--focus",
            "Hermes verifies recovery and final evidence",
            "hermes",
            &session_id,
        ],
    ))?;
    assert_eq!(offline_send["queued"], true);
    assert_eq!(offline_send["dispatched"], false);
    let second_handoff_id = offline_send["handoff_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("second handoff id missing"))?
        .to_owned();

    let persisted_outbox_1 = output_json(run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "replica",
            "outbox",
            "--remote-profile",
            "codex-b",
        ],
    ))?;
    let persisted_outbox_2 = output_json(run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "replica",
            "outbox",
            "--remote-profile",
            "codex-b",
        ],
    ))?;
    assert_eq!(persisted_outbox_1, persisted_outbox_2);
    assert_eq!(persisted_outbox_1["count"], 1);

    faults.offline.store(false, Ordering::SeqCst);
    let (_, b_outbox_before_publish) = http_json(
        "GET",
        &url,
        B_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs?box=outbox&include_completed=true&limit=100"),
        None,
    )?;
    assert_eq!(
        count_handoff(&b_outbox_before_publish, &second_handoff_id),
        0
    );

    let publish = output_json(run(
        home.path(),
        &[
            "--store",
            &b_store,
            "--json",
            "replica",
            "publish",
            "--remote-profile",
            "codex-b",
        ],
    ))?;
    assert_eq!(publish["published"], 1);
    assert_eq!(publish["failed"], 0);
    assert_eq!(publish["conflicts"], 0);

    let (_, b_outbox_after_publish) = http_json(
        "GET",
        &url,
        B_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs?box=outbox&include_completed=true&limit=100"),
        None,
    )?;
    let (_, hermes_inbox) = http_json(
        "GET",
        &url,
        H_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs?box=inbox&include_completed=true&limit=100"),
        None,
    )?;
    assert_eq!(
        count_handoff(&b_outbox_after_publish, &second_handoff_id),
        1
    );
    assert_eq!(count_handoff(&hermes_inbox, &second_handoff_id), 1);

    for token in [A_TOKEN, OUTSIDER_TOKEN] {
        let (status, _) = http_json(
            "GET",
            &url,
            token,
            &format!("/v1/projects/{PROJECT}/handoffs/{second_handoff_id}"),
            None,
        )?;
        assert_eq!(status, 403, "non-participant read was not rejected");
    }

    let accepted_h = output_json(run(
        home.path(),
        &[
            "--store",
            &h_store,
            "--json",
            "handoff",
            "--remote-profile",
            "hermes",
            "accept",
            &second_handoff_id,
        ],
    ))?;
    assert_eq!(accepted_h["actor_id"], "hermes");
    let (_, accepted_status) = http_json(
        "GET",
        &url,
        H_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs/{second_handoff_id}"),
        None,
    )?;
    let current_revision = accepted_status["revision"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("accepted revision missing"))?;
    let claim_id = accepted_status["record"]["claim_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("accepted claim id missing"))?;
    assert!(current_revision > 1);
    let (stale_status, stale_body) = http_json(
        "POST",
        &url,
        H_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs/{second_handoff_id}/heartbeat"),
        Some(json!({
            "command_id": "scenario-t-stale-heartbeat",
            "expected_revision": current_revision - 1,
            "payload": {"claim_id": claim_id}
        })),
    )?;
    assert_eq!(stale_status, 409);
    assert_eq!(stale_body["error"]["code"], "stale_revision");

    assert_success(&run(
        home.path(),
        &[
            "--store",
            &h_store,
            "--json",
            "session",
            "resume",
            "--source-id",
            SOURCE,
            &session_id,
        ],
    ));
    let h_fact = output_json(run_with_env(
        home.path(),
        &[
            ("AIDEMEMO_SESSION_ID", &session_id),
            ("AIDEMEMO_SOURCE_ID", SOURCE),
            ("AIDEMEMO_ACTOR_ID", "hermes"),
        ],
        &[
            "--store",
            &h_store,
            "--json",
            "fact",
            "add",
            "phase1evidence Hermes completed the recovered handoff",
            "--type",
            "note",
            "--entities",
            "ScenarioT",
            "--source-id",
            SOURCE,
            "--actor-id",
            "hermes",
        ],
    ))?;
    let h_fact_id = h_fact["id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("Hermes result fact missing"))?
        .to_owned();
    let final_return = output_json(run(
        home.path(),
        &[
            "--store",
            &h_store,
            "--json",
            "handoff",
            "--remote-profile",
            "hermes",
            "return",
            "--outcome",
            "succeeded",
            "--result-fact-id",
            &h_fact_id,
            &second_handoff_id,
        ],
    ))?;
    let final_seq = final_return["receipt"]["project_seq"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("final return project sequence missing"))?;

    let (search_status, search) = http_json(
        "GET",
        &url,
        H_TOKEN,
        &format!(
            "/v1/projects/{PROJECT}/search?q=phase1evidence&mode=lexical&at_least_seq={final_seq}"
        ),
        None,
    )?;
    assert_eq!(search_status, 200);
    assert!(search["index_seq"].as_u64().unwrap_or_default() >= final_seq);
    assert!(
        search["results"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|entry| entry["fact_id"].as_str() == Some(&h_fact_id)),
        "sequence-aware retrieval did not expose the committed final result fact"
    );

    let (fact_status, canonical_fact) = http_json(
        "GET",
        &url,
        H_TOKEN,
        &format!("/v1/projects/{PROJECT}/resources/fact/{h_fact_id}"),
        None,
    )?;
    assert_eq!(fact_status, 200);
    let fact_body = &canonical_fact["state"]["body"];
    assert_eq!(fact_body["session_id"], session_id);
    assert_eq!(fact_body["source_id"], SOURCE);
    assert_eq!(fact_body["actor_id"], "hermes");

    let (_, final_handoff) = http_json(
        "GET",
        &url,
        H_TOKEN,
        &format!("/v1/projects/{PROJECT}/handoffs/{second_handoff_id}"),
        None,
    )?;
    assert_eq!(final_handoff["record"]["status"], "completed");
    assert_eq!(final_handoff["record"]["result_fact_id"], h_fact_id);

    let (snapshot_status, snapshot) = http_json(
        "GET",
        &url,
        B_TOKEN,
        &format!("/v1/projects/{PROJECT}/snapshot"),
        None,
    )?;
    assert_eq!(snapshot_status, 200);
    let resources = snapshot["resources"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("snapshot resources missing"))?;
    let mut coordinates = HashSet::new();
    for resource in resources {
        let kind = resource["resource"]["kind"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("snapshot resource kind missing"))?;
        let id = resource["resource"]["id"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("snapshot resource id missing"))?;
        assert!(
            coordinates.insert((kind.to_owned(), id.to_owned())),
            "duplicate canonical resource coordinate {kind}/{id}"
        );
    }
    assert_eq!(
        coordinates
            .iter()
            .filter(|(kind, id)| kind == "handoff" && id == &first_handoff_id)
            .count(),
        1
    );
    assert_eq!(
        coordinates
            .iter()
            .filter(|(kind, id)| kind == "handoff" && id == &second_handoff_id)
            .count(),
        1
    );

    assert_stale_claim_fencing()?;

    server.abort();
    Ok(())
}
