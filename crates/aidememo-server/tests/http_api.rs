use aidememo_artifacts::LocalArtifactStore;
#[cfg(feature = "s3")]
use aidememo_artifacts::{S3BodyStore, S3BodyStoreConfig};
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

#[cfg(feature = "s3")]
use std::sync::Arc;
#[cfg(feature = "s3")]
use tokio::sync::Mutex;

const WRITER_TOKEN: &str = "writer-token-0123456789";
const RECEIVER_TOKEN: &str = "receiver-token-0123456789";
const HERMES_TOKEN: &str = "hermes-token-0123456789";
const READER_TOKEN: &str = "reader-token-0123456789";

fn test_store() -> Result<(SqliteCommandStore, ProjectEpoch), Box<dyn std::error::Error>> {
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
    Ok((store, epoch))
}

fn test_app() -> Result<(Router, ProjectEpoch), Box<dyn std::error::Error>> {
    let (store, epoch) = test_store()?;
    Ok((router(ServerState::new(store)), epoch))
}

fn artifact_test_app()
-> Result<(Router, ProjectEpoch, tempfile::TempDir, ServerState), Box<dyn std::error::Error>> {
    let (store, epoch) = test_store()?;
    let directory = tempfile::tempdir()?;
    let artifacts = LocalArtifactStore::open(directory.path())?;
    let state = ServerState::with_artifacts(store, artifacts)?;
    Ok((router(state.clone()), epoch, directory, state))
}

#[cfg(feature = "s3")]
#[derive(Clone, Default)]
struct S3MockState {
    requests: Arc<Mutex<Vec<(axum::http::Method, String)>>>,
}

