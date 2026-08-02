//! Authenticated local artifact HTTP lifecycle for single-node SSOT mode.

use super::{ApiError, ServerState, product::request_context};
use aidememo_artifacts::{
    ArtifactStoreError, LocalArtifactStore, MAX_DIRECT_UPLOAD_BYTES, ReserveRequest,
};
use aidememo_domain::{
    ArtifactBodyRef, ArtifactId, ArtifactPath, ArtifactReservation, DomainError, ProjectId,
    ProjectScope, Revision,
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Query, Request, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

pub(super) fn routes() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/projects/{project_id}/artifact-reservations",
            post(reserve_artifact),
        )
        .route(
            "/v1/projects/{project_id}/artifact-reservations/{reservation_token}/body",
            put(upload_artifact),
        )
        .route(
            "/v1/projects/{project_id}/artifact-reservations/{reservation_token}/publish",
            post(publish_artifact),
        )
        .route(
            "/v1/projects/{project_id}/artifact-reservations/{reservation_token}",
            delete(abort_artifact),
        )
        .route(
            "/v1/projects/{project_id}/artifacts/resolve",
            get(resolve_artifact),
        )
        .route(
            "/v1/projects/{project_id}/artifacts/{artifact_id}/downloads",
            post(download_artifact),
        )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReserveArtifactRequest {
    request_id: String,
    path: ArtifactPath,
    expected_mutation_token: Option<String>,
    ttl_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveArtifactQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DownloadArtifactRequest {
    revision: Revision,
}

async fn reserve_artifact(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    payload: Result<Json<ReserveArtifactRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ArtifactHttpError> {
    let project_id = ProjectId::try_from(project_id)?;
    let scope = authorize(&state, &headers, &project_id, true).await?;
    let Json(request) = decode_json(payload)?;
    let now_ms = unix_time_ms()?;
    let artifacts = artifact_store(&state)?;
    let mut artifacts = artifacts.lock().await;
    let reservation = artifacts.reserve(ReserveRequest {
        request_id: request.request_id,
        scope,
        path: request.path,
        expected_mutation_token: request.expected_mutation_token,
        now_ms,
        ttl_ms: request.ttl_ms,
    })?;
    Ok((StatusCode::OK, Json(reservation)))
}

async fn upload_artifact(
    State(state): State<ServerState>,
    Path((project_id, reservation_token)): Path<(String, String)>,
    request: Request,
) -> Result<impl IntoResponse, ArtifactHttpError> {
    let project_id = ProjectId::try_from(project_id)?;
    let scope = authorize(&state, request.headers(), &project_id, true).await?;
    let artifacts = artifact_store(&state)?;
    let body = to_bytes(request.into_body(), MAX_DIRECT_UPLOAD_BYTES)
        .await
        .map_err(|_| ArtifactStoreError::UploadTooLarge {
            limit_bytes: MAX_DIRECT_UPLOAD_BYTES,
        })?;
    let artifacts = artifacts.lock().await;
    let reservation = live_reservation(&artifacts, &scope, &reservation_token)?;
    let observation = artifacts.upload(&reservation, &body, unix_time_ms()?)?;
    Ok((StatusCode::OK, Json(observation)))
}

async fn publish_artifact(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, reservation_token)): Path<(String, String)>,
) -> Result<impl IntoResponse, ArtifactHttpError> {
    let project_id = ProjectId::try_from(project_id)?;
    let scope = authorize(&state, &headers, &project_id, true).await?;
    let artifacts = artifact_store(&state)?;
    let mut artifacts = artifacts.lock().await;
    if let Some(reference) = artifacts.publication_receipt_by_token(&scope, &reservation_token)? {
        return Ok((StatusCode::OK, Json(reference)));
    }
    let reservation = reservation_for_publish(&artifacts, &scope, &reservation_token)?;
    let now_ms = unix_time_ms()?;
    let observation = artifacts.observe(&reservation, now_ms)?;
    let reference = artifacts.publish(&reservation, &observation, now_ms)?;
    Ok((StatusCode::OK, Json(reference)))
}

async fn abort_artifact(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, reservation_token)): Path<(String, String)>,
) -> Result<impl IntoResponse, ArtifactHttpError> {
    let project_id = ProjectId::try_from(project_id)?;
    let scope = authorize(&state, &headers, &project_id, true).await?;
    let artifacts = artifact_store(&state)?;
    let mut artifacts = artifacts.lock().await;
    let reservation = live_reservation(&artifacts, &scope, &reservation_token)?;
    artifacts.abort(&reservation)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve_artifact(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    query: Result<Query<ResolveArtifactQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<impl IntoResponse, ArtifactHttpError> {
    let project_id = ProjectId::try_from(project_id)?;
    let scope = authorize(&state, &headers, &project_id, false).await?;
    let Query(query) = query.map_err(|error| DomainError::InvalidCommand(error.body_text()))?;
    let path = ArtifactPath::try_from(query.path)?;
    let artifacts = artifact_store(&state)?;
    let artifacts = artifacts.lock().await;
    let reference = artifacts
        .get(&scope, &path)?
        .ok_or(ArtifactStoreError::NotFound)?;
    Ok((StatusCode::OK, Json(reference)))
}

async fn download_artifact(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, artifact_id)): Path<(String, String)>,
    payload: Result<Json<DownloadArtifactRequest>, JsonRejection>,
) -> Result<Response, ArtifactHttpError> {
    let project_id = ProjectId::try_from(project_id)?;
    let scope = authorize(&state, &headers, &project_id, false).await?;
    let artifact_id = ArtifactId::try_from(artifact_id)?;
    let Json(request) = decode_json(payload)?;
    let artifacts = artifact_store(&state)?;
    let artifacts = artifacts.lock().await;
    let reference = artifacts
        .get_by_id(&scope, &artifact_id)?
        .ok_or(ArtifactStoreError::NotFound)?;
    if reference.revision != request.revision {
        return Err(DomainError::StaleRevision {
            expected: request.revision,
            current: reference.revision,
        }
        .into());
    }
    let ArtifactBodyRef::Object { etag, .. } = &reference.body else {
        return Err(ArtifactStoreError::BodyMismatch.into());
    };
    let etag = HeaderValue::from_str(&format!("\"{etag}\""))
        .map_err(|_| ArtifactStoreError::BodyMismatch)?;
    let bytes = artifacts.read(&reference)?;
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(header::ETAG, etag);
    Ok(response)
}

