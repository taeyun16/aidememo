//! Shared MCP tool definitions and JSON-RPC dispatch.
//!
//! Used by both the stdio transport (`wg mcp`) and the HTTP+SSE transport
//! (`wg mcp-serve`). Speaks MCP JSON-RPC 2.0.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wg_core::WikiGraph;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "wg";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn error(id: Value, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool schema types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Serialize)]
pub struct ToolListResult {
    pub tools: Vec<Tool>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallArgs {
    pub name: String,
    #[serde(default)]
    pub arguments: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ToolCallResult {
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: Option<String>,
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            block_type: "text".into(),
            text: Some(s.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

fn tool_search(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("query required")?;
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
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_entity_list(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let entity_type =
        args.get("type")
            .and_then(|v| v.as_str())
            .map(|s| match s.to_lowercase().as_str() {
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
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_traverse(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let entity = args
        .get("entity")
        .and_then(|v| v.as_str())
        .ok_or("entity required")?;
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
        content: vec![ContentBlock::text(format!(
            "Traversed from '{}' (depth={})\n{:#?}",
            entity, depth, result
        ))],
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
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_doctor(wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let issues = wiki.lint().map_err(|e| e.to_string())?;
    let stats = wiki.stats().map_err(|e| e.to_string())?;
    let payload = json!({
        "ok": issues.is_empty(),
        "stats": stats,
        "issue_count": issues.len(),
        "issues": issues,
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_recent(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let last_days = args.get("last_days").and_then(|v| v.as_u64()).unwrap_or(7);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let since = Some(now_ms.saturating_sub(last_days * 24 * 60 * 60 * 1000));

    let opts = wg_core::FactListOpts {
        fact_type: None,
        entity_id: None,
        min_confidence: None,
        limit: Some(limit),
        offset: 0,
        since,
        until: None,
    };
    let facts = wiki.fact_list(opts).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&facts).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_backlinks(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let entity = args
        .get("entity")
        .and_then(|v| v.as_str())
        .ok_or("entity required")?;
    let relations = wiki
        .relations_get(entity, wg_core::TraverseDirection::Reverse)
        .map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&relations).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_query(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let topic = args
        .get("topic")
        .and_then(|v| v.as_str())
        .ok_or("topic required")?;
    let opts = wg_core::QueryOpts {
        search_limit: args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize,
        depth: args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32,
        recent_limit: args
            .get("recent_limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize,
        since: None,
    };
    let result = wiki.query(topic, opts).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_fact_add(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("content required")?;
    let entities: Vec<String> = args
        .get("entities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let tags: Vec<String> = args
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let input = wg_core::types::FactInput {
        content: content.into(),
        fact_type: None,
        entity_ids: if entities.is_empty() {
            None
        } else {
            Some(
                entities
                    .into_iter()
                    .filter_map(|n| wiki.resolve_entity(&n).ok())
                    .collect(),
            )
        },
        tags: if tags.is_empty() { None } else { Some(tags) },
        source: None,
        source_confidence: None,
        observed_at: None,
    };

    let id = wiki.add_fact(input).map_err(|e| e.to_string())?;

    Ok(ToolCallResult {
        content: vec![ContentBlock::text(format!("Fact added: {}", id))],
        is_error: None,
    })
}

// ---------------------------------------------------------------------------
// Tool list & dispatch
// ---------------------------------------------------------------------------

pub fn list_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "wg_search".into(),
            description:
                "Search facts in the wiki using BM25 + semantic vectors. Returns ranked results."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"},
                    "limit": {"type": "number", "default": 10}
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "wg_entity_list".into(),
            description: "List entities in the wiki graph with fact counts.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "number", "default": 20},
                    "type": {"type": "string", "description": "Filter by entity type"}
                }
            }),
        },
        Tool {
            name: "wg_traverse".into(),
            description: "Traverse the entity graph from a starting entity.".into(),
            input_schema: json!({
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
            description: "Check the health of the wiki graph (orphans, duplicates, stale facts)."
                .into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "wg_doctor".into(),
            description: "Wiki health check: counts plus any lint issues (orphans, broken refs, stale facts). Call this first if results look wrong."
                .into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "wg_recent".into(),
            description: "Recently added/updated facts. Defaults to the last 7 days, 20 facts."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "number", "default": 20},
                    "last_days": {"type": "number", "default": 7}
                }
            }),
        },
        Tool {
            name: "wg_backlinks".into(),
            description: "Reverse relations — entities that point AT this entity. Useful for 'what depends on X?'."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "entity": {"type": "string"}
                },
                "required": ["entity"]
            }),
        },
        Tool {
            name: "wg_query".into(),
            description: "Unified context fetch for a topic. One call returns: hybrid search hits, the resolved entity (if any), related entities (graph traversal), and recent facts on that entity. Prefer this over chaining wg_search + wg_traverse + wg_entity_list when you want context."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "topic": {"type": "string", "description": "Topic, entity name, or alias"},
                    "limit": {"type": "number", "default": 10, "description": "Max search hits"},
                    "depth": {"type": "number", "default": 2, "description": "Traverse depth if topic is an entity"},
                    "recent_limit": {"type": "number", "default": 10, "description": "Max recent facts"}
                },
                "required": ["topic"]
            }),
        },
        Tool {
            name: "wg_fact_add".into(),
            description: "Add a new fact to the wiki graph.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string"},
                    "entities": {"type": "array", "items": {"type": "string"}},
                    "tags": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["content"]
            }),
        },
    ]
}