#[cfg(feature = "s3")]
async fn s3_mock(
    axum::extract::State(state): axum::extract::State<S3MockState>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::{
        http::{HeaderMap, HeaderValue, Method, StatusCode, header},
        response::IntoResponse,
    };

    let method = request.method().clone();
    let uri = request.uri().to_string();
    state.requests.lock().await.push((method.clone(), uri));
    match method {
        Method::HEAD => {
            let generation = request
                .uri()
                .path()
                .rsplit('/')
                .next()
                .and_then(|name| name.strip_suffix(".blob"))
                .unwrap_or("unknown");
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("12"));
            headers.insert(header::ETAG, HeaderValue::from_static("\"etag-server\""));
            if let Ok(value) = HeaderValue::from_str(generation) {
                headers.insert("x-amz-meta-aidememo-generation", value);
            }
            headers.insert(
                "x-amz-version-id",
                HeaderValue::from_static("version-server"),
            );
            (StatusCode::OK, headers, Body::empty()).into_response()
        }
        Method::DELETE => StatusCode::NO_CONTENT.into_response(),
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

#[cfg(feature = "s3")]
fn s3_artifact_test_app(
    endpoint: &str,
) -> Result<(Router, ServerState, tempfile::TempDir), Box<dyn std::error::Error>> {
    use aws_credential_types::Credentials;
    use aws_sdk_s3::config::Region;

    let (store, _) = test_store()?;
    let directory = tempfile::tempdir()?;
    let catalog = LocalArtifactStore::open(directory.path())?;
    let config = S3BodyStoreConfig::new(
        "artifacts",
        "aidememo/v1",
        "us-east-1",
        Some(endpoint.to_owned()),
        true,
    )?;
    let sdk = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .credentials_provider(Credentials::for_tests())
        .region(Region::new("us-east-1"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .build();
    let bodies = S3BodyStore::from_client(aws_sdk_s3::Client::from_conf(sdk), config);
    let state = ServerState::with_s3_artifacts(store, catalog, bodies)?;
    Ok((router(state.clone()), state, directory))
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

fn put_bytes_request(
    uri: &str,
    token: Option<&str>,
    bytes: &'static [u8],
) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/octet-stream");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(bytes))
}

fn delete_request(uri: &str, token: Option<&str>) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::builder().method("DELETE").uri(uri);
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
async fn authenticated_artifact_round_trip_retries_and_garbage_collection()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, _, artifact_directory, state) = artifact_test_app()?;
    let reserve_body = json!({
        "request_id": "artifact_reserve_http",
        "path": "/sessions/http/result.bin",
        "expected_mutation_token": null,
        "ttl_ms": 60_000
    });
    let reader_reserve = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/artifact-reservations",
            Some(READER_TOKEN),
            reserve_body.clone(),
        )?)
        .await?;
    assert_eq!(reader_reserve.status(), 403);

    let reserved = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/artifact-reservations",
            Some(WRITER_TOKEN),
            reserve_body.clone(),
        )?)
        .await?;
    assert_eq!(reserved.status(), 200);
    let reservation = response_json(reserved).await?;
    let reservation_token = reservation["mutation_token"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing reservation token"))?
        .to_owned();
    let artifact_id = reservation["artifact_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing artifact id"))?
        .to_owned();

    let replay = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/artifact-reservations",
            Some(WRITER_TOKEN),
            reserve_body,
        )?)
        .await?;
    assert_eq!(response_json(replay).await?, reservation);
    let changed_reuse = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/artifact-reservations",
            Some(WRITER_TOKEN),
            json!({
                "request_id": "artifact_reserve_http",
                "path": "/sessions/http/changed.bin",
                "expected_mutation_token": null,
                "ttl_ms": 60_000
            }),
        )?)
        .await?;
    assert_eq!(changed_reuse.status(), 409);

    let upload_uri =
        format!("/v1/projects/project_http/artifact-reservations/{reservation_token}/body");
    let reader_upload = app
        .clone()
        .oneshot(put_bytes_request(
            &upload_uri,
            Some(READER_TOKEN),
            b"artifact-v1",
        )?)
        .await?;
    assert_eq!(reader_upload.status(), 403);
    let uploaded = app
        .clone()
        .oneshot(put_bytes_request(
            &upload_uri,
            Some(WRITER_TOKEN),
            b"artifact-v1",
        )?)
        .await?;
    assert_eq!(uploaded.status(), 200);
    let observation = response_json(uploaded).await?;
    let first_object_key = observation["object_key"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing object key"))?
        .to_owned();
    let first_object_path = artifact_directory.path().join(&first_object_key);
    assert!(first_object_path.exists());

    let publish_uri =
        format!("/v1/projects/project_http/artifact-reservations/{reservation_token}/publish");
    let published = app
        .clone()
        .oneshot(post_request(&publish_uri, Some(WRITER_TOKEN), json!({}))?)
        .await?;
    assert_eq!(published.status(), 200);
    let published = response_json(published).await?;
    assert_eq!(published["revision"], 1);
    let publish_replay = app
        .clone()
        .oneshot(post_request(&publish_uri, Some(WRITER_TOKEN), json!({}))?)
        .await?;
    assert_eq!(response_json(publish_replay).await?, published);

    let resolved = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/artifacts/resolve?path=%2Fsessions%2Fhttp%2Fresult.bin",
            Some(READER_TOKEN),
        )?)
        .await?;
    assert_eq!(resolved.status(), 200);
    assert_eq!(response_json(resolved).await?, published);

    let download_uri = format!("/v1/projects/project_http/artifacts/{artifact_id}/downloads");
    let stale_download = app
        .clone()
        .oneshot(post_request(
            &download_uri,
            Some(READER_TOKEN),
            json!({"revision": 2}),
        )?)
        .await?;
    assert_eq!(stale_download.status(), 409);
    let downloaded = app
        .clone()
        .oneshot(post_request(
            &download_uri,
            Some(READER_TOKEN),
            json!({"revision": 1}),
        )?)
        .await?;
    assert_eq!(downloaded.status(), 200);
    assert!(downloaded.headers().contains_key("etag"));
    assert_eq!(
        downloaded.into_body().collect().await?.to_bytes(),
        &b"artifact-v1"[..]
    );

    let replacement = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/artifact-reservations",
            Some(WRITER_TOKEN),
            json!({
                "request_id": "artifact_replace_http",
                "path": "/sessions/http/result.bin",
                "expected_mutation_token": published["mutation_token"],
                "ttl_ms": 60_000
            }),
        )?)
        .await?;
    let replacement = response_json(replacement).await?;
    let replacement_token = replacement["mutation_token"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing replacement token"))?;
    let replacement_upload_uri =
        format!("/v1/projects/project_http/artifact-reservations/{replacement_token}/body");
    let replacement_upload = app
        .clone()
        .oneshot(put_bytes_request(
            &replacement_upload_uri,
            Some(WRITER_TOKEN),
            b"artifact-v2",
        )?)
        .await?;
    assert_eq!(replacement_upload.status(), 200);
    let replacement_publish_uri =
        format!("/v1/projects/project_http/artifact-reservations/{replacement_token}/publish");
    let replacement_published = app
        .clone()
        .oneshot(post_request(
            &replacement_publish_uri,
            Some(WRITER_TOKEN),
            json!({}),
        )?)
        .await?;
    assert_eq!(response_json(replacement_published).await?["revision"], 2);
    let replacement_gc_at = replacement["expires_at_ms"]
        .as_i64()
        .ok_or_else(|| std::io::Error::other("missing replacement expiry"))?
        .saturating_sub(59_000);
    let replacement_gc = state.drain_artifact_garbage(replacement_gc_at, 100).await?;
    assert_eq!(replacement_gc.map(|report| report.deleted), Some(1));
    assert!(!first_object_path.exists());
    let late_publish_replay = app
        .clone()
        .oneshot(post_request(&publish_uri, Some(WRITER_TOKEN), json!({}))?)
        .await?;
    assert_eq!(response_json(late_publish_replay).await?, published);

    let aborted = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/artifact-reservations",
            Some(WRITER_TOKEN),
            json!({
                "request_id": "artifact_abort_http",
                "path": "/sessions/http/aborted.bin",
                "expected_mutation_token": null,
                "ttl_ms": 60_000
            }),
        )?)
        .await?;
    let aborted = response_json(aborted).await?;
    let aborted_token = aborted["mutation_token"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing aborted token"))?;
    let aborted_upload_uri =
        format!("/v1/projects/project_http/artifact-reservations/{aborted_token}/body");
    let aborted_upload = app
        .clone()
        .oneshot(put_bytes_request(
            &aborted_upload_uri,
            Some(WRITER_TOKEN),
            b"aborted-body",
        )?)
        .await?;
    let aborted_observation = response_json(aborted_upload).await?;
    let aborted_object = artifact_directory.path().join(
        aborted_observation["object_key"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("missing aborted object key"))?,
    );
    let aborted_uri = format!("/v1/projects/project_http/artifact-reservations/{aborted_token}");
    let aborted_response = app
        .oneshot(delete_request(&aborted_uri, Some(WRITER_TOKEN))?)
        .await?;
    assert_eq!(aborted_response.status(), 204);
    let delete_after_ms = aborted["expires_at_ms"]
        .as_i64()
        .ok_or_else(|| std::io::Error::other("missing reservation expiry"))?
        .saturating_add(60_000);
    let aborted_gc = state.drain_artifact_garbage(delete_after_ms, 100).await?;
    assert_eq!(aborted_gc.map(|report| report.deleted), Some(1));
    assert!(!aborted_object.exists());
    Ok(())
}

