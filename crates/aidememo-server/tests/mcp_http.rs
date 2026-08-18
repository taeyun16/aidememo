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

const WRITER_TOKEN: &str = "mcp-writer-token-0123456789";
const READER_TOKEN: &str = "mcp-reader-token-0123456789";
const MCP_VERSION: &str = "2026-07-28";

fn test_app() -> Result<Router, Box<dyn std::error::Error>> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_mcp")?,
        display_name: "MCP tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_mcp")?,
        display_name: "MCP project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_mcp")?,
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

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MCP_VERSION,
        "io.modelcontextprotocol/clientInfo": {"name": "aidememo-test", "version": "1.0"},
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

fn mcp_request(
    method: &str,
    name: Option<&str>,
    token: Option<&str>,
    params: Value,
) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/projects/project_mcp/mcp")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", MCP_VERSION)
        .header("mcp-method", method);
    if let Some(name) = name {
        builder = builder.header("mcp-name", name);
    }
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string(),
    ))
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

async fn seed_memory(app: &Router) -> Result<u64, Box<dyn std::error::Error>> {
    let session = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_mcp/sessions",
            WRITER_TOKEN,
            json!({
                "command_id": "mcp-seed-session",
                "payload": {
                    "session_id": "mcp-session",
                    "source_id": "alpha",
                    "topic": "MCP memory"
                }
            }),
        )?)
        .await?;
    assert_eq!(session.status(), 200);
    let fact = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_mcp/facts",
            WRITER_TOKEN,
            json!({
                "command_id": "mcp-seed-fact",
                "payload": {
                    "fact_id": "mcp-fact",
                    "session_id": "mcp-session",
                    "content": "redis cluster timeout decision"
                }
            }),
        )?)
        .await?;
    assert_eq!(fact.status(), 200);
    Ok(response_json(fact).await?["project_seq"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("fact receipt omitted project_seq"))?)
}

#[tokio::test]
async fn discover_and_tool_lists_are_authenticated_stateless_and_role_scoped()
-> Result<(), Box<dyn std::error::Error>> {
    let app = test_app()?;
    let missing_auth = app
        .clone()
        .oneshot(mcp_request(
            "server/discover",
            None,
            None,
            json!({"_meta": meta()}),
        )?)
        .await?;
    assert_eq!(missing_auth.status(), 401);

    let discover = app
        .clone()
        .oneshot(mcp_request(
            "server/discover",
            None,
            Some(READER_TOKEN),
            json!({"_meta": meta()}),
        )?)
        .await?;
    assert_eq!(discover.status(), 200);
    let discover = response_json(discover).await?;
    assert_eq!(discover["result"]["resultType"], "complete");
    assert_eq!(discover["result"]["supportedVersions"][0], MCP_VERSION);
    assert_eq!(
        discover["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "aidememo-server"
    );

    let reader_list = app
        .clone()
        .oneshot(mcp_request(
            "tools/list",
            None,
            Some(READER_TOKEN),
            json!({"_meta": meta()}),
        )?)
        .await?;
    assert_eq!(reader_list.status(), 200);
    let reader_list = response_json(reader_list).await?;
    assert_eq!(reader_list["result"]["ttlMs"], 60_000);
    assert_eq!(reader_list["result"]["cacheScope"], "private");
    let reader_names = reader_list["result"]["tools"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("reader tools missing"))?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        reader_names,
        vec!["search", "resource_get", "handoff_list", "handoff_status"]
    );
    let reader_schema = reader_list["result"]["tools"].to_string();
    assert!(!reader_schema.contains("tenant_id"));
    assert!(!reader_schema.contains("project_id"));
    assert!(!reader_schema.contains("actor_id"));

    let writer_list = app
        .oneshot(mcp_request(
            "tools/list",
            None,
            Some(WRITER_TOKEN),
            json!({"_meta": meta()}),
        )?)
        .await?;
    let writer_list = response_json(writer_list).await?;
    let writer_names = writer_list["result"]["tools"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("writer tools missing"))?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(writer_names.contains(&"session_create"));
    assert!(writer_names.contains(&"handoff_return"));
    Ok(())
}

#[tokio::test]
async fn search_matches_rest_and_reader_writes_fail_inside_tool_result()
-> Result<(), Box<dyn std::error::Error>> {
    let app = test_app()?;
    let head = seed_memory(&app).await?;
    let rest = app
        .clone()
        .oneshot(get_request(
            &format!(
                "/v1/projects/project_mcp/search?q=redis&source_id=alpha&at_least_seq={head}"
            ),
            READER_TOKEN,
        )?)
        .await?;
    assert_eq!(rest.status(), 200);
    let rest = response_json(rest).await?;

    let mcp = app
        .clone()
        .oneshot(mcp_request(
            "tools/call",
            Some("search"),
            Some(READER_TOKEN),
            json!({
                "name": "search",
                "arguments": {"q": "redis", "source_id": "alpha", "at_least_seq": head},
                "_meta": meta()
            }),
        )?)
        .await?;
    assert_eq!(mcp.status(), 200);
    let mcp = response_json(mcp).await?;
    assert_eq!(mcp["result"]["resultType"], "complete");
    assert_eq!(mcp["result"]["isError"], false);
    assert_eq!(mcp["result"]["structuredContent"], rest);

    let denied = app
        .oneshot(mcp_request(
            "tools/call",
            Some("session_create"),
            Some(READER_TOKEN),
            json!({
                "name": "session_create",
                "arguments": {
                    "operation_id": "reader-write",
                    "session_id": "reader-session",
                    "topic": "must fail"
                },
                "_meta": meta()
            }),
        )?)
        .await?;
    assert_eq!(denied.status(), 200);
    let denied = response_json(denied).await?;
    assert_eq!(denied["result"]["isError"], true);
    assert_eq!(
        denied["result"]["structuredContent"]["error"]["code"],
        "project_unauthorized"
    );
    Ok(())
}

#[tokio::test]
async fn header_mismatch_and_identity_widening_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let app = test_app()?;
    let mut mismatch = mcp_request(
        "tools/call",
        Some("resource_get"),
        Some(READER_TOKEN),
        json!({
            "name": "search",
            "arguments": {"q": "redis"},
            "_meta": meta()
        }),
    )?;
    mismatch
        .headers_mut()
        .insert("mcp-name", "resource_get".parse()?);
    let mismatch = app.clone().oneshot(mismatch).await?;
    assert_eq!(mismatch.status(), 400);
    assert_eq!(response_json(mismatch).await?["error"]["code"], -32020);

    let widening = app
        .clone()
        .oneshot(mcp_request(
            "tools/call",
            Some("handoff_list"),
            Some(READER_TOKEN),
            json!({
                "name": "handoff_list",
                "arguments": {"box": "inbox", "actor_id": "writer"},
                "_meta": meta()
            }),
        )?)
        .await?;
    assert_eq!(widening.status(), 400);
    assert_eq!(response_json(widening).await?["error"]["code"], -32602);

    let missing_method = Request::builder()
        .method("POST")
        .uri("/v1/projects/project_mcp/mcp")
        .header("authorization", format!("Bearer {READER_TOKEN}"))
        .header("content-type", "application/json")
        .header("mcp-protocol-version", MCP_VERSION)
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {"_meta": meta()}
            })
            .to_string(),
        ))?;
    let missing_method = app.oneshot(missing_method).await?;
    assert_eq!(missing_method.status(), 400);
    assert_eq!(response_json(missing_method).await?["error"]["code"], -32020);
    Ok(())
}
