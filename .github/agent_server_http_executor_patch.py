from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_all_expected(text: str, old: str, new: str, expected: int, label: str) -> str:
    count = text.count(old)
    if count != expected:
        raise SystemExit(f"{label}: expected {expected} matches, found {count}")
    return text.replace(old, new)


def function_span(text: str, marker: str) -> tuple[int, int]:
    start = text.find(marker)
    if start < 0:
        raise SystemExit(f"function marker not found: {marker}")
    brace = text.find("{", start)
    if brace < 0:
        raise SystemExit(f"opening brace not found: {marker}")

    depth = 0
    i = brace
    state = "normal"
    block_depth = 0
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""

        if state == "line_comment":
            if ch == "\n":
                state = "normal"
            i += 1
            continue
        if state == "block_comment":
            if ch == "/" and nxt == "*":
                block_depth += 1
                i += 2
                continue
            if ch == "*" and nxt == "/":
                block_depth -= 1
                i += 2
                if block_depth == 0:
                    state = "normal"
                continue
            i += 1
            continue
        if state == "string":
            if ch == "\\":
                i += 2
                continue
            if ch == '"':
                state = "normal"
            i += 1
            continue
        if state == "char":
            if ch == "\\":
                i += 2
                continue
            if ch == "'":
                state = "normal"
            i += 1
            continue

        if ch == "/" and nxt == "/":
            state = "line_comment"
            i += 2
            continue
        if ch == "/" and nxt == "*":
            state = "block_comment"
            block_depth = 1
            i += 2
            continue
        if ch == '"':
            state = "string"
            i += 1
            continue
        if ch == "'":
            # Lifetimes appear before the function body, while this scanner starts
            # at the opening body brace. Treat apostrophes inside the body as chars.
            state = "char"
            i += 1
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return start, i + 1
        i += 1

    raise SystemExit(f"closing brace not found: {marker}")


def replace_function(text: str, marker: str, replacement: str) -> str:
    start, end = function_span(text, marker)
    return text[:start] + replacement.strip() + text[end:]


lib_path = Path("crates/aidememo-server/src/lib.rs")
lib = lib_path.read_text()
lib = replace_once(lib, "mod artifact;\nmod lexical;", "mod artifact;\nmod executor;\nmod lexical;", "declare executor module")
lib = replace_once(lib, "use aidememo_service::CommandService;\n", "", "remove concrete command service import")
lib = replace_once(lib, "use std::sync::Arc;", "use std::{sync::Arc, time::Duration};", "duration import")
lib = replace_once(
    lib,
    "use serde::{Deserialize, Serialize};",
    "use executor::{BlockingStoreError, BlockingStoreExecutor};\nuse serde::{Deserialize, Serialize};",
    "executor import",
)
lib = replace_once(
    lib,
    "const EXTENSION_RESOURCE_PREFIX: &str = \"custom.\";",
    "const EXTENSION_RESOURCE_PREFIX: &str = \"custom.\";\nconst DEFAULT_STORE_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(1);\nconst DEFAULT_STORE_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);",
    "executor defaults",
)
lib = replace_once(
    lib,
    "    service: Arc<Mutex<CommandService<SqliteCommandStore>>>,",
    "    canonical: BlockingStoreExecutor,",
    "server state canonical field",
)
lib = replace_all_expected(
    lib,
    "            service: Arc::new(Mutex::new(CommandService::new(store))),",
    "            canonical: BlockingStoreExecutor::sqlite(\n                store,\n                DEFAULT_STORE_ACQUIRE_TIMEOUT,\n                DEFAULT_STORE_OPERATION_TIMEOUT,\n            ),",
    3,
    "server state constructors",
)
lib = lib.replace("ApiError(DomainError::", "ApiError::from(DomainError::")

lib = replace_function(
    lib,
    "async fn health(",
    r'''
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
''',
)

lib = replace_function(
    lib,
    "async fn command(",
    r'''
async fn command(
    State(state): State<ServerState>,
    headers: HeaderMap,
    payload: Result<Json<CommandRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = payload
        .map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
    validate_resource_command(&request)?;
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
''',
)

lib = replace_function(
    lib,
    "async fn changes(",
    r'''
async fn changes(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    query: Result<Query<ChangeQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Query(query) = query
        .map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
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
''',
)

