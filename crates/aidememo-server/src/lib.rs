//! Authenticated single-node HTTP boundary for the future AideMemo SSOT mode.
//!
//! The server owns tenant and actor identity. Request bodies select only a
//! project and resource; bearer-token digests resolve to persisted actors, and
//! active membership is loaded from the selected canonical ledger before every
//! read or mutation. This crate does not modify or expose the existing embedded store.

mod artifact;
mod executor;
mod lexical;
mod mcp;
mod product;
mod projection_worker;
#[cfg(feature = "semantic")]
mod semantic;

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
use aidememo_store_local::SqliteCommandStore;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use executor::{BlockingStoreError, BlockingStoreExecutor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(feature = "semantic")]
use std::collections::HashMap;
use std::{sync::Arc, time::Duration};

#[cfg(feature = "semantic")]
pub use semantic::{EmbeddingProvider, HttpEmbeddingProvider, SharedEmbeddingProvider};
use tokio::sync::Mutex;

const DEFAULT_CHANGE_LIMIT: usize = 100;
const MAX_COMMAND_BODY_BYTES: usize = 1024 * 1024;
const MAX_BEARER_BYTES: usize = 4096;
const EXTENSION_RESOURCE_PREFIX: &str = "custom.";
const DEFAULT_STORE_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_STORE_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Cloneable application state around one single-node command service.
#[derive(Clone)]
pub struct ServerState {
    canonical: BlockingStoreExecutor,
    artifacts: Option<Arc<ArtifactState>>,
    #[cfg(feature = "semantic")]
    semantic_provider: Option<SharedEmbeddingProvider>,
    #[cfg(feature = "semantic")]
    semantic_projection: Arc<Mutex<Option<Arc<semantic::SemanticProjection>>>>,
    projection_worker: Option<Arc<projection_worker::ProjectionWorker>>,
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
            canonical: BlockingStoreExecutor::sqlite(
                store,
                DEFAULT_STORE_ACQUIRE_TIMEOUT,
                DEFAULT_STORE_OPERATION_TIMEOUT,
            ),
            artifacts: None,
            #[cfg(feature = "semantic")]
            semantic_provider: None,
            #[cfg(feature = "semantic")]
            semantic_projection: Arc::new(Mutex::new(None)),
            projection_worker: None,
        }
    }

    /// Build an artifact-disabled PostgreSQL server state with verified TLS.
    ///
    /// System trust roots are used by default. `root_ca_pem` may contain one
    /// additional PEM root certificate for a private/internal CA. The adapter
    /// always forces PostgreSQL SSL mode to `require` and keeps hostname and
    /// certificate verification enabled.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when pool construction, TLS, timeout policy,
    /// connection, or schema initialization fails.
    pub async fn postgres_tls(
        url: String,
        root_ca_pem: Option<Vec<u8>>,
        pool_size: usize,
        acquire_timeout: Duration,
        operation_timeout: Duration,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, DomainError> {
        let canonical = BlockingStoreExecutor::postgres_tls(
            url,
            root_ca_pem,
            pool_size,
            acquire_timeout,
            operation_timeout,
            statement_timeout,
            lock_timeout,
        )
        .await
        .map_err(blocking_store_initialization_error)?;
        let worker = projection_worker::ProjectionWorker::new(
            canonical.clone(),
            #[cfg(feature = "semantic")]
            None,
        );
        Ok(Self {
            canonical,
            artifacts: None,
            #[cfg(feature = "semantic")]
            semantic_provider: None,
            #[cfg(feature = "semantic")]
            semantic_projection: Arc::new(Mutex::new(None)),
            projection_worker: Some(Arc::new(worker)),
        })
    }

    /// Build an artifact-disabled PostgreSQL server state without TLS.
    ///
    /// This constructor exists only for explicit local/development profiles.
    /// Production profiles should use [`Self::postgres_tls`].
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when pool construction, timeout policy,
    /// connection, or schema initialization fails.
    pub async fn postgres_no_tls_for_development(
        url: String,
        pool_size: usize,
        acquire_timeout: Duration,
        operation_timeout: Duration,
        statement_timeout: Duration,
        lock_timeout: Duration,
    ) -> Result<Self, DomainError> {
        let canonical = BlockingStoreExecutor::postgres_no_tls(
            url,
            pool_size,
            acquire_timeout,
            operation_timeout,
            statement_timeout,
            lock_timeout,
        )
        .await
        .map_err(blocking_store_initialization_error)?;
        let worker = projection_worker::ProjectionWorker::new(
            canonical.clone(),
            #[cfg(feature = "semantic")]
            None,
        );
        Ok(Self {
            canonical,
            artifacts: None,
            #[cfg(feature = "semantic")]
            semantic_provider: None,
            #[cfg(feature = "semantic")]
            semantic_projection: Arc::new(Mutex::new(None)),
            projection_worker: Some(Arc::new(worker)),
        })
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
            canonical: BlockingStoreExecutor::sqlite(
                store,
                DEFAULT_STORE_ACQUIRE_TIMEOUT,
                DEFAULT_STORE_OPERATION_TIMEOUT,
            ),
            artifacts: Some(Arc::new(ArtifactState {
                catalog: Mutex::new(artifacts),
                bodies: ArtifactBodies::Local,
            })),
            #[cfg(feature = "semantic")]
            semantic_provider: None,
            #[cfg(feature = "semantic")]
            semantic_projection: Arc::new(Mutex::new(None)),
            projection_worker: None,
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
            canonical: BlockingStoreExecutor::sqlite(
                store,
                DEFAULT_STORE_ACQUIRE_TIMEOUT,
                DEFAULT_STORE_OPERATION_TIMEOUT,
            ),
            artifacts: Some(Arc::new(ArtifactState {
                catalog: Mutex::new(catalog),
                bodies: ArtifactBodies::S3(bodies),
            })),
            #[cfg(feature = "semantic")]
            semantic_provider: None,
            #[cfg(feature = "semantic")]
            semantic_projection: Arc::new(Mutex::new(None)),
            projection_worker: None,
        })
    }

    /// Attach a semantic embedding provider to this server state.
    ///
    /// The provider owns no canonical data; cached HNSW state is rebuilt from
    /// canonical project snapshots whenever sequence or model identity changes.
    /// If a projection worker exists, it will also be updated to use this provider.
    #[cfg(feature = "semantic")]
    #[must_use]
    pub fn with_semantic_provider(mut self, provider: SharedEmbeddingProvider) -> Self {
        self.semantic_provider = Some(provider.clone());
        if let Some(_worker) = &self.projection_worker {
            // Create a new worker with the semantic provider
            let new_worker =
                projection_worker::ProjectionWorker::new(self.canonical.clone(), Some(provider));
            self.projection_worker = Some(Arc::new(new_worker));
        }
        self
    }

    /// Start background projection refresh for a specific project scope.
    ///
    /// This spawns a background task that periodically checks for new canonical
    /// data and rebuilds projections when the project sequence advances. The task
    /// uses the bounded executor to avoid blocking Axum workers.
    ///
    /// Returns a join handle that can be used to await worker shutdown, though
    /// in normal operation the worker runs for the server lifetime.
    #[must_use]
    pub fn start_projection_refresh(
        &self,
        scope: ProjectScope,
        refresh_interval: Option<Duration>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        self.projection_worker
            .as_ref()
            .map(|worker| Arc::clone(worker).spawn_refresh_task(scope, refresh_interval))
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

fn blocking_store_initialization_error(error: BlockingStoreError) -> DomainError {
    match error {
        BlockingStoreError::Domain(error) => error,
        error => DomainError::StorageFailure {
            operation: "server_canonical_backend_init",
            detail: error.to_string(),
        },
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
        .route("/v1/projects/{project_id}/search", get(search))
        .route(
            "/v1/projects/{project_id}/resources/{resource_kind}/{resource_id}",
            get(resource),
        )
        .merge(product::routes())
        .merge(mcp::routes())
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
    let schema_version = state
        .canonical
        .run_service(|service| service.store().schema_version())
        .await?;
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
        payload.map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
    let digest = bearer_digest_from_headers(&headers)?;
    let receipt = state
        .canonical
        .run_service(move |service| {
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
            service.execute(
                &authenticated,
                &membership,
                envelope,
                request.resource,
                request.change,
            )
        })
        .await?;
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
        query.map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
    let digest = bearer_digest_from_headers(&headers)?;
    let batch = state
        .canonical
        .run_service(move |service| {
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
            service.changes(
                &authenticated,
                &membership,
                &ChangeCursor {
                    project_epoch: query.project_epoch,
                    after_seq: ProjectSequence::new(query.after_seq),
                },
                query.limit.unwrap_or(DEFAULT_CHANGE_LIMIT),
            )
        })
        .await?;
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
        query.map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
    let digest = bearer_digest_from_headers(&headers)?;
    let batch = state
        .canonical
        .run_service(move |service| {
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
            service.materialized_changes(
                &authenticated,
                &membership,
                &ChangeCursor {
                    project_epoch: query.project_epoch,
                    after_seq: ProjectSequence::new(query.after_seq),
                },
                query.limit.unwrap_or(DEFAULT_CHANGE_LIMIT),
            )
        })
        .await?;
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
    let snapshot = state
        .canonical
        .run_service(move |service| {
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
            service.snapshot(&authenticated, &membership, &project_id)
        })
        .await?;
    Ok((StatusCode::OK, Json(SnapshotResponse::try_from(snapshot)?)))
}

const MAX_SEARCH_QUERY_BYTES: usize = 4096;
const DEFAULT_SEARCH_LIMIT: usize = 20;
const MAX_SEARCH_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery {
    q: String,
    source_id: Option<aidememo_domain::SourceId>,
    limit: Option<usize>,
    at_least_seq: Option<u64>,
    mode: Option<String>,
}

#[derive(Clone, Serialize)]
struct SearchHit {
    fact_id: aidememo_domain::FactId,
    session_id: aidememo_domain::SessionId,
    source_id: Option<aidememo_domain::SourceId>,
    actor_id: aidememo_domain::ActorId,
    content: String,
    score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    lexical_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_score: Option<f64>,
}

#[derive(Serialize)]
struct SearchResponse {
    project_epoch: ProjectEpoch,
    index_seq: ProjectSequence,
    mode: &'static str,
    semantic_model: Option<String>,
    results: Vec<SearchHit>,
}

async fn search(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    query: Result<Query<SearchQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Query(query) =
        query.map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
    let query_text = query.q.trim();
    if query_text.is_empty() || query_text.len() > MAX_SEARCH_QUERY_BYTES {
        return Err(ApiError::from(DomainError::InvalidCommand(format!(
            "search query must contain 1..={MAX_SEARCH_QUERY_BYTES} bytes"
        ))));
    }
    let limit = query.limit.unwrap_or(DEFAULT_SEARCH_LIMIT);
    if !(1..=MAX_SEARCH_LIMIT).contains(&limit) {
        return Err(ApiError::from(DomainError::InvalidCommand(format!(
            "search limit must be between 1 and {MAX_SEARCH_LIMIT}"
        ))));
    }
    let digest = bearer_digest_from_headers(&headers)?;
    let snapshot = state
        .canonical
        .run_service(move |service| {
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
            service.snapshot(&authenticated, &membership, &project_id)
        })
        .await?;
    if let Some(at_least_seq) = query.at_least_seq {
        let requested = ProjectSequence::new(at_least_seq);
        if requested > snapshot.at_seq {
            return Err(ApiError::from(DomainError::CursorOutOfRange {
                after_seq: requested,
                current: snapshot.at_seq,
            }));
        }
    }
    let requested_mode = query
        .mode
        .as_deref()
        .unwrap_or("auto")
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        requested_mode.as_str(),
        "auto" | "lexical" | "semantic" | "hybrid"
    ) {
        return Err(ApiError::from(DomainError::InvalidCommand(
            "search mode must be auto, lexical, semantic, or hybrid".to_owned(),
        )));
    }
    // Try to use cached projection from worker if available and fresh enough
    let projection = if let Some(worker) = &state.projection_worker {
        if let Some(cached) = worker.get_lexical(&snapshot.scope).await {
            if cached.project_epoch() == &snapshot.project_epoch
                && cached.index_seq() >= snapshot.at_seq
            {
                cached.as_ref().clone()
            } else {
                // Cache is stale or wrong epoch, rebuild
                lexical::LexicalProjection::rebuild(&snapshot)?
            }
        } else {
            // No cache yet, rebuild
            lexical::LexicalProjection::rebuild(&snapshot)?
        }
    } else {
        // No worker, always rebuild
        lexical::LexicalProjection::rebuild(&snapshot)?
    };
    let candidate_limit = limit.saturating_mul(8).max(limit).min(512);
    let lexical_hits = projection.search(
        query_text,
        query.source_id.as_ref().map(|source_id| source_id.as_str()),
        candidate_limit,
    );
    #[cfg(feature = "semantic")]
    let lexical_strong = lexical_hits.len() >= limit.min(3)
        && lexical_hits.first().is_some_and(|hit| hit.score >= 1.25);

    #[cfg(feature = "semantic")]
    let (effective_mode, semantic_model, mut results) = {
        let wants_semantic = match requested_mode.as_str() {
            "lexical" => false,
            "auto" => !lexical_strong,
            "semantic" | "hybrid" => true,
            _ => unreachable!(),
        };
        if !wants_semantic {
            ("lexical", None, lexical_search_hits(&lexical_hits, limit))
        } else if let Some(provider) = state.semantic_provider.clone() {
            let semantic_projection = semantic_projection_for(&state, &snapshot, &provider).await?;
            let semantic_hits = semantic_projection.search(
                provider.as_ref(),
                query_text,
                query.source_id.as_ref().map(|source_id| source_id.as_str()),
                candidate_limit,
            )?;
            let model = Some(semantic_projection.model().to_owned());
            if requested_mode == "semantic" {
                (
                    "semantic",
                    model,
                    semantic_search_hits(&semantic_hits, limit),
                )
            } else {
                (
                    "hybrid",
                    model,
                    hybrid_search_hits(&lexical_hits, &semantic_hits, limit),
                )
            }
        } else if requested_mode == "auto" {
            ("lexical", None, lexical_search_hits(&lexical_hits, limit))
        } else {
            return Err(ApiError::from(DomainError::InvalidCommand(
                "semantic retrieval is not configured on this server; use mode=lexical or configure an embedding endpoint".to_owned(),
            )));
        }
    };

    #[cfg(not(feature = "semantic"))]
    let (effective_mode, semantic_model, mut results) = {
        if matches!(requested_mode.as_str(), "semantic" | "hybrid") {
            return Err(ApiError::from(DomainError::InvalidCommand(
                "semantic retrieval requires an aidememo-server build with --features semantic"
                    .to_owned(),
            )));
        }
        ("lexical", None, lexical_search_hits(&lexical_hits, limit))
    };

    results.truncate(limit);
    Ok((
        StatusCode::OK,
        Json(SearchResponse {
            project_epoch: projection.project_epoch().clone(),
            index_seq: projection.index_seq(),
            mode: effective_mode,
            semantic_model,
            results,
        }),
    ))
}

fn lexical_search_hits(hits: &[lexical::LexicalHit], limit: usize) -> Vec<SearchHit> {
    hits.iter()
        .take(limit)
        .map(|hit| SearchHit {
            fact_id: hit.fact_id.clone(),
            session_id: hit.session_id.clone(),
            source_id: hit.source_id.clone(),
            actor_id: hit.actor_id.clone(),
            content: hit.content.clone(),
            score: hit.score,
            lexical_score: Some(hit.score),
            semantic_score: None,
        })
        .collect()
}

#[cfg(feature = "semantic")]
fn semantic_search_hits(hits: &[semantic::SemanticHit], limit: usize) -> Vec<SearchHit> {
    hits.iter()
        .take(limit)
        .map(|hit| SearchHit {
            fact_id: hit.fact_id.clone(),
            session_id: hit.session_id.clone(),
            source_id: hit.source_id.clone(),
            actor_id: hit.actor_id.clone(),
            content: hit.content.clone(),
            score: hit.score,
            lexical_score: None,
            semantic_score: Some(hit.score),
        })
        .collect()
}

#[cfg(feature = "semantic")]
fn hybrid_search_hits(
    lexical_hits: &[lexical::LexicalHit],
    semantic_hits: &[semantic::SemanticHit],
    limit: usize,
) -> Vec<SearchHit> {
    const RRF_K: f64 = 60.0;
    let mut merged = HashMap::<String, SearchHit>::new();
    for (rank, hit) in lexical_hits.iter().enumerate() {
        let score = 1.0 / (RRF_K + rank as f64 + 1.0);
        merged.insert(
            hit.fact_id.as_str().to_owned(),
            SearchHit {
                fact_id: hit.fact_id.clone(),
                session_id: hit.session_id.clone(),
                source_id: hit.source_id.clone(),
                actor_id: hit.actor_id.clone(),
                content: hit.content.clone(),
                score,
                lexical_score: Some(hit.score),
                semantic_score: None,
            },
        );
    }
    for (rank, hit) in semantic_hits.iter().enumerate() {
        let rrf = 1.0 / (RRF_K + rank as f64 + 1.0);
        let entry = merged
            .entry(hit.fact_id.as_str().to_owned())
            .or_insert_with(|| SearchHit {
                fact_id: hit.fact_id.clone(),
                session_id: hit.session_id.clone(),
                source_id: hit.source_id.clone(),
                actor_id: hit.actor_id.clone(),
                content: hit.content.clone(),
                score: 0.0,
                lexical_score: None,
                semantic_score: None,
            });
        entry.score += rrf;
        entry.semantic_score = Some(hit.score);
    }
    let mut results = merged.into_values().collect::<Vec<_>>();
    results.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.fact_id.as_str().cmp(right.fact_id.as_str()))
    });
    results.truncate(limit);
    results
}

