//! Authenticated single-node HTTP boundary for the future AideMemo SSOT mode.
//!
//! The server owns tenant and actor identity. Request bodies select only a
//! project and resource; bearer-token digests resolve to persisted actors, and
//! active membership is loaded from the same SQLite ledger before every read or
//! mutation. This crate does not modify or expose the existing embedded store.

mod artifact;
mod product;

use aidememo_artifacts::{
    ArtifactStoreError, GarbageCollectionReport, LOCAL_BODY_STORE_IDENTITY, LocalArtifactStore,
    MAX_DIRECT_UPLOAD_BYTES,
};
#[cfg(feature = "s3")]
use aidememo_artifacts::{GarbageClaim, S3BodyStore};
use aidememo_domain::{
    CanonicalResource, ChangeCursor, ChangeEntry, ChangeOperation, CommandEnvelope, CommandId,
    DomainError, ErrorCode, MaterializedChangeBatch, OperationName, ProjectEpoch, ProjectId,
    ProjectScope, ProjectSequence, ProjectSnapshot, ResourceId, ResourceKind, ResourceRef,
    ResourceState, Revision,
};
use aidememo_service::CommandService;
use aidememo_store_local::SqliteCommandStore;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;

const DEFAULT_CHANGE_LIMIT: usize = 100;
const MAX_COMMAND_BODY_BYTES: usize = 1024 * 1024;
const MAX_BEARER_BYTES: usize = 4096;
const EXTENSION_RESOURCE_PREFIX: &str = "custom.";

/// Cloneable application state around one single-node command service.
#[derive(Clone)]
pub struct ServerState {
    service: Arc<Mutex<CommandService<SqliteCommandStore>>>,
    artifacts: Option<Arc<ArtifactState>>,
}

pub(crate) struct ArtifactState {
    pub(crate) catalog: Mutex<LocalArtifactStore>,
    pub(crate) bodies: ArtifactBodies,
}

pub(crate) enum ArtifactBodies {
    Local,
    #[cfg(feature = "s3")]
    S3(S3BodyStore),
}

impl ServerState {
    /// Wrap an opened and migrated SQLite command store.
    #[must_use]
    pub fn new(store: SqliteCommandStore) -> Self {
        Self {
            service: Arc::new(Mutex::new(CommandService::new(store))),
            artifacts: None,
        }
    }

    /// Wrap the command ledger together with an isolated local artifact repository.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog is already bound to another body store.
    pub fn with_artifacts(
        store: SqliteCommandStore,
        mut artifacts: LocalArtifactStore,
    ) -> Result<Self, ArtifactStoreError> {
        artifacts.bind_body_store(LOCAL_BODY_STORE_IDENTITY)?;
        Ok(Self {
            service: Arc::new(Mutex::new(CommandService::new(store))),
            artifacts: Some(Arc::new(ArtifactState {
                catalog: Mutex::new(artifacts),
                bodies: ArtifactBodies::Local,
            })),
        })
    }

    /// Wrap the command ledger and artifact catalog with an S3-compatible body store.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog is already bound to another S3
    /// configuration or to the local body layout.
    #[cfg(feature = "s3")]
    pub fn with_s3_artifacts(
        store: SqliteCommandStore,
        mut catalog: LocalArtifactStore,
        bodies: S3BodyStore,
    ) -> Result<Self, ArtifactStoreError> {
        catalog.bind_body_store(&bodies.catalog_identity()?)?;
        Ok(Self {
            service: Arc::new(Mutex::new(CommandService::new(store))),
            artifacts: Some(Arc::new(ArtifactState {
                catalog: Mutex::new(catalog),
                bodies: ArtifactBodies::S3(bodies),
            })),
        })
    }