fn call_tool(name: &str, args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    match name {
        "wg_search" => tool_search(args, wiki),
        "wg_entity_list" => tool_entity_list(args, wiki),
        "wg_traverse" => tool_traverse(args, wiki),
        "wg_lint" => tool_lint(wiki),
        "wg_doctor" => tool_doctor(wiki),
        "wg_recent" => tool_recent(args, wiki),
        "wg_backlinks" => tool_backlinks(args, wiki),
        "wg_query" => tool_query(args, wiki),
        "wg_fact_add" => tool_fact_add(args, wiki),
        _ => Err(format!("Unknown tool: {}", name)),
    }
}

/// Dispatch a single JSON-RPC request to a response.
///
/// Returns `None` for notifications (which have no response).
pub fn dispatch(req: JsonRpcRequest, wiki: &WikiGraph) -> Option<JsonRpcResponse> {
    // Notifications have no response.
    if req.id.is_null() && req.method.starts_with("notifications/") {
        return None;
    }

    let id = req.id.clone();

    match req.method.as_str() {
        "initialize" => Some(JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}
            }),
        )),
        "ping" => Some(JsonRpcResponse::success(id, json!({}))),
        "tools/list" => Some(JsonRpcResponse::success(
            id,
            serde_json::to_value(ToolListResult {
                tools: list_tools(),
            })
            .unwrap_or_else(|_| json!({"tools": []})),
        )),
        "tools/call" => {
            let args: ToolCallArgs = match serde_json::from_value(req.params.unwrap_or_default()) {
                Ok(a) => a,
                Err(e) => {
                    return Some(JsonRpcResponse::error(
                        id,
                        -32602,
                        &format!("Invalid params: {}", e),
                    ));
                }
            };
            let arg_value = args.arguments.unwrap_or(Value::Null);
            match call_tool(&args.name, &arg_value, wiki) {
                Ok(r) => Some(JsonRpcResponse::success(
                    id,
                    serde_json::to_value(r).unwrap_or_else(|_| json!({"content": []})),
                )),
                Err(e) => Some(JsonRpcResponse::success(
                    id,
                    serde_json::to_value(ToolCallResult {
                        content: vec![ContentBlock::text(e)],
                        is_error: Some(true),
                    })
                    .unwrap_or_else(|_| json!({})),
                )),
            }
        }
        _ => Some(JsonRpcResponse::error(
            id,
            -32601,
            &format!("Method not found: {}", req.method),
        )),
    }
}