#[cfg(feature = "s3")]
#[tokio::test]
async fn authenticated_s3_grants_publication_retention_and_gc_are_connected()
-> Result<(), Box<dyn std::error::Error>> {
    let mock_state = S3MockState::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let mock = axum::Router::new()
        .route("/{*path}", axum::routing::any(s3_mock))
        .with_state(mock_state.clone());
    let mock_server = tokio::spawn(async move { axum::serve(listener, mock).await });
    let (app, state, _directory) = s3_artifact_test_app(&format!("http://{address}"))?;

    let reserved = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/artifact-reservations",
            Some(WRITER_TOKEN),
            json!({
                "request_id": "artifact_s3_server_v1",
                "path": "/sessions/http/hosted.bin",
                "expected_mutation_token": null,
                "ttl_ms": 120_000
            }),
        )?)
        .await?;
    assert_eq!(reserved.status(), 200);
    let reserved = response_json(reserved).await?;
    let token = reserved["mutation_token"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing hosted reservation token"))?;
    let artifact_id = reserved["artifact_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing hosted artifact id"))?;

    let upload_grant_uri =
        format!("/v1/projects/project_http/artifact-reservations/{token}/upload-grants");
    let reader_grant = app
        .clone()
        .oneshot(post_request(
            &upload_grant_uri,
            Some(READER_TOKEN),
            json!({
                "content_length": 12,
                "content_type": "application/octet-stream",
                "ttl_ms": 30_000
            }),
        )?)
        .await?;
    assert_eq!(reader_grant.status(), 403);
    let upload_grant = app
        .clone()
        .oneshot(post_request(
            &upload_grant_uri,
            Some(WRITER_TOKEN),
            json!({
                "content_length": 12,
                "content_type": "application/octet-stream",
                "ttl_ms": 30_000
            }),
        )?)
        .await?;
    assert_eq!(upload_grant.status(), 200);
    let upload_grant = response_json(upload_grant).await?;
    assert_eq!(upload_grant["method"], "PUT");
    assert_eq!(upload_grant["headers"]["if-none-match"], "*");
    assert!(
        upload_grant["url"]
            .as_str()
            .is_some_and(|url| url.contains("X-Amz-Signature="))
    );

    let raw_upload = app
        .clone()
        .oneshot(put_bytes_request(
            &format!("/v1/projects/project_http/artifact-reservations/{token}/body"),
            Some(WRITER_TOKEN),
            b"artifact-s3",
        )?)
        .await?;
    assert_eq!(raw_upload.status(), 409);

    let publish_uri = format!("/v1/projects/project_http/artifact-reservations/{token}/publish");
    let published = app
        .clone()
        .oneshot(post_request(
            &publish_uri,
            Some(WRITER_TOKEN),
            json!({"size_bytes": 12}),
        )?)
        .await?;
    assert_eq!(published.status(), 200);
    let published = response_json(published).await?;
    assert_eq!(published["revision"], 1);
    assert!(published["body"]["digest"].is_null());

    let raw_download = app
        .clone()
        .oneshot(post_request(
            &format!("/v1/projects/project_http/artifacts/{artifact_id}/downloads"),
            Some(READER_TOKEN),
            json!({"revision": 1}),
        )?)
        .await?;
    assert_eq!(raw_download.status(), 409);
    let download_grant = app
        .clone()
        .oneshot(post_request(
            &format!("/v1/projects/project_http/artifacts/{artifact_id}/download-grants"),
            Some(READER_TOKEN),
            json!({"revision": 1, "ttl_ms": 1_000}),
        )?)
        .await?;
    assert_eq!(download_grant.status(), 200);
    let download_grant = response_json(download_grant).await?;
    assert_eq!(download_grant["method"], "GET");
    assert_eq!(download_grant["headers"]["if-match"], "etag-server");
    let retain_until_ms = download_grant["expires_at_ms"]
        .as_i64()
        .ok_or_else(|| std::io::Error::other("missing download grant expiry"))?
        .saturating_add(60_000);

    let replacement = app
        .clone()
        .oneshot(post_request(
            "/v1/projects/project_http/artifact-reservations",
            Some(WRITER_TOKEN),
            json!({
                "request_id": "artifact_s3_server_v2",
                "path": "/sessions/http/hosted.bin",
                "expected_mutation_token": published["mutation_token"],
                "ttl_ms": 120_000
            }),
        )?)
        .await?;
    let replacement = response_json(replacement).await?;
    let replacement_token = replacement["mutation_token"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing replacement token"))?;
    let replaced = app
        .oneshot(post_request(
            &format!("/v1/projects/project_http/artifact-reservations/{replacement_token}/publish"),
            Some(WRITER_TOKEN),
            json!({"size_bytes": 12}),
        )?)
        .await?;
    assert_eq!(response_json(replaced).await?["revision"], 2);

    let retained = state
        .drain_artifact_garbage(retain_until_ms.saturating_sub(1), 100)
        .await?
        .ok_or_else(|| std::io::Error::other("missing hosted GC report"))?;
    assert_eq!(retained.claimed, 0);
    let collected = state
        .drain_artifact_garbage(retain_until_ms, 100)
        .await?
        .ok_or_else(|| std::io::Error::other("missing hosted GC report"))?;
    assert_eq!(collected.deleted, 1);
    assert_eq!(collected.pruned_read_retentions, 1);
    assert!(
        mock_state
            .requests
            .lock()
            .await
            .iter()
            .any(|(method, _)| method == axum::http::Method::DELETE)
    );

    mock_server.abort();
    let _ = mock_server.await;
    Ok(())
}

