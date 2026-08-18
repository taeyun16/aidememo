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

const SENDER_TOKEN: &str = "lease-sender-token-0123456789";
const RECEIVER_TOKEN: &str = "lease-receiver-token-0123456789";

fn test_app() -> Result<Router, Box<dyn std::error::Error>> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_lease")?,
        display_name: "Lease tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_lease")?,
        display_name: "Lease project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_lease")?,
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
        "sender",
        SENDER_TOKEN,
        timestamp,
    )?;
    provision(
        &mut store,
        &tenant,
        &project,
        "receiver",
        RECEIVER_TOKEN,
        timestamp,
    )?;
    Ok(router(ServerState::new(store)))
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

fn post_request(uri: &str, token: &str, body: Value) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
}

fn get_request(uri: &str, token: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
}

async fn response_json(response: axum::response::Response) -> Result<Value, axum::Error> {
    let bytes = response.into_body().collect().await?.to_bytes();
    serde_json::from_slice(&bytes).map_err(axum::Error::new)
}

#[tokio::test]
async fn remote_accept_assigns_lease_and_heartbeat_renews_only_active_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let app = test_app()?;
    let session = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_lease/sessions",
            SENDER_TOKEN,
            json!({
                "command_id": "lease-session",
                "payload": {
                    "session_id": "lease-session",
                    "source_id": "lease-source",
                    "topic": "Lease lifecycle"
                }
            }),
        )?)
        .await?;
    assert_eq!(session.status(), 200);

    let sent = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_lease/handoffs",
            SENDER_TOKEN,
            json!({
                "command_id": "lease-send",
                "payload": {
                    "handoff_id": "lease-handoff",
                    "session_id": "lease-session",
                    "to_actor": "receiver",
                    "focus": "keep lease alive",
                    "done_when": null,
                    "context_id": null
                }
            }),
        )?)
        .await?;
    assert_eq!(sent.status(), 200);
    let sent = response_json(sent).await?;
    let sent_revision = sent["revision"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("send receipt omitted revision"))?;

    let accepted = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_lease/handoffs/lease-handoff/accept",
            RECEIVER_TOKEN,
            json!({
                "command_id": "lease-accept",
                "expected_revision": sent_revision,
                "payload": {"claim_id": "lease-claim-one"}
            }),
        )?)
        .await?;
    assert_eq!(accepted.status(), 200);
    let accepted = response_json(accepted).await?;
    let accepted_revision = accepted["revision"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("accept receipt omitted revision"))?;

    let current = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_lease/handoffs/lease-handoff",
            RECEIVER_TOKEN,
        )?)
        .await?;
    assert_eq!(current.status(), 200);
    let current = response_json(current).await?;
    let first_heartbeat = current["record"]["claim_heartbeat_at_ms"]
        .as_i64()
        .ok_or_else(|| std::io::Error::other("accepted handoff omitted heartbeat"))?;
    let first_expiry = current["record"]["claim_expires_at_ms"]
        .as_i64()
        .ok_or_else(|| std::io::Error::other("accepted handoff omitted expiry"))?;
    assert!(first_expiry > first_heartbeat);
    assert_eq!(current["record"]["attempt_count"], 1);

    let heartbeat = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_lease/handoffs/lease-handoff/heartbeat",
            RECEIVER_TOKEN,
            json!({
                "command_id": "lease-heartbeat",
                "expected_revision": accepted_revision,
                "payload": {"claim_id": "lease-claim-one"}
            }),
        )?)
        .await?;
    assert_eq!(heartbeat.status(), 200);
    let heartbeat = response_json(heartbeat).await?;
    let heartbeat_revision = heartbeat["revision"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("heartbeat receipt omitted revision"))?;

    let wrong_claim = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_lease/handoffs/lease-handoff/heartbeat",
            RECEIVER_TOKEN,
            json!({
                "command_id": "lease-heartbeat-wrong",
                "expected_revision": heartbeat_revision,
                "payload": {"claim_id": "lease-claim-other"}
            }),
        )?)
        .await?;
    assert_eq!(wrong_claim.status(), 409);
    assert_eq!(
        response_json(wrong_claim).await?["error"]["code"],
        "handoff_conflict"
    );

    let wrong_actor = app
        .oneshot(post_request(
            "/v1/projects/project_lease/handoffs/lease-handoff/heartbeat",
            SENDER_TOKEN,
            json!({
                "command_id": "lease-heartbeat-sender",
                "expected_revision": heartbeat_revision,
                "payload": {"claim_id": "lease-claim-one"}
            }),
        )?)
        .await?;
    assert_eq!(wrong_actor.status(), 403);
    Ok(())
}
