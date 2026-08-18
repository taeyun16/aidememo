//! Stateless HTTP MCP gateway over the authenticated typed SSOT surface.
//!
//! The gateway intentionally delegates product operations back through the
//! server's existing typed REST router. Authentication, membership, CAS,
//! participant visibility, and canonical validation therefore have one source
//! of truth instead of a second MCP-specific implementation.

use super::ServerState;
use aidememo_domain::{DomainError, ProjectId};
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tower::ServiceExt;

const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
const MCP_PROTOCOL_HEADER: &str = "mcp-protocol-version";
const MCP_METHOD_HEADER: &str = "mcp-method";
const MCP_NAME_HEADER: &str = "mcp-name";
const SERVER_NAME: &str = "aidememo-server";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const TOOLS_TTL_MS: u64 = 60_000;

/// Routes owned by the stateless MCP gateway.
pub(super) fn routes() -> axum::Router<ServerState> {
    axum::Router::new().route(
        "/v1/projects/{project_id}/mcp",
        axum::routing::post(mcp_post),
    )
}

#[derive(Debug, Deserialize)]
struct McpRequest {
    jsonrpc: String,
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

async fn mcp_post(
    State(state): State<ServerState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let project_id = match ProjectId::try_from(project_id) {
        Ok(project_id) => project_id,
        Err(error) => return protocol_error(StatusCode::BAD_REQUEST, Value::Null, -32602, error.to_string()),
    };
    let raw = match payload {
        Ok(Json(raw)) => raw,
        Err(error) => {
            return protocol_error(
                StatusCode::BAD_REQUEST,
                Value::Null,
                -32700,
                error.body_text(),
            );
        }
    };
    let request = match serde_json::from_value::<McpRequest>(raw) {
        Ok(request) if request.jsonrpc == "2.0" && !request.id.is_null() => request,
        Ok(_) => {
            return protocol_error(
                StatusCode::BAD_REQUEST,
                Value::Null,
                -32600,
                "MCP requests must use JSON-RPC 2.0 and include a non-null id",
            );
        }
        Err(error) => {
            return protocol_error(
                StatusCode::BAD_REQUEST,
                Value::Null,
                -32600,
                format!("invalid MCP request: {error}"),
            );
        }
    };

    if let Err(error) = validate_protocol_headers(&headers, &request) {
        return protocol_error(StatusCode::BAD_REQUEST, request.id, -32020, error);
    }
    if let Err(error) = validate_meta_protocol(&request.params) {
        return protocol_error(StatusCode::BAD_REQUEST, request.id, -32022, error);
    }

    let authorization = match headers.get(AUTHORIZATION).cloned() {
        Some(authorization) => authorization,
        None => return unauthorized_response(),
    };
    let identity = match authenticated_identity(&state, &project_id, &authorization).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };

    let result = match request.method.as_str() {
        "server/discover" => Ok(discover_result()),
        "tools/list" => list_tools(&request.params, identity.get("role").and_then(Value::as_str)),
        "tools/call" => call_tool(
            &state,
            &project_id,
            &authorization,
            &request.params,
        )
        .await,
        _ => Err(McpFailure::protocol(
            StatusCode::NOT_FOUND,
            -32601,
            format!("unsupported MCP method {}", request.method),
        )),
    };

    match result {
        Ok(result) => Json(json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": stamp_server_info(result),
        }))
        .into_response(),
        Err(error) => protocol_error(error.status, request.id, error.code, error.message),
    }
}

fn validate_protocol_headers(headers: &HeaderMap, request: &McpRequest) -> Result<(), String> {
    let protocol = header_text(headers, MCP_PROTOCOL_HEADER)
        .ok_or_else(|| "missing MCP-Protocol-Version header".to_owned())?;
    if protocol != MCP_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported MCP protocol version {protocol}; supported={MCP_PROTOCOL_VERSION}"
        ));
    }
    let method = header_text(headers, MCP_METHOD_HEADER)
        .ok_or_else(|| "missing Mcp-Method header".to_owned())?;
    if method != request.method {
        return Err(format!(
            "Mcp-Method header {method} does not match body method {}",
            request.method
        ));
    }
    if request.method == "tools/call" {
        let body_name = request
            .params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "tools/call params.name must be a string".to_owned())?;
        let header_name = header_text(headers, MCP_NAME_HEADER)
            .ok_or_else(|| "tools/call requires Mcp-Name".to_owned())?;
        if header_name != body_name {
            return Err(format!(
                "Mcp-Name header {header_name} does not match body tool name {body_name}"
            ));
        }
    }
    Ok(())
}

