use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, DomainError, MembershipRole, MembershipStatus, ProjectEpoch,
    ProjectId, ProjectMembership, ProjectRecord, RecordStatus, Revision, ServerIdentityStore,
    TenantId, TenantRecord,
};
use aidememo_server::{ServerState, bearer_token_digest, router};
use aidememo_store_postgres::PostgresCommandStore;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{fs, time::Duration};
use tower::ServiceExt;

const WRITER_TOKEN: &str = "postgres-tls-http-writer-token-0123456789";

fn bootstrap_postgres(url: &str, ca: &[u8]) -> Result<(), DomainError> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_pg_tls_http")?,
        display_name: "PostgreSQL TLS HTTP tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_pg_tls_http")?,
        display_name: "PostgreSQL TLS HTTP project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_pg_tls_http")?,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let actor = ActorRecord {
        tenant_id: tenant.tenant_id.clone(),
        actor_id: ActorId::try_from("writer_pg_tls_http")?,
        display_name: "PostgreSQL TLS HTTP writer".to_owned(),
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
    let mut store = PostgresCommandStore::connect_tls_with_timeouts(
        url,
        Some(ca),
        Duration::from_millis(1_500),
        Duration::from_millis(250),
    )?;
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(
        &actor,
        &membership,
        &bearer_token_digest(WRITER_TOKEN)?,
        timestamp,
    )?;
    Ok(())
}

fn get_request(uri: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {WRITER_TOKEN}"))
        .body(Body::empty())
}

fn command_request(body: &Value) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("POST")
        .uri("/v1/commands")
        .header("authorization", format!("Bearer {WRITER_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
}

async fn response_json(response: axum::response::Response) -> Result<Value, axum::Error> {
    let bytes = response.into_body().collect().await?.to_bytes();
    serde_json::from_slice(&bytes).map_err(axum::Error::new)
}

#[tokio::test]
#[ignore = "requires disposable TLS PostgreSQL via AIDEMEMO_POSTGRES_TLS_URL"]
async fn tls_postgres_profile_serves_authenticated_canonical_http()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_TLS_URL")?;
    let ca = fs::read(std::env::var("AIDEMEMO_POSTGRES_TLS_CA")?)?;
    let bootstrap_url = url.clone();
    let bootstrap_ca = ca.clone();
    tokio::task::spawn_blocking(move || bootstrap_postgres(&bootstrap_url, &bootstrap_ca))
        .await??;

    let state = ServerState::postgres_tls(
        url,
        Some(ca),
        2,
        Duration::from_millis(250),
        Duration::from_secs(2),
        Duration::from_millis(1_500),
        Duration::from_millis(250),
    )
    .await?;
    let app = router(state);

    let health = app
        .clone()
        .oneshot(Request::builder().uri("/health").body(Body::empty())?)
        .await?;
    assert_eq!(health.status(), 200);
    assert_eq!(response_json(health).await?["schema_version"], 2);

    let identity = app
        .clone()
        .oneshot(get_request("/v1/projects/project_pg_tls_http/identity")?)
        .await?;
    assert_eq!(identity.status(), 200);
    assert_eq!(
        response_json(identity).await?["actor_id"],
        "writer_pg_tls_http"
    );

    let command = json!({
        "command_id": "command_pg_tls_http",
        "project_id": "project_pg_tls_http",
        "expected_revision": null,
        "operation": "resource.put",
        "payload": {"content": "persisted through verified PostgreSQL TLS"},
        "resource": {"kind": "custom.note", "id": "note_pg_tls_http"},
        "change": "upsert"
    });
    let first = app.clone().oneshot(command_request(&command)?).await?;
    assert_eq!(first.status(), 200);
    let receipt = response_json(first).await?;
    let replay = app.clone().oneshot(command_request(&command)?).await?;
    assert_eq!(replay.status(), 200);
    assert_eq!(response_json(replay).await?, receipt);

    let resource = app
        .oneshot(get_request(
            "/v1/projects/project_pg_tls_http/resources/custom.note/note_pg_tls_http",
        )?)
        .await?;
    assert_eq!(resource.status(), 200);
    Ok(())
}