#[cfg(feature = "semantic")]
async fn semantic_projection_for(
    state: &ServerState,
    snapshot: &ProjectSnapshot,
    provider: &SharedEmbeddingProvider,
) -> Result<Arc<semantic::SemanticProjection>, ApiError> {
    {
        let cached = state.semantic_projection.lock().await;
        if let Some(projection) = cached.as_ref()
            && projection.matches(snapshot, provider.as_ref())
        {
            return Ok(projection.clone());
        }
    }
    let projection = Arc::new(semantic::SemanticProjection::rebuild(
        snapshot,
        provider.as_ref(),
    )?);
    let mut cached = state.semantic_projection.lock().await;
    if let Some(current) = cached.as_ref()
        && current.matches(snapshot, provider.as_ref())
    {
        return Ok(current.clone());
    }
    *cached = Some(projection.clone());
    Ok(projection)
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
    let canonical = state
        .canonical
        .run_service(move |service| {
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
            service
                .visible_resource(&authenticated, &membership, &project_id, &resource)?
                .ok_or(DomainError::ResourceNotFound)
        })
        .await?;
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

enum ApiError {
    Domain(DomainError),
    Executor(BlockingStoreError),
}

impl From<DomainError> for ApiError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<BlockingStoreError> for ApiError {
    fn from(error: BlockingStoreError) -> Self {
        Self::Executor(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response<Body> {
        match self {
            Self::Domain(error) => domain_error_response(error),
            Self::Executor(BlockingStoreError::Domain(error)) => domain_error_response(error),
            Self::Executor(
                error @ (BlockingStoreError::Saturated
                | BlockingStoreError::TimedOut
                | BlockingStoreError::BackendUnavailable),
            ) => {
                tracing::warn!(error = %error, "canonical store temporarily unavailable");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(ErrorResponse {
                        error: ErrorDetail {
                            code: ErrorCode::StorageFailure,
                            message: "canonical storage temporarily unavailable".to_owned(),
                        },
                    }),
                )
                    .into_response()
            }
            Self::Executor(
                error @ (BlockingStoreError::Configuration(_) | BlockingStoreError::Join(_)),
            ) => {
                tracing::error!(error = %error, "canonical store executor failed internally");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: ErrorDetail {
                            code: ErrorCode::StorageFailure,
                            message: "internal server error".to_owned(),
                        },
                    }),
                )
                    .into_response()
            }
        }
    }
}

fn domain_error_response(error: DomainError) -> Response<Body> {
    let status = status_for_error(&error);
    let code = error.code();
    let message = match &error {
        DomainError::StorageFailure { .. } | DomainError::ConformanceViolation { .. } => {
            tracing::error!(error = %error, "server request failed internally");
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

#[cfg(test)]
mod executor_error_tests {
    use super::*;

    #[test]
    fn capacity_and_backend_errors_map_to_service_unavailable() {
        for error in [
            BlockingStoreError::Saturated,
            BlockingStoreError::TimedOut,
            BlockingStoreError::BackendUnavailable,
        ] {
            assert_eq!(
                ApiError::from(error).into_response().status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
    }

    #[test]
    fn executor_internal_errors_remain_internal_server_errors() {
        for error in [
            BlockingStoreError::Configuration("invalid".to_owned()),
            BlockingStoreError::Join("panic".to_owned()),
        ] {
            assert_eq!(
                ApiError::from(error).into_response().status(),
                StatusCode::INTERNAL_SERVER_ERROR
            );
        }
    }
}