lib = replace_function(
    lib,
    "async fn materialized_changes(",
    r'''
async fn materialized_changes(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    query: Result<Query<ChangeQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Query(query) = query
        .map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
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
''',
)

lib = replace_function(
    lib,
    "async fn snapshot(",
    r'''
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
''',
)

# Only the canonical snapshot load moves to the blocking executor. Embedding/network
# work below this block intentionally remains on the async side.
old_search_snapshot = '''    let digest = bearer_digest_from_headers(&headers)?;\n    let snapshot = {\n        let service = state.service.lock().await;\n        let authenticated = service\n            .store()\n            .authenticate_token(&digest)?\n            .ok_or(DomainError::AuthenticationFailed)?;\n        let membership = service\n            .store()\n            .membership(&authenticated, &project_id)?\n            .ok_or_else(|| DomainError::ProjectUnauthorized {\n                project_id: project_id.clone(),\n            })?;\n        service.snapshot(&authenticated, &membership, &project_id)?\n    };'''
new_search_snapshot = '''    let digest = bearer_digest_from_headers(&headers)?;\n    let snapshot = state\n        .canonical\n        .run_service(move |service| {\n            let authenticated = service\n                .store()\n                .authenticate_token(&digest)?\n                .ok_or(DomainError::AuthenticationFailed)?;\n            let membership = service\n                .store()\n                .membership(&authenticated, &project_id)?\n                .ok_or_else(|| DomainError::ProjectUnauthorized {\n                    project_id: project_id.clone(),\n                })?;\n            service.snapshot(&authenticated, &membership, &project_id)\n        })\n        .await?;'''
lib = replace_once(lib, old_search_snapshot, new_search_snapshot, "search snapshot executor wiring")

lib = replace_function(
    lib,
    "async fn resource(",
    r'''
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
''',
)

api_start = lib.find("struct ApiError(DomainError);")
api_end = lib.find("fn status_for_error", api_start)
if api_start < 0 or api_end < 0:
    raise SystemExit("ApiError section not found")
new_api = r'''
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
                error
                @ (BlockingStoreError::Saturated
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

'''
lib = lib[:api_start] + new_api + lib[api_end:]

lib += r'''

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
'''

if "state.service" in lib:
    raise SystemExit("lib.rs still contains direct state.service access")
lib_path.write_text(lib)


product_path = Path("crates/aidememo-server/src/product.rs")
product = product_path.read_text()
product = replace_once(
    product,
    "    ProjectScope, ProjectSequence, ResourceId, ResourceKind, ResourceRef, ResourceState, Revision,\n    SessionId, SessionRecord, SourceId,",
    "    ProjectScope, ProjectSequence, ResourceId, ResourceKind, ResourceRef, ResourceState, Revision,\n    ServerCanonicalStore, SessionId, SessionRecord, SourceId,",
    "product canonical trait import",
)
product = replace_once(product, "use aidememo_store_local::SqliteCommandStore;\n", "", "remove product sqlite import")
product = replace_once(
    product,
    "const HANDOFF_LEASE_MS: i64 = 120_000;",
    "const HANDOFF_LEASE_MS: i64 = 120_000;\n\ntype CanonicalService<'store> = CommandService<&'store mut dyn ServerCanonicalStore>;",
    "product service alias",
)
product = product.replace("ApiError(DomainError::", "ApiError::from(DomainError::")

product = replace_function(
    product,
    "async fn get_identity(",
    r'''
async fn get_identity(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let digest = bearer_digest_from_headers(&headers)?;
    let response = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            let scope = ProjectScope::new(authenticated.tenant_id().clone(), project_id.clone());
            let project_epoch = service
                .store()
                .project_epoch(&scope)?
                .ok_or_else(|| DomainError::ProjectUnauthorized {
                    project_id: project_id.clone(),
                })?;
            Ok(IdentityResponse {
                tenant_id: authenticated.tenant_id().clone(),
                project_id,
                project_epoch,
                actor_id: authenticated.actor_id().clone(),
                role: membership.role,
            })
        })
        .await?;
    Ok(Json(response))
}
''',
)

