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
    // Opt-in lazy fast path. Caller asks for BM25-only when latency
    // matters more than semantic recall (agent hot path).
    let bm25_only = args
        .get("bm25_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let results = wiki
        .hybrid_search(
            query,
            wg_core::SearchOpts {
                limit: Some(limit),
                bm25_only,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;

    // Surface fact.source as a citation alongside content + score so agents
    // can attribute each hit. Pattern requested by mem0 #467.
    let text = results
        .into_iter()
        .map(|r| {
            let src = r
                .source
                .as_deref()
                .map(|s| format!("\n  source: {s}"))
                .unwrap_or_default();
            format!(
                "[{}] {}\n  score={:.3}{src}",
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

    // Two ways to express the lookback window:
    //   - `last`      — duration DSL string (e.g. "30d", "12h", "4w") —
    //                   matches the CLI `wg recent --last` surface.
    //   - `last_days` — integer days, kept for backwards compatibility.
    // If both are present `last` wins. Defaults to 7 days when neither
    // is given, matching the tool's documented default.
    let window_ms = if let Some(s) = args.get("last").and_then(|v| v.as_str()) {
        crate::parse_duration_to_ms(s).map_err(|e| e.to_string())?
    } else {
        let last_days = args.get("last_days").and_then(|v| v.as_u64()).unwrap_or(7);
        last_days * 24 * 60 * 60 * 1000
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let since = Some(now_ms.saturating_sub(window_ms));

    let opts = wg_core::FactListOpts {
        fact_type: None,
        entity_id: None,
        min_confidence: None,
        limit: Some(limit),
        offset: 0,
        since,
        until: None,
        current_only: false,
        as_of: None,
    };
    let facts = wiki.fact_list(opts).map_err(|e| e.to_string())?;
    // Wrap the array in {"facts": [...]} for shape consistency with
    // the other list-style tools.
    let payload = json!({ "facts": facts });
    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
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
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .map(wg_core::QueryMode::parse)
        .unwrap_or_default();
    let opts = wg_core::QueryOpts {
        search_limit: args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize,
        depth: args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32,
        recent_limit: args
            .get("recent_limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize,
        since: None,
        current_only: args
            .get("current_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        mode,
        bm25_only: args
            .get("bm25_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };
    let result = wiki.query(topic, opts).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_entity_describe(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("name required")?;
    let summary = args.get("summary").and_then(|v| v.as_str()).unwrap_or("");
    wiki.entity_describe(name, summary)
        .map_err(|e| e.to_string())?;
    let msg = if summary.is_empty() {
        format!("Cleared summary for '{}'", name)
    } else {
        format!("Updated summary for '{}' ({} chars)", name, summary.len())
    };
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(msg)],
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

    // Return JSON {"id": "<ULID>"} so callers don't have to string-parse.
    let payload = json!({ "id": id.to_string() });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
        is_error: None,
    })
}

fn parse_fact_id(s: &str) -> Result<wg_core::FactId, String> {
    wg_core::ulid::Ulid::from_string(s)
        .map(wg_core::FactId)
        .map_err(|_| format!("invalid fact ID: {s}"))
}

fn tool_fact_add_many(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let items = args
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or("items array required")?;
    if items.is_empty() {
        return Ok(ToolCallResult {
            content: vec![ContentBlock::text("No items to add.".to_string())],
            is_error: None,
        });
    }
    let mut inputs = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("items[{i}] must be an object"))?;
        let content = obj
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("items[{i}].content is required"))?
            .to_string();
        let entity_ids = obj
            .get("entities")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|n| wiki.resolve_entity(n).ok())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty());
        let tags = obj
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty());
        inputs.push(wg_core::types::FactInput {
            content,
            fact_type: None,
            entity_ids,
            tags,
            source: None,
            source_confidence: None,
            observed_at: None,
        });
    }
    let count = inputs.len();
    let ids = wiki.fact_add_many(inputs).map_err(|e| e.to_string())?;
    let id_strs: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(format!(
            "Added {count} facts:\n{}",
            id_strs.join("\n")
        ))],
        is_error: None,
    })
}

fn tool_fact_supersede(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let old_id = args
        .get("old_id")
        .and_then(|v| v.as_str())
        .ok_or("old_id required")?;
    let new_id = args
        .get("new_id")
        .and_then(|v| v.as_str())
        .ok_or("new_id required")?;
    let old = parse_fact_id(old_id)?;
    let new = parse_fact_id(new_id)?;
    wiki.fact_supersede(&old, &new).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(format!(
            "Superseded {old_id} by {new_id}"
        ))],
        is_error: None,
    })
}