fn validate_meta_protocol(params: &Value) -> Result<(), String> {
    let Some(meta) = params.get("_meta") else {
        return Err("MCP 2026-07-28 request params must include _meta".to_owned());
    };
    let version = meta
        .get("io.modelcontextprotocol/protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "request _meta must include io.modelcontextprotocol/protocolVersion".to_owned()
        })?;
    if version != MCP_PROTOCOL_VERSION {
        return Err(format!(
            "request metadata protocol version {version} is unsupported; supported={MCP_PROTOCOL_VERSION}"
        ));
    }
    Ok(())
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

async fn authenticated_identity(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
) -> Result<Value, Response> {
    let response = dispatch(
        state,
        Method::GET,
        &format!("/v1/projects/{}/identity", project_id.as_str()),
        authorization,
        None,
    )
    .await;
    let status = response.status();
    let (parts, body) = response.into_parts();
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return Err(protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                Value::Null,
                -32603,
                format!("read internal identity response: {error}"),
            ));
        }
    };
    if !status.is_success() {
        return Err(Response::from_parts(parts, Body::from(bytes)));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            Value::Null,
            -32603,
            format!("decode internal identity response: {error}"),
        )
    })
}

fn discover_result() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": [MCP_PROTOCOL_VERSION],
        "capabilities": {
            "tools": {}
        },
        "instructions": "Use search/resource tools for canonical memory reads and typed session/fact/handoff tools for writes. Project, tenant, and actor identity are fixed by this authenticated MCP endpoint and cannot be overridden.",
        "ttlMs": TOOLS_TTL_MS,
        "cacheScope": "private"
    })
}

fn list_tools(params: &Value, role: Option<&str>) -> Result<Value, McpFailure> {
    let params = params.as_object().ok_or_else(|| McpFailure::invalid_params("tools/list params must be an object"))?;
    ensure_only(params, &["cursor", "_meta"])?;
    if params.get("cursor").is_some_and(|cursor| !cursor.is_null()) {
        return Err(McpFailure::invalid_params(
            "AideMemo tools/list currently has one deterministic page; cursor must be omitted",
        ));
    }
    let read_only = role == Some("reader");
    let mut tools = vec![
        tool_search(),
        tool_resource_get(),
        tool_handoff_list(),
        tool_handoff_status(),
    ];
    if !read_only {
        tools.extend([
            tool_session_create(),
            tool_fact_create(),
            tool_handoff_context_create(),
            tool_handoff_send(),
            tool_handoff_accept(),
            tool_handoff_return(),
        ]);
    }
    Ok(json!({
        "resultType": "complete",
        "tools": tools,
        "ttlMs": TOOLS_TTL_MS,
        "cacheScope": "private"
    }))
}

