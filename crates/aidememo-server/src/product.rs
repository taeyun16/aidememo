//! Typed product-domain HTTP slice for remote sessions, facts, and handoffs.

use super::{ApiError, ServerState, bearer_digest_from_headers};
use aidememo_domain::{
    ActorId, AuthenticatedActor, CanonicalResource, ChangeOperation, ClaimId, CommandEnvelope,
    CommandId, DomainError, FactId, FactRecord, HandoffContextRecord, HandoffId, HandoffMailbox,
    HandoffOutcome, HandoffQuery, HandoffRecord, OperationName, ProjectId, ProjectMembership,
    ProjectScope, ProjectSequence, ResourceId, ResourceKind, ResourceRef, ResourceState, Revision,
    SessionId, SessionRecord, SourceId,
};
use aidememo_service::CommandService;
use aidememo_store_local::SqliteCommandStore;
use axum::{
    Json, Router,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::{SystemTime, UNIX_EPOCH};

const SESSION_KIND: &str = "session";
const FACT_KIND: &str = "fact";
const HANDOFF_KIND: &str = "handoff";
const HANDOFF_CONTEXT_KIND: &str = "handoff_context";
const HANDOFF_LEASE_MS: i64 = 120_000;

pub(super) fn routes() -> Router<ServerState> {
    Router::new()
        .route("/v1/projects/{project_id}/identity", get(get_identity))
        .route("/v1/projects/{project_id}/sessions", post(create_session))
        .route("/v1/projects/{project_id}/facts", post(create_fact))
        .route(
            "/v1/projects/{project_id}/handoff-contexts",
            post(create_handoff_context),
        )
        .route(
            "/v1/projects/{project_id}/handoffs",
            post(send_handoff).get(list_handoffs),
        )
        .route(
            "/v1/projects/{project_id}/handoffs/{handoff_id}",
            get(get_handoff),
        )
        .route(
            "/v1/projects/{project_id}/handoffs/{handoff_id}/accept",
            post(accept_handoff),
        )
        .route(
            "/v1/projects/{project_id}/handoffs/{handoff_id}/heartbeat",
            post(heartbeat_handoff),
        )
        .route(
            "/v1/projects/{project_id}/handoffs/{handoff_id}/return",
            post(return_handoff),
        )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest<T> {
    command_id: CommandId,
    payload: T,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransitionRequest<T> {
    command_id: CommandId,
    expected_revision: Revision,
    payload: T,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionCreatePayload {
    session_id: SessionId,
    source_id: Option<SourceId>,
    topic: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FactCreatePayload {
    fact_id: FactId,
    session_id: SessionId,
    content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffSendPayload {
    handoff_id: HandoffId,
    session_id: SessionId,
    to_actor: ActorId,
    focus: Option<String>,
    done_when: Option<String>,
    context_id: Option<ResourceId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffContextCreatePayload {
    context_id: ResourceId,
    handoff_id: HandoffId,
    session_id: SessionId,
    to_actor: ActorId,
    content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffAcceptPayload {
    claim_id: ClaimId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffHeartbeatPayload {
    claim_id: ClaimId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffReturnPayload {
    claim_id: ClaimId,
    result_fact_id: FactId,
    outcome: HandoffOutcome,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffListQuery {
    #[serde(rename = "box")]
    mailbox: HandoffMailbox,
    source_id: Option<SourceId>,
    include_completed: Option<bool>,
    before_seq: Option<u64>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct TypedRecordResponse<T> {
    revision: Revision,
    record: T,
}

#[derive(Serialize)]
struct IdentityResponse {
    tenant_id: aidememo_domain::TenantId,
    project_id: ProjectId,
    project_epoch: aidememo_domain::ProjectEpoch,
    actor_id: ActorId,
    role: aidememo_domain::MembershipRole,
}

async fn get_identity(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    let scope = ProjectScope::new(authenticated.tenant_id().clone(), project_id.clone());
    let project_epoch =
        service
            .store()
            .project_epoch(&scope)?
            .ok_or_else(|| DomainError::ProjectUnauthorized {
                project_id: project_id.clone(),
            })?;
    Ok(Json(IdentityResponse {
        tenant_id: authenticated.tenant_id().clone(),
        project_id,
        project_epoch,
        actor_id: authenticated.actor_id().clone(),
        role: membership.role,
    }))
}

async fn list_handoffs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    query: Result<Query<HandoffListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Query(query) =
        query.map_err(|error| ApiError(DomainError::InvalidCommand(error.body_text())))?;
    let include_completed = query
        .include_completed
        .unwrap_or(matches!(query.mailbox, HandoffMailbox::Outbox));
    let query = HandoffQuery::new(
        query.mailbox,
        query.source_id,
        include_completed,
        query.before_seq.map(ProjectSequence::new),
        query.limit.unwrap_or(20),
    )?;
    let service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    Ok(Json(service.handoffs(
        &authenticated,
        &membership,
        &project_id,
        &query,
    )?))
}

async fn create_session(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    payload: Result<Json<CreateRequest<SessionCreatePayload>>, JsonRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Json(request) = decode_json(payload)?;
    let resource = resource_ref(SESSION_KIND, request.payload.session_id.as_str())?;
    let envelope = create_envelope(
        request.command_id,
        project_id.clone(),
        "session.create",
        request.payload.clone(),
    )?;
    let mut service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    if let Some(receipt) = service.replay(
        &authenticated,
        &membership,
        &envelope,
        &resource,
        ChangeOperation::Upsert,
    )? {
        return Ok(Json(receipt));
    }
    ensure_absent(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource,
    )?;
    let record = SessionRecord::new(
        request.payload.session_id,
        request.payload.source_id,
        request.payload.topic,
        authenticated.actor_id().clone(),
    )?;
    let receipt = service.execute_with_body(
        &authenticated,
        &membership,
        envelope,
        resource,
        ChangeOperation::Upsert,
        &record,
    )?;
    Ok(Json(receipt))
}

async fn create_fact(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    payload: Result<Json<CreateRequest<FactCreatePayload>>, JsonRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Json(request) = decode_json(payload)?;
    let resource = resource_ref(FACT_KIND, request.payload.fact_id.as_str())?;
    let envelope = create_envelope(
        request.command_id,
        project_id.clone(),
        "fact.create",
        request.payload.clone(),
    )?;
    let mut service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    if let Some(receipt) = service.replay(
        &authenticated,
        &membership,
        &envelope,
        &resource,
        ChangeOperation::Upsert,
    )? {
        return Ok(Json(receipt));
    }
    let (_, session): (_, SessionRecord) = load_record(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource_ref(SESSION_KIND, request.payload.session_id.as_str())?,
    )?;
    ensure_absent(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource,
    )?;
    let record = FactRecord::new(
        request.payload.fact_id,
        session.session_id,
        session.source_id,
        authenticated.actor_id().clone(),
        request.payload.content,
    )?;
    let receipt = service.execute_with_body(
        &authenticated,
        &membership,
        envelope,
        resource,
        ChangeOperation::Upsert,
        &record,
    )?;
    Ok(Json(receipt))
}

async fn send_handoff(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    payload: Result<Json<CreateRequest<HandoffSendPayload>>, JsonRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Json(request) = decode_json(payload)?;
    let resource = resource_ref(HANDOFF_KIND, request.payload.handoff_id.as_str())?;
    let envelope = create_envelope(
        request.command_id,
        project_id.clone(),
        "handoff.send",
        request.payload.clone(),
    )?;
    let mut service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    if let Some(receipt) = service.replay(
        &authenticated,
        &membership,
        &envelope,
        &resource,
        ChangeOperation::Upsert,
    )? {
        return Ok(Json(receipt));
    }
    let (_, session): (_, SessionRecord) = load_record(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource_ref(SESSION_KIND, request.payload.session_id.as_str())?,
    )?;
    require_writable_receiver(
        &service,
        &authenticated,
        &project_id,
        &request.payload.to_actor,
    )?;
    ensure_absent(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource,
    )?;
    let mut record = HandoffRecord::new(
        request.payload.handoff_id,
        &session,
        authenticated.actor_id().clone(),
        request.payload.to_actor,
        request.payload.focus,
        request.payload.done_when,
    )?;
    if let Some(context_id) = request.payload.context_id {
        let (_, context): (_, HandoffContextRecord) = load_record(
            &service,
            &authenticated,
            &membership,
            &project_id,
            &resource_ref(HANDOFF_CONTEXT_KIND, context_id.as_str())?,
        )?;
        record.attach_context(&context)?;
    }
    let receipt = service.execute_with_body(
        &authenticated,
        &membership,
        envelope,
        resource,
        ChangeOperation::Upsert,
        &record,
    )?;
    Ok(Json(receipt))
}

async fn create_handoff_context(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    payload: Result<Json<CreateRequest<HandoffContextCreatePayload>>, JsonRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Json(request) = decode_json(payload)?;
    let resource = resource_ref(HANDOFF_CONTEXT_KIND, request.payload.context_id.as_str())?;
    let envelope = create_envelope(
        request.command_id,
        project_id.clone(),
        "handoff_context.create",
        request.payload.clone(),
    )?;
    let mut service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    if let Some(receipt) = service.replay(
        &authenticated,
        &membership,
        &envelope,
        &resource,
        ChangeOperation::Upsert,
    )? {
        return Ok(Json(receipt));
    }
    let (_, session): (_, SessionRecord) = load_record(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource_ref(SESSION_KIND, request.payload.session_id.as_str())?,
    )?;
    require_writable_receiver(
        &service,
        &authenticated,
        &project_id,
        &request.payload.to_actor,
    )?;
    ensure_absent(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource,
    )?;
    let record = HandoffContextRecord::new(
        request.payload.context_id,
        request.payload.handoff_id,
        &session,
        authenticated.actor_id().clone(),
        request.payload.to_actor,
        request.payload.content,
    )?;
    let receipt = service.execute_with_body(
        &authenticated,
        &membership,
        envelope,
        resource,
        ChangeOperation::Upsert,
        &record,
    )?;
    Ok(Json(receipt))
}

async fn accept_handoff(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, handoff_id)): Path<(String, String)>,
    payload: Result<Json<TransitionRequest<HandoffAcceptPayload>>, JsonRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let handoff_id = HandoffId::try_from(handoff_id)?;
    let Json(request) = decode_json(payload)?;
    let resource = resource_ref(HANDOFF_KIND, handoff_id.as_str())?;
    let envelope = transition_envelope(
        request.command_id,
        project_id.clone(),
        request.expected_revision,
        "handoff.accept",
        request.payload.clone(),
    )?;
    let mut service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    if let Some(receipt) = service.replay(
        &authenticated,
        &membership,
        &envelope,
        &resource,
        ChangeOperation::Upsert,
    )? {
        return Ok(Json(receipt));
    }
    let (revision, mut record): (_, HandoffRecord) = load_record(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource,
    )?;
    require_revision(request.expected_revision, revision)?;
    record.accept_with_lease(
        authenticated.actor_id(),
        request.payload.claim_id,
        current_epoch_ms()?,
        HANDOFF_LEASE_MS,
    )?;
    let receipt = service.execute_with_body(
        &authenticated,
        &membership,
        envelope,
        resource,
        ChangeOperation::Upsert,
        &record,
    )?;
    Ok(Json(receipt))
}

async fn heartbeat_handoff(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, handoff_id)): Path<(String, String)>,
    payload: Result<Json<TransitionRequest<HandoffHeartbeatPayload>>, JsonRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let handoff_id = HandoffId::try_from(handoff_id)?;
    let Json(request) = decode_json(payload)?;
    let resource = resource_ref(HANDOFF_KIND, handoff_id.as_str())?;
    let envelope = transition_envelope(
        request.command_id,
        project_id.clone(),
        request.expected_revision,
        "handoff.heartbeat",
        request.payload.clone(),
    )?;
    let mut service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    if let Some(receipt) = service.replay(
        &authenticated,
        &membership,
        &envelope,
        &resource,
        ChangeOperation::Upsert,
    )? {
        return Ok(Json(receipt));
    }
    let (revision, mut record): (_, HandoffRecord) = load_record(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource,
    )?;
    require_revision(request.expected_revision, revision)?;
    record.heartbeat(
        authenticated.actor_id(),
        &request.payload.claim_id,
        current_epoch_ms()?,
        HANDOFF_LEASE_MS,
    )?;
    let receipt = service.execute_with_body(
        &authenticated,
        &membership,
        envelope,
        resource,
        ChangeOperation::Upsert,
        &record,
    )?;
    Ok(Json(receipt))
}

async fn return_handoff(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, handoff_id)): Path<(String, String)>,
    payload: Result<Json<TransitionRequest<HandoffReturnPayload>>, JsonRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let handoff_id = HandoffId::try_from(handoff_id)?;
    let Json(request) = decode_json(payload)?;
    let resource = resource_ref(HANDOFF_KIND, handoff_id.as_str())?;
    let envelope = transition_envelope(
        request.command_id,
        project_id.clone(),
        request.expected_revision,
        "handoff.return",
        request.payload.clone(),
    )?;
    let mut service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    if let Some(receipt) = service.replay(
        &authenticated,
        &membership,
        &envelope,
        &resource,
        ChangeOperation::Upsert,
    )? {
        return Ok(Json(receipt));
    }
    let (revision, mut record): (_, HandoffRecord) = load_record(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource,
    )?;
    require_revision(request.expected_revision, revision)?;
    let (_, fact): (_, FactRecord) = load_record(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource_ref(FACT_KIND, request.payload.result_fact_id.as_str())?,
    )?;
    record.return_result_at(
        authenticated.actor_id(),
        &request.payload.claim_id,
        &fact,
        request.payload.outcome,
        current_epoch_ms()?,
    )?;
    let receipt = service.execute_with_body(
        &authenticated,
        &membership,
        envelope,
        resource,
        ChangeOperation::Upsert,
        &record,
    )?;
    Ok(Json(receipt))
}

async fn get_handoff(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, handoff_id)): Path<(String, String)>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let handoff_id = HandoffId::try_from(handoff_id)?;
    let service = state.service.lock().await;
    let (authenticated, membership) = request_context(&service, &headers, &project_id)?;
    let (revision, record): (_, HandoffRecord) = load_record(
        &service,
        &authenticated,
        &membership,
        &project_id,
        &resource_ref(HANDOFF_KIND, handoff_id.as_str())?,
    )?;
    if !record.is_visible_to(authenticated.actor_id()) {
        return Err(DomainError::HandoffActorMismatch.into());
    }
    Ok(Json(TypedRecordResponse { revision, record }))
}

pub(super) fn request_context(
    service: &CommandService<SqliteCommandStore>,
    headers: &HeaderMap,
    project_id: &ProjectId,
) -> Result<(AuthenticatedActor, ProjectMembership), DomainError> {
    let digest = bearer_digest_from_headers(headers)?;
    let authenticated = service
        .store()
        .authenticate_token(&digest)?
        .ok_or(DomainError::AuthenticationFailed)?;
    let membership = service
        .store()
        .membership(&authenticated, project_id)?
        .ok_or_else(|| DomainError::ProjectUnauthorized {
            project_id: project_id.clone(),
        })?;
    Ok((authenticated, membership))
}

fn current_epoch_ms() -> Result<i64, DomainError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| DomainError::StorageFailure {
            operation: "system_time",
            detail: error.to_string(),
        })?;
    i64::try_from(duration.as_millis()).map_err(|error| DomainError::StorageFailure {
        operation: "system_time",
        detail: error.to_string(),
    })
}

fn ensure_absent(
    service: &CommandService<SqliteCommandStore>,
    authenticated: &AuthenticatedActor,
    membership: &ProjectMembership,
    project_id: &ProjectId,
    resource: &ResourceRef,
) -> Result<(), DomainError> {
    if service
        .resource(authenticated, membership, project_id, resource)?
        .is_some()
    {
        Err(DomainError::ResourceAlreadyExists)
    } else {
        Ok(())
    }
}

fn load_record<T: DeserializeOwned>(
    service: &CommandService<SqliteCommandStore>,
    authenticated: &AuthenticatedActor,
    membership: &ProjectMembership,
    project_id: &ProjectId,
    resource: &ResourceRef,
) -> Result<(Revision, T), DomainError> {
    let canonical = service
        .resource(authenticated, membership, project_id, resource)?
        .ok_or(DomainError::ResourceNotFound)?;
    decode_record(canonical)
}

fn require_writable_receiver(
    service: &CommandService<SqliteCommandStore>,
    authenticated: &AuthenticatedActor,
    project_id: &ProjectId,
    receiver_id: &ActorId,
) -> Result<(), DomainError> {
    let scope = ProjectScope::new(authenticated.tenant_id().clone(), project_id.clone());
    let receiver = service
        .store()
        .project_membership(&scope, receiver_id)?
        .ok_or_else(|| {
            DomainError::InvalidCommand(
                "handoff receiver is not an active project member".to_owned(),
            )
        })?;
    if !receiver.role.can_mutate() {
        return Err(DomainError::InvalidCommand(
            "handoff receiver must have a writable project membership".to_owned(),
        ));
    }
    Ok(())
}

fn decode_record<T: DeserializeOwned>(
    canonical: CanonicalResource,
) -> Result<(Revision, T), DomainError> {
    match canonical.state {
        ResourceState::Present { body } => {
            let record =
                serde_json::from_slice(&body).map_err(|error| DomainError::StorageFailure {
                    operation: "typed_resource_decode",
                    detail: error.to_string(),
                })?;
            Ok((canonical.revision, record))
        }
        ResourceState::Deleted => Err(DomainError::ResourceNotFound),
    }
}

fn create_envelope<T>(
    command_id: CommandId,
    project_id: ProjectId,
    operation: &str,
    payload: T,
) -> Result<CommandEnvelope<T>, DomainError> {
    Ok(CommandEnvelope {
        command_id,
        project_id,
        expected_revision: None,
        operation: OperationName::try_from(operation)?,
        payload,
    })
}

fn transition_envelope<T>(
    command_id: CommandId,
    project_id: ProjectId,
    expected_revision: Revision,
    operation: &str,
    payload: T,
) -> Result<CommandEnvelope<T>, DomainError> {
    Ok(CommandEnvelope {
        command_id,
        project_id,
        expected_revision: Some(expected_revision),
        operation: OperationName::try_from(operation)?,
        payload,
    })
}

fn resource_ref(kind: &str, id: &str) -> Result<ResourceRef, DomainError> {
    Ok(ResourceRef {
        kind: ResourceKind::try_from(kind)?,
        id: ResourceId::try_from(id)?,
    })
}

fn require_revision(expected: Revision, current: Revision) -> Result<(), DomainError> {
    if expected == current {
        Ok(())
    } else {
        Err(DomainError::StaleRevision { expected, current })
    }
}

fn decode_json<T>(payload: Result<Json<T>, JsonRejection>) -> Result<Json<T>, ApiError> {
    payload.map_err(|error| ApiError(DomainError::InvalidCommand(error.body_text())))
}
