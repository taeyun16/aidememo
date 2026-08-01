use aidememo_client::{HttpReplicaClient, RemoteProfile, ReplicaStore, pull_to_current};
use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, ProjectSequence, RecordStatus, ResourceId, ResourceKind,
    ResourceRef, ResourceState, Revision, TenantId, TenantRecord,
};
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_local::SqliteCommandStore;
use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const WRITER_TOKEN: &str = "writer-token-0123456789";
const RECEIVER_TOKEN: &str = "receiver-token-0123456789";
const HERMES_TOKEN: &str = "hermes-token-0123456789";
const READER_TOKEN: &str = "reader-token-0123456789";

fn test_app() -> Result<(Router, ProjectEpoch), Box<dyn std::error::Error>> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_http")?,
        display_name: "HTTP tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let epoch = ProjectEpoch::try_from("epoch_http")?;
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_http")?,
        display_name: "HTTP project".to_owned(),
        project_epoch: epoch.clone(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let mut store = SqliteCommandStore::open_in_memory()?;
    store.bootstrap_project(&tenant, &project)?;
    provision(
        &mut store,
        &tenant,
        &project,
        "codex-p1",
        MembershipRole::Writer,
        WRITER_TOKEN,
        timestamp,
    )?;
    provision(
        &mut store,
        &tenant,
        &project,
        "codex-p2",
        MembershipRole::Writer,
        RECEIVER_TOKEN,
        timestamp,
    )?;
    provision(
        &mut store,
        &tenant,
        &project,
        "hermes",
        MembershipRole::Writer,
        HERMES_TOKEN,
        timestamp,
    )?;
    provision(
        &mut store,
        &tenant,
        &project,
        "reader_actor",
        MembershipRole::Reader,
        READER_TOKEN,
        timestamp,
    )?;
    Ok((router(ServerState::new(store)), epoch))
}

#[allow(clippy::too_many_arguments)]
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

fn command_request(token: Option<&str>, body: Value) -> Result<Request<Body>, axum::http::Error> {
    post_request("/v1/commands", token, body)
}

fn post_request(
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string()))
}

fn get_request(uri: &str, token: Option<&str>) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty())
}

async fn response_json(response: axum::response::Response) -> Result<Value, axum::Error> {
    let bytes = response.into_body().collect().await?.to_bytes();
    serde_json::from_slice(&bytes).map_err(axum::Error::new)
}

#[tokio::test]
async fn authentication_and_identity_override_fail_closed() -> Result<(), Box<dyn std::error::Error>>
{
    let (app, _) = test_app()?;
    let health = app
        .clone()
        .oneshot(Request::builder().uri("/health").body(Body::empty())?)
        .await?;
    assert_eq!(health.status(), 200);
    let health_body = response_json(health).await?;
    assert_eq!(health_body["schema_version"], 4);

    let writer_identity = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/identity",
            Some(WRITER_TOKEN),
        )?)
        .await?;
    assert_eq!(writer_identity.status(), 200);
    let writer_identity = response_json(writer_identity).await?;
    assert_eq!(writer_identity["tenant_id"], "tenant_http");
    assert_eq!(writer_identity["project_id"], "project_http");
    assert_eq!(writer_identity["project_epoch"], "epoch_http");
    assert_eq!(writer_identity["actor_id"], "codex-p1");
    assert_eq!(writer_identity["role"], "writer");

    let receiver_identity = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/identity",
            Some(RECEIVER_TOKEN),
        )?)
        .await?;
    assert_eq!(
        response_json(receiver_identity).await?["actor_id"],
        "codex-p2"
    );

    let missing_identity = app
        .clone()
        .oneshot(get_request("/v1/projects/project_http/identity", None)?)
        .await?;
    assert_eq!(missing_identity.status(), 401);

    let body = json!({
        "command_id": "command_auth",
        "project_id": "project_http",
        "expected_revision": null,
        "operation": "resource.put",
        "payload": {"content": "blocked"},
        "resource": {"kind": "fact", "id": "fact_auth"},
        "change": "upsert"
    });
    let missing = app
        .clone()
        .oneshot(command_request(None, body.clone())?)
        .await?;
    assert_eq!(missing.status(), 401);
    assert_eq!(
        response_json(missing).await?["error"]["code"],
        "authentication_failed"
    );

    let unknown = app
        .clone()
        .oneshot(command_request(Some("unknown-token"), body.clone())?)
        .await?;
    assert_eq!(unknown.status(), 401);

    let mut unsupported_body = body.clone();
    unsupported_body["operation"] = json!("fact.add");
    let unsupported = app
        .clone()
        .oneshot(command_request(Some(WRITER_TOKEN), unsupported_body)?)
        .await?;
    assert_eq!(unsupported.status(), 400);
    assert_eq!(
        response_json(unsupported).await?["error"]["code"],
        "invalid_command"
    );

    let reserved = app
        .clone()
        .oneshot(command_request(Some(WRITER_TOKEN), body.clone())?)
        .await?;
    assert_eq!(reserved.status(), 400);
    assert!(
        response_json(reserved).await?["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("custom.*"))
    );

    let mut override_body = body;
    override_body["tenant_id"] = json!("tenant_other");
    let override_attempt = app
        .oneshot(command_request(Some(WRITER_TOKEN), override_body)?)
        .await?;
    assert_eq!(override_attempt.status(), 400);
    assert_eq!(
        response_json(override_attempt).await?["error"]["code"],
        "invalid_command"
    );
    Ok(())
}