product = replace_function(
    product,
    "async fn list_handoffs(",
    r'''
async fn list_handoffs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    query: Result<Query<HandoffListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let Query(query) = query
        .map_err(|error| ApiError::from(DomainError::InvalidCommand(error.body_text())))?;
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
    let digest = bearer_digest_from_headers(&headers)?;
    let handoffs = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            service.handoffs(&authenticated, &membership, &project_id, &query)
        })
        .await?;
    Ok(Json(handoffs))
}
''',
)

product = replace_function(
    product,
    "async fn create_session(",
    r'''
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
    let digest = bearer_digest_from_headers(&headers)?;
    let receipt = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            if let Some(receipt) = service.replay(
                &authenticated,
                &membership,
                &envelope,
                &resource,
                ChangeOperation::Upsert,
            )? {
                return Ok(receipt);
            }
            ensure_absent(service, &authenticated, &membership, &project_id, &resource)?;
            let record = SessionRecord::new(
                request.payload.session_id,
                request.payload.source_id,
                request.payload.topic,
                authenticated.actor_id().clone(),
            )?;
            service.execute_with_body(
                &authenticated,
                &membership,
                envelope,
                resource,
                ChangeOperation::Upsert,
                &record,
            )
        })
        .await?;
    Ok(Json(receipt))
}
''',
)

product = replace_function(
    product,
    "async fn create_fact(",
    r'''
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
    let digest = bearer_digest_from_headers(&headers)?;
    let receipt = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            if let Some(receipt) = service.replay(
                &authenticated,
                &membership,
                &envelope,
                &resource,
                ChangeOperation::Upsert,
            )? {
                return Ok(receipt);
            }
            let (_, session): (_, SessionRecord) = load_record(
                service,
                &authenticated,
                &membership,
                &project_id,
                &resource_ref(SESSION_KIND, request.payload.session_id.as_str())?,
            )?;
            ensure_absent(service, &authenticated, &membership, &project_id, &resource)?;
            let record = FactRecord::new(
                request.payload.fact_id,
                session.session_id,
                session.source_id,
                authenticated.actor_id().clone(),
                request.payload.content,
            )?;
            service.execute_with_body(
                &authenticated,
                &membership,
                envelope,
                resource,
                ChangeOperation::Upsert,
                &record,
            )
        })
        .await?;
    Ok(Json(receipt))
}
''',
)

product = replace_function(
    product,
    "async fn send_handoff(",
    r'''
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
    let digest = bearer_digest_from_headers(&headers)?;
    let receipt = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            if let Some(receipt) = service.replay(
                &authenticated,
                &membership,
                &envelope,
                &resource,
                ChangeOperation::Upsert,
            )? {
                return Ok(receipt);
            }
            let (_, session): (_, SessionRecord) = load_record(
                service,
                &authenticated,
                &membership,
                &project_id,
                &resource_ref(SESSION_KIND, request.payload.session_id.as_str())?,
            )?;
            require_writable_receiver(
                service,
                &authenticated,
                &project_id,
                &request.payload.to_actor,
            )?;
            ensure_absent(service, &authenticated, &membership, &project_id, &resource)?;
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
                    service,
                    &authenticated,
                    &membership,
                    &project_id,
                    &resource_ref(HANDOFF_CONTEXT_KIND, context_id.as_str())?,
                )?;
                record.attach_context(&context)?;
            }
            service.execute_with_body(
                &authenticated,
                &membership,
                envelope,
                resource,
                ChangeOperation::Upsert,
                &record,
            )
        })
        .await?;
    Ok(Json(receipt))
}
''',
)

product = replace_function(
    product,
    "async fn create_handoff_context(",
    r'''
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
    let digest = bearer_digest_from_headers(&headers)?;
    let receipt = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            if let Some(receipt) = service.replay(
                &authenticated,
                &membership,
                &envelope,
                &resource,
                ChangeOperation::Upsert,
            )? {
                return Ok(receipt);
            }
            let (_, session): (_, SessionRecord) = load_record(
                service,
                &authenticated,
                &membership,
                &project_id,
                &resource_ref(SESSION_KIND, request.payload.session_id.as_str())?,
            )?;
            require_writable_receiver(
                service,
                &authenticated,
                &project_id,
                &request.payload.to_actor,
            )?;
            ensure_absent(service, &authenticated, &membership, &project_id, &resource)?;
            let record = HandoffContextRecord::new(
                request.payload.context_id,
                request.payload.handoff_id,
                &session,
                authenticated.actor_id().clone(),
                request.payload.to_actor,
                request.payload.content,
            )?;
            service.execute_with_body(
                &authenticated,
                &membership,
                envelope,
                resource,
                ChangeOperation::Upsert,
                &record,
            )
        })
        .await?;
    Ok(Json(receipt))
}
''',
)