fn tool_fact_edit(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("id required")?;
    let fact_id = parse_fact_id(id)?;

    let append = args.get("append").and_then(|v| v.as_str());
    let prepend = args.get("prepend").and_then(|v| v.as_str());
    let find = args.get("find").and_then(|v| v.as_str());
    let replace = args.get("replace").and_then(|v| v.as_str());
    let content = args.get("content").and_then(|v| v.as_str());

    let mut ops = 0;
    if append.is_some() {
        ops += 1;
    }
    if prepend.is_some() {
        ops += 1;
    }
    if find.is_some() || replace.is_some() {
        ops += 1;
    }
    if content.is_some() {
        ops += 1;
    }
    if ops == 0 {
        return Err("no edit op (use append / prepend / find+replace / content)".into());
    }
    if ops > 1 {
        return Err("specify exactly one edit op".into());
    }
    if find.is_some() != replace.is_some() {
        return Err("find and replace must be used together".into());
    }

    let current = wiki.fact_get(&fact_id).map_err(|e| e.to_string())?;
    let original = current.content.clone();
    let mut new_content = current.content;

    if let Some(extra) = append {
        let sep = if new_content.is_empty() || new_content.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        new_content.push_str(sep);
        new_content.push_str(extra);
    } else if let Some(extra) = prepend {
        let sep = if extra.ends_with('\n') { "" } else { "\n" };
        new_content = format!("{extra}{sep}{new_content}");
    } else if let (Some(f), Some(r)) = (find, replace) {
        if !new_content.contains(f) {
            return Err(format!("find substring not present: {f:?}"));
        }
        new_content = new_content.replace(f, r);
    } else if let Some(full) = content {
        new_content = full.to_string();
    }

    wiki.fact_update(
        &fact_id,
        wg_core::FactUpdate {
            content: Some(new_content.clone()),
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;

    Ok(ToolCallResult {
        content: vec![ContentBlock::text(format!(
            "Updated fact {id}\n  before: {original}\n  after:  {new_content}"
        ))],
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
                "Search facts in the wiki using BM25 + semantic vectors. Returns ranked results. Pass `bm25_only:true` to skip the embedding model load (cuts cold-start ~700-900ms; loses semantic recall)."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"},
                    "limit": {"type": "number", "default": 10},
                    "bm25_only": {
                        "type": "boolean",
                        "default": false,
                        "description": "Skip embedding model — pure BM25. Use when agent hot-path latency matters more than semantic recall."
                    }
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
            description: "Recently added/updated facts. Defaults to the last 7 days, 20 facts. Returns {\"facts\": [...]}."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "number", "default": 20},
                    "last": {
                        "type": "string",
                        "description": "Lookback window, e.g. '30d', '12h', '4w', '1y'. Wins over last_days when both set."
                    },
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
            description: "Unified context fetch for a topic. One call returns: hybrid search hits, the resolved entity (if any), related entities (graph traversal), and recent facts. Modes: naive (search only), local (entity + neighbors, no global search), hybrid (default), global (broader scan)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "topic": {"type": "string", "description": "Topic, entity name, or alias"},
                    "limit": {"type": "number", "default": 10, "description": "Max search hits"},
                    "depth": {"type": "number", "default": 2, "description": "Traverse depth if topic is an entity"},
                    "recent_limit": {"type": "number", "default": 10, "description": "Max recent facts"},
                    "mode": {"type": "string", "enum": ["naive", "local", "hybrid", "global"], "default": "hybrid"},
                    "current_only": {"type": "boolean", "default": false}
                },
                "required": ["topic"]
            }),
        },
        Tool {
            name: "wg_entity_describe".into(),
            description: "Set the entity's compiled-truth summary — a synthesized prose understanding distinct from the fact list. Pass an empty string to clear. The summary is the headline answer for 'what do we know about X?'."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "summary": {"type": "string", "description": "Prose summary; empty string clears."}
                },
                "required": ["name", "summary"]
            }),
        },
        Tool {
            name: "wg_fact_add".into(),
            description: "Add a new fact to the wiki graph. Returns {\"id\": \"<ULID>\"}.".into(),
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
        Tool {
            name: "wg_fact_add_many".into(),
            description: "Add many facts to the wiki graph in a single \
                transaction. Use this for bulk imports — a single \
                `wg_fact_add_many` call is dramatically faster than \
                many sequential `wg_fact_add` calls because the disk \
                fsync cost is paid once per batch instead of once per \
                fact. Each item is an object with the same shape as \
                wg_fact_add's args."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string"},
                                "entities": {"type": "array", "items": {"type": "string"}},
                                "tags": {"type": "array", "items": {"type": "string"}}
                            },
                            "required": ["content"]
                        }
                    }
                },
                "required": ["items"]
            }),
        },
        Tool {
            name: "wg_fact_supersede".into(),
            description: "Mark an old fact as superseded by a new one. The old \
                fact stays in the store but won't appear in current_only \
                queries — use this when a decision was overturned or a value \
                changed.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "old_id": {"type": "string", "description": "ULID of the fact being replaced"},
                    "new_id": {"type": "string", "description": "ULID of the replacement fact (must already exist; create it first via wg_fact_add)"}
                },
                "required": ["old_id", "new_id"]
            }),
        },
        Tool {
            name: "wg_fact_edit".into(),
            description: "Edit a fact's content in place. Choose exactly one of \
                append / prepend / find+replace / content. Use this for typo \
                fixes or clarifications — for semantic changes use \
                wg_fact_supersede instead so the timeline is preserved.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id":      {"type": "string", "description": "Fact ULID"},
                    "append":  {"type": "string", "description": "Text to append (newline-joined)"},
                    "prepend": {"type": "string", "description": "Text to prepend (newline-joined)"},
                    "find":    {"type": "string", "description": "Substring to find (must use with replace)"},
                    "replace": {"type": "string", "description": "Replacement text (must use with find)"},
                    "content": {"type": "string", "description": "Replace the entire content"}
                },
                "required": ["id"]
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
        "wg_entity_describe" => tool_entity_describe(args, wiki),
        "wg_fact_add" => tool_fact_add(args, wiki),
        "wg_fact_add_many" => tool_fact_add_many(args, wiki),
        "wg_fact_supersede" => tool_fact_supersede(args, wiki),
        "wg_fact_edit" => tool_fact_edit(args, wiki),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use wg_core::{Config, FactInput, FactType, types::EntityInput};

    fn open_temp_wiki() -> (TempDir, WikiGraph) {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.store.path = dir.path().join("store").to_string_lossy().into_owned();
        let wiki = WikiGraph::open(&PathBuf::from(&config.store.path), config).unwrap();
        (dir, wiki)
    }

    fn add_fact(wiki: &WikiGraph, content: &str, entity: &str) -> wg_core::FactId {
        wiki.entity_add(EntityInput {
            name: entity.to_string(),
            ..Default::default()
        })
        .ok();
        let id = wiki.resolve_entity(entity).unwrap();
        wiki.add_fact(FactInput {
            content: content.to_string(),
            fact_type: Some(FactType::Decision),
            entity_ids: Some(vec![id]),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn fact_supersede_marks_old_and_succeeds() {
        let (_dir, wiki) = open_temp_wiki();
        let old = add_fact(&wiki, "use Redis 6", "Redis");
        let new = add_fact(&wiki, "use Redis 7", "Redis");

        let result = tool_fact_supersede(
            &json!({"old_id": old.0.to_string(), "new_id": new.0.to_string()}),
            &wiki,
        )
        .unwrap();
        assert_eq!(result.is_error, None);

        let old_record = wiki.fact_get(&old).unwrap();
        assert!(old_record.superseded_at.is_some());
        assert_eq!(old_record.superseded_by, Some(new));
    }

    #[test]
    fn fact_supersede_rejects_invalid_ids() {
        let (_dir, wiki) = open_temp_wiki();
        let err = tool_fact_supersede(
            &json!({"old_id": "not-a-ulid", "new_id": "also-bad"}),
            &wiki,
        )
        .unwrap_err();
        assert!(err.contains("invalid fact ID"));
    }

    #[test]
    fn fact_edit_append_updates_content() {
        let (_dir, wiki) = open_temp_wiki();
        let id = add_fact(&wiki, "first line", "Redis");

        tool_fact_edit(
            &json!({"id": id.0.to_string(), "append": "second line"}),
            &wiki,
        )
        .unwrap();

        let updated = wiki.fact_get(&id).unwrap();
        assert_eq!(updated.content, "first line\nsecond line");
    }

    #[test]
    fn fact_edit_find_replace_updates_content() {
        let (_dir, wiki) = open_temp_wiki();
        let id = add_fact(&wiki, "use Redis 6", "Redis");

        tool_fact_edit(
            &json!({
                "id": id.0.to_string(),
                "find": "Redis 6",
                "replace": "Redis 7"
            }),
            &wiki,
        )
        .unwrap();

        let updated = wiki.fact_get(&id).unwrap();
        assert_eq!(updated.content, "use Redis 7");
    }

    #[test]
    fn fact_edit_rejects_zero_ops() {
        let (_dir, wiki) = open_temp_wiki();
        let id = add_fact(&wiki, "x", "Redis");

        let err = tool_fact_edit(&json!({"id": id.0.to_string()}), &wiki).unwrap_err();
        assert!(err.contains("no edit op"));
    }

    #[test]
    fn fact_edit_rejects_multiple_ops() {
        let (_dir, wiki) = open_temp_wiki();
        let id = add_fact(&wiki, "x", "Redis");

        let err = tool_fact_edit(
            &json!({
                "id": id.0.to_string(),
                "append": "a",
                "content": "b"
            }),
            &wiki,
        )
        .unwrap_err();
        assert!(err.contains("exactly one"));
    }

    #[test]
    fn fact_edit_rejects_find_without_replace() {
        let (_dir, wiki) = open_temp_wiki();
        let id = add_fact(&wiki, "x", "Redis");

        let err = tool_fact_edit(&json!({"id": id.0.to_string(), "find": "x"}), &wiki).unwrap_err();
        assert!(err.contains("find and replace"));
    }

    #[test]
    fn list_tools_includes_new_write_tools() {
        let names: Vec<String> = list_tools().into_iter().map(|t| t.name).collect();
        assert!(names.contains(&"wg_fact_supersede".to_string()));
        assert!(names.contains(&"wg_fact_edit".to_string()));
        assert!(names.contains(&"wg_fact_add_many".to_string()));
    }

    #[test]
    fn fact_add_many_inserts_a_batch() {
        let (_dir, wiki) = open_temp_wiki();
        wiki.entity_add(EntityInput {
            name: "Redis".to_string(),
            ..Default::default()
        })
        .ok();

        let result = tool_fact_add_many(
            &json!({
                "items": [
                    {"content": "Redis 6 is in production", "entities": ["Redis"]},
                    {"content": "Redis 7 introduces functions", "entities": ["Redis"]},
                    {"content": "Redis Sentinel handles HA"}
                ]
            }),
            &wiki,
        )
        .unwrap();
        assert_eq!(result.is_error, None);
        let text = result
            .content
            .first()
            .and_then(|b| b.text.as_deref())
            .unwrap_or("");
        assert!(text.starts_with("Added 3 facts:"));
    }

    #[test]
    fn fact_add_many_rejects_missing_content() {
        let (_dir, wiki) = open_temp_wiki();
        let err = tool_fact_add_many(&json!({"items": [{}]}), &wiki).unwrap_err();
        assert!(err.contains("content is required"));
    }

    #[test]
    fn fact_add_returns_json_id() {
        let (_dir, wiki) = open_temp_wiki();
        let result = tool_fact_add(
            &json!({"content": "Redis is a cache", "entities": ["Redis"]}),
            &wiki,
        )
        .unwrap();
        let text = result
            .content
            .first()
            .and_then(|b| b.text.as_deref())
            .unwrap_or("");
        let payload: serde_json::Value = serde_json::from_str(text).expect("response is JSON");
        let id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(id.len(), 26, "expected ULID, got {id:?}");
    }

    #[test]
    fn recent_wraps_facts_in_object() {
        let (_dir, wiki) = open_temp_wiki();
        add_fact(&wiki, "Redis 7 introduces functions", "Redis");

        let result = tool_recent(&json!({"limit": 10}), &wiki).unwrap();
        let text = result
            .content
            .first()
            .and_then(|b| b.text.as_deref())
            .unwrap_or("");
        let payload: serde_json::Value = serde_json::from_str(text).expect("response is JSON");
        assert!(
            payload.get("facts").is_some(),
            "expected {{\"facts\": ...}}"
        );
        assert_eq!(payload["facts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn recent_accepts_last_dsl_string() {
        let (_dir, wiki) = open_temp_wiki();
        add_fact(&wiki, "fresh fact", "Redis");

        // `last="1h"` must be honoured (and produce >=1 hit since the
        // fact was just inserted).
        let result = tool_recent(&json!({"last": "1h"}), &wiki).unwrap();
        let text = result
            .content
            .first()
            .and_then(|b| b.text.as_deref())
            .unwrap_or("");
        let payload: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["facts"].as_array().unwrap().len(), 1);

        // The DSL parses correctly across units. Use a non-zero
        // small window for the boundary test — `0s` is timing-flaky
        // because `since = now()` collides with a just-inserted
        // fact's created_at on systems with coarse clock resolution
        // (macOS gives 0 hits, Ubuntu's millisecond clock returns
        // the just-inserted fact). Confirm the parser accepts
        // multiple unit suffixes.
        for unit in ["1y", "1w", "1d", "1h", "1m", "1s"] {
            let result = tool_recent(&json!({"last": unit}), &wiki).unwrap();
            let text = result
                .content
                .first()
                .and_then(|b| b.text.as_deref())
                .unwrap_or("");
            let payload: serde_json::Value = serde_json::from_str(text).unwrap();
            // Just-inserted fact is within any of these windows.
            assert_eq!(
                payload["facts"].as_array().unwrap().len(),
                1,
                "unit {unit} should keep the freshly-added fact"
            );
        }
    }

    #[test]
    fn recent_rejects_bad_duration() {
        let (_dir, wiki) = open_temp_wiki();
        let err = tool_recent(&json!({"last": "30"}), &wiki).unwrap_err();
        assert!(err.contains("duration"), "got: {err}");
    }
}