#[tokio::test]
async fn codex_profiles_and_hermes_complete_typed_remote_handoffs()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, epoch) = test_app()?;
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

    let hidden_generic = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/resources/handoff/handoff_p2",
            Some(READER_TOKEN),
        )?)
        .await?;
    assert_eq!(hidden_generic.status(), 404);

    let reader_snapshot = app
        .clone()
        .oneshot(get_request(
            "/v1/projects/project_http/snapshot",
            Some(READER_TOKEN),
        )?)
        .await?;
    assert_eq!(reader_snapshot.status(), 200);
    let reader_snapshot = response_json(reader_snapshot).await?;
    let canonical_head = reader_snapshot["at_seq"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("reader snapshot omitted head"))?;
    assert!(
        reader_snapshot["resources"]
            .as_array()
            .is_some_and(|resources| resources
                .iter()
                .all(|resource| { resource["resource"]["kind"].as_str() != Some("handoff") }))
    );

    for path in ["changes", "changes/materialized"] {
        let hidden_only = app
            .clone()
            .oneshot(get_request(
                &format!(
                    "/v1/projects/project_http/{path}?project_epoch={epoch}&after_seq=1&limit=1"
                ),
                Some(READER_TOKEN),
            )?)
            .await?;
        assert_eq!(hidden_only.status(), 200);
        let hidden_only = response_json(hidden_only).await?;
        assert!(hidden_only["entries"].as_array().is_some_and(Vec::is_empty));
        assert_eq!(hidden_only["next_cursor"]["after_seq"], 2);
        assert_eq!(hidden_only["has_more"], true);
    }

    for path in ["changes", "changes/materialized"] {
        let response = app
            .clone()
            .oneshot(get_request(
                &format!(
                    "/v1/projects/project_http/{path}?project_epoch={epoch}&after_seq=0&limit=100"
                ),
                Some(READER_TOKEN),
            )?)
            .await?;
        assert_eq!(response.status(), 200);
        let response = response_json(response).await?;
        assert_eq!(response["next_cursor"]["after_seq"], canonical_head);
        assert!(response["entries"].as_array().is_some_and(|entries| {
            entries.iter().all(|entry| {
                let change = entry.get("change").unwrap_or(entry);
                change["resource"]["kind"].as_str() != Some("handoff")
            })
        }));
    }

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
    assert_eq!(
        replica.status()?.actor_id,
        Some(ActorId::try_from("reader_actor")?)
    );

    let other_actor = HttpReplicaClient::new(RemoteProfile::new(
        format!("http://{address}"),
        ProjectId::try_from("project_http")?,
        WRITER_TOKEN,
    )?);
    assert!(matches!(
        pull_to_current(&other_actor, &mut replica, 1),
        Err(aidememo_client::ClientError::ActorMismatch { .. })
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
