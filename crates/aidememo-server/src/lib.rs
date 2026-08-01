//! Authenticated single-node HTTP boundary for the future AideMemo SSOT mode.
//!
//! The server owns tenant and actor identity. Request bodies select only a
//! project and resource; bearer-token digests resolve to persisted actors, and
//! active membership is loaded from the same SQLite ledger before every read or
//! mutation. This crate does not modify or expose the existing embedded store.

use aidememo_domain::{
    CanonicalResource, ChangeCursor, ChangeOperation, CommandEnvelope, CommandId, DomainError,
    ErrorCode, OperationName, ProjectEpoch, ProjectId, ProjectSequence, ResourceId, ResourceKind,
    ResourceRef, ResourceState, Revision,
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
}

impl ServerState {
    /// Wrap an opened and migrated SQLite command store.
    #[must_use]
    pub fn new(store: SqliteCommandStore) -> Self {
        Self {
            service: Arc::new(Mutex::new(CommandService::new(store))),
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
            "/v1/projects/{project_id}/resources/{resource_kind}/{resource_id}",
            get(resource),
        )
        .layer(DefaultBodyLimit::max(MAX_COMMAND_BODY_BYTES))
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
        .resource(&authenticated, &membership, &project_id, &resource)?
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
        | DomainError::ProjectUnauthorized { .. }
        | DomainError::ProjectScopeMismatch { .. } => StatusCode::FORBIDDEN,
        DomainError::ResourceNotFound => StatusCode::NOT_FOUND,
        DomainError::CommandConflict
        | DomainError::StaleRevision { .. }
        | DomainError::CursorEpochMismatch { .. }
        | DomainError::CursorOutOfRange { .. } => StatusCode::CONFLICT,
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