async fn call_tool(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    params: &Value,
) -> Result<Value, McpFailure> {
    let params = params.as_object().ok_or_else(|| McpFailure::invalid_params("tools/call params must be an object"))?;
    ensure_only(params, &["name", "arguments", "_meta", "inputResponses", "requestState"])?;
    let name = required_string(params, "name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let arguments = arguments
        .as_object()
        .ok_or_else(|| McpFailure::invalid_params("tools/call arguments must be an object"))?;

    let response = match name {
        "search" => dispatch_search(state, project_id, authorization, arguments).await?,
        "resource_get" => dispatch_resource_get(state, project_id, authorization, arguments).await?,
        "session_create" => dispatch_session_create(state, project_id, authorization, arguments).await?,
        "fact_create" => dispatch_fact_create(state, project_id, authorization, arguments).await?,
        "handoff_context_create" => {
            dispatch_handoff_context_create(state, project_id, authorization, arguments).await?
        }
        "handoff_send" => dispatch_handoff_send(state, project_id, authorization, arguments).await?,
        "handoff_list" => dispatch_handoff_list(state, project_id, authorization, arguments).await?,
        "handoff_status" => dispatch_handoff_status(state, project_id, authorization, arguments).await?,
        "handoff_accept" => dispatch_handoff_accept(state, project_id, authorization, arguments).await?,
        "handoff_return" => dispatch_handoff_return(state, project_id, authorization, arguments).await?,
        _ => {
            return Err(McpFailure::protocol(
                StatusCode::NOT_FOUND,
                -32601,
                format!("unknown tool {name}"),
            ));
        }
    };
    rest_response_to_tool_result(response).await
}

async fn dispatch_search(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(args, &["q", "source_id", "limit", "at_least_seq"])?;
    let q = required_string(args, "q")?;
    let mut query = vec![format!("q={}", encode_query(q))];
    push_optional_string(&mut query, args, "source_id")?;
    push_optional_u64(&mut query, args, "limit")?;
    push_optional_u64(&mut query, args, "at_least_seq")?;
    Ok(dispatch(
        state,
        Method::GET,
        &format!(
            "/v1/projects/{}/search?{}",
            project_id.as_str(),
            query.join("&")
        ),
        authorization,
        None,
    )
    .await)
}

async fn dispatch_resource_get(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(args, &["kind", "id"])?;
    let kind = required_path_segment(args, "kind")?;
    let id = required_path_segment(args, "id")?;
    Ok(dispatch(
        state,
        Method::GET,
        &format!(
            "/v1/projects/{}/resources/{kind}/{id}",
            project_id.as_str()
        ),
        authorization,
        None,
    )
    .await)
}

async fn dispatch_session_create(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(args, &["operation_id", "session_id", "source_id", "topic"])?;
    let operation_id = required_identifier(args, "operation_id")?;
    let session_id = required_identifier(args, "session_id")?;
    let topic = required_string(args, "topic")?;
    let source_id = optional_string(args, "source_id")?;
    let body = json!({
        "command_id": operation_id,
        "payload": {
            "session_id": session_id,
            "source_id": source_id,
            "topic": topic
        }
    });
    Ok(dispatch(
        state,
        Method::POST,
        &format!("/v1/projects/{}/sessions", project_id.as_str()),
        authorization,
        Some(body),
    )
    .await)
}

async fn dispatch_fact_create(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(args, &["operation_id", "fact_id", "session_id", "content"])?;
    let body = json!({
        "command_id": required_identifier(args, "operation_id")?,
        "payload": {
            "fact_id": required_identifier(args, "fact_id")?,
            "session_id": required_identifier(args, "session_id")?,
            "content": required_string(args, "content")?
        }
    });
    Ok(dispatch(
        state,
        Method::POST,
        &format!("/v1/projects/{}/facts", project_id.as_str()),
        authorization,
        Some(body),
    )
    .await)
}

async fn dispatch_handoff_context_create(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(
        args,
        &[
            "operation_id",
            "context_id",
            "handoff_id",
            "session_id",
            "to_actor",
            "content",
        ],
    )?;
    let body = json!({
        "command_id": required_identifier(args, "operation_id")?,
        "payload": {
            "context_id": required_identifier(args, "context_id")?,
            "handoff_id": required_identifier(args, "handoff_id")?,
            "session_id": required_identifier(args, "session_id")?,
            "to_actor": required_identifier(args, "to_actor")?,
            "content": required_string(args, "content")?
        }
    });
    Ok(dispatch(
        state,
        Method::POST,
        &format!("/v1/projects/{}/handoff-contexts", project_id.as_str()),
        authorization,
        Some(body),
    )
    .await)
}

async fn dispatch_handoff_send(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(
        args,
        &[
            "operation_id",
            "handoff_id",
            "session_id",
            "to_actor",
            "focus",
            "done_when",
            "context_id",
        ],
    )?;
    let body = json!({
        "command_id": required_identifier(args, "operation_id")?,
        "payload": {
            "handoff_id": required_identifier(args, "handoff_id")?,
            "session_id": required_identifier(args, "session_id")?,
            "to_actor": required_identifier(args, "to_actor")?,
            "focus": optional_string(args, "focus")?,
            "done_when": optional_string(args, "done_when")?,
            "context_id": optional_string(args, "context_id")?
        }
    });
    Ok(dispatch(
        state,
        Method::POST,
        &format!("/v1/projects/{}/handoffs", project_id.as_str()),
        authorization,
        Some(body),
    )
    .await)
}

async fn dispatch_handoff_list(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(
        args,
        &["box", "source_id", "include_completed", "before_seq", "limit"],
    )?;
    let mailbox = required_string(args, "box")?;
    if !matches!(mailbox, "inbox" | "outbox") {
        return Err(McpFailure::invalid_params("box must be inbox or outbox"));
    }
    let mut query = vec![format!("box={mailbox}")];
    push_optional_string(&mut query, args, "source_id")?;
    push_optional_bool(&mut query, args, "include_completed")?;
    push_optional_u64(&mut query, args, "before_seq")?;
    push_optional_u64(&mut query, args, "limit")?;
    Ok(dispatch(
        state,
        Method::GET,
        &format!(
            "/v1/projects/{}/handoffs?{}",
            project_id.as_str(),
            query.join("&")
        ),
        authorization,
        None,
    )
    .await)
}

async fn dispatch_handoff_status(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(args, &["handoff_id"])?;
    let handoff_id = required_path_segment(args, "handoff_id")?;
    Ok(dispatch(
        state,
        Method::GET,
        &format!(
            "/v1/projects/{}/handoffs/{handoff_id}",
            project_id.as_str()
        ),
        authorization,
        None,
    )
    .await)
}

async fn dispatch_handoff_accept(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(
        args,
        &["operation_id", "handoff_id", "expected_revision", "claim_id"],
    )?;
    let handoff_id = required_path_segment(args, "handoff_id")?;
    let body = json!({
        "command_id": required_identifier(args, "operation_id")?,
        "expected_revision": required_u64(args, "expected_revision")?,
        "payload": {"claim_id": required_identifier(args, "claim_id")?}
    });
    Ok(dispatch(
        state,
        Method::POST,
        &format!(
            "/v1/projects/{}/handoffs/{handoff_id}/accept",
            project_id.as_str()
        ),
        authorization,
        Some(body),
    )
    .await)
}

async fn dispatch_handoff_return(
    state: &ServerState,
    project_id: &ProjectId,
    authorization: &HeaderValue,
    args: &Map<String, Value>,
) -> Result<Response, McpFailure> {
    ensure_only(
        args,
        &[
            "operation_id",
            "handoff_id",
            "expected_revision",
            "claim_id",
            "result_fact_id",
            "outcome",
        ],
    )?;
    let handoff_id = required_path_segment(args, "handoff_id")?;
    let outcome = required_string(args, "outcome")?;
    if !matches!(outcome, "succeeded" | "failed") {
        return Err(McpFailure::invalid_params(
            "outcome must be succeeded or failed",
        ));
    }
    let body = json!({
        "command_id": required_identifier(args, "operation_id")?,
        "expected_revision": required_u64(args, "expected_revision")?,
        "payload": {
            "claim_id": required_identifier(args, "claim_id")?,
            "result_fact_id": required_identifier(args, "result_fact_id")?,
            "outcome": outcome
        }
    });
    Ok(dispatch(
        state,
        Method::POST,
        &format!(
            "/v1/projects/{}/handoffs/{handoff_id}/return",
            project_id.as_str()
        ),
        authorization,
        Some(body),
    )
    .await)
}

async fn dispatch(
    state: &ServerState,
    method: Method,
    uri: &str,
    authorization: &HeaderValue,
    body: Option<Value>,
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(AUTHORIZATION, authorization.clone());
    let request_body = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    match builder.body(request_body) {
        Ok(request) => match super::router(state.clone()).oneshot(request).await {
            Ok(response) => response,
            Err(error) => protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                Value::Null,
                -32603,
                format!("internal MCP dispatch failed: {error}"),
            ),
        },
        Err(error) => protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            Value::Null,
            -32603,
            format!("construct internal MCP dispatch: {error}"),
        ),
    }
}

