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

/// Parse a `fact_type` MCP argument against the closed [`FactType`] enum.
/// Returns `None` for missing/null. Errors with a list of accepted values
/// when the agent passes a typo (e.g. "decisions", "fact") so the model
/// learns the right vocabulary at the next turn instead of getting a
/// generic deserialize panic.
fn parse_fact_type_arg(arg: Option<&Value>) -> Result<Option<wg_core::FactType>, String> {
    let Some(v) = arg else { return Ok(None) };
    if v.is_null() {
        return Ok(None);
    }
    let s = v
        .as_str()
        .ok_or("fact_type must be a string")?
        .to_lowercase();
    let parsed = match s.as_str() {
        "decision" => wg_core::FactType::Decision,
        "pattern" => wg_core::FactType::Pattern,
        "convention" => wg_core::FactType::Convention,
        "claim" => wg_core::FactType::Claim,
        "note" => wg_core::FactType::Note,
        "question" => wg_core::FactType::Question,
        "unknown" => wg_core::FactType::Unknown,
        other => {
            return Err(format!(
                "invalid fact_type {other:?}; accepted: decision, pattern, convention, claim, note, question, unknown"
            ));
        }
    };
    Ok(Some(parsed))
}

/// Parse a time-spec MCP argument. Accepts three shapes (in priority):
///
/// 1. `null` / missing → `Ok(None)`
/// 2. number → epoch milliseconds verbatim
/// 3. string → ISO date (`2026-04-01`, RFC3339), or for `since`/`until` only,
///    a duration DSL (`30d`, `12h`, `4w`, `1y`) interpreted as
///    `now - duration`. The `as_of_mode` flag suppresses duration parsing
///    so `as_of` can't accidentally be a relative window.
fn parse_time_arg(arg: Option<&Value>, as_of_mode: bool) -> Result<Option<u64>, String> {
    let Some(v) = arg else { return Ok(None) };
    if v.is_null() {
        return Ok(None);
    }
    if let Some(n) = v.as_u64() {
        return Ok(Some(n));
    }
    let s = v
        .as_str()
        .ok_or("time argument must be a number or string")?;
    if let Ok(ms) = crate::parse_iso_to_epoch_ms(s) {
        return Ok(Some(ms));
    }
    if !as_of_mode {
        if let Ok(window_ms) = crate::parse_duration_to_ms(s) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            return Ok(Some(now_ms.saturating_sub(window_ms)));
        }
    }
    Err(format!("unrecognised time argument: {s:?}"))
}

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
    // Default `current_only=true` because most agent queries are
    // "what do we know NOW?" — superseded facts mixed into results was
    // the biggest correctness footgun. Pass `false` explicitly for
    // historical or `--as-of` queries.
    let current_only = args
        .get("current_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let since = parse_time_arg(args.get("since"), false)?;
    let until = parse_time_arg(args.get("until"), false)?;
    let as_of = parse_time_arg(args.get("as_of"), true)?;
    let entity_filter = match args.get("entity").and_then(|v| v.as_str()) {
        Some(name) => Some(vec![wiki.resolve_entity(name).map_err(|e| e.to_string())?]),
        None => None,
    };
    let min_confidence = args
        .get("min_confidence")
        .and_then(|v| v.as_f64())
        .map(|x| x as f32);
    // Caller can pin a session_id (e.g. one session covering several
    // queries before a single feedback call) — otherwise we mint a
    // fresh ULID per request. Either way it round-trips so the agent
    // can later POST feedback via wg_feedback against the exact hits
    // returned here.
    let session_id = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| wg_core::ulid::Ulid::new().to_string());

    let results = wiki
        .hybrid_search(
            query,
            wg_core::SearchOpts {
                limit: Some(limit),
                bm25_only,
                current_only,
                since,
                until,
                as_of,
                entity_filter,
                min_confidence,
                session_id: Some(session_id.clone()),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;

    let results_json: Vec<Value> = results
        .into_iter()
        .map(|r| {
            json!({
                "fact_id": r.fact_id.to_string(),
                "content": r.content,
                "score": r.score,
                "rank": r.rank,
                "source": r.source,
            })
        })
        .collect();
    let payload = json!({
        "session_id": session_id,
        "results": results_json,
    });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
        is_error: None,
    })
}

fn tool_feedback(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let session_id = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or("session_id required (returned by wg_search)")?
        .to_string();
    let fact_id_str = args
        .get("fact_id")
        .and_then(|v| v.as_str())
        .ok_or("fact_id required")?;
    let fact_id = parse_fact_id(fact_id_str)?;
    let helpful = args
        .get("helpful")
        .and_then(|v| v.as_bool())
        .ok_or("helpful (boolean) required")?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    wiki.search_feedback_add(&wg_core::SearchFeedback {
        session_id,
        fact_id,
        helpful,
        timestamp,
    })
    .map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(
            json!({"ok": true, "fact_id": fact_id_str, "helpful": helpful}).to_string(),
        )],
        is_error: None,
    })
}