product = replace_function(
    product,
    "async fn accept_handoff(",
    r'''
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
    let digest = bearer_digest_from_headers(&headers)?;
    let now_ms = current_epoch_ms()?;
    let receipt = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            if let Some(receipt) = service.replay(
                &authenticated,
                &membership,
                &envelope,
                &resource,
                ChangeOperation::Upsert,
            )? {
                return Ok(receipt);
            }
            let (revision, mut record): (_, HandoffRecord) = load_record(
                service,
                &authenticated,
                &membership,
                &project_id,
                &resource,
            )?;
            require_revision(request.expected_revision, revision)?;
            record.accept_with_lease(
                authenticated.actor_id(),
                request.payload.claim_id,
                now_ms,
                HANDOFF_LEASE_MS,
            )?;
            service.execute_with_body(
                &authenticated,
                &membership,
                envelope,
                resource,
                ChangeOperation::Upsert,
                &record,
            )
        })
        .await?;
    Ok(Json(receipt))
}
''',
)

product = replace_function(
    product,
    "async fn heartbeat_handoff(",
    r'''
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
    let digest = bearer_digest_from_headers(&headers)?;
    let now_ms = current_epoch_ms()?;
    let receipt = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            if let Some(receipt) = service.replay(
                &authenticated,
                &membership,
                &envelope,
                &resource,
                ChangeOperation::Upsert,
            )? {
                return Ok(receipt);
            }
            let (revision, mut record): (_, HandoffRecord) = load_record(
                service,
                &authenticated,
                &membership,
                &project_id,
                &resource,
            )?;
            require_revision(request.expected_revision, revision)?;
            record.heartbeat(
                authenticated.actor_id(),
                &request.payload.claim_id,
                now_ms,
                HANDOFF_LEASE_MS,
            )?;
            service.execute_with_body(
                &authenticated,
                &membership,
                envelope,
                resource,
                ChangeOperation::Upsert,
                &record,
            )
        })
        .await?;
    Ok(Json(receipt))
}
''',
)

product = replace_function(
    product,
    "async fn return_handoff(",
    r'''
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
    let digest = bearer_digest_from_headers(&headers)?;
    let now_ms = current_epoch_ms()?;
    let receipt = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            if let Some(receipt) = service.replay(
                &authenticated,
                &membership,
                &envelope,
                &resource,
                ChangeOperation::Upsert,
            )? {
                return Ok(receipt);
            }
            let (revision, mut record): (_, HandoffRecord) = load_record(
                service,
                &authenticated,
                &membership,
                &project_id,
                &resource,
            )?;
            require_revision(request.expected_revision, revision)?;
            let (_, fact): (_, FactRecord) = load_record(
                service,
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
                now_ms,
            )?;
            service.execute_with_body(
                &authenticated,
                &membership,
                envelope,
                resource,
                ChangeOperation::Upsert,
                &record,
            )
        })
        .await?;
    Ok(Json(receipt))
}
''',
)

product = replace_function(
    product,
    "async fn get_handoff(",
    r'''
async fn get_handoff(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((project_id, handoff_id)): Path<(String, String)>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let project_id = ProjectId::try_from(project_id)?;
    let handoff_id = HandoffId::try_from(handoff_id)?;
    let digest = bearer_digest_from_headers(&headers)?;
    let response = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            let (revision, record): (_, HandoffRecord) = load_record(
                service,
                &authenticated,
                &membership,
                &project_id,
                &resource_ref(HANDOFF_KIND, handoff_id.as_str())?,
            )?;
            if !record.is_visible_to(authenticated.actor_id()) {
                return Err(DomainError::HandoffActorMismatch);
            }
            Ok(TypedRecordResponse { revision, record })
        })
        .await?;
    Ok(Json(response))
}
''',
)

