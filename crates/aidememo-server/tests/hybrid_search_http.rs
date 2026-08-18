#![cfg(feature = "semantic")]

use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, DomainError, MembershipRole, MembershipStatus, ProjectEpoch,
    ProjectId, ProjectMembership, ProjectRecord, RecordStatus, Revision, TenantId, TenantRecord,
};
use aidememo_server::{EmbeddingProvider, ServerState, bearer_token_digest, router};
use aidememo_store_local::SqliteCommandStore;
use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "hybrid-reader-token-0123456789";

struct FakeProvider;
struct FailingProvider;

impl EmbeddingProvider for FakeProvider {
    fn name(&self) -> String {
        "fake-hybrid-v1".to_owned()
    }

    fn dimension(&self) -> usize {
        3
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, DomainError> {
        Ok(if text.contains("database") || text.contains("postgres") {
            vec![1.0, 0.0, 0.0]
        } else {
            vec![0.0, 1.0, 0.0]
        })
    }

    fn embed_document_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, DomainError> {
        Ok(texts
            .iter()
            .map(|text| {
                if text.contains("postgres") || text.contains("mysql") {
                    vec![1.0, 0.0, 0.0]
                } else {
                    vec![0.0, 1.0, 0.0]
                }
            })
            .collect())
    }
}

impl EmbeddingProvider for FailingProvider {
    fn name(&self) -> String {
        "failing-hybrid-v1".to_owned()
    }

    fn dimension(&self) -> usize {
        3
    }

    fn embed_query(&self, _text: &str) -> Result<Vec<f32>, DomainError> {
        Err(DomainError::StorageFailure {
            operation: "test_embedding_query",
            detail: "provider unavailable".to_owned(),
        })
    }

    fn embed_document_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, DomainError> {
        Err(DomainError::StorageFailure {
            operation: "test_embedding_documents",
            detail: "provider unavailable".to_owned(),
        })
    }
}

fn store() -> Result<SqliteCommandStore, Box<dyn std::error::Error>> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_hybrid")?,
        display_name: "Hybrid tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_hybrid")?,
        display_name: "Hybrid project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_hybrid")?,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let actor = ActorRecord {
        tenant_id: tenant.tenant_id.clone(),
        actor_id: ActorId::try_from("reader")?,
        display_name: "reader".to_owned(),
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
    let mut store = SqliteCommandStore::open_in_memory()?;
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(&actor, &membership, &bearer_token_digest(TOKEN)?, timestamp)?;
    Ok(store)
}

fn app(with_semantic: bool) -> Result<Router, Box<dyn std::error::Error>> {
    let provider = with_semantic.then(|| Arc::new(FakeProvider) as Arc<dyn EmbeddingProvider>);
    app_with_provider(provider)
}

fn app_with_provider(
    provider: Option<Arc<dyn EmbeddingProvider>>,
) -> Result<Router, Box<dyn std::error::Error>> {
    let state = ServerState::new(store()?);
    let state = match provider {
        Some(provider) => state.with_semantic_provider(provider),
        None => state,
    };
    Ok(router(state))
}

fn post_request(uri: &str, body: Value) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
}

fn get_request(uri: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
}

async fn response_json(response: axum::response::Response) -> Result<Value, axum::Error> {
    let bytes = response.into_body().collect().await?.to_bytes();
    serde_json::from_slice(&bytes).map_err(axum::Error::new)
}

async fn create_session(app: &Router) -> Result<(), Box<dyn std::error::Error>> {
    let response = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_hybrid/sessions",
            json!({
                "command_id": "hybrid-session",
                "payload": {
                    "session_id": "hybrid-session",
                    "source_id": "alpha",
                    "topic": "Hybrid retrieval"
                }
            }),
        )?)
        .await?;
    assert_eq!(response.status(), 200);
    Ok(())
}

async fn create_fact(
    app: &Router,
    command_id: &str,
    fact_id: &str,
    content: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let response = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_hybrid/facts",
            json!({
                "command_id": command_id,
                "payload": {
                    "fact_id": fact_id,
                    "session_id": "hybrid-session",
                    "content": content
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
async fn auto_hybrid_finds_semantic_match_and_rebuilds_to_new_head()
-> Result<(), Box<dyn std::error::Error>> {
    let app = app(true)?;
    create_session(&app).await?;
    create_fact(
        &app,
        "hybrid-fact-db",
        "fact-db",
        "postgres connection pool exhausted",
    )
    .await?;
    let first_head = create_fact(
        &app,
        "hybrid-fact-cache",
        "fact-cache",
        "redis cache fallback",
    )
    .await?;

    let first = app
        .clone()
        .oneshot(get_request(&format!(
            "/v1/projects/project_hybrid/search?q=database%20saturation&mode=auto&at_least_seq={first_head}"
        ))?)
        .await?;
    assert_eq!(first.status(), 200);
    let first = response_json(first).await?;
    assert_eq!(first["mode"], "hybrid");
    assert_eq!(first["semantic_model"], "fake-hybrid-v1");
    assert_eq!(first["index_seq"], first_head);
    assert_eq!(first["results"][0]["fact_id"], "fact-db");
    assert!(first["results"][0]["semantic_score"].is_number());

    let new_head = create_fact(
        &app,
        "hybrid-fact-mysql",
        "fact-mysql",
        "mysql database proxy saturation",
    )
    .await?;
    let second = app
        .clone()
        .oneshot(get_request(&format!(
            "/v1/projects/project_hybrid/search?q=database%20saturation&mode=semantic&at_least_seq={new_head}"
        ))?)
        .await?;
    assert_eq!(second.status(), 200);
    let second = response_json(second).await?;
    assert_eq!(second["mode"], "semantic");
    assert_eq!(second["index_seq"], new_head);
    assert_eq!(second["results"][0]["fact_id"], "fact-db");
    Ok(())
}

#[tokio::test]
async fn missing_semantic_provider_falls_back_only_for_auto()
-> Result<(), Box<dyn std::error::Error>> {
    let app = app(false)?;
    create_session(&app).await?;
    create_fact(
        &app,
        "hybrid-lexical-fact",
        "fact-lexical",
        "redis cache fallback",
    )
    .await?;

    let automatic = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_hybrid/search?q=redis&mode=auto",
        )?)
        .await?;
    assert_eq!(automatic.status(), 200);
    assert_eq!(response_json(automatic).await?["mode"], "lexical");

    let explicit = app
        .oneshot(get_request(
            "/v1/projects/project_hybrid/search?q=redis&mode=hybrid",
        )?)
        .await?;
    assert_eq!(explicit.status(), 400);
    assert_eq!(
        response_json(explicit).await?["error"]["code"],
        "invalid_command"
    );
    Ok(())
}

#[tokio::test]
async fn auto_degrades_to_lexical_when_configured_provider_is_unavailable()
-> Result<(), Box<dyn std::error::Error>> {
    let app = app_with_provider(Some(Arc::new(FailingProvider)))?;
    create_session(&app).await?;
    create_fact(
        &app,
        "hybrid-failing-provider-fact",
        "fact-lexical-fallback",
        "redis cache fallback",
    )
    .await?;

    let automatic = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_hybrid/search?q=unseen&mode=auto",
        )?)
        .await?;
    assert_eq!(automatic.status(), 200);
    assert_eq!(response_json(automatic).await?["mode"], "lexical");

    let explicit = app
        .oneshot(get_request(
            "/v1/projects/project_hybrid/search?q=unseen&mode=semantic",
        )?)
        .await?;
    assert_eq!(explicit.status(), 500);
    Ok(())
}