fn tool_path(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let from = args
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or("from required")?;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or("to required")?;
    let result = wiki.path_find(from, to).map_err(|e| e.to_string())?;
    let payload = serde_json::json!({
        "from": from,
        "to": to,
        "path": result,
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_fact_list(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let entity = args.get("entity").and_then(|v| v.as_str());
    let entity_id = match entity {
        Some(name) => Some(wiki.resolve_entity(name).map_err(|e| e.to_string())?),
        None => None,
    };
    let opts = wg_core::FactListOpts {
        fact_type: None,
        entity_id,
        min_confidence: None,
        limit: Some(limit),
        offset,
        since: None,
        until: None,
        current_only: args
            .get("current_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        as_of: None,
    };
    let facts = wiki.fact_list(opts).map_err(|e| e.to_string())?;
    // `next_offset` is None when the page came back short of the
    // requested limit — that's the agent's signal to stop paging.
    let next_offset = if facts.len() == limit {
        Some(offset + facts.len())
    } else {
        None
    };
    let payload = serde_json::json!({
        "facts": facts,
        "next_offset": next_offset,
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_entity_get(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("name required")?;
    let entity = wiki.entity_get(name).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&entity).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_fact_get(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("id required")?;
    let fact_id = parse_fact_id(id)?;
    let fact = wiki.fact_get(&fact_id).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&fact).map_err(|e| e.to_string())?;
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

    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let opts = wg_core::types::ListOpts {
        entity_type,
        min_facts: None,
        limit: Some(limit),
        sort_by: Default::default(),
        offset,
    };
    let entities = wiki.entity_list(opts).map_err(|e| e.to_string())?;
    let next_offset = if entities.len() == limit {
        Some(offset + entities.len())
    } else {
        None
    };

    // Build a structured payload with the page + cursor so agents can
    // enumerate without a hidden truncation. The legacy human-readable
    // text still ships as the first content line for backwards-compat
    // with anything that just printed the result.
    let lines: Vec<String> = entities
        .iter()
        .map(|e| format!("- {} ({}) [{} facts]", e.name, e.entity_type, e.fact_count))
        .collect();
    let payload = json!({
        "summary": lines.join("\n"),
        "entities": entities,
        "next_offset": next_offset,
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
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

/// Map a lint code to an actionable hint the agent can carry out
/// without further deliberation. Generic enough to apply to any
/// instance of that code, specific enough to suggest the right next
/// tool.
fn lint_action_hint(code: &str) -> &'static str {
    match code {
        "orphan" => {
            "No relations point to or from this entity. Either link it to a related one (`wg relation add`) or delete it via the CLI if it's irrelevant."
        }
        "duplicate" => {
            "Two facts have near-identical content. Read both via wg_fact_get; if they say the same thing, retire the older one with wg_fact_supersede."
        }
        "conflict" => {
            "Atomic fact types (decision / pattern / convention) are mutually exclusive per entity. Pick the survivor and run wg_fact_supersede on the others — the timeline is preserved."
        }
        "stale" => {
            "Fact hasn't been touched in a while. Verify it's still accurate. If reality changed, run wg_fact_supersede with an updated version."
        }
        "malformed_entity" => {
            "Entity record is incomplete or broken. Read it with wg_entity_get and repair via wg_entity_describe + fact_update."
        }
        _ => "See the message field for details.",
    }
}

fn tool_doctor(wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let issues = wiki.lint().map_err(|e| e.to_string())?;
    let stats = wiki.stats().map_err(|e| e.to_string())?;
    // Group issues by code so an agent can triage "I have 5 conflicts
    // to fix" without scanning the full issue list. Each group gets
    // an action hint pointing at the right next tool — gives the agent
    // a fix recipe instead of just a list of complaints.
    let mut codes: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for issue in &issues {
        *codes.entry(issue.code.clone()).or_insert(0) += 1;
    }
    let by_code: Vec<Value> = codes
        .into_iter()
        .map(|(code, count)| {
            json!({
                "code": code,
                "count": count,
                "action": lint_action_hint(&code),
            })
        })
        .collect();
    let payload = json!({
        "ok": issues.is_empty(),
        "stats": stats,
        "issue_count": issues.len(),
        "by_code": by_code,
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
            .unwrap_or(true),
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

/// Outcome of resolving (and possibly creating) the entity-name list
/// passed to `wg_fact_add{,_many}`.
struct ResolvedEntities {
    /// Entity IDs in the same order as the input names.
    ids: Vec<wg_core::EntityId>,
    /// Names that did not previously exist and were freshly created.
    created: Vec<String>,
    /// Per-newly-created name, the trigram-similar entities that
    /// already exist. Empty Vec when no candidates passed the
    /// similarity bar — the field is only populated when there's
    /// genuine risk of typo-induced graph fragmentation, like
    /// auto-creating "Postgrs" while "Postgres" already exists.
    alternatives: Vec<EntityNameAlternative>,
}

struct EntityNameAlternative {
    requested: String,
    suggestions: Vec<String>,
}

/// Resolve entity names to IDs, **auto-creating** any name that doesn't
/// already exist. Mirrors the CLI behavior in `wg fact add` so MCP and
/// CLI no longer diverge: previously the MCP path silently dropped
/// unknown names via `filter_map`, leaving the fact attached to fewer
/// entities than the agent expected. New entities default to
/// `EntityType::Unknown`.
///
/// On every auto-create the helper also runs the existing
/// `suggest_similar_entities` fuzzy matcher; when a candidate scores
/// above the trigram threshold the (`requested`, `suggestions`) pair is
/// returned as an `EntityNameAlternative`. Auto-create still proceeds
/// — the caller decides whether to merge with `wg_fact_supersede` /
/// alias the new entity. The default is non-blocking because the agent
/// might genuinely mean the new name, but the warning makes typo-driven
/// fragmentation visible at the moment it would otherwise happen
/// silently.
fn resolve_or_create_entities(
    wiki: &WikiGraph,
    names: &[String],
) -> Result<ResolvedEntities, String> {
    let mut ids = Vec::with_capacity(names.len());
    let mut created = Vec::new();
    let mut alternatives = Vec::new();
    for name in names {
        match wiki.resolve_entity(name) {
            Ok(id) => ids.push(id),
            Err(_) => {
                // Look for a near-miss BEFORE creating so the lookup
                // sees only entities that pre-date this batch — keeps
                // a "Postgres + Postgrs" pair from masking each other.
                let suggestions = wiki.suggest_similar_entities(name).unwrap_or_default();
                let id = wiki
                    .entity_add(wg_core::EntityInput {
                        name: name.clone(),
                        entity_type: Some(wg_core::EntityType::Unknown),
                        ..Default::default()
                    })
                    .map_err(|e| e.to_string())?;
                ids.push(id);
                created.push(name.clone());
                if !suggestions.is_empty() {
                    alternatives.push(EntityNameAlternative {
                        requested: name.clone(),
                        suggestions,
                    });
                }
            }
        }
    }
    Ok(ResolvedEntities {
        ids,
        created,
        alternatives,
    })
}

/// Serialize `EntityNameAlternative` array for inclusion in MCP
/// responses. Returns `Value::Null` when the input is empty so the
/// JSON field is `null` rather than `[]` — agents can short-circuit
/// the check with a simple null test.
fn alternatives_payload(alts: &[EntityNameAlternative]) -> Value {
    if alts.is_empty() {
        return Value::Null;
    }
    let array: Vec<Value> = alts
        .iter()
        .map(|alt| {
            json!({
                "requested": alt.requested,
                "suggestions": alt.suggestions,
            })
        })
        .collect();
    Value::Array(array)
}

fn tool_fact_add(args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("content required")?;
    let entity_names: Vec<String> = args
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

    let resolved = resolve_or_create_entities(wiki, &entity_names)?;
    let entity_ids = resolved.ids;
    let auto_created = resolved.created;
    let alternatives = resolved.alternatives;
    let fact_type = parse_fact_type_arg(args.get("fact_type"))?;

    // Pre-add similarity check (non-blocking). BM25-only so we don't
    // pay the embedding-model load on every add — the goal is just to
    // surface "this looks like an existing fact" so the agent can opt
    // to wg_fact_supersede instead of stacking duplicates. Set
    // `dedup_check: false` to skip (e.g. for trusted bulk imports).
    let dedup_check = args
        .get("dedup_check")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let existing_similar = if dedup_check {
        wiki.hybrid_search(
            content,
            wg_core::SearchOpts {
                limit: Some(1),
                bm25_only: true,
                current_only: true,
                ..Default::default()
            },
        )
        .ok()
        .and_then(|results| results.into_iter().next())
        // BM25 score threshold — empirically anything above ~1.0
        // represents a meaningful term-overlap match. Below that, the
        // hint adds noise to every add against a populated wiki.
        .filter(|hit| hit.score >= 1.0)
        .map(|hit| {
            json!({
                "fact_id": hit.fact_id.to_string(),
                "content": hit.content,
                "score": hit.score,
            })
        })
    } else {
        None
    };

    let input = wg_core::types::FactInput {
        content: content.into(),
        fact_type,
        entity_ids: if entity_ids.is_empty() {
            None
        } else {
            Some(entity_ids.clone())
        },
        tags: if tags.is_empty() { None } else { Some(tags) },
        source: None,
        source_confidence: None,
        observed_at: None,
    };

    let id = wiki.add_fact(input).map_err(|e| e.to_string())?;

    // Verify-symmetry: return the persisted record so the agent can
    // confirm the write landed without a separate `wg_fact_get` round
    // trip. The `auto_created_entities` field surfaces the side effect
    // of CLI-parity entity creation — invisible failures (typo →
    // silent new-entity) become visible.
    let record = wiki.fact_get(&id).map_err(|e| e.to_string())?;
    let entity_names_resolved: Vec<String> = record
        .entity_ids
        .iter()
        .filter_map(|eid| wiki.entity_get_by_id(*eid).ok())
        .map(|e| e.name)
        .collect();
    let payload = json!({
        "id": id.to_string(),
        "content": record.content,
        "entity_names": entity_names_resolved,
        "created_at": record.created_at,
        "auto_created_entities": auto_created,
        "entity_name_alternatives": alternatives_payload(&alternatives),
        "existing_similar": existing_similar,
    });
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
    let mut per_item_entity_names: Vec<Vec<String>> = Vec::with_capacity(items.len());
    let mut auto_created_total: Vec<String> = Vec::new();
    let mut alternatives_total: Vec<EntityNameAlternative> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("items[{i}] must be an object"))?;
        let content = obj
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("items[{i}].content is required"))?
            .to_string();
        let names: Vec<String> = obj
            .get("entities")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let resolved = resolve_or_create_entities(wiki, &names)?;
        let entity_ids_vec = resolved.ids;
        // Dedup auto-created across items in case the same new entity
        // is referenced by multiple facts in this batch.
        for name in resolved.created {
            if !auto_created_total.contains(&name) {
                auto_created_total.push(name);
            }
        }
        for alt in resolved.alternatives {
            if !alternatives_total
                .iter()
                .any(|a| a.requested == alt.requested)
            {
                alternatives_total.push(alt);
            }
        }
        per_item_entity_names.push(names);
        let entity_ids = if entity_ids_vec.is_empty() {
            None
        } else {
            Some(entity_ids_vec)
        };
        let tags = obj
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty());
        let fact_type = parse_fact_type_arg(obj.get("fact_type"))
            .map_err(|e| format!("items[{i}].fact_type: {e}"))?;
        inputs.push(wg_core::types::FactInput {
            content,
            fact_type,
            entity_ids,
            tags,
            source: None,
            source_confidence: None,
            observed_at: None,
        });
    }
    let ids = wiki.fact_add_many(inputs).map_err(|e| e.to_string())?;
    // Build a per-item record array reusing data we already have — no
    // extra fact_get calls needed in the batch path.
    let facts_array: Vec<Value> = ids
        .iter()
        .zip(per_item_entity_names.iter())
        .map(|(id, names)| {
            json!({
                "id": id.to_string(),
                "entity_names": names,
            })
        })
        .collect();
    let payload = json!({
        "count": ids.len(),
        "facts": facts_array,
        "auto_created_entities": auto_created_total,
        "entity_name_alternatives": alternatives_payload(&alternatives_total),
    });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
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
    let dry_run = args
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let old = parse_fact_id(old_id)?;
    let new = parse_fact_id(new_id)?;
    // Resolve both records before any write so the agent sees the
    // before/after content, the entity links that survive, and the
    // existence checks both fail-fast on bad ULIDs.
    let old_record = wiki.fact_get(&old).map_err(|e| e.to_string())?;
    let new_record = wiki.fact_get(&new).map_err(|e| e.to_string())?;
    if !dry_run {
        wiki.fact_supersede(&old, &new).map_err(|e| e.to_string())?;
    }
    let payload = json!({
        "dry_run": dry_run,
        "old_id": old_id,
        "new_id": new_id,
        "old_content": old_record.content,
        "new_content": new_record.content,
        "applied": !dry_run,
    });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
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
    let dry_run = args
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

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

    if !dry_run {
        wiki.fact_update(
            &fact_id,
            wg_core::FactUpdate {
                content: Some(new_content.clone()),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
    }

    let payload = json!({
        "dry_run": dry_run,
        "id": id,
        "before": original,
        "after": new_content,
        "applied": !dry_run,
    });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
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
                "Search facts in the wiki using BM25 + semantic vectors. Returns ranked results. Defaults to current-only (excludes superseded facts) — pass `current_only:false` for historical/timeline queries. Pass `bm25_only:true` to skip the embedding model load (cuts cold-start ~700-900ms; loses semantic recall). For graph context (related entities + recent facts) prefer `wg_query` instead — it wraps this tool plus traversal in one call."
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
                    },
                    "current_only": {
                        "type": "boolean",
                        "default": true,
                        "description": "Exclude superseded facts. Pass false to search the historical timeline."
                    },
                    "since": {
                        "type": ["string", "number"],
                        "description": "Lower bound on observed_at. Accepts ISO date (2026-01-15), RFC3339, epoch ms, or duration DSL (30d / 12h / 4w / 1y, interpreted as now - window)."
                    },
                    "until": {
                        "type": ["string", "number"],
                        "description": "Upper bound on observed_at. Same parsing as `since`."
                    },
                    "as_of": {
                        "type": ["string", "number"],
                        "description": "Replay the wiki at a given timestamp — only facts that were valid (not yet superseded) at this point appear. ISO date, RFC3339, or epoch ms (no relative duration)."
                    },
                    "entity": {
                        "type": "string",
                        "description": "Restrict to facts attached to this entity (name or alias)."
                    },
                    "min_confidence": {
                        "type": "number",
                        "description": "Filter facts with source_confidence below this threshold."
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Optional. Pin a session_id (e.g. group several queries under one logical session). If omitted, a fresh ULID is minted and returned alongside the results — feed it back into wg_feedback to record helpful/not-helpful signal that trains the ranking adapter."
                    }
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "wg_feedback".into(),
            description: "Record helpful / not-helpful feedback on a fact returned by a recent wg_search call. Pass the session_id from that search response. Feedback feeds into the domain adapter (`wg adapt train`) which, when applied (`config.search.use_adapter=true`, default), nudges future ranking toward facts the agent confirmed were useful."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "session_id from the wg_search response"},
                    "fact_id":    {"type": "string", "description": "ULID of the fact in question"},
                    "helpful":    {"type": "boolean", "description": "true = the fact answered the query; false = it did not"}
                },
                "required": ["session_id", "fact_id", "helpful"]
            }),
        },
        Tool {
            name: "wg_path".into(),
            description: "Find the shortest path between two entities (BFS over typed relations). Returns {from, to, path: [hops]}. For breadth-first exploration of one neighborhood, use wg_traverse instead.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string", "description": "Source entity name/alias"},
                    "to":   {"type": "string", "description": "Target entity name/alias"}
                },
                "required": ["from", "to"]
            }),
        },
        Tool {
            name: "wg_fact_list".into(),
            description: "List facts with optional entity filter. Defaults to current_only=true. Use wg_recent for time-windowed listing or wg_search when you have a query string.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "entity": {"type": "string", "description": "Filter by entity name/alias"},
                    "limit":  {"type": "number", "default": 20},
                    "offset": {"type": "number", "default": 0, "description": "Skip the first N facts. Combined with `limit`, paginate through the full result. Response includes `next_offset` (null when the page is the last)."},
                    "current_only": {"type": "boolean", "default": true, "description": "Exclude superseded facts. Pass false to include historical timeline."}
                }
            }),
        },
        Tool {
            name: "wg_entity_get".into(),
            description: "Get a single entity by name (or alias). On miss, returns suggestions in the error so you can correct the name. Returns the JSON record on success."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
        },
        Tool {
            name: "wg_fact_get".into(),
            description: "Get a single fact by ULID. Returns the JSON record.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"id": {"type": "string", "description": "Fact ULID"}},
                "required": ["id"]
            }),
        },
        Tool {
            name: "wg_entity_list".into(),
            description: "List entities in the wiki graph with fact counts. To fetch one entity's record use wg_entity_get; to find related entities by graph use wg_traverse.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "number", "default": 20},
                    "offset": {"type": "number", "default": 0, "description": "Skip the first N entities. Combined with `limit`, paginate through the full set. Response includes `next_offset`."},
                    "type": {
                        "type": "string",
                        "description": "Filter by entity type. Built-in: technology, concept, comparison, query, person, team, unknown. Any other string is a Custom type (e.g. service, rfc, incident)."
                    }
                }
            }),
        },
        Tool {
            name: "wg_traverse".into(),
            description: "Forward graph walk from a starting entity (returns reachable entities up to depth). For 'what depends on X' (reverse direction) use wg_backlinks; for shortest path between two known entities use wg_path.".into(),
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
            description: "Raw lint issues — orphan entities, duplicate facts, stale facts, broken refs. Returns the array directly. For a friendly health summary that wraps these issues with stats, prefer wg_doctor."
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
            description: "Recently added/updated facts. Defaults to the last 7 days, 20 facts. Returns {\"facts\": [...]}. For full context on a topic (search + graph + recent in one call) use wg_query."
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
            description: "Unified context fetch for a topic — preferred entry point when an agent needs context. One call returns: hybrid search hits, the resolved entity (if any), related entities (graph traversal), and recent facts. Defaults to current_only=true. Modes: naive (search only), local (entity + neighbors, no global search), hybrid (default), global (broader scan). For pure search without graph context use wg_search; for last-N-days listing use wg_recent."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "topic": {"type": "string", "description": "Topic, entity name, or alias"},
                    "limit": {"type": "number", "default": 10, "description": "Max search hits"},
                    "depth": {"type": "number", "default": 2, "description": "Traverse depth if topic is an entity"},
                    "recent_limit": {"type": "number", "default": 10, "description": "Max recent facts"},
                    "mode": {"type": "string", "enum": ["naive", "local", "hybrid", "global"], "default": "hybrid"},
                    "current_only": {"type": "boolean", "default": true, "description": "Exclude superseded facts. Pass false for historical / timeline queries."}
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
            description: "Add a fact to the wiki graph. By default the tool runs a BM25 dedup check on the new content first — if a high-overlap existing fact is found it appears as `existing_similar` in the response (the new fact is still added; the agent decides whether to wg_fact_supersede the older one). Missing entities are auto-created (default type Unknown) and reported in `auto_created_entities`. If an auto-created name is fuzzily similar to an existing entity (e.g. typo: 'Postgrs' vs existing 'Postgres'), the candidates appear as `entity_name_alternatives` so the agent can decide to alias or merge instead of leaving a fragmented graph. Returns {id, content, entity_names, created_at, auto_created_entities, entity_name_alternatives, existing_similar}."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string"},
                    "entities": {"type": "array", "items": {"type": "string"}, "description": "Entity names or aliases. Unknown names are auto-created."},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "fact_type": {
                        "type": "string",
                        "enum": ["decision", "pattern", "convention", "claim", "note", "question", "unknown"],
                        "description": "Atomic types (decision/pattern/convention) are mutually exclusive per entity — use wg_fact_supersede to retire the old one. Non-atomic (claim/note/question) coexist freely."
                    },
                    "dedup_check": {
                        "type": "boolean",
                        "default": true,
                        "description": "Run a BM25 similarity search against the new content before adding. Disable for trusted bulk imports to save the latency."
                    }
                },
                "required": ["content"]
            }),
        },
        Tool {
            name: "wg_fact_add_many".into(),
            description: "Add many facts in a single transaction. Dramatically faster than many sequential wg_fact_add calls because the disk fsync cost is paid once per batch. Each item has the same shape as wg_fact_add's args. Returns {count, facts:[{id, entity_names}], auto_created_entities} — the dedup'd auto-created list lets you confirm new entities at a glance."
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
                                "tags": {"type": "array", "items": {"type": "string"}},
                                "fact_type": {
                                    "type": "string",
                                    "enum": ["decision", "pattern", "convention", "claim", "note", "question", "unknown"]
                                }
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
                queries (the default for wg_search / wg_query / wg_fact_list). \
                Use this when a decision was overturned or a value \
                changed; for typo fixes use wg_fact_edit. The historical \
                timeline is preserved — wg_search with `as_of:<date>` \
                replays past state.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "old_id": {"type": "string", "description": "ULID of the fact being replaced"},
                    "new_id": {"type": "string", "description": "ULID of the replacement fact (must already exist; create it first via wg_fact_add)"},
                    "dry_run": {"type": "boolean", "default": false, "description": "Validate both ULIDs and return before/after content without writing. Use this to confirm you're about to retire the right fact."}
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
                    "content": {"type": "string", "description": "Replace the entire content"},
                    "dry_run": {"type": "boolean", "default": false, "description": "Compute the new content (and validate the operation) but skip the write. Returns `{before, after}` so the agent can review."}
                },
                "required": ["id"]
            }),
        },
    ]
}