async fn authorize(
    state: &ServerState,
    headers: &HeaderMap,
    project_id: &ProjectId,
    mutation: bool,
) -> Result<ProjectScope, ArtifactHttpError> {
    let service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, headers, project_id)?;
    if mutation && !membership.role.can_mutate() {
        return Err(DomainError::ProjectUnauthorized {
            project_id: project_id.clone(),
        }
        .into());
    }
    Ok(ProjectScope::new(
        authenticated.tenant_id().clone(),
        project_id.clone(),
    ))
}

fn artifact_store(
    state: &ServerState,
) -> Result<Arc<Mutex<LocalArtifactStore>>, ArtifactHttpError> {
    state
        .artifacts
        .clone()
        .ok_or(ArtifactHttpError::Unavailable)
}

fn live_reservation(
    artifacts: &LocalArtifactStore,
    scope: &ProjectScope,
    token: &str,
) -> Result<ArtifactReservation, ArtifactHttpError> {
    artifacts
        .reservation_by_token(scope, token)?
        .ok_or_else(|| ArtifactStoreError::NotFound.into())
}

fn reservation_for_publish(
    artifacts: &LocalArtifactStore,
    scope: &ProjectScope,
    token: &str,
) -> Result<ArtifactReservation, ArtifactHttpError> {
    if let Some(reservation) = artifacts.reservation_by_token(scope, token)? {
        return Ok(reservation);
    }
    artifacts
        .reservation_receipt_by_token(scope, token)?
        .ok_or_else(|| ArtifactStoreError::NotFound.into())
}

fn decode_json<T>(payload: Result<Json<T>, JsonRejection>) -> Result<Json<T>, ArtifactHttpError> {
    payload.map_err(|error| DomainError::InvalidCommand(error.body_text()).into())
}

fn unix_time_ms() -> Result<i64, ArtifactHttpError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| DomainError::StorageFailure {
            operation: "artifact_clock",
            detail: error.to_string(),
        })?;
    i64::try_from(duration.as_millis()).map_err(|_| {
        DomainError::StorageFailure {
            operation: "artifact_clock",
            detail: "current time exceeds i64 milliseconds".to_owned(),
        }
        .into()
    })
}

#[derive(Serialize)]
struct ArtifactErrorResponse {
    error: ArtifactErrorDetail,
}

#[derive(Serialize)]
struct ArtifactErrorDetail {
    code: &'static str,
    message: String,
}

enum ArtifactHttpError {
    Domain(DomainError),
    Store(ArtifactStoreError),
    Unavailable,
}

impl From<DomainError> for ArtifactHttpError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<ArtifactStoreError> for ArtifactHttpError {
    fn from(error: ArtifactStoreError) -> Self {
        match error {
            ArtifactStoreError::Domain(error) => Self::Domain(error),
            error => Self::Store(error),
        }
    }
}

impl IntoResponse for ArtifactHttpError {
    fn into_response(self) -> Response {
        match self {
            Self::Domain(error) => ApiError::from(error).into_response(),
            Self::Unavailable => artifact_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "artifact_store_unavailable",
                "artifact storage is not configured".to_owned(),
            ),
            Self::Store(error) => {
                let (status, code, message, internal) = match &error {
                    ArtifactStoreError::Conflict => (
                        StatusCode::CONFLICT,
                        "artifact_conflict",
                        error.to_string(),
                        false,
                    ),
                    ArtifactStoreError::ReservationExpired => (
                        StatusCode::CONFLICT,
                        "artifact_reservation_expired",
                        error.to_string(),
                        false,
                    ),
                    ArtifactStoreError::NotFound => (
                        StatusCode::NOT_FOUND,
                        "artifact_not_found",
                        error.to_string(),
                        false,
                    ),
                    ArtifactStoreError::BodyMismatch => (
                        StatusCode::CONFLICT,
                        "artifact_body_mismatch",
                        error.to_string(),
                        false,
                    ),
                    ArtifactStoreError::UploadTooLarge { .. } => (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "artifact_upload_too_large",
                        error.to_string(),
                        false,
                    ),
                    ArtifactStoreError::Storage { .. }
                    | ArtifactStoreError::Filesystem { .. }
                    | ArtifactStoreError::Provider { .. }
                    | ArtifactStoreError::Domain(_) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "artifact_internal_error",
                        "internal server error".to_owned(),
                        true,
                    ),
                };
                if internal {
                    tracing::error!(error = %error, "artifact server request failed internally");
                }
                artifact_error(status, code, message)
            }
        }
    }
}

fn artifact_error(status: StatusCode, code: &'static str, message: String) -> Response {
    (
        status,
        Json(ArtifactErrorResponse {
            error: ArtifactErrorDetail { code, message },
        }),
    )
        .into_response()
}