async fn rest_response_to_tool_result(response: Response) -> Result<Value, McpFailure> {
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|error| McpFailure::internal(format!("read internal REST response: {error}")))?
        .to_bytes();
    let structured = serde_json::from_slice::<Value>(&bytes)
        .unwrap_or_else(|_| json!({"http_status": status.as_u16(), "body": String::from_utf8_lossy(&bytes)}));
    let is_error = !status.is_success();
    let text = if is_error {
        format!("AideMemo tool request failed with HTTP {}: {structured}", status.as_u16())
    } else {
        structured.to_string()
    };
    Ok(json!({
        "resultType": "complete",
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured,
        "isError": is_error
    }))
}

fn stamp_server_info(mut result: Value) -> Value {
    let Some(object) = result.as_object_mut() else {
        return result;
    };
    let meta = object
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(meta) = meta.as_object_mut() {
        meta.insert(
            "io.modelcontextprotocol/serverInfo".to_owned(),
            json!({"name": SERVER_NAME, "version": SERVER_VERSION}),
        );
    }
    result
}

fn protocol_error(
    status: StatusCode,
    id: Value,
    code: i64,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message.into()
            }
        })),
    )
        .into_response()
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "code": "authentication_failed",
                "message": "authentication failed"
            }
        })),
    )
        .into_response()
}