#[tokio::test]
async fn codex_profiles_and_hermes_complete_typed_remote_handoffs()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, _) = test_app()?;
    let session_request = json!({
        "command_id": "command_session_remote",
        "payload": {
            "session_id": "session_remote",
            "source_id": "project:aidememo",
            "topic": "Remote handoff contract"
        }
    });
    let session = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/sessions",
            Some(WRITER_TOKEN),
            session_request.clone(),
        )?)
        .await?;
    assert_eq!(session.status(), 200);
    let session_receipt = response_json(session).await?;
    assert_eq!(session_receipt["revision"], 1);

    let session_replay = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/sessions",
            Some(WRITER_TOKEN),
            session_request.clone(),
        )?)
        .await?;
    assert_eq!(response_json(session_replay).await?, session_receipt);

    let actor_collision = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/sessions",
            Some(RECEIVER_TOKEN),
            session_request,
        )?)
        .await?;
    assert_eq!(actor_collision.status(), 409);
    assert_eq!(
        response_json(actor_collision).await?["error"]["code"],
        "command_conflict"
    );

    let send_to_p2 = json!({
        "command_id": "command_send_p2",
        "payload": {
            "handoff_id": "handoff_p2",
            "session_id": "session_remote",
            "to_actor": "codex-p2",
            "focus": "Review the typed server boundary",
            "done_when": "Return a session-scoped fact"
        }
    });
    let sent = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs",
            Some(WRITER_TOKEN),
            send_to_p2,
        )?)
        .await?;
    assert_eq!(response_json(sent).await?["revision"], 1);

    let sent_second = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs",
            Some(WRITER_TOKEN),
            json!({
                "command_id": "command_send_p2_second",
                "payload": {
                    "handoff_id": "handoff_p2_second",
                    "session_id": "session_remote",
                    "to_actor": "codex-p2",
                    "focus": "Keep pagination deterministic",
                    "done_when": null
                }
            }),
        )?)
        .await?;
    assert_eq!(response_json(sent_second).await?["revision"], 1);

    let first_inbox_page = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/handoffs?box=inbox&limit=1",
            Some(RECEIVER_TOKEN),
        )?)
        .await?;
    assert_eq!(first_inbox_page.status(), 200);
    let first_inbox_page = response_json(first_inbox_page).await?;
    assert_eq!(
        first_inbox_page["assignments"][0]["record"]["handoff_id"],
        "handoff_p2_second"
    );
    let before_seq = first_inbox_page["next_before_seq"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("missing mailbox cursor"))?;
    let second_inbox_page = app
        .clone()
        .oneshot(get_request(
            &format!(
                "/v1/projects/project_http/handoffs?box=inbox&limit=1&before_seq={before_seq}"
            ),
            Some(RECEIVER_TOKEN),
        )?)
        .await?;
    let second_inbox_page = response_json(second_inbox_page).await?;
    assert_eq!(
        second_inbox_page["assignments"][0]["record"]["handoff_id"],
        "handoff_p2"
    );
    assert!(second_inbox_page["next_before_seq"].is_null());

    let actor_override = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/handoffs?box=inbox&actor_id=hermes",
            Some(RECEIVER_TOKEN),
        )?)
        .await?;
    assert_eq!(actor_override.status(), 400);

    let p1_inbox = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/handoffs?box=inbox",
            Some(WRITER_TOKEN),
        )?)
        .await?;
    assert!(
        response_json(p1_inbox).await?["assignments"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    let p1_outbox = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/handoffs?box=outbox",
            Some(WRITER_TOKEN),
        )?)
        .await?;
    assert_eq!(
        response_json(p1_outbox).await?["assignments"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let wrong_source = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/handoffs?box=inbox&source_id=project%3Aother",
            Some(RECEIVER_TOKEN),
        )?)
        .await?;
    assert!(
        response_json(wrong_source).await?["assignments"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    let read_only_receiver = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs",
            Some(WRITER_TOKEN),
            json!({
                "command_id": "command_send_reader",
                "payload": {
                    "handoff_id": "handoff_reader",
                    "session_id": "session_remote",
                    "to_actor": "reader_actor",
                    "focus": null,
                    "done_when": null
                }
            }),
        )?)
        .await?;
    assert_eq!(read_only_receiver.status(), 400);

    let accept_p2 = json!({
        "command_id": "command_accept_p2",
        "expected_revision": 1,
        "payload": {"claim_id": "claim_p2"}
    });
    let wrong_accept = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs/handoff_p2/accept",
            Some(WRITER_TOKEN),
            accept_p2.clone(),
        )?)
        .await?;
    assert_eq!(wrong_accept.status(), 403);
    assert_eq!(
        response_json(wrong_accept).await?["error"]["code"],
        "handoff_actor_mismatch"
    );

    let accepted = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs/handoff_p2/accept",
            Some(RECEIVER_TOKEN),
            accept_p2.clone(),
        )?)
        .await?;
    let accepted_receipt = response_json(accepted).await?;
    assert_eq!(accepted_receipt["revision"], 2);

    let p2_fact = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/facts",
            Some(RECEIVER_TOKEN),
            json!({
                "command_id": "command_fact_p2",
                "payload": {
                    "fact_id": "fact_p2_result",
                    "session_id": "session_remote",
                    "content": "Codex P2 completed the review"
                }
            }),
        )?)
        .await?;
    assert_eq!(p2_fact.status(), 200);

    let hermes_fact = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/facts",
            Some(HERMES_TOKEN),
            json!({
                "command_id": "command_fact_hermes",
                "payload": {
                    "fact_id": "fact_hermes_result",
                    "session_id": "session_remote",
                    "content": "Hermes verified the shared project memory"
                }
            }),
        )?)
        .await?;
    assert_eq!(hermes_fact.status(), 200);

    let wrong_result = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs/handoff_p2/return",
            Some(RECEIVER_TOKEN),
            json!({
                "command_id": "command_return_wrong_actor_fact",
                "expected_revision": 2,
                "payload": {
                    "claim_id": "claim_p2",
                    "result_fact_id": "fact_hermes_result",
                    "outcome": "succeeded"
                }
            }),
        )?)
        .await?;
    assert_eq!(wrong_result.status(), 409);
    assert_eq!(
        response_json(wrong_result).await?["error"]["code"],
        "handoff_conflict"
    );

    let wrong_claim = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs/handoff_p2/return",
            Some(RECEIVER_TOKEN),
            json!({
                "command_id": "command_return_wrong_claim",
                "expected_revision": 2,
                "payload": {
                    "claim_id": "claim_other",
                    "result_fact_id": "fact_p2_result",
                    "outcome": "succeeded"
                }
            }),
        )?)
        .await?;
    assert_eq!(wrong_claim.status(), 409);
    assert_eq!(
        response_json(wrong_claim).await?["error"]["code"],
        "handoff_conflict"
    );

    let returned_p2 = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs/handoff_p2/return",
            Some(RECEIVER_TOKEN),
            json!({
                "command_id": "command_return_p2",
                "expected_revision": 2,
                "payload": {
                    "claim_id": "claim_p2",
                    "result_fact_id": "fact_p2_result",
                    "outcome": "succeeded"
                }
            }),
        )?)
        .await?;
    assert_eq!(response_json(returned_p2).await?["revision"], 3);

    let late_accept_replay = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs/handoff_p2/accept",
            Some(RECEIVER_TOKEN),
            accept_p2,
        )?)
        .await?;
    assert_eq!(response_json(late_accept_replay).await?, accepted_receipt);

    let p2_status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/projects/project_http/handoffs/handoff_p2")
                .header("authorization", format!("Bearer {WRITER_TOKEN}"))
                .body(Body::empty())?,
        )
        .await?;
    let p2_status = response_json(p2_status).await?;
    assert_eq!(p2_status["record"]["status"], "completed");
    assert_eq!(p2_status["record"]["result_fact_id"], "fact_p2_result");

    let open_p2_inbox = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/handoffs?box=inbox",
            Some(RECEIVER_TOKEN),
        )?)
        .await?;
    let open_p2_inbox = response_json(open_p2_inbox).await?;
    assert_eq!(
        open_p2_inbox["assignments"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        open_p2_inbox["assignments"][0]["record"]["handoff_id"],
        "handoff_p2_second"
    );

    let complete_p2_inbox = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/handoffs?box=inbox&include_completed=true",
            Some(RECEIVER_TOKEN),
        )?)
        .await?;
    assert_eq!(
        response_json(complete_p2_inbox).await?["assignments"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let hidden_from_reader = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/projects/project_http/handoffs/handoff_p2")
                .header("authorization", format!("Bearer {READER_TOKEN}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(hidden_from_reader.status(), 403);

    let sent_to_hermes = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs",
            Some(RECEIVER_TOKEN),
            json!({
                "command_id": "command_send_hermes",
                "payload": {
                    "handoff_id": "handoff_hermes",
                    "session_id": "session_remote",
                    "to_actor": "hermes",
                    "focus": "Validate shared gateway memory",
                    "done_when": null
                }
            }),
        )?)
        .await?;
    assert_eq!(response_json(sent_to_hermes).await?["revision"], 1);

    let accepted_hermes = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs/handoff_hermes/accept",
            Some(HERMES_TOKEN),
            json!({
                "command_id": "command_accept_hermes",
                "expected_revision": 1,
                "payload": {"claim_id": "claim_hermes"}
            }),
        )?)
        .await?;
    assert_eq!(response_json(accepted_hermes).await?["revision"], 2);

    let returned_hermes = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/handoffs/handoff_hermes/return",
            Some(HERMES_TOKEN),
            json!({
                "command_id": "command_return_hermes",
                "expected_revision": 2,
                "payload": {
                    "claim_id": "claim_hermes",
                    "result_fact_id": "fact_hermes_result",
                    "outcome": "succeeded"
                }
            }),
        )?)
        .await?;
    assert_eq!(response_json(returned_hermes).await?["revision"], 3);
    Ok(())
}

