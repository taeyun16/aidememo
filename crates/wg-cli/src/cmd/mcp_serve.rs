//! `wg mcp-serve` — MCP server mode (HTTP + SSE JSON-RPC).
//!
//! Implements the MCP JSON-RPC 2.0 protocol over HTTP POST + SSE.
//! Tools: wg_search, wg_entity_list, wg_fact_add, wg_lint, wg_traverse
//!
//! Usage:
//!   wg mcp-serve --port 3000

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bpaf::*;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{cmd::Command, Config, WikiGraph};

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

type SharedState = Arc<RwLock<Option<WikiGraph>>>;

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }
    fn error(id: serde_json::Value, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }
}

#[derive(Debug, Serialize)]
struct Tool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ToolListResult { tools: Vec<Tool> }

#[derive(Debug, Deserialize)]
struct ToolCallArgs {
    name: String,
    arguments: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ToolCallResult {
    content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

fn tool_search(args: &serde_json::Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let query = args.get("query").and_then(|v| v.as_str()).ok_or("query required")?;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    let results = wiki
        .hybrid_search(
            query,
            wg_core::SearchOpts {
                limit: Some(limit),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;

    let text = results
        .into_iter()
        .map(|r| {
            format!(
                "[{}] {}\n  score={:.3}",
                r.fact_id,
                r.content.chars().take(120).collect::<String>(),
                r.score
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    Ok(ToolCallResult {
        content: vec![ContentBlock { block_type: "text".into(), text: Some(text) }],
        is_error: None,
    })
}

fn tool_entity_list(args: &serde_json::Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let entity_type = args.get("type").and_then(|v| v.as_str()).map(|s| match s.to_lowercase().as_str() {
        "technology" | "tech" => wg_core::EntityType::Technology,
        "concept" => wg_core::EntityType::Concept,
        "comparison" | "compare" => wg_core::EntityType::Comparison,
        "query" | "question" => wg_core::EntityType::Query,
        "person" => wg_core::EntityType::Person,
        "team" => wg_core::EntityType::Team,
        _ => wg_core::EntityType::Unknown,
    });

    let opts = wg_core::types::ListOpts {
        entity_type,
        min_facts: None,
        limit: Some(limit),
        sort_by: Default::default(),
        offset: 0,
    };
    let entities = wiki.entity_list(opts).map_err(|e| e.to_string())?;

    let text = entities
        .into_iter()
        .map(|e| format!("- {} ({}) [{} facts]", e.name, e.entity_type, e.fact_count))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(ToolCallResult {
        content: vec![ContentBlock { block_type: "text".into(), text: Some(text) }],
        is_error: None,
    })
}

fn tool_traverse(args: &serde_json::Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let entity = args.get("entity").and_then(|v| v.as_str()).ok_or("entity required")?;
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32;

    let result = wiki
        .traverse(
            entity,
            wg_core::TraverseOpts {
                depth,
                relation_types: None,
                direction: wg_core::TraverseDirection::Forward,
            },
        )
        .map_err(|e| e.to_string())?;

    Ok(ToolCallResult {
        content: vec![ContentBlock {
            block_type: "text".into(),
            text: Some(format!("Traversed from '{}' (depth={})\n{:#?}", entity, depth, result)),
        }],
        is_error: None,
    })
}

fn tool_lint(wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let issues = wiki.lint().map_err(|e| e.to_string())?;

    let text = if issues.is_empty() {
        "Graph is healthy.".into()
    } else {
        issues
            .iter()
            .map(|i| format!("- [{}] {}: {}", i.severity, i.code, i.message))
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(ToolCallResult {
        content: vec![ContentBlock { block_type: "text".into(), text: Some(text) }],
        is_error: None,
    })
}

fn tool_fact_add(args: &serde_json::Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let content = args.get("content").and_then(|v| v.as_str()).ok_or("content required")?;
    let entities: Vec<String> = args
        .get("entities")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let tags: Vec<String> = args
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let input = wg_core::types::FactInput {
        content: content.into(),
        fact_type: None,
        entity_ids: if entities.is_empty() { None } else { Some(entities.into_iter().filter_map(|n| wiki.resolve_entity(&n).ok()).collect()) },
        tags: if tags.is_empty() { None } else { Some(tags) },
        source: None,
        source_confidence: None,
    };

    let id = wiki.add_fact(input).map_err(|e| e.to_string())?;

    Ok(ToolCallResult {
        content: vec![ContentBlock {
            block_type: "text".into(),
            text: Some(format!("Fact added: {}", id)),
        }],
        is_error: None,
    })
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn handle_post(
    State(state): State<SharedState>,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    let guard = state.read().await;
    let wiki = match guard.as_ref() {
        Some(w) => w,
        None => {
            let resp = JsonRpcResponse::error(req.id, -32603, "wiki not initialized");
            return Json(resp).into_response();
        }
    };

    let result = match req.method.as_str() {
        "tools/list" => {
            let tools = vec![
                Tool {
                    name: "wg_search".into(),
                    description: "Search facts using full-text (BM25) and semantic vectors. Returns ranked results with scores.".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"},
                            "limit": {"type": "number", "default": 10}
                        },
                        "required": ["query"]
                    }),
                },
                Tool {
                    name: "wg_entity_list".into(),
                    description: "List all entities in the wiki graph with fact counts.".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "limit": {"type": "number", "default": 20},
                            "type": {"type": "string"}
                        }
                    }),
                },
                Tool {
                    name: "wg_traverse".into(),
                    description: "Traverse the entity graph from a starting entity.".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "entity": {"type": "string"},
                            "depth": {"type": "number", "default": 2}
                        },
                        "required": ["entity"]
                    }),
                },
                Tool {
                    name: "wg_lint".into(),
                    description: "Check the health of the wiki graph.".into(),
                    input_schema: serde_json::json!({"type": "object", "properties": {}}),
                },
                Tool {
                    name: "wg_fact_add".into(),
                    description: "Add a new fact to the wiki graph.".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "content": {"type": "string"},
                            "fact_type": {"type": "string"},
                            "entities": {"type": "array", "items": {"type": "string"}},
                            "tags": {"type": "array", "items": {"type": "string"}}
                        },
                        "required": ["content"]
                    }),
                },
            ];
            Json(JsonRpcResponse::success(
                req.id,
                serde_json::to_value(ToolListResult { tools }).unwrap(),
            ))
            .into_response()
        }
        "tools/call" => {
            let args: ToolCallArgs =
                match serde_json::from_value(req.params.unwrap_or_default()) {
                    Ok(a) => a,
                    Err(e) => {
                        return Json(JsonRpcResponse::error(
                            req.id,
                            -32602,
                            &format!("Invalid params: {}", e),
                        ))
                        .into_response();
                    }
                };

            let result = match args.name.as_str() {
                "wg_search" => {
                    tool_search(args.arguments.as_ref().unwrap_or(&serde_json::Value::Null), wiki)
                }
                "wg_entity_list" => {
                    tool_entity_list(args.arguments.as_ref().unwrap_or(&serde_json::Value::Null), wiki)
                }
                "wg_traverse" => {
                    tool_traverse(args.arguments.as_ref().unwrap_or(&serde_json::Value::Null), wiki)
                }
                "wg_lint" => tool_lint(wiki),
                "wg_fact_add" => {
                    tool_fact_add(args.arguments.as_ref().unwrap_or(&serde_json::Value::Null), wiki)
                }
                _ => Err(format!("Unknown tool: {}", args.name)),
            };

            match result {
                Ok(r) => {
                    Json(JsonRpcResponse::success(req.id, serde_json::to_value(r).unwrap()))
                        .into_response()
                }
                Err(e) => Json(JsonRpcResponse::error(req.id, -32603, &e)).into_response(),
            }
        }
        _ => Json(JsonRpcResponse::error(
            req.id,
            -32601,
            &format!("Method not found: {}", req.method),
        ))
        .into_response(),
    };

    result
}