    /// Run one bounded artifact garbage-collection pass when artifacts are configured.
    ///
    /// # Errors
    ///
    /// Returns an artifact catalog or filesystem error from the durable drain.
    pub async fn drain_artifact_garbage(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Option<GarbageCollectionReport>, ArtifactStoreError> {
        let Some(artifacts) = &self.artifacts else {
            return Ok(None);
        };
        match &artifacts.bodies {
            ArtifactBodies::Local => {
                let mut catalog = artifacts.catalog.lock().await;
                catalog.drain_garbage(now_ms, limit).map(Some)
            }
            #[cfg(feature = "s3")]
            ArtifactBodies::S3(bodies) => {
                let GarbageClaim { mut report, leases } = {
                    let mut catalog = artifacts.catalog.lock().await;
                    catalog.claim_garbage(now_ms, limit)?
                };
                for lease in leases {
                    match bodies
                        .delete_generation(&lease.scope, &lease.generation)
                        .await
                    {
                        Ok(()) => {
                            let mut catalog = artifacts.catalog.lock().await;
                            catalog.acknowledge_garbage(&lease)?;
                            report.deleted += 1;
                        }
                        Err(error) => {
                            let mut catalog = artifacts.catalog.lock().await;
                            catalog.fail_garbage(&lease, now_ms, &error.to_string())?;
                            report.failed += 1;
                        }
                    }
                }
                Ok(Some(report))
            }
        }
    }
}

/// Build the authenticated server router.
pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/commands", post(command))
        .route("/v1/projects/{project_id}/changes", get(changes))
        .route(
            "/v1/projects/{project_id}/changes/materialized",
            get(materialized_changes),
        )
        .route("/v1/projects/{project_id}/snapshot", get(snapshot))
        .route(
            "/v1/projects/{project_id}/resources/{resource_kind}/{resource_id}",
            get(resource),
        )
        .merge(product::routes())
        .layer(DefaultBodyLimit::max(MAX_COMMAND_BODY_BYTES))
        .merge(artifact::routes().layer(DefaultBodyLimit::max(MAX_DIRECT_UPLOAD_BYTES)))
        .with_state(state)
}

/// Hash bearer-token plaintext for provisioning or authentication.
///
/// # Errors
///
/// Returns [`DomainError::AuthenticationFailed`] when the token is empty,
/// oversized, or contains whitespace or control characters.
pub fn bearer_token_digest(token: &str) -> Result<[u8; 32], DomainError> {
    if token.is_empty()
        || token.len() > MAX_BEARER_BYTES
        || token
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(DomainError::AuthenticationFailed);
    }
    let digest = Sha256::digest(token.as_bytes());
    let mut result = [0_u8; 32];
    result.copy_from_slice(&digest);
    Ok(result)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    mode: &'static str,
    schema_version: u32,
}

async fn health(State(state): State<ServerState>) -> Result<Json<HealthResponse>, ApiError> {
    let service = state.service.lock().await;
    let schema_version = service.store().schema_version()?;
    Ok(Json(HealthResponse {
        status: "ok",
        mode: "single_node",
        schema_version,
    }))
}

/// Untrusted command HTTP body. Tenant and actor fields are deliberately absent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    /// Client-generated idempotency key.
    pub command_id: CommandId,
    /// Project selected within the authenticated actor's memberships.
    pub project_id: ProjectId,
    /// Optional optimistic concurrency precondition.
    pub expected_revision: Option<Revision>,
    /// Stable operation name: `resource.put` or `resource.delete`.
    pub operation: OperationName,
    /// Full canonical resource representation for `resource.put`; must be
    /// JSON null for `resource.delete`.
    pub payload: Value,
    /// `custom.*` extension resource coordinate. Product kinds use typed APIs.
    pub resource: ResourceRef,
    /// Upsert or deletion tombstone.
    pub change: ChangeOperation,
}

async fn command(
    State(state): State<ServerState>,
    headers: HeaderMap,
    payload: Result<Json<CommandRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) =
        payload.map_err(|error| ApiError(DomainError::InvalidCommand(error.body_text())))?;
    let digest = bearer_digest_from_headers(&headers)?;
    let mut service = state.service.lock().await;
    let authenticated = service
        .store()
        .authenticate_token(&digest)?
        .ok_or(DomainError::AuthenticationFailed)?;
    let membership = service
        .store()
        .membership(&authenticated, &request.project_id)?
        .ok_or_else(|| DomainError::ProjectUnauthorized {
            project_id: request.project_id.clone(),
        })?;
    validate_resource_command(&request)?;
    let envelope = CommandEnvelope {
        command_id: request.command_id,
        project_id: request.project_id,
        expected_revision: request.expected_revision,
        operation: request.operation,
        payload: request.payload,
    };
    let receipt = service.execute(
        &authenticated,
        &membership,
        envelope,
        request.resource,
        request.change,
    )?;
    Ok((StatusCode::OK, Json(receipt)))
}