product = replace_function(
    product,
    "pub(super) fn request_context(",
    r'''
pub(super) fn request_context(
    service: &CanonicalService<'_>,
    digest: &[u8; 32],
    project_id: &ProjectId,
) -> Result<(AuthenticatedActor, ProjectMembership), DomainError> {
    let authenticated = service
        .store()
        .authenticate_token(digest)?
        .ok_or(DomainError::AuthenticationFailed)?;
    let membership = service
        .store()
        .membership(&authenticated, project_id)?
        .ok_or_else(|| DomainError::ProjectUnauthorized {
            project_id: project_id.clone(),
        })?;
    Ok((authenticated, membership))
}
''',
)

product = replace_function(
    product,
    "fn ensure_absent(",
    r'''
fn ensure_absent(
    service: &CanonicalService<'_>,
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
''',
)

product = replace_function(
    product,
    "fn load_record<T: DeserializeOwned>(",
    r'''
fn load_record<T: DeserializeOwned>(
    service: &CanonicalService<'_>,
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
''',
)

product = replace_function(
    product,
    "fn require_writable_receiver(",
    r'''
fn require_writable_receiver(
    service: &CanonicalService<'_>,
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
''',
)

if "state.service" in product:
    raise SystemExit("product.rs still contains direct state.service access")
if "CommandService<SqliteCommandStore>" in product:
    raise SystemExit("product.rs still contains concrete SQLite service signatures")
product_path.write_text(product)


artifact_path = Path("crates/aidememo-server/src/artifact.rs")
artifact = artifact_path.read_text()
artifact = replace_once(
    artifact,
    "use super::{ApiError, ArtifactBodies, ArtifactState, ServerState, product::request_context};",
    "use super::{\n    ApiError, ArtifactBodies, ArtifactState, ServerState, bearer_digest_from_headers,\n    executor::BlockingStoreError, product::request_context,\n};",
    "artifact executor imports",
)
artifact = replace_function(
    artifact,
    "async fn authorize(",
    r'''
async fn authorize(
    state: &ServerState,
    headers: &HeaderMap,
    project_id: &ProjectId,
    mutation: bool,
) -> Result<ProjectScope, ArtifactHttpError> {
    let digest = bearer_digest_from_headers(headers)?;
    let project_id = project_id.clone();
    let scope = state
        .canonical
        .run_service(move |service| {
            let (authenticated, membership) = request_context(service, &digest, &project_id)?;
            if mutation && !membership.role.can_mutate() {
                return Err(DomainError::ProjectUnauthorized {
                    project_id: project_id.clone(),
                });
            }
            Ok(ProjectScope::new(
                authenticated.tenant_id().clone(),
                project_id,
            ))
        })
        .await?;
    Ok(scope)
}
''',
)
artifact = replace_once(
    artifact,
    "enum ArtifactHttpError {\n    Domain(DomainError),\n    Store(ArtifactStoreError),",
    "enum ArtifactHttpError {\n    Domain(DomainError),\n    Executor(BlockingStoreError),\n    Store(ArtifactStoreError),",
    "artifact executor error variant",
)
artifact = replace_once(
    artifact,
    "impl From<ArtifactStoreError> for ArtifactHttpError {",
    "impl From<BlockingStoreError> for ArtifactHttpError {\n    fn from(error: BlockingStoreError) -> Self {\n        match error {\n            BlockingStoreError::Domain(error) => Self::Domain(error),\n            error => Self::Executor(error),\n        }\n    }\n}\n\nimpl From<ArtifactStoreError> for ArtifactHttpError {",
    "artifact executor conversion",
)
artifact = replace_once(
    artifact,
    "        match self {\n            Self::Domain(error) => ApiError::from(error).into_response(),",
    "        match self {\n            Self::Domain(error) => ApiError::from(error).into_response(),\n            Self::Executor(error) => ApiError::from(error).into_response(),",
    "artifact executor response mapping",
)
if "state.service" in artifact:
    raise SystemExit("artifact.rs still contains direct state.service access")
artifact_path.write_text(artifact)

print("HTTP executor wiring patch applied")