fn call_tool(name: &str, args: &Value, wiki: &WikiGraph) -> Result<ToolCallResult, String> {
    match name {
        "wg_search" => tool_search(args, wiki),
        "wg_feedback" => tool_feedback(args, wiki),
        "wg_entity_get" => tool_entity_get(args, wiki),
        "wg_entity_list" => tool_entity_list(args, wiki),
        "wg_fact_get" => tool_fact_get(args, wiki),
        "wg_fact_list" => tool_fact_list(args, wiki),
        "wg_path" => tool_path(args, wiki),
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
        let payload: Value = serde_json::from_str(text).expect("response is JSON");
        assert_eq!(payload["count"], 3);
        let facts = payload["facts"].as_array().expect("facts array");
        assert_eq!(facts.len(), 3);
        // "Redis" was pre-created by the test setup, so it must NOT be
        // reported as auto-created. The third item has no entities at
        // all, so nothing to create either.
        assert!(
            payload["auto_created_entities"]
                .as_array()
                .unwrap()
                .is_empty(),
            "pre-existing entity must not be reported as auto-created",
        );
    }

    #[test]
    fn fact_add_many_auto_creates_unknown_entities_once() {
        // Three items, the same novel entity referenced by two of them.
        // The MCP path must auto-create it (CLI parity) and dedupe so
        // the agent doesn't see "Postgres" twice in the response.
        let (_dir, wiki) = open_temp_wiki();
        let result = tool_fact_add_many(
            &json!({
                "items": [
                    {"content": "Postgres 16 ships", "entities": ["Postgres"]},
                    {"content": "Postgres has logical replication", "entities": ["Postgres"]},
                    {"content": "MySQL 8 has window functions", "entities": ["MySQL"]}
                ]
            }),
            &wiki,
        )
        .unwrap();
        let text = result.content[0].text.as_deref().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        let mut created: Vec<&str> = payload["auto_created_entities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        created.sort();
        assert_eq!(created, vec!["MySQL", "Postgres"]);
    }

    #[test]
    fn fact_add_many_rejects_missing_content() {
        let (_dir, wiki) = open_temp_wiki();
        let err = tool_fact_add_many(&json!({"items": [{}]}), &wiki).unwrap_err();
        assert!(err.contains("content is required"));
    }

    #[test]
    fn fact_list_defaults_current_only_true() {
        // Add two facts about Redis, supersede one. The default-current
        // contract for agent queries means the superseded fact must be
        // hidden unless `current_only:false` is passed explicitly.
        let (_dir, wiki) = open_temp_wiki();
        let old = add_fact(&wiki, "Redis 6 in production", "Redis");
        let new = add_fact(&wiki, "Redis 7 in production", "Redis");
        wiki.fact_supersede(&old, &new).unwrap();

        let default_call = tool_fact_list(&json!({"entity": "Redis"}), &wiki).unwrap();
        let payload: Value =
            serde_json::from_str(default_call.content[0].text.as_deref().unwrap()).unwrap();
        let ids: Vec<&str> = payload["facts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["id"].as_str())
            .collect();
        let new_str = new.0.to_string();
        let old_str = old.0.to_string();
        assert!(ids.contains(&new_str.as_str()));
        assert!(
            !ids.contains(&old_str.as_str()),
            "superseded fact must be hidden by default",
        );

        // Opt-in to historical view.
        let history =
            tool_fact_list(&json!({"entity": "Redis", "current_only": false}), &wiki).unwrap();
        let payload: Value =
            serde_json::from_str(history.content[0].text.as_deref().unwrap()).unwrap();
        let ids: Vec<&str> = payload["facts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["id"].as_str())
            .collect();
        assert!(ids.contains(&old_str.as_str()));
    }

    #[test]
    fn search_parses_iso_date_for_since() {
        // Just verify the parser doesn't reject a legitimate ISO date —
        // we can't easily stage facts at past observed_at without
        // bypassing the public API, so this is a smoke check.
        let (_dir, wiki) = open_temp_wiki();
        add_fact(&wiki, "an old decision about caching", "Redis");
        let result =
            tool_search(&json!({"query": "caching", "since": "2020-01-01"}), &wiki).unwrap();
        assert!(result.is_error.is_none());
    }

    #[test]
    fn fact_add_accepts_typed_fact() {
        // Agents should be able to tag a "decision" via MCP — previously
        // fact_type was hardcoded to None and the schema didn't expose it,
        // so the tool was less expressive than the CLI.
        let (_dir, wiki) = open_temp_wiki();
        let result = tool_fact_add(
            &json!({
                "content": "Use Redis for hot path caching",
                "entities": ["Redis"],
                "fact_type": "decision"
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        let id = wg_core::ulid::Ulid::from_string(payload["id"].as_str().unwrap()).unwrap();
        let record = wiki.fact_get(&wg_core::FactId(id)).unwrap();
        assert_eq!(record.fact_type, wg_core::FactType::Decision);
    }

    #[test]
    fn fact_add_rejects_unknown_fact_type_with_helpful_message() {
        let (_dir, wiki) = open_temp_wiki();
        let err =
            tool_fact_add(&json!({"content": "x", "fact_type": "decisions"}), &wiki).unwrap_err();
        assert!(
            err.contains("decision") && err.contains("pattern"),
            "expected accepted-values list in error, got {err}",
        );
    }

    #[test]
    fn search_rejects_unparseable_time_argument() {
        let (_dir, wiki) = open_temp_wiki();
        let err = tool_search(&json!({"query": "x", "since": "not-a-date"}), &wiki).unwrap_err();
        assert!(err.contains("unrecognised time argument"));
    }

    #[test]
    #[ignore = "downloads HF embedding model — local only"]
    fn search_returns_session_id_round_trippable_to_feedback() {
        // Seed a fact so the search returns at least one hit, then feed
        // the session_id from wg_search into wg_feedback. The latter
        // must not error and the request must persist.
        let (_dir, wiki) = open_temp_wiki();
        let fact_id = add_fact(&wiki, "Redis is an in-memory store", "Redis");

        let response = tool_search(&json!({"query": "redis"}), &wiki).unwrap();
        let payload: Value =
            serde_json::from_str(response.content[0].text.as_deref().unwrap()).unwrap();
        let session_id = payload["session_id"].as_str().expect("session_id present");
        assert_eq!(session_id.len(), 26, "session_id should be a ULID");

        let feedback = tool_feedback(
            &json!({
                "session_id": session_id,
                "fact_id": fact_id.0.to_string(),
                "helpful": true,
            }),
            &wiki,
        )
        .unwrap();
        let fb: Value = serde_json::from_str(feedback.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(fb["ok"], true);
    }

    #[test]
    fn feedback_rejects_missing_session_id() {
        let (_dir, wiki) = open_temp_wiki();
        let err = tool_feedback(
            &json!({"fact_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "helpful": true}),
            &wiki,
        )
        .unwrap_err();
        assert!(err.contains("session_id required"));
    }

    #[test]
    fn fact_list_pagination_yields_next_offset() {
        // Add five facts on the same entity, page 2 at a time. The
        // first two pages each return next_offset; the last page
        // (length < limit) returns null so the agent stops paging.
        let (_dir, wiki) = open_temp_wiki();
        for i in 0..5 {
            add_fact(&wiki, &format!("note number {i}"), "Topic");
        }
        let page1 = tool_fact_list(&json!({"limit": 2, "offset": 0}), &wiki).unwrap();
        let p1: Value = serde_json::from_str(page1.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(p1["facts"].as_array().unwrap().len(), 2);
        assert_eq!(p1["next_offset"], 2);

        let page3 = tool_fact_list(&json!({"limit": 2, "offset": 4}), &wiki).unwrap();
        let p3: Value = serde_json::from_str(page3.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(p3["facts"].as_array().unwrap().len(), 1);
        assert!(
            p3["next_offset"].is_null(),
            "short page must signal end-of-stream",
        );
    }

    #[test]
    fn fact_supersede_dry_run_does_not_write() {
        // Build the supersede pair, run dry_run, confirm both facts are
        // still queryable as 'current' (the old one was NOT marked).
        // Then run for real and confirm only the new one survives in
        // current_only views.
        let (_dir, wiki) = open_temp_wiki();
        let old = add_fact(&wiki, "use Redis 6 for cache", "Redis");
        let new = add_fact(&wiki, "use Redis 7 for cache", "Redis");

        let preview = tool_fact_supersede(
            &json!({
                "old_id": old.0.to_string(),
                "new_id": new.0.to_string(),
                "dry_run": true,
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(preview.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["dry_run"], true);
        assert_eq!(payload["applied"], false);
        assert!(payload["old_content"].as_str().unwrap().contains("Redis 6"));
        let current = wiki.fact_get(&old).unwrap();
        assert!(
            current.superseded_at.is_none(),
            "dry_run must not mutate the store",
        );

        // For-real run: applied=true and the old fact is now superseded.
        let applied = tool_fact_supersede(
            &json!({
                "old_id": old.0.to_string(),
                "new_id": new.0.to_string(),
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(applied.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["applied"], true);
        let after = wiki.fact_get(&old).unwrap();
        assert!(after.superseded_at.is_some());
    }

    #[test]
    fn fact_edit_dry_run_returns_diff_without_writing() {
        let (_dir, wiki) = open_temp_wiki();
        let id = add_fact(&wiki, "original content", "Topic");
        let preview = tool_fact_edit(
            &json!({
                "id": id.0.to_string(),
                "content": "completely rewritten",
                "dry_run": true,
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(preview.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["dry_run"], true);
        assert_eq!(payload["before"], "original content");
        assert_eq!(payload["after"], "completely rewritten");
        assert_eq!(payload["applied"], false);
        let still = wiki.fact_get(&id).unwrap();
        assert_eq!(still.content, "original content");
    }

    #[test]
    fn feedback_rejects_invalid_fact_id() {
        let (_dir, wiki) = open_temp_wiki();
        let err = tool_feedback(
            &json!({
                "session_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
                "fact_id": "not-a-ulid",
                "helpful": true,
            }),
            &wiki,
        )
        .unwrap_err();
        assert!(err.contains("invalid fact ID"));
    }

    #[test]
    fn doctor_groups_by_code_with_action_hints() {
        // Add an entity without any relations — this triggers the
        // "orphan" lint code. Confirm doctor's by_code section maps
        // it to a hint that names the right next tool.
        let (_dir, wiki) = open_temp_wiki();
        wiki.entity_add(EntityInput {
            name: "FloatingEntity".into(),
            ..Default::default()
        })
        .unwrap();

        let result = tool_doctor(&wiki).unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        let by_code = payload["by_code"].as_array().expect("by_code present");
        let orphan = by_code
            .iter()
            .find(|g| g["code"] == "orphan")
            .expect("orphan group");
        assert!(orphan["count"].as_u64().unwrap_or(0) >= 1);
        assert!(
            orphan["action"]
                .as_str()
                .unwrap_or("")
                .contains("relations"),
            "action hint should explain how to resolve an orphan",
        );
    }

    #[test]
    fn fact_add_surfaces_entity_name_alternatives_for_typos() {
        // Live agent test caught this: "Postgres" exists, agent
        // accidentally posts a fact about "Postgrs", auto-create
        // silently splits the graph. Now the response must surface a
        // typo hint pointing at the existing entity so the agent can
        // wg_fact_supersede / alias instead of leaving the fragment.
        let (_dir, wiki) = open_temp_wiki();
        add_fact(&wiki, "PostgreSQL is a relational DB", "Postgres");
        let result = tool_fact_add(
            &json!({
                "content": "Postgres has VACUUM",
                "entities": ["Postgrs"],
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        let alts = payload["entity_name_alternatives"]
            .as_array()
            .expect("alternatives present for typo");
        assert_eq!(alts.len(), 1);
        assert_eq!(alts[0]["requested"], "Postgrs");
        let suggestions: Vec<&str> = alts[0]["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            suggestions.iter().any(|s| s.contains("Postgres")),
            "Postgres must appear as a suggestion: {suggestions:?}",
        );
    }

    #[test]
    fn fact_add_omits_alternatives_when_no_near_match() {
        // Brand-new entity name with no fuzzy neighbours — the field
        // must be `null` (not `[]`) so callers can short-circuit with
        // a single null check.
        let (_dir, wiki) = open_temp_wiki();
        let result = tool_fact_add(
            &json!({
                "content": "MongoDB document store",
                "entities": ["MongoDB"],
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        assert!(payload["entity_name_alternatives"].is_null());
    }

    #[test]
    fn fact_add_skips_dedup_check_when_disabled() {
        // When the agent passes dedup_check:false the response must
        // include `existing_similar: null` even if a near-duplicate
        // exists. Useful for trusted bulk imports.
        let (_dir, wiki) = open_temp_wiki();
        add_fact(&wiki, "Redis is an in-memory cache", "Redis");
        let result = tool_fact_add(
            &json!({
                "content": "Redis is an in-memory cache",
                "entities": ["Redis"],
                "dedup_check": false,
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        assert!(
            payload["existing_similar"].is_null(),
            "dedup_check=false must suppress the similarity hint",
        );
    }

    #[test]
    fn fact_add_returns_full_record_with_auto_created() {
        let (_dir, wiki) = open_temp_wiki();
        // "Redis" doesn't exist yet — MCP path must auto-create it
        // (matching CLI behavior) instead of dropping it silently.
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
        let id = payload["id"].as_str().unwrap_or("");
        assert_eq!(id.len(), 26, "expected ULID, got {id:?}");
        assert_eq!(payload["content"], "Redis is a cache");
        let names: Vec<&str> = payload["entity_names"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(names, vec!["Redis"]);
        let created: Vec<&str> = payload["auto_created_entities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(created, vec!["Redis"]);
        assert!(payload["created_at"].is_number());
    }

    #[test]
    fn fact_add_does_not_double_create_existing_entities() {
        let (_dir, wiki) = open_temp_wiki();
        wiki.entity_add(wg_core::EntityInput {
            name: "Redis".into(),
            entity_type: Some(wg_core::EntityType::Technology),
            ..Default::default()
        })
        .unwrap();
        let result = tool_fact_add(
            &json!({"content": "Redis 7 introduces functions", "entities": ["Redis"]}),
            &wiki,
        )
        .unwrap();
        let text = result.content[0].text.as_deref().unwrap();
        let payload: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(
            payload["auto_created_entities"]
                .as_array()
                .unwrap()
                .is_empty(),
            "existing entity must not be reported as auto-created"
        );
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