fn validate_resource_command(request: &CommandRequest) -> Result<(), DomainError> {
    if !request
        .resource
        .kind
        .as_str()
        .starts_with(EXTENSION_RESOURCE_PREFIX)
    {
        return Err(DomainError::InvalidCommand(
            "raw resource commands only accept custom.* extension kinds; product kinds require a typed API"
                .to_owned(),
        ));
    }
    match (request.operation.as_str(), request.change) {
        ("resource.put", ChangeOperation::Upsert) => Ok(()),
        ("resource.delete", ChangeOperation::Delete) if request.payload.is_null() => Ok(()),
        ("resource.delete", ChangeOperation::Delete) => Err(DomainError::InvalidCommand(
            "resource.delete payload must be JSON null".to_owned(),
        )),
        _ => Err(DomainError::InvalidCommand(
            "operation/change must be resource.put/upsert or resource.delete/delete".to_owned(),
        )),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeQuery {
    project_epoch: ProjectEpoch,
    after_seq: u64,
    limit: Option<usize>,
}

async fn changes(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    query: Result<Query<ChangeQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Query(query) =
        query.map_err(|error| ApiError(DomainError::InvalidCommand(error.body_text())))?;
    let digest = bearer_digest_from_headers(&headers)?;
    let service = state.service.lock().await;
    let authenticated = service
        .store()
        .authenticate_token(&digest)?
        .ok_or(DomainError::AuthenticationFailed)?;
    let membership = service
        .store()
        .membership(&authenticated, &project_id)?
        .ok_or_else(|| DomainError::ProjectUnauthorized {
            project_id: project_id.clone(),
        })?;
    let batch = service.changes(
        &authenticated,
        &membership,
        &ChangeCursor {
            project_epoch: query.project_epoch,
            after_seq: ProjectSequence::new(query.after_seq),
        },
        query.limit.unwrap_or(DEFAULT_CHANGE_LIMIT),
    )?;
    Ok((StatusCode::OK, Json(batch)))
}

async fn materialized_changes(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    query: Result<Query<ChangeQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Query(query) =
        query.map_err(|error| ApiError(DomainError::InvalidCommand(error.body_text())))?;
    let digest = bearer_digest_from_headers(&headers)?;
    let service = state.service.lock().await;
    let authenticated = service
        .store()
        .authenticate_token(&digest)?
        .ok_or(DomainError::AuthenticationFailed)?;
    let membership = service
        .store()
        .membership(&authenticated, &project_id)?
        .ok_or_else(|| DomainError::ProjectUnauthorized {
            project_id: project_id.clone(),
        })?;
    let batch = service.materialized_changes(
        &authenticated,
        &membership,
        &ChangeCursor {
            project_epoch: query.project_epoch,
            after_seq: ProjectSequence::new(query.after_seq),
        },
        query.limit.unwrap_or(DEFAULT_CHANGE_LIMIT),
    )?;
    Ok((
        StatusCode::OK,
        Json(MaterializedChangeResponseBatch::try_from(batch)?),
    ))
}

async fn snapshot(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let digest = bearer_digest_from_headers(&headers)?;
    let service = state.service.lock().await;
    let authenticated = service
        .store()
        .authenticate_token(&digest)?
        .ok_or(DomainError::AuthenticationFailed)?;
    let membership = service
        .store()
        .membership(&authenticated, &project_id)?
        .ok_or_else(|| DomainError::ProjectUnauthorized {
            project_id: project_id.clone(),
        })?;
    let snapshot = service.snapshot(&authenticated, &membership, &project_id)?;
    Ok((StatusCode::OK, Json(SnapshotResponse::try_from(snapshot)?)))
}

#[derive(Serialize)]
struct ResourceResponse {
    scope: aidememo_domain::ProjectScope,
    resource: ResourceRef,
    revision: Revision,
    state: ResourceResponseState,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ResourceResponseState {
    Present { body: Value },
    Deleted,
}

impl TryFrom<CanonicalResource> for ResourceResponse {
    type Error = DomainError;

    fn try_from(resource: CanonicalResource) -> Result<Self, Self::Error> {
        let state = match resource.state {
            ResourceState::Present { body } => ResourceResponseState::Present {
                body: serde_json::from_slice(&body).map_err(|error| {
                    DomainError::StorageFailure {
                        operation: "resource_json_decode",
                        detail: error.to_string(),
                    }
                })?,
            },
            ResourceState::Deleted => ResourceResponseState::Deleted,
        };
        Ok(Self {
            scope: resource.scope,
            resource: resource.resource,
            revision: resource.revision,
            state,
        })
    }
}

#[derive(Serialize)]
struct MaterializedChangeResponse {
    change: ChangeEntry,
    resource: ResourceResponse,
}

#[derive(Serialize)]
struct MaterializedChangeResponseBatch {
    scope: ProjectScope,
    cursor: ChangeCursor,
    entries: Vec<MaterializedChangeResponse>,
    next_cursor: ChangeCursor,
    has_more: bool,
}

impl TryFrom<MaterializedChangeBatch> for MaterializedChangeResponseBatch {
    type Error = DomainError;

    fn try_from(batch: MaterializedChangeBatch) -> Result<Self, Self::Error> {
        let entries = batch
            .entries
            .into_iter()
            .map(|entry| {
                Ok(MaterializedChangeResponse {
                    change: entry.change,
                    resource: ResourceResponse::try_from(entry.resource)?,
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        Ok(Self {
            scope: batch.scope,
            cursor: batch.cursor,
            entries,
            next_cursor: batch.next_cursor,
            has_more: batch.has_more,
        })
    }
}

#[derive(Serialize)]
struct SnapshotResponse {
    scope: ProjectScope,
    project_epoch: ProjectEpoch,
    at_seq: ProjectSequence,
    resources: Vec<ResourceResponse>,
}

impl TryFrom<ProjectSnapshot> for SnapshotResponse {
    type Error = DomainError;

    fn try_from(snapshot: ProjectSnapshot) -> Result<Self, Self::Error> {
        Ok(Self {
            scope: snapshot.scope,
            project_epoch: snapshot.project_epoch,
            at_seq: snapshot.at_seq,
            resources: snapshot
                .resources
                .into_iter()
                .map(ResourceResponse::try_from)
                .collect::<Result<Vec<_>, DomainError>>()?,
        })
    }
}

async fn resource(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, resource_kind, resource_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let resource = ResourceRef {
        kind: ResourceKind::try_from(resource_kind)?,
        id: ResourceId::try_from(resource_id)?,
    };
    let digest = bearer_digest_from_headers(&headers)?;
    let service = state.service.lock().await;
    let authenticated = service
        .store()
        .authenticate_token(&digest)?
        .ok_or(DomainError::AuthenticationFailed)?;
    let membership = service
        .store()
        .membership(&authenticated, &project_id)?
        .ok_or_else(|| DomainError::ProjectUnauthorized {
            project_id: project_id.clone(),
        })?;
    let canonical = service
        .visible_resource(&authenticated, &membership, &project_id, &resource)?
        .ok_or(DomainError::ResourceNotFound)?;
    Ok((StatusCode::OK, Json(ResourceResponse::try_from(canonical)?)))
}

fn bearer_digest_from_headers(headers: &HeaderMap) -> Result<[u8; 32], DomainError> {
    let header = headers
        .get(AUTHORIZATION)
        .ok_or(DomainError::AuthenticationFailed)?
        .to_str()
        .map_err(|_| DomainError::AuthenticationFailed)?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or(DomainError::AuthenticationFailed)?;
    bearer_token_digest(token)
}

/// JSON error response used by every server endpoint.
#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: ErrorCode,
    message: String,
}

struct ApiError(DomainError);

impl From<DomainError> for ApiError {
    fn from(error: DomainError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response<Body> {
        let status = status_for_error(&self.0);
        let code = self.0.code();
        let message = match &self.0 {
            DomainError::StorageFailure { .. } | DomainError::ConformanceViolation { .. } => {
                tracing::error!(error = %self.0, "server request failed internally");
                "internal server error".to_owned()
            }
            error => error.to_string(),
        };
        (
            status,
            Json(ErrorResponse {
                error: ErrorDetail { code, message },
            }),
        )
            .into_response()
    }
}

fn status_for_error(error: &DomainError) -> StatusCode {
    match error {
        DomainError::AuthenticationFailed => StatusCode::UNAUTHORIZED,
        DomainError::IdentityMismatch
        | DomainError::HandoffActorMismatch
        | DomainError::ProjectUnauthorized { .. }
        | DomainError::ProjectScopeMismatch { .. } => StatusCode::FORBIDDEN,
        DomainError::ResourceNotFound => StatusCode::NOT_FOUND,
        DomainError::CommandConflict
        | DomainError::ResourceAlreadyExists
        | DomainError::HandoffConflict(_)
        | DomainError::StaleRevision { .. }
        | DomainError::CursorEpochMismatch { .. }
        | DomainError::CursorOutOfRange { .. }
        | DomainError::SnapshotRequired => StatusCode::CONFLICT,
        DomainError::SnapshotTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        DomainError::InvalidIdentifier { .. }
        | DomainError::InvalidChangeBatch(_)
        | DomainError::InvalidArtifactPath(_)
        | DomainError::InvalidArtifactReference(_)
        | DomainError::InvalidCommand(_) => StatusCode::BAD_REQUEST,
        DomainError::StorageFailure { .. } | DomainError::ConformanceViolation { .. } => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