struct McpFailure {
    status: StatusCode,
    code: i64,
    message: String,
}

impl McpFailure {
    fn protocol(status: StatusCode, code: i64, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::protocol(StatusCode::BAD_REQUEST, -32602, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::protocol(StatusCode::INTERNAL_SERVER_ERROR, -32603, message)
    }
}

fn ensure_only(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), McpFailure> {
    if let Some(unexpected) = args
        .keys()
        .find(|key| !allowed.iter().any(|allowed| *allowed == key.as_str()))
    {
        return Err(McpFailure::invalid_params(format!(
            "unexpected argument {unexpected}"
        )));
    }
    Ok(())
}

fn required_string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, McpFailure> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpFailure::invalid_params(format!("{key} must be a non-empty string")))
}

fn optional_string(args: &Map<String, Value>, key: &str) -> Result<Option<String>, McpFailure> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(McpFailure::invalid_params(format!(
            "{key} must be a string or null"
        ))),
    }
}

fn required_u64(args: &Map<String, Value>, key: &str) -> Result<u64, McpFailure> {
    args.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| McpFailure::invalid_params(format!("{key} must be an unsigned integer")))
}

fn required_identifier<'a>(
    args: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, McpFailure> {
    let value = required_string(args, key)?;
    validate_path_segment(value)
        .map_err(|error| McpFailure::invalid_params(format!("invalid {key}: {error}")))?;
    Ok(value)
}

fn required_path_segment<'a>(
    args: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, McpFailure> {
    required_identifier(args, key)
}

fn validate_path_segment(value: &str) -> Result<(), DomainError> {
    let _ = aidememo_domain::ResourceId::try_from(value)?;
    Ok(())
}

fn push_optional_string(
    query: &mut Vec<String>,
    args: &Map<String, Value>,
    key: &str,
) -> Result<(), McpFailure> {
    if let Some(value) = optional_string(args, key)? {
        query.push(format!("{key}={}", encode_query(&value)));
    }
    Ok(())
}

fn push_optional_u64(
    query: &mut Vec<String>,
    args: &Map<String, Value>,
    key: &str,
) -> Result<(), McpFailure> {
    if let Some(value) = args.get(key) {
        let value = value.as_u64().ok_or_else(|| {
            McpFailure::invalid_params(format!("{key} must be an unsigned integer"))
        })?;
        query.push(format!("{key}={value}"));
    }
    Ok(())
}

