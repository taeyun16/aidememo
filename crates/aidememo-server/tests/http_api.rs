use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, RecordStatus, Revision, TenantId, TenantRecord,
};
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_local::SqliteCommandStore;
use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const WRITER_TOKEN: &str = "writer-token-0123456789";
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
        "writer_actor",
        MembershipRole::Writer,
        WRITER_TOKEN,
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
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/commands")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string()))
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
    assert_eq!(health_body["schema_version"], 2);

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
async fn writer_round_trip_and_reader_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let (app, epoch) = test_app()?;
    let upsert = json!({
        "command_id": "command_upsert",
        "project_id": "project_http",
        "expected_revision": null,
        "operation": "resource.put",
        "payload": {"z": 2, "a": {"d": 4, "b": 3}},
        "resource": {"kind": "fact", "id": "fact_http"},
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
                .uri("/v1/projects/project_http/resources/fact/fact_http")
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
        "resource": {"kind": "fact", "id": "fact_http"},
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