#[tokio::test]
async fn writer_round_trip_and_reader_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let (app, epoch) = test_app()?;
    let upsert = json!({
        "command_id": "command_upsert",
        "project_id": "project_http",
        "expected_revision": null,
        "operation": "resource.put",
        "payload": {"z": 2, "a": {"d": 4, "b": 3}},
        "resource": {"kind": "custom.note", "id": "note_http"},
        "change": "upsert"
    });
    let first = app
        .clone()
        .oneshot(command_request(Some(WRITER_TOKEN), upsert.clone())?)
        .await?;
    assert_eq!(first.status(), 200);
    let first_body = response_json(first).await?;
    assert_eq!(first_body["project_seq"], 1);
    assert_eq!(first_body["revision"], 1);

    let replay = app
        .clone()
        .oneshot(command_request(Some(WRITER_TOKEN), upsert.clone())?)
        .await?;
    assert_eq!(replay.status(), 200);
    assert_eq!(response_json(replay).await?, first_body);

    let mut coordinate_conflict = upsert.clone();
    coordinate_conflict["resource"]["id"] = json!("note_other");
    let coordinate_conflict_response = app
        .clone()
        .oneshot(command_request(Some(WRITER_TOKEN), coordinate_conflict)?)
        .await?;
    assert_eq!(coordinate_conflict_response.status(), 409);
    assert_eq!(
        response_json(coordinate_conflict_response).await?["error"]["code"],
        "command_conflict"
    );

    let mut conflict = upsert;
    conflict["payload"] = json!({"content": "different"});
    let conflict_response = app
        .clone()
        .oneshot(command_request(Some(WRITER_TOKEN), conflict)?)
        .await?;
    assert_eq!(conflict_response.status(), 409);
    assert_eq!(
        response_json(conflict_response).await?["error"]["code"],
        "command_conflict"
    );

    let resource = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/projects/project_http/resources/custom.note/note_http")
                .header("authorization", format!("Bearer {READER_TOKEN}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(resource.status(), 200);
    let resource_body = response_json(resource).await?;
    assert_eq!(resource_body["state"]["state"], "present");
    assert_eq!(resource_body["state"]["body"]["a"]["b"], 3);

    let changes = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/projects/project_http/changes?project_epoch={epoch}&after_seq=0&limit=10"
                ))
                .header("authorization", format!("Bearer {READER_TOKEN}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(changes.status(), 200);
    assert_eq!(
        response_json(changes).await?["entries"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let reader_write = json!({
        "command_id": "command_reader_write",
        "project_id": "project_http",
        "expected_revision": 1,
        "operation": "resource.put",
        "payload": {"content": "forbidden"},
        "resource": {"kind": "custom.note", "id": "note_http"},
        "change": "upsert"
    });
    let forbidden = app
        .oneshot(command_request(Some(READER_TOKEN), reader_write)?)
        .await?;
    assert_eq!(forbidden.status(), 403);
    assert_eq!(
        response_json(forbidden).await?["error"]["code"],
        "project_unauthorized"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_replica_bootstraps_pulls_incrementally_and_reads_offline()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, _) = test_app()?;
    let create = json!({
        "command_id": "command_replica_create",
        "project_id": "project_http",
        "expected_revision": null,
        "operation": "resource.put",
        "payload": {"content": "replica-v1"},
        "resource": {"kind": "custom.note", "id": "note_replica"},
        "change": "upsert"
    });
    let created = app
        .clone()
        .oneshot(command_request(Some(WRITER_TOKEN), create)?)
        .await?;
    assert_eq!(created.status(), 200);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server_app = app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, server_app).await });
    let profile = RemoteProfile::new(
        format!("http://{address}"),
        ProjectId::try_from("project_http")?,
        READER_TOKEN,
    )?;
    let client = HttpReplicaClient::new(profile);
    let dir = tempfile::tempdir()?;
    let mut replica = ReplicaStore::open(dir.path().join("replica.sqlite"))?;
    let first = pull_to_current(&client, &mut replica, 1)?;
    assert!(first.bootstrapped);
    assert_eq!(first.after_seq, ProjectSequence::new(1));
    assert_eq!(first.changes, 0);

    let coordinate = ResourceRef {
        kind: ResourceKind::try_from("custom.note")?,
        id: ResourceId::try_from("note_replica")?,
    };
    let cached = replica
        .resource(&coordinate)?
        .ok_or("missing replica body")?;
    assert!(matches!(cached.state, ResourceState::Present { .. }));

    let delete = json!({
        "command_id": "command_replica_delete",
        "project_id": "project_http",
        "expected_revision": 1,
        "operation": "resource.delete",
        "payload": null,
        "resource": {"kind": "custom.note", "id": "note_replica"},
        "change": "delete"
    });
    let deleted = app
        .clone()
        .oneshot(command_request(Some(WRITER_TOKEN), delete)?)
        .await?;
    assert_eq!(deleted.status(), 200);

    let second = pull_to_current(&client, &mut replica, 1)?;
    assert!(!second.bootstrapped);
    assert_eq!(second.after_seq, ProjectSequence::new(2));
    assert_eq!(second.tombstone_count, 1);
    assert!(matches!(
        replica
            .resource(&coordinate)?
            .map(|resource| resource.state),
        Some(ResourceState::Deleted)
    ));

    server.abort();
    let _ = server.await;
    let status_before = replica.status()?;
    assert!(pull_to_current(&client, &mut replica, 1).is_err());
    assert_eq!(replica.status()?, status_before);
    assert!(matches!(
        replica
            .resource(&coordinate)?
            .map(|resource| resource.state),
        Some(ResourceState::Deleted)
    ));
    Ok(())
}