async fn handle_sse(State(_state): State<SharedState>) -> Response {
    use axum::response::sse::{Event, Sse};
    use std::convert::Infallible;

    async fn event_stream(
    ) -> impl tokio_stream::Stream<Item = Result<Event, Infallible>> + Send + 'static {
        tokio_stream::iter(vec![Ok(Event::default()
            .event("message")
            .data(r#"{"jsonrpc":"2.0","method":"initialized","params":{} }"#))])
    }

    let stream = event_stream().await;
    Sse::new(stream).into_response()
}

// ---------------------------------------------------------------------------
// CLI command definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct McpSub {
    pub port: Option<u16>,          // listen port (default 3000)
    pub wiki_root: Option<PathBuf>, // optional positional (must be rightmost)
}

pub fn mcp_serve_command() -> impl Parser<Command> {
    let port = long("port")
        .short('p')
        .help("Port to listen on (default: 3000)")
        .argument::<u16>("PORT")
        .optional();

    let wiki_root = positional::<PathBuf>("WIKI_ROOT")
        .help("Path to wiki root (uses store path if omitted)")
        .optional();

    construct!(McpSub { port, wiki_root })
        .map(Command::McpServe)
        .to_options()
        .command("mcp-serve")
        .help("Start MCP server (HTTP+SSE JSON-RPC)")
}

// ---------------------------------------------------------------------------
// Run the MCP server
// ---------------------------------------------------------------------------

pub fn run_mcp_serve(port: Option<u16>, wiki_root: Option<PathBuf>) -> Result<String, wg_core::WgError> {
    let config = Config::load().unwrap_or_default();
    let store_path = match wiki_root {
        Some(p) => p,
        None => PathBuf::from(&config.store.path),
    };

    let port: u16 = port.unwrap_or(3000);
    let wiki = WikiGraph::open(store_path.as_ref(), config)?;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| wg_core::WgError::Internal(format!("failed to create runtime: {}", e)))?;

    runtime.block_on(async {
        let state: SharedState = Arc::new(RwLock::new(Some(wiki)));

        let app = Router::new()
            .route("/mcp", post(handle_post))
            .route("/sse", get(handle_sse))
            .route("/health", get(|| async { "ok" }))
            .with_state(state);

        let addr = format!("0.0.0.0:{}", port);
        println!("MCP server listening on http://{}", addr);
        println!("MCP endpoint: http://{}/mcp", addr);
        println!("SSE endpoint: http://{}/sse", addr);

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| wg_core::WgError::Internal(format!("failed to bind: {}", e)))?;

        axum::serve(listener, app)
            .await
            .map_err(|e| wg_core::WgError::Internal(format!("server error: {}", e)))?;

        Ok::<(), wg_core::WgError>(())
    })?;

    Ok("MCP server stopped".into())
}