fn push_optional_bool(
    query: &mut Vec<String>,
    args: &Map<String, Value>,
    key: &str,
) -> Result<(), McpFailure> {
    if let Some(value) = args.get(key) {
        let value = value
            .as_bool()
            .ok_or_else(|| McpFailure::invalid_params(format!("{key} must be boolean")))?;
        query.push(format!("{key}={value}"));
    }
    Ok(())
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn tool_search() -> Value {
    json!({
        "name": "search",
        "title": "Search canonical memory",
        "description": "Search canonical fact memory with an optional source namespace and sequence freshness requirement.",
        "inputSchema": object_schema(
            json!({
                "q": {"type": "string", "minLength": 1},
                "source_id": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100},
                "at_least_seq": {"type": "integer", "minimum": 0}
            }),
            &["q"]
        ),
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

fn tool_resource_get() -> Value {
    json!({
        "name": "resource_get",
        "title": "Read canonical resource",
        "description": "Read one canonical resource through authenticated visibility rules.",
        "inputSchema": object_schema(
            json!({"kind": {"type": "string"}, "id": {"type": "string"}}),
            &["kind", "id"]
        ),
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

fn tool_session_create() -> Value {
    write_tool(
        "session_create",
        "Create canonical session",
        "Create a canonical tracked session. operation_id is the retry-stable idempotency key.",
        json!({
            "operation_id": {"type": "string"},
            "session_id": {"type": "string"},
            "source_id": {"type": ["string", "null"]},
            "topic": {"type": "string", "minLength": 1}
        }),
        &["operation_id", "session_id", "topic"],
    )
}

fn tool_fact_create() -> Value {
    write_tool(
        "fact_create",
        "Create canonical fact",
        "Create result or memory evidence attached to an existing canonical session.",
        json!({
            "operation_id": {"type": "string"},
            "fact_id": {"type": "string"},
            "session_id": {"type": "string"},
            "content": {"type": "string", "minLength": 1}
        }),
        &["operation_id", "fact_id", "session_id", "content"],
    )
}

fn tool_handoff_context_create() -> Value {
    write_tool(
        "handoff_context_create",
        "Create handoff context",
        "Persist the canonical context packet referenced by a handoff.",
        json!({
            "operation_id": {"type": "string"},
            "context_id": {"type": "string"},
            "handoff_id": {"type": "string"},
            "session_id": {"type": "string"},
            "to_actor": {"type": "string"},
            "content": {"type": "string", "minLength": 1}
        }),
        &["operation_id", "context_id", "handoff_id", "session_id", "to_actor", "content"],
    )
}

fn tool_handoff_send() -> Value {
    write_tool(
        "handoff_send",
        "Send canonical handoff",
        "Send a typed handoff to another project actor; sender identity always comes from the bearer binding.",
        json!({
            "operation_id": {"type": "string"},
            "handoff_id": {"type": "string"},
            "session_id": {"type": "string"},
            "to_actor": {"type": "string"},
            "focus": {"type": ["string", "null"]},
            "done_when": {"type": ["string", "null"]},
            "context_id": {"type": ["string", "null"]}
        }),
        &["operation_id", "handoff_id", "session_id", "to_actor"],
    )
}

fn tool_handoff_list() -> Value {
    json!({
        "name": "handoff_list",
        "title": "List handoff mailbox",
        "description": "List the authenticated actor's inbox or outbox with participant visibility enforced.",
        "inputSchema": object_schema(
            json!({
                "box": {"type": "string", "enum": ["inbox", "outbox"]},
                "source_id": {"type": "string"},
                "include_completed": {"type": "boolean"},
                "before_seq": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100}
            }),
            &["box"]
        ),
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

fn tool_handoff_status() -> Value {
    json!({
        "name": "handoff_status",
        "title": "Read handoff status",
        "description": "Read one canonical handoff when the authenticated actor is a participant.",
        "inputSchema": object_schema(json!({"handoff_id": {"type": "string"}}), &["handoff_id"]),
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

fn tool_handoff_accept() -> Value {
    write_tool(
        "handoff_accept",
        "Accept canonical handoff",
        "Claim a pending handoff using optimistic revision and a stable claim id.",
        json!({
            "operation_id": {"type": "string"},
            "handoff_id": {"type": "string"},
            "expected_revision": {"type": "integer", "minimum": 1},
            "claim_id": {"type": "string"}
        }),
        &["operation_id", "handoff_id", "expected_revision", "claim_id"],
    )
}

fn tool_handoff_return() -> Value {
    write_tool(
        "handoff_return",
        "Return handoff result",
        "Return canonical result evidence under the active claim.",
        json!({
            "operation_id": {"type": "string"},
            "handoff_id": {"type": "string"},
            "expected_revision": {"type": "integer", "minimum": 1},
            "claim_id": {"type": "string"},
            "result_fact_id": {"type": "string"},
            "outcome": {"type": "string", "enum": ["succeeded", "failed"]}
        }),
        &["operation_id", "handoff_id", "expected_revision", "claim_id", "result_fact_id", "outcome"],
    )
}

fn write_tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Value,
    required: &[&str],
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": object_schema(properties, required),
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}
