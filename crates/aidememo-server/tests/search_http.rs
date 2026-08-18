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

const WRITER_TOKEN: &str = "search-writer-token-0123456789";
const READER_TOKEN: &str = "search-reader-token-0123456789";

fn test_app() -> Result<Router, Box<dyn std::error::Error>> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_search")?,
        display_name: "Search tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_search")?,
        display_name: "Search project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_search")?,
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
        "writer",
        MembershipRole::Writer,
        WRITER_TOKEN,
        timestamp,
    )?;
    provision(
        &mut store,
        &tenant,
        &project,
        "reader",
        MembershipRole::Reader,
        READER_TOKEN,
        timestamp,
    )?;
    Ok(router(ServerState::new(store)))
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

fn post_request(uri: &str, token: &str, body: Value) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
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

async fn create_session(
    app: &Router,
    command_id: &str,
    session_id: &str,
    source_id: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let response = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_search/sessions",
            WRITER_TOKEN,
            json!({
                "command_id": command_id,
                "payload": {
                    "session_id": session_id,
                    "source_id": source_id,
                    "topic": format!("topic-{session_id}"),
                }
            }),
        )?)
        .await?;
    assert_eq!(response.status(), 200);
    Ok(response_json(response).await?["project_seq"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("session receipt omitted project_seq"))?)
}

async fn create_fact(
    app: &Router,
    command_id: &str,
    fact_id: &str,
    session_id: &str,
    content: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let response = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_search/facts",
            WRITER_TOKEN,
            json!({
                "command_id": command_id,
                "payload": {
                    "fact_id": fact_id,
                    "session_id": session_id,
                    "content": content,
                }
            }),
        )?)
        .await?;
    assert_eq!(response.status(), 200);
    Ok(response_json(response).await?["project_seq"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("fact receipt omitted project_seq"))?)
}

#[tokio::test]
async fn lexical_search_is_authenticated_source_scoped_and_sequence_explicit()
-> Result<(), Box<dyn std::error::Error>> {
    let app = test_app()?;

    let unauthenticated = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_search/search?q=redis",
            None,
        )?)
        .await?;
    assert_eq!(unauthenticated.status(), 401);

    create_session(&app, "cmd-session-alpha", "session-alpha", "alpha").await?;
    let alpha_seq = create_fact(
        &app,
        "cmd-fact-alpha",
        "fact-alpha",
        "session-alpha",
        "redis cluster timeout decision",
    )
    .await?;
    create_session(&app, "cmd-session-beta", "session-beta", "beta").await?;
    let latest_seq = create_fact(
        &app,
        "cmd-fact-beta",
        "fact-beta",
        "session-beta",
        "redis cache fallback",
    )
    .await?;
    assert!(latest_seq > alpha_seq);

    let reader_search = app
        .clone()
        .oneshot(get_request(
            &format!(
                "/v1/projects/project_search/search?q=redis&source_id=alpha&at_least_seq={latest_seq}"
            ),
            Some(READER_TOKEN),
        )?)
        .await?;
    assert_eq!(reader_search.status(), 200);
    let reader_search = response_json(reader_search).await?;
    assert_eq!(reader_search["project_epoch"], "epoch_search");
    assert_eq!(reader_search["index_seq"], latest_seq);
    assert_eq!(reader_search["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(reader_search["results"][0]["fact_id"], "fact-alpha");
    assert_eq!(reader_search["results"][0]["source_id"], "alpha");

    let all_sources = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_search/search?q=redis&limit=10",
            Some(READER_TOKEN),
        )?)
        .await?;
    assert_eq!(all_sources.status(), 200);
    assert_eq!(
        response_json(all_sources).await?["results"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let future = app
        .clone()
        .oneshot(get_request(
            &format!(
                "/v1/projects/project_search/search?q=redis&at_least_seq={}",
                latest_seq + 1
            ),
            Some(READER_TOKEN),
        )?)
        .await?;
    assert_eq!(future.status(), 409);
    assert_eq!(
        response_json(future).await?["error"]["code"],
        "cursor_out_of_range"
    );

    Ok(())
}
