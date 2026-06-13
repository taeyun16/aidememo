//! Shared MCP tool definitions and JSON-RPC dispatch.
//!
//! Used by both the stdio transport (`aidememo mcp`) and the HTTP+SSE transport
//! (`aidememo mcp-serve`). Speaks MCP JSON-RPC 2.0.

use aidememo_core::{AideMemo, WorkflowStartOpts};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::doctor::collect_sharing_status;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "aidememo";
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

/// Parse a `fact_type` MCP argument against the closed `FactType` enum.
/// Returns `None` for missing/null. Errors with a list of accepted values
/// when the agent passes a typo (e.g. "decisions", "fact") so the model
/// learns the right vocabulary at the next turn instead of getting a
/// generic deserialize panic.
fn parse_fact_type_arg(arg: Option<&Value>) -> Result<Option<aidememo_core::FactType>, String> {
    let Some(v) = arg else { return Ok(None) };
    if v.is_null() {
        return Ok(None);
    }
    let s = v.as_str().ok_or("fact_type must be a string")?;
    // Delegate to aidememo_core's central alias table. Reject only the
    // catch-all "unknown" → that means the agent passed a string we
    // don't recognise, which is more useful as an error than as a
    // silent degrade to Note.
    let parsed = aidememo_core::FactType::parse(s);
    if matches!(parsed, aidememo_core::FactType::Unknown) && s.to_lowercase() != "unknown" {
        return Err(format!(
            "invalid fact_type {s:?}; accepted: decision, pattern, convention, claim, note, question, preference, lesson, error, unknown"
        ));
    }
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

fn tool_search(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
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
    // can later POST feedback via aidememo_feedback against the exact hits
    // returned here.
    let session_id = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| aidememo_core::ulid::Ulid::new().to_string());
    let include_archive = args
        .get("include_archive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let source_id = mcp_source_id(args);

    let results = wiki
        .hybrid_search(
            query,
            aidememo_core::SearchOpts {
                limit: Some(limit),
                bm25_only,
                current_only,
                since,
                until,
                as_of,
                entity_filter,
                min_confidence,
                source_id,
                session_id: Some(session_id.clone()),
                include_archive,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;

    // Agent UX budget knobs (mirrors aidememo_query). format=text emits
    // markdown bullets — ~4× smaller than JSON envelope. max_chars
    // hard-caps; on overflow drops trailing hits (always keeps top
    // match). preview_chars caps each hit's content in compact/text.
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("full");
    let preview_chars = args
        .get("preview_chars")
        .and_then(|v| v.as_u64())
        .unwrap_or(200) as usize;
    let max_chars = args
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    if format == "text" {
        let mut out = format!("# search: {}\n", query);
        out.push_str(&format!("session: {}\n\n", session_id));
        for r in &results {
            let mut content = r.content.clone();
            truncate_in_place(&mut content, preview_chars);
            let id_short: String = r
                .fact_id
                .to_string()
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            let src = r.source.as_deref().unwrap_or("");
            out.push_str(&format!(
                "- ({:.2}{}{} …{}) {}\n",
                r.score,
                if src.is_empty() { "" } else { " " },
                src,
                id_short,
                content
            ));
        }
        if let Some(b) = max_chars
            && out.len() > b
        {
            while out.len() > b.saturating_sub(20)
                && let Some(idx) = out.rfind('\n')
            {
                out.truncate(idx);
            }
            out.push_str("\n… (truncated)\n");
        }
        return Ok(ToolCallResult {
            content: vec![ContentBlock::text(out)],
            is_error: None,
        });
    }

    let mut results_for_json: Vec<aidememo_core::SearchResult> = results;
    if format == "compact" {
        for r in &mut results_for_json {
            truncate_in_place(&mut r.content, preview_chars);
        }
    }
    let results_json: Vec<Value> = results_for_json
        .iter()
        .map(|r| {
            json!({
                "fact_id": r.fact_id.to_string(),
                "content": r.content,
                "score": r.score,
                "rank": r.rank,
                "source": r.source,
                "source_id": r.source_id,
            })
        })
        .collect();
    let mut payload = json!({
        "session_id": session_id,
        "results": results_json,
    });
    if let Some(b) = max_chars {
        let mut text = payload.to_string();
        // Drop trailing hits until under budget; always keep at least 1.
        while text.len() > b
            && payload
                .get("results")
                .and_then(|v| v.as_array())
                .map(|a| a.len() > 1)
                .unwrap_or(false)
        {
            if let Some(arr) = payload.get_mut("results").and_then(|v| v.as_array_mut()) {
                arr.pop();
            }
            text = payload.to_string();
        }
    }
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
        is_error: None,
    })
}

fn tool_feedback(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let session_id = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or("session_id required (returned by aidememo_search)")?
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
    wiki.search_feedback_add(&aidememo_core::SearchFeedback {
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

fn tool_pinned_context(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let pinned = wiki.pinned_facts(limit).map_err(|e| e.to_string())?;
    let entries: Vec<Value> = pinned
        .into_iter()
        .map(|f| {
            let entity_names: Vec<String> = f
                .entity_ids
                .iter()
                .filter_map(|eid| wiki.entity_get_by_id(*eid).ok())
                .map(|e| e.name)
                .collect();
            json!({
                "id": f.id.to_string(),
                "content": f.content,
                "fact_type": f.fact_type.to_string(),
                "entity_names": entity_names,
                "tags": f.tags,
                "created_at": f.created_at,
                "last_accessed_at": f.last_accessed_at,
            })
        })
        .collect();
    let payload = json!({"count": entries.len(), "facts": entries});
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
        is_error: None,
    })
}

fn tool_fact_pin(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let id_str = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("id required")?;
    let pinned = args
        .get("pinned")
        .and_then(|v| v.as_bool())
        .ok_or("pinned (boolean) required — true to pin, false to unpin")?;
    let fact_id = parse_fact_id(id_str)?;
    wiki.fact_pin(&fact_id, pinned).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(
            json!({"id": id_str, "pinned": pinned}).to_string(),
        )],
        is_error: None,
    })
}

/// One-call session warmup envelope. Bundles the four read calls an
/// agent makes at the top of a new conversation — pinned tier, recent
/// activity, top entities, open lint issues — into a single MCP
/// invocation so the model doesn't have to chain four sequential
/// reads (`aidememo_pinned_context`, `aidememo_recent`, `aidememo_entity_list`,
/// `aidememo_doctor`). Each section honors its own limit so agents can
/// shape the envelope size to their context budget.
fn tool_session_start(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let pinned_limit = args
        .get("pinned_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;
    let recent_limit = args
        .get("recent_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10) as usize;
    let recent_days = args
        .get("recent_days")
        .and_then(|v| v.as_u64())
        .unwrap_or(7);
    let top_entities_limit = args
        .get("top_entities_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10) as usize;

    let pinned = wiki.pinned_facts(pinned_limit).map_err(|e| e.to_string())?;
    let pinned_json: Vec<Value> = pinned
        .iter()
        .map(|f| {
            json!({
                "id": f.id.to_string(),
                "content": f.content,
                "fact_type": f.fact_type.to_string(),
            })
        })
        .collect();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let since = Some(now_ms.saturating_sub(recent_days * 24 * 60 * 60 * 1000));
    let recent = wiki
        .fact_list(aidememo_core::FactListOpts {
            fact_type: None,
            entity_id: None,
            min_confidence: None,
            source_id: None,
            limit: Some(recent_limit),
            offset: 0,
            since,
            until: None,
            current_only: true,
            as_of: None,
        })
        .map_err(|e| e.to_string())?;
    let recent_json: Vec<Value> = recent
        .iter()
        .map(|f| {
            json!({
                "id": f.id.to_string(),
                "content": f.content,
                "fact_type": f.fact_type.to_string(),
                "created_at": f.created_at,
            })
        })
        .collect();

    let top_entities = wiki
        .entity_list(aidememo_core::ListOpts {
            entity_type: None,
            min_facts: None,
            sort_by: aidememo_core::EntitySort::FactCount,
            limit: Some(top_entities_limit),
            offset: 0,
        })
        .map_err(|e| e.to_string())?;
    let top_entities_json: Vec<Value> = top_entities
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "type": e.entity_type.to_string(),
                "fact_count": e.fact_count,
            })
        })
        .collect();

    let issues = wiki.lint().map_err(|e| e.to_string())?;
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

    // Personalisation tier — ALL preference / lesson / error facts
    // (decay-exempt by default). These are the OMEGA-equivalent of
    // 'profile' + 'lessons' surfaced unconditionally at session start
    // so the agent doesn't have to remember to look them up.
    let personalisation = collect_personalisation(wiki, 50, None);

    let stats = wiki.stats().map_err(|e| e.to_string())?;
    let payload = json!({
        "stats": stats,
        "pinned": pinned_json,
        "personalisation": personalisation,
        "recent": recent_json,
        "top_entities": top_entities_json,
        "open_issues": {
            "total": issues.len(),
            "by_code": by_code,
        },
    });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
        is_error: None,
    })
}

/// Pull every current Preference / Lesson / Error fact, capped, as
/// a slim JSON array. These are the durable, agent-relevant signals
/// that should always be in the model's context window — surfaced
/// unconditionally at session start.
fn source_id_matches(f: &aidememo_core::FactRecord, source_id: Option<&str>) -> bool {
    match source_id {
        Some(source_id) => f.source_id.as_deref() == Some(source_id),
        None => true,
    }
}

fn source_id_from_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn default_mcp_source_id() -> Option<String> {
    normalise_source_id(std::env::var("AIDEMEMO_SOURCE_ID").ok())
}

fn normalise_source_id(source_id: Option<String>) -> Option<String> {
    source_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn mcp_source_id_with_env(
    args: &Value,
    env_source_id: impl FnOnce() -> Option<String>,
) -> Option<String> {
    source_id_from_value(args.get("source_id")).or_else(|| normalise_source_id(env_source_id()))
}

fn mcp_source_id(args: &Value) -> Option<String> {
    mcp_source_id_with_env(args, default_mcp_source_id)
}

fn resolve_session_entity(
    wiki: &AideMemo,
    session_id: Option<&str>,
) -> Result<Option<aidememo_core::EntityId>, String> {
    let Some(session_id) = session_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    wiki.entity_get(session_id)
        .map(|entity| Some(entity.id))
        .map_err(|e| format!("session_id {session_id:?} does not resolve to an entity: {e}"))
}

fn attach_session_entity(
    entity_ids: &mut Vec<aidememo_core::EntityId>,
    session: Option<aidememo_core::EntityId>,
) {
    if let Some(session_id) = session {
        if !entity_ids.contains(&session_id) {
            entity_ids.push(session_id);
        }
    }
}

fn collect_personalisation(
    wiki: &AideMemo,
    per_type_limit: usize,
    source_id: Option<&str>,
) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for ftype in [
        aidememo_core::FactType::Preference,
        aidememo_core::FactType::Lesson,
        aidememo_core::FactType::Error,
    ] {
        let facts = wiki
            .fact_list(aidememo_core::FactListOpts {
                fact_type: Some(ftype),
                entity_id: None,
                min_confidence: None,
                source_id: source_id.map(str::to_string),
                limit: Some(per_type_limit),
                offset: 0,
                since: None,
                until: None,
                current_only: true,
                as_of: None,
            })
            .unwrap_or_default();
        for f in facts {
            out.push(json!({
                "id": f.id.to_string(),
                "type": f.fact_type.to_string(),
                "content": f.content,
            }));
        }
    }
    out
}

fn tool_extract(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("text required")?;
    let max_candidates = args
        .get("max_candidates")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let llm = args.get("llm").and_then(|v| v.as_bool()).unwrap_or(false);
    let min_confidence = args
        .get("min_confidence")
        .and_then(|v| v.as_f64())
        .map(|x| x as f32)
        .unwrap_or(0.5);

    let mut candidates = if llm {
        match wiki.extract_candidates_llm(text, max_candidates) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("LLM extract failed, falling back to heuristic: {e}");
                wiki.extract_candidates(text, max_candidates)
                    .map_err(|e| e.to_string())?
            }
        }
    } else {
        wiki.extract_candidates(text, max_candidates)
            .map_err(|e| e.to_string())?
    };
    candidates.retain(|c| c.confidence >= min_confidence);

    // Preview path: hand back the structured candidates so the agent
    // can edit / drop / approve before committing.
    if !apply {
        let payload = json!({
            "applied": false,
            "candidates": candidates.iter().map(|c| json!({
                "content": c.content,
                "suggested_entities": c.suggested_entities,
                "suggested_fact_type": c.suggested_fact_type.to_string(),
                "confidence": c.confidence,
            })).collect::<Vec<_>>(),
        });
        return Ok(ToolCallResult {
            content: vec![ContentBlock::text(payload.to_string())],
            is_error: None,
        });
    }

    // Apply path: persist every candidate that survived the
    // confidence filter via fact_add (one redb commit each so we
    // capture the per-fact dedup hint and entity-name alternatives;
    // batch insert path doesn't run those checks). This is the
    // observational-memory shape — agent dumps a transcript, gets
    // back a list of `id`s plus the candidates that were used.
    let mut added: Vec<Value> = Vec::new();
    for cand in &candidates {
        let resolved = resolve_or_create_entities(wiki, &cand.suggested_entities)?;
        let entity_ids = if resolved.ids.is_empty() {
            None
        } else {
            Some(resolved.ids)
        };
        let input = aidememo_core::types::FactInput {
            content: cand.content.clone(),
            fact_type: Some(cand.suggested_fact_type),
            entity_ids,
            tags: None,
            source: None,
            source_id: None,
            source_confidence: Some(cand.confidence),
            observed_at: None,
        };
        let id = wiki.add_fact(input).map_err(|e| e.to_string())?;
        added.push(json!({
            "id": id.to_string(),
            "content": cand.content,
            "fact_type": cand.suggested_fact_type.to_string(),
            "entities": cand.suggested_entities,
            "confidence": cand.confidence,
            "auto_created_entities": resolved.created,
        }));
    }

    let payload = json!({
        "applied": true,
        "added": added,
    });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
        is_error: None,
    })
}

fn tool_path(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
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

fn tool_fact_list(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let entity = args.get("entity").and_then(|v| v.as_str());
    let entity_id = match entity {
        Some(name) => Some(wiki.resolve_entity(name).map_err(|e| e.to_string())?),
        None => None,
    };
    let source_id = mcp_source_id(args);
    let opts = aidememo_core::FactListOpts {
        fact_type: None,
        entity_id,
        min_confidence: None,
        source_id,
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
    let slim: Vec<Value> = facts.iter().map(|f| slim_fact_record(f, wiki)).collect();
    let payload = serde_json::json!({
        "facts": slim,
        "next_offset": next_offset,
    });
    // `to_string` (not _pretty) is intentional — the MCP wire is
    // token-budgeted, JSON pretty-printing alone adds ~30%.
    let text = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

/// Compact JSON representation of a `FactRecord` for MCP wire output.
/// Drops noise that the agent never reads:
///   - `access_count` / `last_accessed_at` (telemetry, not content)
///   - `relevance_score`, `source_confidence` (defaults `0.5` for ~all facts)
///   - `created_at` / `updated_at` epoch ms (use `aidememo_recent` for time
///     filtering — agents rarely diff these in answer composition)
///   - empty `tags`, null `source` / `observed_at` / `superseded_*`
///
/// Resolves `entity_ids` → entity names so the agent doesn't need a
/// follow-up `aidememo_entity_get` for every reference.
fn slim_fact_record(f: &aidememo_core::FactRecord, wiki: &AideMemo) -> Value {
    let mut entity_names: Vec<String> = Vec::with_capacity(f.entity_ids.len());
    for id in &f.entity_ids {
        if let Ok(rec) = wiki.entity_get_by_id(*id) {
            entity_names.push(rec.name);
        }
    }
    let mut obj = serde_json::Map::new();
    obj.insert("id".into(), Value::String(f.id.to_string()));
    obj.insert("content".into(), Value::String(f.content.clone()));
    obj.insert("type".into(), Value::String(f.fact_type.to_string()));
    obj.insert(
        "entities".into(),
        serde_json::to_value(entity_names).unwrap_or(Value::Null),
    );
    // Freshness signal — agents use this to decide whether to
    // double-check stale info (knowledge-update category bottleneck).
    // Decay-exempt types (preference / lesson / error / decision /
    // convention / pattern by default) skip the warning since 'old'
    // is fine for them.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let age_ms = now_ms.saturating_sub(f.created_at);
    let age_days = (age_ms / (24 * 60 * 60 * 1000)) as u32;
    if age_days >= 1 {
        obj.insert("age_days".into(), Value::Number(age_days.into()));
    }
    let durable_types = matches!(
        f.fact_type,
        aidememo_core::FactType::Decision
            | aidememo_core::FactType::Convention
            | aidememo_core::FactType::Pattern
            | aidememo_core::FactType::Preference
            | aidememo_core::FactType::Lesson
            | aidememo_core::FactType::Error
    );
    if !durable_types && age_days >= 60 {
        // 60d threshold for note/claim/question/unknown — stale
        // observational fact is the classic knowledge-update miss
        // mode. Agent should re-verify via aidememo_search or the source.
        obj.insert(
            "freshness_warning".into(),
            Value::String(format!(
                "fact is {age_days} days old; consider re-verifying"
            )),
        );
    }
    if f.pinned {
        obj.insert("pinned".into(), Value::Bool(true));
    }
    if let Some(ts) = f.observed_at {
        obj.insert("observed_at".into(), Value::Number(ts.into()));
    }
    if let Some(s) = &f.source {
        obj.insert("source".into(), Value::String(s.clone()));
    }
    if let Some(source_id) = &f.source_id {
        obj.insert("source_id".into(), Value::String(source_id.clone()));
    }
    if let Some(by) = &f.superseded_by {
        obj.insert("superseded_by".into(), Value::String(by.to_string()));
    }
    if !f.tags.is_empty() {
        obj.insert(
            "tags".into(),
            serde_json::to_value(&f.tags).unwrap_or(Value::Null),
        );
    }
    Value::Object(obj)
}

/// Compact JSON representation of an `EntitySummary` for MCP wire output.
/// Drops the ULID (agents look up by name) and empty `tags`.
fn slim_entity_summary(e: &aidememo_core::EntitySummary) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("name".into(), Value::String(e.name.clone()));
    obj.insert("type".into(), Value::String(e.entity_type.to_string()));
    obj.insert("facts".into(), Value::Number(e.fact_count.into()));
    if !e.tags.is_empty() {
        obj.insert(
            "tags".into(),
            serde_json::to_value(&e.tags).unwrap_or(Value::Null),
        );
    }
    Value::Object(obj)
}

fn tool_entity_get(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("name required")?;
    let entity = wiki.entity_get(name).map_err(|e| e.to_string())?;
    let mut obj = serde_json::Map::new();
    obj.insert("name".into(), Value::String(entity.name.clone()));
    obj.insert("type".into(), Value::String(entity.entity_type.to_string()));
    if !entity.aliases.is_empty() {
        obj.insert(
            "aliases".into(),
            serde_json::to_value(&entity.aliases).unwrap_or(Value::Null),
        );
    }
    if !entity.tags.is_empty() {
        obj.insert(
            "tags".into(),
            serde_json::to_value(&entity.tags).unwrap_or(Value::Null),
        );
    }
    if let Some(s) = &entity.summary {
        if !s.is_empty() {
            obj.insert("summary".into(), Value::String(s.clone()));
        }
    }
    if let Some(p) = &entity.source_page {
        obj.insert("source_page".into(), Value::String(p.clone()));
    }
    let text = serde_json::to_string(&Value::Object(obj)).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_fact_get(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("id required")?;
    let fact_id = parse_fact_id(id)?;
    let fact = wiki.fact_get(&fact_id).map_err(|e| e.to_string())?;
    let text = serde_json::to_string(&slim_fact_record(&fact, wiki)).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_entity_list(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let entity_type =
        args.get("type")
            .and_then(|v| v.as_str())
            .map(|s| match s.to_lowercase().as_str() {
                "technology" | "tech" => aidememo_core::EntityType::Technology,
                "concept" => aidememo_core::EntityType::Concept,
                "comparison" | "compare" => aidememo_core::EntityType::Comparison,
                "query" | "question" => aidememo_core::EntityType::Query,
                "person" => aidememo_core::EntityType::Person,
                "team" => aidememo_core::EntityType::Team,
                _ => aidememo_core::EntityType::Unknown,
            });

    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let opts = aidememo_core::types::ListOpts {
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

    let slim: Vec<Value> = entities.iter().map(slim_entity_summary).collect();
    let payload = json!({
        "entities": slim,
        "next_offset": next_offset,
    });
    let text = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_traverse(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let entity = args
        .get("entity")
        .and_then(|v| v.as_str())
        .ok_or("entity required")?;
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
    let direction = match args
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("forward")
    {
        "reverse" | "backward" | "back" => aidememo_core::TraverseDirection::Reverse,
        _ => aidememo_core::TraverseDirection::Forward,
    };

    let result = wiki
        .traverse(
            entity,
            aidememo_core::TraverseOpts {
                depth,
                relation_types: None,
                direction,
            },
        )
        .map_err(|e| e.to_string())?;

    Ok(ToolCallResult {
        content: vec![ContentBlock::text(format!(
            "Traversed from '{}' (depth={}, direction={:?})\n{:#?}",
            entity, depth, direction, result
        ))],
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
            "No relations point to or from this entity. Either link it to a related one (`aidememo relation add`) or delete it via the CLI if it's irrelevant."
        }
        "duplicate" => {
            "Two facts have near-identical content. Read both via aidememo_fact_get; if they say the same thing, retire the older one with aidememo_fact_supersede."
        }
        "conflict" => {
            "Atomic fact types (decision / pattern / convention) are mutually exclusive per entity. Pick the survivor and run aidememo_fact_supersede on the others — the timeline is preserved."
        }
        "stale" => {
            "Fact hasn't been touched in a while. Verify it's still accurate. If reality changed, run aidememo_fact_supersede with an updated version."
        }
        "malformed_entity" => {
            "Entity record is incomplete or broken. Read it with aidememo_entity_get and repair via aidememo_entity_describe + fact_update."
        }
        _ => "See the message field for details.",
    }
}

fn tool_overview(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let mut opts = aidememo_core::OverviewOpts::default();
    if let Some(n) = args.get("top_n").and_then(|v| v.as_u64()) {
        opts.top_n_entities = n as usize;
    }
    if let Some(d) = args.get("recent_days").and_then(|v| v.as_u64()) {
        opts.recent_days = d;
    }
    let result = wiki.overview(opts).map_err(|e| e.to_string())?;

    // Compact wire format for the MCP tool call: strips ULIDs, empty
    // tags, and the redundant `entity_type` repetition inside each
    // bucket's top_examples. Cuts the agent-visible payload roughly
    // 4× (~75%) versus the pretty-printed `OverviewResult`. The CLI
    // and Rust API still see the full record.
    let entity_types: Vec<Value> = result
        .entity_types
        .iter()
        .map(|b| {
            let top: Vec<Value> = b
                .top_examples
                .iter()
                .map(|e| json!({"name": e.name, "facts": e.fact_count}))
                .collect();
            json!({
                "type": b.entity_type.to_string(),
                "count": b.count,
                "top": top,
            })
        })
        .collect();
    let fact_types: Vec<Value> = result
        .fact_types
        .iter()
        .map(|b| json!({"type": b.fact_type.to_string(), "count": b.count}))
        .collect();
    let top_entities: Vec<Value> = result
        .top_entities
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "type": e.entity_type.to_string(),
                "facts": e.fact_count,
            })
        })
        .collect();
    let payload = json!({
        "stats": {
            "entities": result.stats.entity_count,
            "facts": result.stats.fact_count,
            "relations": result.stats.relation_count,
        },
        "entity_types": entity_types,
        "fact_types": fact_types,
        "top_entities": top_entities,
        "orphans": result.orphan_entity_count,
        "recent_facts": result.recent_fact_count,
        "current_facts": result.current_fact_count,
        "pinned_facts": result.pinned_fact_count,
    });
    // `to_string` (not _pretty) drops indentation — saves another 30%.
    let text = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_doctor(wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let issues = wiki.lint().map_err(|e| e.to_string())?;
    let stats = wiki.stats().map_err(|e| e.to_string())?;
    let sharing = collect_sharing_status(wiki.store_path(), wiki.config());
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
        "sharing": sharing,
        "issues": issues,
    });
    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_recent(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

    // Two ways to express the lookback window:
    //   - `last`      — duration DSL string (e.g. "30d", "12h", "4w") —
    //                   matches the CLI `aidememo recent --last` surface.
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

    let opts = aidememo_core::FactListOpts {
        fact_type: None,
        entity_id: None,
        min_confidence: None,
        source_id: None,
        limit: Some(limit),
        offset: 0,
        since,
        until: None,
        current_only: false,
        as_of: None,
    };
    let facts = wiki.fact_list(opts).map_err(|e| e.to_string())?;
    // Wrap the array in {"facts": [...]} for shape consistency with
    // the other list-style tools. Uses the same slim representation
    // as `tool_fact_list` so the agent sees identical schema across
    // every facts-returning tool.
    let slim: Vec<Value> = facts.iter().map(|f| slim_fact_record(f, wiki)).collect();
    let payload = json!({ "facts": slim });
    let text = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

/// `aidememo_aggregate` — deterministic counting / enumeration on top of
/// hybrid search. Solves the multi-session aggregation failure mode
/// observed in the LongMemEval bench: readers see 30 snippets but
/// inconsistently count or sum. Pulling the reader out of the loop
/// for the arithmetic step is the closest agentic analog to OMEGA's
/// multi-session category prompt's STEP A/B/C synthesis.
///
/// Operations:
/// * count     — N facts matching the query (after fact_type filter)
/// * enumerate — same N as a deduped item list (id + content preview)
/// * by_entity — N facts grouped by primary entity, with per-group
///   count and fact_type set
fn tool_aggregate(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("query required")?;
    let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("count");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let preview_chars = args
        .get("preview_chars")
        .and_then(|v| v.as_u64())
        .unwrap_or(120) as usize;
    let fact_type_filter: Option<aidememo_core::FactType> = args
        .get("fact_type")
        .and_then(|v| v.as_str())
        .map(aidememo_core::FactType::parse);
    let entity_filter = match args.get("entity").and_then(|v| v.as_str()) {
        Some(name) => Some(vec![wiki.resolve_entity(name).map_err(|e| e.to_string())?]),
        None => None,
    };
    let since = parse_time_arg(args.get("since"), false)?;
    let current_only = args
        .get("current_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let source_id = mcp_source_id(args);

    let hits = wiki
        .hybrid_search(
            query,
            aidememo_core::SearchOpts {
                limit: Some(limit),
                bm25_only: false,
                current_only,
                since,
                entity_filter,
                source_id,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;

    // Apply fact_type filter post-hoc (keep the search wide; don't
    // miss candidates that the BM25/RRF fusion ranked highly even if
    // they happen to be a different type).
    let filtered: Vec<&aidememo_core::SearchResult> = hits
        .iter()
        .filter(|h| {
            fact_type_filter
                .as_ref()
                .map(|t| &h.fact_type == t)
                .unwrap_or(true)
        })
        .collect();

    let payload = match op {
        "count" => json!({
            "op": "count",
            "query": query,
            "matched": filtered.len(),
            "facts_considered": hits.len(),
        }),
        "enumerate" => {
            let items: Vec<Value> = filtered
                .iter()
                .map(|h| {
                    let mut content = h.content.clone();
                    truncate_in_place(&mut content, preview_chars);
                    json!({
                        "id": h.fact_id.to_string(),
                        "content": content,
                        "fact_type": format!("{:?}", h.fact_type).to_lowercase(),
                        "score": h.score,
                        "entities": h.entity_names,
                    })
                })
                .collect();
            json!({
                "op": "enumerate",
                "query": query,
                "matched": filtered.len(),
                "items": items,
            })
        }
        "by_entity" => {
            use std::collections::BTreeMap;
            let mut groups: BTreeMap<String, Vec<&aidememo_core::SearchResult>> = BTreeMap::new();
            for hit in &filtered {
                let key = hit
                    .entity_names
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "(no entity)".into());
                groups.entry(key).or_default().push(hit);
            }
            let mut group_arr: Vec<Value> = groups
                .into_iter()
                .map(|(entity, hits)| {
                    let mut types: Vec<String> = hits
                        .iter()
                        .map(|h| format!("{:?}", h.fact_type).to_lowercase())
                        .collect();
                    types.sort();
                    types.dedup();
                    let max_score = hits.iter().map(|h| h.score).fold(0.0_f32, f32::max);
                    json!({
                        "entity": entity,
                        "count": hits.len(),
                        "fact_types": types,
                        "max_score": max_score,
                    })
                })
                .collect();
            // Order groups by max_score descending so the agent sees
            // the strongest matches first.
            group_arr.sort_by(|a, b| {
                let s_a = a.get("max_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let s_b = b.get("max_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                s_b.partial_cmp(&s_a).unwrap_or(std::cmp::Ordering::Equal)
            });
            json!({
                "op": "by_entity",
                "query": query,
                "matched": filtered.len(),
                "groups": group_arr,
            })
        }
        "sum_currency" | "sum_duration" | "count_distinct_dates" | "timeline" => {
            // Layer-1 structured-extraction ops. Walk every matching
            // fact's text through aidememo_core::extract_structured, then
            // aggregate by the requested kind. Deterministic — no
            // LLM call. Respects the `relevance` cosine threshold
            // when set so off-topic facts that happen to match BM25
            // don't pollute the sum.
            let relevance_threshold: f32 = args
                .get("relevance_threshold")
                .and_then(|v| v.as_f64())
                .map(|x| x as f32)
                .unwrap_or(0.0);
            structured_aggregate(op, query, &filtered, wiki, relevance_threshold)?
        }
        _ => {
            return Err(format!(
                "invalid op '{op}': must be one of count, enumerate, by_entity, \
             sum_currency, sum_duration, count_distinct_dates, timeline"
            ));
        }
    };

    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

/// Walk each matched fact's text through Layer-1 structured extraction
/// and aggregate the typed slots. Deterministic — no LLM call. The
/// query embedding is computed once and re-used to score per-fact
/// semantic relevance so the caller can drop off-topic facts that
/// BM25 surfaced (e.g., a $400 hotel quote leaking into a "bike
/// expenses" sum).
fn structured_aggregate(
    op: &str,
    query: &str,
    facts: &[&aidememo_core::SearchResult],
    wiki: &AideMemo,
    relevance_threshold: f32,
) -> Result<Value, String> {
    use chrono::{DateTime, Utc};
    use std::collections::BTreeSet;

    // Embed the query once for relevance filtering. If embedding
    // fails we fall through with everything included — caller can opt
    // out by setting threshold=0.
    let q_vec: Option<Vec<f32>> = wiki.embed(query).ok();

    let mut currency_total: std::collections::BTreeMap<String, f64> =
        std::collections::BTreeMap::new();
    let mut currency_mentions: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let mut duration_total_secs: f64 = 0.0;
    let mut duration_mentions: Vec<String> = Vec::new();
    let mut distinct_dates: BTreeSet<String> = BTreeSet::new();
    let mut timeline: Vec<(i64, String, String)> = Vec::new(); // (epoch_ms, fact_id, raw)

    let mut considered = 0;
    let mut filtered_out = 0;
    for hit in facts {
        // Per-fact relevance filter via cosine similarity.
        if let Some(q) = &q_vec {
            let rel = wiki
                .embed(&hit.content)
                .ok()
                .map(|f| AideMemo::cosine_similarity(q, &f))
                .unwrap_or(1.0);
            if rel < relevance_threshold {
                filtered_out += 1;
                continue;
            }
        }
        considered += 1;
        // Anchor relative dates ("yesterday") on this fact's
        // observed_at if present.
        let anchor = hit
            .observed_at
            .and_then(|ms| DateTime::<Utc>::from_timestamp_millis(ms as i64));
        for v in aidememo_core::extract_structured::extract(&hit.content, anchor) {
            match v.kind {
                aidememo_core::extract_structured::ValueKind::Currency => {
                    let divisor = if matches!(v.unit.as_str(), "KRW" | "JPY") {
                        1.0
                    } else {
                        100.0
                    };
                    *currency_total.entry(v.unit.clone()).or_insert(0.0) += v.value / divisor;
                    currency_mentions
                        .entry(v.unit.clone())
                        .or_default()
                        .push(v.raw.clone());
                }
                aidememo_core::extract_structured::ValueKind::Duration => {
                    duration_total_secs += v.value;
                    duration_mentions.push(v.raw.clone());
                }
                aidememo_core::extract_structured::ValueKind::EventDate => {
                    let d = DateTime::<Utc>::from_timestamp_millis(v.value as i64)
                        .map(|dt| dt.date_naive().to_string())
                        .unwrap_or_default();
                    if !d.is_empty() {
                        distinct_dates.insert(d);
                        timeline.push((v.value as i64, hit.fact_id.to_string(), v.raw.clone()));
                    }
                }
                aidememo_core::extract_structured::ValueKind::Count => {
                    // Counts aren't summed by these ops — caller asks
                    // for op="count" / "enumerate" if they want raw
                    // count semantics.
                }
            }
        }
    }
    let payload = match op {
        "sum_currency" => {
            let totals: Vec<Value> = currency_total
                .iter()
                .map(|(unit, total)| {
                    let mentions = currency_mentions.get(unit).cloned().unwrap_or_default();
                    let preview: Vec<String> = mentions.iter().take(20).cloned().collect();
                    json!({
                        "unit": unit,
                        "total": total,
                        "mentions": mentions.len(),
                        "samples": preview,
                    })
                })
                .collect();
            json!({
                "op": "sum_currency",
                "query": query,
                "facts_considered": considered,
                "facts_filtered_out": filtered_out,
                "by_unit": totals,
            })
        }
        "sum_duration" => {
            json!({
                "op": "sum_duration",
                "query": query,
                "facts_considered": considered,
                "facts_filtered_out": filtered_out,
                "total_seconds": duration_total_secs,
                "total_minutes": duration_total_secs / 60.0,
                "total_hours": duration_total_secs / 3600.0,
                "total_days": duration_total_secs / 86400.0,
                "total_weeks": duration_total_secs / (86400.0 * 7.0),
                "mentions": duration_mentions.len(),
                "samples": duration_mentions.iter().take(20).cloned().collect::<Vec<_>>(),
            })
        }
        "count_distinct_dates" => {
            let dates: Vec<String> = distinct_dates.iter().cloned().collect();
            json!({
                "op": "count_distinct_dates",
                "query": query,
                "facts_considered": considered,
                "facts_filtered_out": filtered_out,
                "distinct_count": dates.len(),
                "dates": dates,
            })
        }
        "timeline" => {
            timeline.sort_by_key(|t| t.0);
            let entries: Vec<Value> = timeline
                .iter()
                .map(|(ms, fid, raw)| {
                    let dt = DateTime::<Utc>::from_timestamp_millis(*ms)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default();
                    json!({
                        "date": dt,
                        "fact_id": fid,
                        "raw": raw,
                    })
                })
                .collect();
            json!({
                "op": "timeline",
                "query": query,
                "facts_considered": considered,
                "facts_filtered_out": filtered_out,
                "events": entries,
            })
        }
        _ => unreachable!("op pre-validated by caller"),
    };
    Ok(payload)
}

fn tool_query(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let topic = args
        .get("topic")
        .and_then(|v| v.as_str())
        .ok_or("topic required")?;
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .map(aidememo_core::QueryMode::parse)
        .unwrap_or_default();
    let opts = aidememo_core::QueryOpts {
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
        source_id: mcp_source_id(args),
    };
    // Agent UX: format / max_chars budget. Default "full" preserves the
    // existing contract; "compact" truncates each snippet's content to
    // a preview length (cuts agent context cost ~3×). max_chars hard-caps
    // the serialized result text so the agent can size its turn budget
    // before invoking — overflow drops `related` first, then truncates
    // longest snippets uniformly.
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("full");
    let max_chars = args
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let preview_chars = args
        .get("preview_chars")
        .and_then(|v| v.as_u64())
        .unwrap_or(200) as usize;
    // level: agent's granularity choice. "fact" (default) returns flat
    // snippets — best for SS-user / pinpoint lookup. "entity" rolls up
    // hits into per-entity groups — closer to the bench's session-level
    // ingest pattern that lifted multi-session +20pt and temporal +20pt
    // on LongMemEval. Currently only the markdown text format honours
    // level=entity; JSON formats stay snippet-flat so the schema is
    // stable for downstream parsers.
    let level = args.get("level").and_then(|v| v.as_str()).unwrap_or("fact");
    let mut result = wiki.query(topic, opts).map_err(|e| e.to_string())?;
    if format == "compact" {
        compact_query_result(&mut result, preview_chars);
    }
    if let Some(budget) = max_chars {
        fit_query_result_to_budget(&mut result, budget, preview_chars);
    }
    let text = match (format, level) {
        ("text", "session") => {
            let blocks = collect_session_blocks(&result.search, wiki, 20);
            render_session_blocks_markdown(&result, &blocks, preview_chars, max_chars)
        }
        ("text", "entity") => {
            render_query_result_markdown_by_entity(&result, preview_chars, max_chars)
        }
        ("text", _) => render_query_result_markdown(&result, preview_chars, max_chars),
        _ => serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?,
    };
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

/// Markdown rendering — drops JSON envelope bloat. Agent gets a terse
/// human-readable summary suitable for direct prompt injection. Loses
/// ULID precision (still includes short suffix for follow-up
/// aidememo_fact_get) and timestamps but preserves entity names + scores.
/// When `budget` is set, drops trailing snippets/recent_facts to fit.
fn render_query_result_markdown(
    r: &aidememo_core::QueryResult,
    preview_chars: usize,
    budget: Option<usize>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n", r.topic));
    if let Some(e) = &r.entity {
        out.push_str(&format!(
            "entity: **{}** ({})\n",
            e.name,
            format!("{:?}", e.entity_type).to_lowercase()
        ));
        if let Some(s) = &e.summary
            && !s.is_empty()
        {
            out.push_str(&format!("> {s}\n"));
        }
    }
    if !r.search.is_empty() {
        out.push_str("\n## hits\n");
        for h in &r.search {
            let mut content = h.content.clone();
            truncate_in_place(&mut content, preview_chars);
            let id_short = &h
                .fact_id
                .to_string()
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            let ents = if h.entity_names.is_empty() {
                String::new()
            } else {
                format!(" _[{}]_", h.entity_names.join(", "))
            };
            let ftype = format!("{:?}", h.fact_type).to_lowercase();
            out.push_str(&format!(
                "- ({:.2} {} …{}) {}{}\n",
                h.score, ftype, id_short, content, ents
            ));
        }
    }
    if !r.related.is_empty() {
        out.push_str("\n## related\n");
        let names: Vec<String> = r.related.iter().map(|e| e.name.clone()).collect();
        out.push_str(&format!("- {}\n", names.join(", ")));
    }
    if !r.recent_facts.is_empty() {
        out.push_str("\n## recent\n");
        for f in &r.recent_facts {
            let mut content = f.content.clone();
            truncate_in_place(&mut content, preview_chars);
            let id_short = &f
                .id
                .to_string()
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            out.push_str(&format!("- (…{}) {}\n", id_short, content));
        }
    }
    if let Some(b) = budget {
        if out.len() > b {
            // Truncate from the back (recent_facts disappear first because
            // they're rendered last). Strip trailing lines until under
            // budget, then add an ellipsis marker.
            while out.len() > b.saturating_sub(20)
                && let Some(idx) = out.rfind('\n')
            {
                out.truncate(idx);
            }
            out.push_str("\n… (truncated)\n");
        }
    }
    out
}

/// Markdown rendering with entity-level grouping. Each top-K hit is
/// associated with its entities; we group by primary entity and emit
/// one section per group with the top facts inside. Mirrors the
/// bench's session-level ingest pattern at the agent UX layer —
/// reader sees per-entity blocks instead of flat snippets, which
/// helped multi-session aggregation +20pt and temporal +20pt on
/// LongMemEval.
///
/// Group ordering: by best score within group, descending. Within a
/// group, facts are ordered by score descending. The query topic's
/// entity (when matched) is always emitted first.
fn render_query_result_markdown_by_entity(
    r: &aidememo_core::QueryResult,
    preview_chars: usize,
    budget: Option<usize>,
) -> String {
    use std::collections::BTreeMap;

    let mut out = String::new();
    out.push_str(&format!("# {}\n", r.topic));
    if let Some(e) = &r.entity {
        out.push_str(&format!(
            "entity: **{}** ({})\n",
            e.name,
            format!("{:?}", e.entity_type).to_lowercase()
        ));
        if let Some(s) = &e.summary
            && !s.is_empty()
        {
            out.push_str(&format!("> {s}\n"));
        }
    }

    // Group hits by primary entity name (first entity in entity_names).
    // Hits with no entity go into "(no entity)" group, last.
    let mut groups: BTreeMap<String, Vec<&aidememo_core::SearchResult>> = BTreeMap::new();
    let mut group_order: Vec<String> = Vec::new();
    for hit in &r.search {
        let key = hit
            .entity_names
            .first()
            .cloned()
            .unwrap_or_else(|| "(no entity)".to_string());
        if !groups.contains_key(&key) {
            group_order.push(key.clone());
        }
        groups.entry(key).or_default().push(hit);
    }
    // Re-order groups: topic-entity first, then by max-score per group
    // (descending). Within a group facts already arrive in score order.
    let topic_entity_name = r.entity.as_ref().map(|e| e.name.clone());
    group_order.sort_by(|a, b| {
        if Some(a) == topic_entity_name.as_ref() {
            return std::cmp::Ordering::Less;
        }
        if Some(b) == topic_entity_name.as_ref() {
            return std::cmp::Ordering::Greater;
        }
        let max_a = groups[a].iter().map(|h| h.score).fold(0.0_f32, f32::max);
        let max_b = groups[b].iter().map(|h| h.score).fold(0.0_f32, f32::max);
        max_b
            .partial_cmp(&max_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if !group_order.is_empty() {
        out.push_str("\n## hits by entity\n");
        for ent_name in &group_order {
            let hits = &groups[ent_name];
            let n = hits.len();
            // List distinct fact_types in the group for at-a-glance tagging.
            let mut types: Vec<String> = hits
                .iter()
                .map(|h| format!("{:?}", h.fact_type).to_lowercase())
                .collect();
            types.sort();
            types.dedup();
            out.push_str(&format!(
                "### {} _({} · {} fact{})_\n",
                ent_name,
                types.join(" · "),
                n,
                if n == 1 { "" } else { "s" }
            ));
            for hit in hits {
                let mut content = hit.content.clone();
                truncate_in_place(&mut content, preview_chars);
                let id_short: String = hit
                    .fact_id
                    .to_string()
                    .chars()
                    .rev()
                    .take(6)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                out.push_str(&format!("- ({:.2} …{}) {}\n", hit.score, id_short, content));
            }
        }
    }
    if !r.related.is_empty() {
        out.push_str("\n## related\n");
        let names: Vec<String> = r.related.iter().map(|e| e.name.clone()).collect();
        out.push_str(&format!("- {}\n", names.join(", ")));
    }
    if let Some(b) = budget
        && out.len() > b
    {
        while out.len() > b.saturating_sub(20)
            && let Some(idx) = out.rfind('\n')
        {
            out.truncate(idx);
        }
        out.push_str("\n… (truncated)\n");
    }
    out
}

/// Truncate every snippet/fact `content` to `preview_chars` chars (+ "…").
/// Leaves IDs, scores, entity names, and metadata intact so the agent
/// can still drill in via aidememo_fact_get for any specific hit.
/// One session-rolled-up block. Built at READ time from search hits
/// whose entity_names contain a "session:" prefix (the convention
/// `aidememo session new` creates). Each block's content is the FULL
/// session — every fact attached to that session entity, joined in
/// chronological order — not just the matched turns. Lets the agent
/// see coherent dialog blocks at zero storage overhead (vs the
/// bench's --hybrid-ingest 2× storage approach).
struct SessionBlock {
    session_id: String,
    content: String,
    n_facts: usize,
    matched_count: usize,
    max_score: f32,
}

/// Group search hits by their session entity (name prefixed with
/// "session:"), then for each unique session fetch the FULL list of
/// facts attached to that entity and concat in chronological order.
/// Caps at `max_blocks` ordered by best-matching session first.
///
/// Storage cost: 0 (computed on read). Latency cost: one
/// `fact_list(entity_id=..)` per unique session in the top-K — bound
/// by max_blocks × tens of ms. The bench's --hybrid-ingest writes
/// session-summary records at ingest time (2× storage); this mirrors
/// that lift purely on the read path.
fn collect_session_blocks(
    hits: &[aidememo_core::SearchResult],
    wiki: &AideMemo,
    max_blocks: usize,
) -> Vec<SessionBlock> {
    use std::collections::BTreeMap;
    // Group hits by their first session-prefixed entity name. Hits
    // with no session entity skip — the rollup only operates on
    // tracked-session writes.
    let mut by_session: BTreeMap<String, Vec<&aidememo_core::SearchResult>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for hit in hits {
        for name in &hit.entity_names {
            if name.starts_with("session-") || name.starts_with("session:") {
                if !by_session.contains_key(name) {
                    order.push(name.clone());
                }
                by_session.entry(name.clone()).or_default().push(hit);
                break;
            }
        }
    }

    // For each unique session entity, fact_list ALL its facts (full
    // session) and concat. Skip sessions where the entity can't be
    // resolved or fact_list fails — defensive against partial state.
    let mut blocks: Vec<SessionBlock> = Vec::new();
    for sess_name in &order {
        let Some(hits_in_sess) = by_session.get(sess_name) else {
            continue;
        };
        let Ok(eid) = wiki.resolve_entity(sess_name) else {
            continue;
        };
        let mut facts = match wiki.fact_list(aidememo_core::FactListOpts {
            fact_type: None,
            entity_id: Some(eid),
            min_confidence: None,
            source_id: None,
            limit: Some(200),
            offset: 0,
            since: None,
            until: None,
            current_only: true,
            as_of: None,
        }) {
            Ok(f) => f,
            Err(_) => continue,
        };
        // Chronological sort — observed_at when present, fall back to
        // created_at. Matches the bench's session_text construction.
        facts.sort_by_key(|f| f.observed_at.unwrap_or(f.created_at));
        let content = facts
            .iter()
            .map(|f| f.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let max_score = hits_in_sess.iter().map(|h| h.score).fold(0.0_f32, f32::max);
        blocks.push(SessionBlock {
            session_id: sess_name
                .strip_prefix("session-")
                .or_else(|| sess_name.strip_prefix("session:"))
                .unwrap_or(sess_name)
                .to_string(),
            content,
            n_facts: facts.len(),
            matched_count: hits_in_sess.len(),
            max_score,
        });
    }

    // Order by best-match score within block (desc), then truncate.
    blocks.sort_by(|a, b| {
        b.max_score
            .partial_cmp(&a.max_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    blocks.truncate(max_blocks);
    blocks
}

/// Render session-rolled-up blocks as markdown. Mirrors the
/// markdown-by-entity layout but blocks are session-scoped and the
/// content is the FULL session (every turn), not just matched turns.
fn render_session_blocks_markdown(
    r: &aidememo_core::QueryResult,
    blocks: &[SessionBlock],
    preview_chars: usize,
    budget: Option<usize>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n", r.topic));
    if let Some(e) = &r.entity {
        out.push_str(&format!(
            "entity: **{}** ({})\n",
            e.name,
            format!("{:?}", e.entity_type).to_lowercase()
        ));
        if let Some(s) = &e.summary
            && !s.is_empty()
        {
            out.push_str(&format!("> {s}\n"));
        }
    }
    if blocks.is_empty() {
        if !r.search.is_empty() {
            out.push_str("\n_(no session-tagged hits — see flat snippets via level=\"fact\")_\n");
        }
    } else {
        out.push_str(&format!(
            "\n## hits by session ({} block{})\n",
            blocks.len(),
            if blocks.len() == 1 { "" } else { "s" }
        ));
        for b in blocks {
            out.push_str(&format!(
                "\n### session {} _(score {:.2}, {} fact{}, {} matched in search)_\n",
                b.session_id,
                b.max_score,
                b.n_facts,
                if b.n_facts == 1 { "" } else { "s" },
                b.matched_count,
            ));
            let mut content = b.content.clone();
            truncate_in_place(&mut content, preview_chars);
            out.push_str(&content);
            out.push('\n');
        }
    }
    if !r.related.is_empty() {
        out.push_str("\n## related\n");
        let names: Vec<String> = r.related.iter().map(|e| e.name.clone()).collect();
        out.push_str(&format!("- {}\n", names.join(", ")));
    }
    if let Some(b) = budget
        && out.len() > b
    {
        while out.len() > b.saturating_sub(20)
            && let Some(idx) = out.rfind('\n')
        {
            out.truncate(idx);
        }
        out.push_str("\n… (truncated)\n");
    }
    out
}

fn compact_query_result(result: &mut aidememo_core::QueryResult, preview_chars: usize) {
    for hit in &mut result.search {
        truncate_in_place(&mut hit.content, preview_chars);
    }
    for fact in &mut result.recent_facts {
        truncate_in_place(&mut fact.content, preview_chars);
    }
}

fn truncate_in_place(s: &mut String, n: usize) {
    if s.chars().count() <= n {
        return;
    }
    let byte_idx = s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len());
    s.truncate(byte_idx);
    s.push('…');
}

/// Iteratively shrink the result until its serialized form fits
/// `budget` chars. Order:
/// 1. Drop `related` (graph context — agent can re-query).
/// 2. Truncate previews progressively (200 → 30 chars).
/// 3. Drop recent_facts from the tail.
/// 4. Drop search hits from the tail (keep top-rank).
fn fit_query_result_to_budget(
    result: &mut aidememo_core::QueryResult,
    budget: usize,
    _min_preview: usize,
) {
    fn size_of(r: &aidememo_core::QueryResult) -> usize {
        serde_json::to_string(r).map(|s| s.len()).unwrap_or(0)
    }
    if size_of(result) <= budget {
        return;
    }
    // 1. Drop graph traversal (most expendable for budget-pinched queries).
    if !result.related.is_empty() {
        result.related.clear();
        if size_of(result) <= budget {
            return;
        }
    }
    // 2. Stepped preview shrink — 200 down to 30 chars.
    for preview in [200, 120, 80, 50, 30] {
        for hit in &mut result.search {
            truncate_in_place(&mut hit.content, preview);
        }
        for fact in &mut result.recent_facts {
            truncate_in_place(&mut fact.content, preview);
        }
        if size_of(result) <= budget {
            return;
        }
    }
    // 3. Drop recent_facts from tail until fits.
    while size_of(result) > budget && result.recent_facts.pop().is_some() {}
    if size_of(result) <= budget {
        return;
    }
    // 4. Drop search hits from tail, but keep at least 1 so the agent
    // sees the top match. If even 1 hit blows budget, the agent has to
    // raise the budget — we can't lose the entity envelope without
    // breaking the contract.
    while result.search.len() > 1 && size_of(result) > budget {
        result.search.pop();
    }
}

/// `aidememo_context` — single-call agent-turn entry point. Returns one
/// envelope with everything an agent typically wants at the top of
/// a turn: pinned facts (always-on tier), personalisation
/// (preference / lesson / error), recent activity, top entities,
/// and (when `topic` is given) topic-specific search hits +
/// traverse + topic-related lessons / errors.
///
/// Replaces the common 3-call chain (`aidememo_session_start` →
/// `aidememo_query` → `aidememo_search`) with one round-trip. Designed so the
/// agent can call it on every turn without worrying about which
/// retrieval shape to use first. When `topic` is omitted the
/// response is identical in scope to `aidememo_session_start` plus the
/// new personalisation tier.
fn tool_context(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let topic = args
        .get("topic")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let pinned_limit = args
        .get("pinned_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10) as usize;
    let recent_limit = args
        .get("recent_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10) as usize;
    let recent_days = args
        .get("recent_days")
        .and_then(|v| v.as_u64())
        .unwrap_or(7);
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
    let bm25_only = args
        .get("bm25_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let source_id = mcp_source_id(args);
    let source_id = source_id.as_deref();

    // ── Tier 1: always-on context ─────────────────────────────────
    let pinned: Vec<Value> = wiki
        .pinned_facts(if source_id.is_some() {
            pinned_limit.saturating_mul(4).max(pinned_limit)
        } else {
            pinned_limit
        })
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|f| source_id_matches(f, source_id))
        .take(pinned_limit)
        .map(|f| slim_fact_record(f, wiki))
        .collect();
    let personalisation = collect_personalisation(wiki, 50, source_id);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let since = Some(now_ms.saturating_sub(recent_days * 24 * 60 * 60 * 1000));
    let recent: Vec<Value> = wiki
        .fact_list(aidememo_core::FactListOpts {
            fact_type: None,
            entity_id: None,
            min_confidence: None,
            source_id: source_id.map(str::to_string),
            limit: Some(recent_limit),
            offset: 0,
            since,
            until: None,
            current_only: true,
            as_of: None,
        })
        .map_err(|e| e.to_string())?
        .iter()
        .map(|f| slim_fact_record(f, wiki))
        .collect();

    // ── Tier 2: topic-specific context (optional) ─────────────────
    let topic_section = if let Some(t) = topic {
        let q_opts = aidememo_core::QueryOpts {
            search_limit: limit,
            depth,
            recent_limit: 5,
            since: None,
            current_only: true,
            mode: aidememo_core::QueryMode::default(),
            bm25_only,
            source_id: source_id.map(str::to_string),
        };
        let qres = wiki.query(t, q_opts).map_err(|e| e.to_string())?;

        // Topic-specific lessons + errors — pull all that match the
        // hybrid_search hits' entity ids so the agent gets prior
        // attempts + known failure patterns inline. Cheaper than a
        // separate aidememo_search per type.
        let mut topic_entity_ids: std::collections::HashSet<aidememo_core::EntityId> =
            std::collections::HashSet::new();
        for hit in &qres.search {
            for name in &hit.entity_names {
                if let Ok(id) = wiki.resolve_entity(name) {
                    topic_entity_ids.insert(id);
                }
            }
        }
        let mut topic_lessons: Vec<Value> = Vec::new();
        let mut topic_errors: Vec<Value> = Vec::new();
        for eid in topic_entity_ids {
            for ftype in [
                aidememo_core::FactType::Lesson,
                aidememo_core::FactType::Error,
            ] {
                let facts = wiki
                    .fact_list(aidememo_core::FactListOpts {
                        fact_type: Some(ftype),
                        entity_id: Some(eid),
                        min_confidence: None,
                        source_id: source_id.map(str::to_string),
                        limit: Some(5),
                        offset: 0,
                        since: None,
                        until: None,
                        current_only: true,
                        as_of: None,
                    })
                    .unwrap_or_default();
                for f in facts {
                    let v = slim_fact_record(&f, wiki);
                    match ftype {
                        aidememo_core::FactType::Lesson => topic_lessons.push(v),
                        aidememo_core::FactType::Error => topic_errors.push(v),
                        _ => {}
                    }
                }
            }
        }
        Some(json!({
            "topic": t,
            "query_result": qres,
            "topic_lessons": topic_lessons,
            "topic_errors": topic_errors,
        }))
    } else {
        None
    };

    let mut payload = serde_json::Map::new();
    payload.insert("pinned".into(), Value::Array(pinned));
    payload.insert("personalisation".into(), Value::Array(personalisation));
    payload.insert("recent".into(), Value::Array(recent));
    if let Some(t) = topic_section {
        payload.insert("topic".into(), t);
    }

    // Agent UX: format=text emits a sectioned markdown summary much
    // smaller than the JSON envelope. max_chars hard-caps text output
    // by trimming sections back-to-front (topic > recent >
    // personalisation > pinned). For format=full / compact the JSON
    // schema stays as-is; downstream parsers don't break.
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("full");
    let preview_chars = args
        .get("preview_chars")
        .and_then(|v| v.as_u64())
        .unwrap_or(160) as usize;
    let max_chars = args
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    if format == "text" {
        let text = render_context_markdown(&payload, topic, preview_chars, max_chars);
        return Ok(ToolCallResult {
            content: vec![ContentBlock::text(text)],
            is_error: None,
        });
    }

    let text = serde_json::to_string(&Value::Object(payload)).map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(text)],
        is_error: None,
    })
}

fn tool_workflow_start(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("title required")?;
    let body = args
        .get("body")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let source = args
        .get("source")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let source_id = mcp_source_id(args);
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
    let recent_limit = args
        .get("recent_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(5) as usize;
    let bm25_only = args
        .get("bm25_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let payload = wiki
        .workflow_start(
            title,
            WorkflowStartOpts {
                body: body.map(str::to_string),
                source: source.map(str::to_string),
                source_id,
                limit,
                depth,
                recent_limit,
                bm25_only,
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(
            serde_json::to_string(&payload).map_err(|e| e.to_string())?,
        )],
        is_error: None,
    })
}

fn render_context_markdown(
    payload: &serde_json::Map<String, Value>,
    topic: Option<&str>,
    preview_chars: usize,
    budget: Option<usize>,
) -> String {
    fn fact_line(f: &Value, preview: usize) -> String {
        let id = f.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let id_short: String = id
            .chars()
            .rev()
            .take(6)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        // slim_fact_record uses "type" / "entities" — keep render in
        // sync with that schema.
        let ftype = f.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let mut content = f
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        truncate_in_place(&mut content, preview);
        let entities = f
            .get("entities")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let ent_tag = if entities.is_empty() {
            String::new()
        } else {
            format!(" _[{}]_", entities)
        };
        format!("- ({} …{}) {}{}", ftype, id_short, content, ent_tag)
    }

    let mut out = String::new();
    let header = if let Some(t) = topic {
        format!("# context · topic: {}\n", t)
    } else {
        "# context\n".to_string()
    };
    out.push_str(&header);

    if let Some(arr) = payload.get("pinned").and_then(|v| v.as_array())
        && !arr.is_empty()
    {
        out.push_str(&format!("\n## pinned ({})\n", arr.len()));
        for f in arr {
            out.push_str(&fact_line(f, preview_chars));
            out.push('\n');
        }
    }
    if let Some(arr) = payload.get("personalisation").and_then(|v| v.as_array())
        && !arr.is_empty()
    {
        out.push_str(&format!("\n## personalisation ({})\n", arr.len()));
        for f in arr {
            out.push_str(&fact_line(f, preview_chars));
            out.push('\n');
        }
    }
    if let Some(arr) = payload.get("recent").and_then(|v| v.as_array())
        && !arr.is_empty()
    {
        out.push_str(&format!("\n## recent ({})\n", arr.len()));
        for f in arr {
            out.push_str(&fact_line(f, preview_chars));
            out.push('\n');
        }
    }
    if let Some(topic_section) = payload.get("topic").and_then(|v| v.as_object()) {
        let t = topic_section
            .get("topic")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        out.push_str(&format!("\n## topic: {}\n", t));
        if let Some(qres) = topic_section.get("query_result")
            && let Some(arr) = qres.get("search").and_then(|v| v.as_array())
            && !arr.is_empty()
        {
            out.push_str(&format!("### hits ({})\n", arr.len()));
            for hit in arr {
                let mut content = hit
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                truncate_in_place(&mut content, preview_chars);
                let id_short: String = hit
                    .get("fact_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .rev()
                    .take(6)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                let score = hit.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let ftype = hit.get("fact_type").and_then(|v| v.as_str()).unwrap_or("");
                out.push_str(&format!(
                    "- ({:.2} {} …{}) {}\n",
                    score, ftype, id_short, content
                ));
            }
        }
        for (key, header) in [
            ("topic_lessons", "### lessons"),
            ("topic_errors", "### errors"),
        ] {
            if let Some(arr) = topic_section.get(key).and_then(|v| v.as_array())
                && !arr.is_empty()
            {
                out.push_str(&format!("{} ({})\n", header, arr.len()));
                for f in arr {
                    out.push_str(&fact_line(f, preview_chars));
                    out.push('\n');
                }
            }
        }
    }

    if let Some(b) = budget
        && out.len() > b
    {
        // Trim from tail — topic_section last, then recent,
        // personalisation, pinned. Just chop suffix lines.
        while out.len() > b.saturating_sub(20)
            && let Some(idx) = out.rfind('\n')
        {
            out.truncate(idx);
        }
        out.push_str("\n… (truncated)\n");
    }
    out
}

fn tool_entity_describe(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
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
/// passed to `aidememo_fact_add{,_many}`.
struct ResolvedEntities {
    /// Entity IDs in the same order as the input names.
    ids: Vec<aidememo_core::EntityId>,
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
/// already exist. Mirrors the CLI behavior in `aidememo fact add` so MCP and
/// CLI no longer diverge: previously the MCP path silently dropped
/// unknown names via `filter_map`, leaving the fact attached to fewer
/// entities than the agent expected. New entities default to
/// `EntityType::Unknown`.
///
/// On every auto-create the helper also runs the existing
/// `suggest_similar_entities` fuzzy matcher; when a candidate scores
/// above the trigram threshold the (`requested`, `suggestions`) pair is
/// returned as an `EntityNameAlternative`. Auto-create still proceeds
/// — the caller decides whether to merge with `aidememo_fact_supersede` /
/// alias the new entity. The default is non-blocking because the agent
/// might genuinely mean the new name, but the warning makes typo-driven
/// fragmentation visible at the moment it would otherwise happen
/// silently.
fn resolve_or_create_entities(
    wiki: &AideMemo,
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
                    .entity_add(aidememo_core::EntityInput {
                        name: name.clone(),
                        entity_type: Some(aidememo_core::EntityType::Unknown),
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

fn tool_fact_add(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
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
    let mut entity_ids = resolved.ids;
    let auto_created = resolved.created;
    let alternatives = resolved.alternatives;
    let session_id_arg = args.get("session_id").and_then(|v| v.as_str());
    let session_entity_id = resolve_session_entity(wiki, session_id_arg)?;
    attach_session_entity(&mut entity_ids, session_entity_id);
    let fact_type = parse_fact_type_arg(args.get("fact_type"))?;
    let source_id = mcp_source_id(args);

    // Pre-add similarity check (non-blocking). BM25-only so we don't
    // pay the embedding-model load on every add — the goal is just to
    // surface "this looks like an existing fact" so the agent can opt
    // to aidememo_fact_supersede instead of stacking duplicates. Set
    // `dedup_check: false` to skip (e.g. for trusted bulk imports).
    let dedup_check = args
        .get("dedup_check")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let existing_similar = if dedup_check {
        wiki.hybrid_search(
            content,
            aidememo_core::SearchOpts {
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

    let input = aidememo_core::types::FactInput {
        content: content.into(),
        fact_type,
        entity_ids: if entity_ids.is_empty() {
            None
        } else {
            Some(entity_ids.clone())
        },
        tags: if tags.is_empty() { None } else { Some(tags) },
        source: None,
        source_id: source_id.clone(),
        source_confidence: None,
        observed_at: None,
    };

    let id = wiki.add_fact(input).map_err(|e| e.to_string())?;

    // Verify-symmetry: return the persisted record so the agent can
    // confirm the write landed without a separate `aidememo_fact_get` round
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
        "source_id": record.source_id,
        "auto_created_entities": auto_created,
        "entity_name_alternatives": alternatives_payload(&alternatives),
        "existing_similar": existing_similar,
    });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
        is_error: None,
    })
}

fn parse_fact_id(s: &str) -> Result<aidememo_core::FactId, String> {
    aidememo_core::ulid::Ulid::from_string(s)
        .map(aidememo_core::FactId)
        .map_err(|_| format!("invalid fact ID: {s}"))
}

fn tool_fact_add_many(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
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
    let default_session_id = args.get("session_id").and_then(|v| v.as_str());
    let default_session_entity_id = resolve_session_entity(wiki, default_session_id)?;
    let default_source_id = mcp_source_id(args);
    for (i, item) in items.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("items[{i}] must be an object"))?;
        let content = obj
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("items[{i}].content is required"))?
            .to_string();
        let mut names: Vec<String> = obj
            .get("entities")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let resolved = resolve_or_create_entities(wiki, &names)?;
        let mut entity_ids_vec = resolved.ids;
        let item_session_id = obj
            .get("session_id")
            .and_then(|v| v.as_str())
            .or(default_session_id);
        let session_entity_id = if obj.get("session_id").is_some() {
            resolve_session_entity(wiki, item_session_id)?
        } else {
            default_session_entity_id
        };
        attach_session_entity(&mut entity_ids_vec, session_entity_id);
        if let Some(session_name) = item_session_id.map(str::trim).filter(|s| !s.is_empty()) {
            if !names.iter().any(|name| name == session_name) {
                names.push(session_name.to_string());
            }
        }
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
        let source_id =
            source_id_from_value(obj.get("source_id")).or_else(|| default_source_id.clone());
        inputs.push(aidememo_core::types::FactInput {
            content,
            fact_type,
            entity_ids,
            tags,
            source: None,
            source_id,
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

fn tool_fact_supersede(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
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

fn tool_fact_archive(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    let raw_ids = args
        .get("ids")
        .and_then(|v| v.as_array())
        .ok_or("ids array required")?;
    let mut targets: Vec<aidememo_core::FactId> = Vec::with_capacity(raw_ids.len());
    for v in raw_ids {
        let s = v.as_str().ok_or("each id must be a string")?;
        targets.push(parse_fact_id(s)?);
    }
    let dry_run = args
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if targets.is_empty() {
        return Ok(ToolCallResult {
            content: vec![ContentBlock::text(
                json!({"moved": 0, "candidates": 0, "dry_run": dry_run}).to_string(),
            )],
            is_error: None,
        });
    }
    let moved = if dry_run {
        // Pre-check existence to give the agent a useful preview count
        // without actually moving anything.
        targets
            .iter()
            .filter(|id| wiki.fact_get(id).is_ok())
            .count()
    } else {
        wiki.archive_facts(&targets).map_err(|e| e.to_string())?
    };
    let payload = json!({
        "moved": moved,
        "candidates": targets.len(),
        "dry_run": dry_run,
    });
    Ok(ToolCallResult {
        content: vec![ContentBlock::text(payload.to_string())],
        is_error: None,
    })
}

fn tool_fact_edit(args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
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
            aidememo_core::FactUpdate {
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
            name: "aidememo_search".into(),
            description:
                "Search facts in the wiki using BM25 + semantic vectors. Returns ranked results. Defaults to current-only (excludes superseded facts) — pass `current_only:false` for historical/timeline queries. Pass `bm25_only:true` to skip the embedding model load (cuts cold-start ~700-900ms; loses semantic recall). For graph context (related entities + recent facts) prefer `aidememo_query` instead — it wraps this tool plus traversal in one call."
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
                    "source_id": {
                        "type": "string",
                        "description": "Restrict to facts from this source namespace / tenant / upstream id. If omitted, MCP falls back to AIDEMEMO_SOURCE_ID when set."
                    },
                    "min_confidence": {
                        "type": "number",
                        "description": "Filter facts with source_confidence below this threshold."
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Optional. Pin a session_id (e.g. group several queries under one logical session). If omitted, a fresh ULID is minted and returned alongside the results — feed it back into aidememo_feedback to record helpful/not-helpful signal that trains the ranking adapter."
                    },
                    "include_archive": {
                        "type": "boolean",
                        "default": false,
                        "description": "Also search the cold-tier archive (`<store>.cold.redb`) and merge any matches in to fill remaining slots up to `limit`. Off by default — most callers want only live facts. Use when an `audit / what-did-I-once-say-about-X` shape needs to reach archived content."
                    },
                    "format": {"type": "string", "enum": ["full", "compact", "text"], "default": "full", "description": "full = JSON results array. compact = JSON with each content truncated to preview_chars. text = markdown bullet list (~4× smaller, no JSON envelope, drops session_id from rendered output but the agent still gets it via the underlying record). Use text for prompt injection, full when piping to a downstream parser, compact for budget-sensitive JSON."},
                    "preview_chars": {"type": "number", "default": 200, "description": "Per-hit content cap when format ∈ {compact, text}. Agent drills in via aidememo_fact_get for full content."},
                    "max_chars": {"type": "number", "description": "Hard cap on serialized result size. Drops trailing hits until under budget; always keeps top match. Use to bound agent turn cost."}
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "aidememo_feedback".into(),
            description: "Record helpful / not-helpful feedback on a fact returned by a recent aidememo_search call. Pass the session_id from that search response. Feedback feeds into the domain adapter (`aidememo adapt train`) which, when applied (`config.search.use_adapter=true`, default), nudges future ranking toward facts the agent confirmed were useful."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "session_id from the aidememo_search response"},
                    "fact_id":    {"type": "string", "description": "ULID of the fact in question"},
                    "helpful":    {"type": "boolean", "description": "true = the fact answered the query; false = it did not"}
                },
                "required": ["session_id", "fact_id", "helpful"]
            }),
        },
        Tool {
            name: "aidememo_session_start".into(),
            description: "One-call session warmup. Returns the four things an agent typically needs at the top of a new conversation — pinned-tier facts, recent activity (last 7d default), top entities by fact_count, and open lint issues with action hints — in one envelope so the agent doesn't chain four reads. Each section has its own limit knob to keep the response bounded."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pinned_limit": {"type": "number", "default": 20},
                    "recent_limit": {"type": "number", "default": 10},
                    "recent_days":  {"type": "number", "default": 7, "description": "Lookback window for the 'recent' section."},
                    "top_entities_limit": {"type": "number", "default": 10}
                }
            }),
        },
        Tool {
            name: "aidememo_pinned_context".into(),
            description: "Return the agent's 'always loaded' tier — every fact tagged `pinned=true` (and not superseded), sorted by recent access. Inspired by Letta's core / archival memory split: an agent calls this once at session start to seed working context with the handful of facts it should know without searching. Pin / unpin via aidememo_fact_pin."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "number", "default": 20, "description": "Cap on the returned set so the warmup envelope stays bounded."}
                }
            }),
        },
        Tool {
            name: "aidememo_fact_pin".into(),
            description: "Pin or unpin a fact in the always-loaded tier (see aidememo_pinned_context). Pass `pinned: true` to add or `pinned: false` to remove. Use sparingly — pinned facts compete with the agent's working-memory budget; reserve for long-lived rules and headline decisions."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id":     {"type": "string", "description": "Fact ULID"},
                    "pinned": {"type": "boolean", "description": "true = pin, false = unpin"}
                },
                "required": ["id", "pinned"]
            }),
        },
        Tool {
            name: "aidememo_extract".into(),
            description: "Conversation → fact extractor. Pass a chat transcript / paragraph as `text`; returns ranked candidate facts above `min_confidence`. Default extractor is heuristic (sentence split + entity-substring + fact-type keyword cues — fully offline). Pass `llm: true` to dispatch to the LLM provider configured by `extract.provider` (e.g. `gpt-4o-mini` via OpenAI) for higher-quality extraction; the call falls back to heuristic with a warning if the provider is unset or the request fails. By default returns previews so the agent can edit / approve before committing — pass `apply: true` to persist every surviving candidate via `fact_add` (each runs the dedup-hint and entity-name-alternatives checks).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Raw text to extract from. Dialog markers (`> `, `Speaker:`) are auto-stripped from each sentence."},
                    "max_candidates": {"type": "number", "default": 20, "description": "Cap on candidates returned, scored highest first."},
                    "min_confidence": {"type": "number", "default": 0.5, "description": "Drop candidates below this threshold. 0.0 returns everything; 0.7 keeps only high-signal hits."},
                    "llm": {"type": "boolean", "default": false, "description": "Use the LLM extractor (extract.provider). Falls back to heuristic on failure."},
                    "apply": {"type": "boolean", "default": false, "description": "If true, persist every surviving candidate via fact_add (each runs the standard dedup + typo guards) and return the ULIDs alongside."}
                },
                "required": ["text"]
            }),
        },
        Tool {
            name: "aidememo_path".into(),
            description: "Find the shortest path between two entities (BFS over typed relations). Returns {from, to, path: [hops]}. For breadth-first exploration of one neighborhood, use aidememo_traverse instead.".into(),
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
            name: "aidememo_fact_list".into(),
            description: "List facts with optional entity filter. Defaults to current_only=true. Use aidememo_recent for time-windowed listing or aidememo_search when you have a query string.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "entity": {"type": "string", "description": "Filter by entity name/alias"},
                    "source_id": {"type": "string", "description": "Filter by source namespace / tenant / upstream id. If omitted, MCP falls back to AIDEMEMO_SOURCE_ID when set."},
                    "limit":  {"type": "number", "default": 20},
                    "offset": {"type": "number", "default": 0, "description": "Skip the first N facts. Combined with `limit`, paginate through the full result. Response includes `next_offset` (null when the page is the last)."},
                    "current_only": {"type": "boolean", "default": true, "description": "Exclude superseded facts. Pass false to include historical timeline."}
                }
            }),
        },
        Tool {
            name: "aidememo_entity_get".into(),
            description: "Get a single entity by name (or alias). On miss, returns suggestions in the error so you can correct the name. Returns the JSON record on success."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
        },
        Tool {
            name: "aidememo_fact_get".into(),
            description: "Get a single fact by ULID. Returns the JSON record.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"id": {"type": "string", "description": "Fact ULID"}},
                "required": ["id"]
            }),
        },
        Tool {
            name: "aidememo_entity_list".into(),
            description: "List entities in AideMemo with fact counts. To fetch one entity's record use aidememo_entity_get; to find related entities by graph use aidememo_traverse.".into(),
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
            name: "aidememo_traverse".into(),
            description: "Graph walk from a starting entity. direction=\"forward\" (default) → entities X reaches; direction=\"reverse\" → entities that reach X (\"what depends on X\"). For shortest path between two known entities use aidememo_path. Replaces the separate aidememo_backlinks tool — backlinks is now an alias kept for backwards compatibility.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "entity": {"type": "string"},
                    "depth": {"type": "number", "default": 2},
                    "direction": {"type": "string", "enum": ["forward", "reverse"], "default": "forward", "description": "forward = outbound (X → ?); reverse = inbound (? → X), the 'who depends on X' query."}
                },
                "required": ["entity"]
            }),
        },
        Tool {
            name: "aidememo_aggregate".into(),
            description: "Deterministic aggregation primitives on top of hybrid search. Pulls the agent out of the synthesis loop for 'how much / how many / between when' questions — instead of scanning snippets and miscounting, this tool walks matching facts and returns a structured count / sum / timeline the agent reads directly.\n\n**CALL THIS when the user's question is one of**:\n  * 'How much total / combined / in total $X did I spend on Y across (all sessions / this year / etc.)?' → op=sum_currency\n  * 'How many hours / days / weeks total did I spend on X?' → op=sum_duration\n  * 'How many distinct days / dates did event X happen?' → op=count_distinct_dates\n  * 'List events about X in chronological order' → op=timeline\n  * 'How many times did I do / decide / try X?' → op=count or enumerate\n  * 'Group facts about X by entity' → op=by_entity\n\nThe focused 60-question LongMemEval run showed a large multi-session gain when aggregation replaced in-head arithmetic, but the later 240-question balanced run put agentic-loop dispatch within reader noise of the single-call baseline. Treat aidememo_aggregate as insurance for exact counting / summing / timelines, not as a general accuracy lever.\n\n**DON'T CALL THIS for simple retrieval** — 'What did I say about X?', 'When did I last do Y?', 'What's my preference for Z?'. Those are answered by aidememo_query / aidememo_search / aidememo_context. Calling aidememo_aggregate on simple-recall questions wastes a round-trip and (in our measurement) mildly degrades single-fact reasoning by adding tool-call structure where prose suffices.\n\n**Fact-level ops** (each result row = one STORED FACT):\n  * count — N facts matching the query\n  * enumerate — deduped item list (id + content preview)\n  * by_entity — grouped by primary entity with per-group count + fact_types + max_score\n\n**Value-level ops** (Layer-1 structured extraction; walks fact text via aidememo_core::extract_structured to pull typed slots — currency / duration / event_date — without any LLM call):\n  * sum_currency — sum of dollar/won/etc values across matching facts, broken down by ISO unit\n  * sum_duration — sum of durations in seconds + minutes/hours/days/weeks\n  * count_distinct_dates — count of unique dates referenced across matching facts\n  * timeline — chronological list of dated events with fact_id back-reference\n\nValue-level ops accept an optional relevance_threshold (cosine similarity 0..1) that filters facts whose semantic similarity to the query is below the threshold — drops off-topic facts BM25 surfaced (e.g., a $400 hotel quote leaking into a 'bike expenses' sum). Empirically: 0 = no filter, 0.4 ≈ p50 on LongMemEval data; threshold tuning trades coverage vs noise.\n\nIMPORTANT — what each op counts: count is fact rows, sum_currency is sum-of-dollar-values-mentioned-in-any-matching-fact. They answer different question shapes — pick the right primitive.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Hybrid search query (BM25 + semantic). Use the natural-language form of the question — the tool widens K internally."},
                    "op": {"type": "string", "enum": ["count", "enumerate", "by_entity", "sum_currency", "sum_duration", "count_distinct_dates", "timeline"], "default": "count", "description": "Fact-level: count / enumerate / by_entity. Value-level (Layer-1 structured extraction): sum_currency / sum_duration / count_distinct_dates / timeline."},
                    "fact_type": {"type": "string", "enum": ["decision", "pattern", "convention", "claim", "note", "question", "preference", "lesson", "error"], "description": "Optional post-hoc filter on fact_type. Search runs wide; this narrows the count."},
                    "entity": {"type": "string", "description": "Optional entity scope — restrict to facts attached to this entity."},
                    "since": {"type": ["string", "number"], "description": "Lower bound on observed_at. ISO date / RFC3339 / epoch ms / duration DSL (30d, 4w)."},
                    "current_only": {"type": "boolean", "default": true, "description": "Exclude superseded facts."},
                    "limit": {"type": "number", "default": 50, "description": "Top-K to consider before fact_type filter. Larger → more recall, slower."},
                    "preview_chars": {"type": "number", "default": 120, "description": "Per-item content preview length when op=enumerate."},
                    "relevance_threshold": {"type": "number", "default": 0.0, "description": "For value-level ops only. Cosine similarity 0..1; facts below the threshold are dropped before structured extraction. 0 = no filter (default). 0.4 ≈ p50 on LongMemEval data — drops half the off-topic facts."}
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "aidememo_doctor".into(),
            description: "Wiki health check: counts, lint issues, and shared-store ergonomics. The `sharing` block reports lock_retry_ms, daemon state, the measured serverless writer envelope, and concrete retry/daemon hints. Call this first if results look wrong or multiple agents share one store."
                .into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "aidememo_overview".into(),
            description: "First-impression snapshot of the wiki: entity-type buckets with top examples, fact-type distribution, top central entities by fact_count, recent activity, and current/pinned/orphan counts. Designed for an agent arriving at an unfamiliar wiki — one call instead of stats + entity_list + fact_list. \
                          IMPORTANT: This is an *orientation map* only. It tells you WHICH entities and topics exist; it does NOT contain the underlying facts. To answer any question that needs specific facts (decisions, conventions, notes, claims) you MUST follow up with `aidememo_query`/`aidememo_search`/`aidememo_fact_list` against the entity names this returns. Don't compose final answers from overview output alone."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "top_n": {"type": "number", "default": 10, "description": "Top-N entities globally and per entity_type bucket"},
                    "recent_days": {"type": "number", "default": 7, "description": "Window in days for the recent_fact_count field"}
                }
            }),
        },
        Tool {
            name: "aidememo_recent".into(),
            description: "Recently added/updated facts. Defaults to the last 7 days, 20 facts. Returns {\"facts\": [...]}. For full context on a topic (search + graph + recent in one call) use aidememo_query."
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
            name: "aidememo_context".into(),
            description: "Single-call agent-turn entry point. One round-trip returns the full retrieval envelope: pinned facts (always-on tier), personalisation (preference / lesson / error — decay-exempt), recent activity, and (when `topic` is given) topic-specific search hits + traverse + relevant lessons + relevant errors for the matched entities. Replaces the aidememo_session_start → aidememo_query → aidememo_search chain most agents do at the top of every turn. Use aidememo_search / aidememo_query / aidememo_fact_list for follow-up specific lookups; use aidememo_context for the broad opening read.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "topic": {"type": "string", "description": "Optional topic / entity name. Without it the response is session-start-style (pinned + personalisation + recent). With it, adds topic-specific search + traverse + topic-related lessons / errors."},
                    "limit": {"type": "number", "default": 10, "description": "Max topic search hits"},
                    "pinned_limit": {"type": "number", "default": 10},
                    "recent_limit": {"type": "number", "default": 10},
                    "recent_days": {"type": "number", "default": 7},
                    "depth": {"type": "number", "default": 2, "description": "Traverse depth if topic resolves to an entity"},
                    "source_id": {"type": "string", "description": "Restrict pinned, personalisation, recent, and topic context to this source namespace / tenant / upstream id. If omitted, MCP falls back to AIDEMEMO_SOURCE_ID when set."},
                    "format": {"type": "string", "enum": ["full", "text"], "default": "full", "description": "full = JSON envelope (4 sections, full metadata). text = sectioned markdown summary (~3-4× smaller, drops timestamps + entity metadata, keeps last-6 ULID for follow-up aidememo_fact_get). Use text for opening-turn prompt injection, full for downstream parsing."},
                    "preview_chars": {"type": "number", "default": 160, "description": "Per-fact content cap when format=text."},
                    "max_chars": {"type": "number", "description": "Hard cap on text output. When set, trims sections back-to-front (topic > recent > personalisation > pinned). Only honoured for format=text."}
                }
            }),
        },
        Tool {
            name: "aidememo_workflow_start".into(),
            description: concat!(
                "Start an issue/PR/ticket-driven coding workflow. Creates a tracked session entity, stores the incoming ticket as a question fact, and returns a context pack with relevant search hits, decisions, lessons, and errors. Use this when an automation trigger has only a short title/body and the agent needs project memory before acting. ",
                "Pass the returned session_id to aidememo_fact_add or aidememo_fact_add_many for facts learned during the workflow so later session-level queries can reconstruct the task thread."
            )
            .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Issue / PR / ticket title or short workflow trigger text."},
                    "body": {"type": "string", "description": "Optional issue / PR / ticket body."},
                    "source": {"type": "string", "description": "Optional upstream source id, e.g. github:org/repo#123 or linear:ENG-42."},
                    "source_id": {"type": "string", "description": "Optional source namespace / tenant / agent id for shared-store scoping. If omitted, MCP falls back to AIDEMEMO_SOURCE_ID when set."},
                    "limit": {"type": "number", "default": 8, "description": "Max context search hits."},
                    "depth": {"type": "number", "default": 2, "description": "Graph traversal depth for the topic query."},
                    "recent_limit": {"type": "number", "default": 5, "description": "Recent facts attached to the resolved entity."},
                    "bm25_only": {"type": "boolean", "default": false, "description": "Skip semantic embedding lookup. Use for deterministic CI/demo smoke runs or when surface-form recall is enough."}
                },
                "required": ["title"]
            }),
        },
        Tool {
            name: "aidememo_query".into(),
            description: "Topic-only retrieval (search + entity + traverse + recent). Lighter than aidememo_context — no pinned / personalisation tier. Prefer aidememo_context for an agent's opening turn; use aidememo_query for follow-up topic dives. Defaults to current_only=true. Modes: naive (search only), local (entity + neighbors, no global search), hybrid (default), global (broader scan). Agent context cost: pass `format:\"compact\"` to truncate each snippet's content to a preview (cuts result size ~3×). Hard-cap with `max_chars:N` to fit a turn budget — overflow drops `related` first, then uniformly shrinks snippet previews."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "topic": {"type": "string", "description": "Topic, entity name, or alias"},
                    "limit": {"type": "number", "default": 10, "description": "Max search hits"},
                    "depth": {"type": "number", "default": 2, "description": "Traverse depth if topic is an entity"},
                    "recent_limit": {"type": "number", "default": 10, "description": "Max recent facts"},
                    "mode": {"type": "string", "enum": ["naive", "local", "hybrid", "global"], "default": "hybrid"},
                    "current_only": {"type": "boolean", "default": true, "description": "Exclude superseded facts. Pass false for historical / timeline queries."},
                    "source_id": {"type": "string", "description": "Restrict search and recent facts to this source namespace / tenant / upstream id. If omitted, MCP falls back to AIDEMEMO_SOURCE_ID when set."},
                    "format": {"type": "string", "enum": ["full", "compact", "text"], "default": "full", "description": "full = JSON with all metadata. compact = JSON with truncated snippet content (drops bytes ~10%). text = markdown bullet summary, drops JSON envelope (~5× smaller, drops timestamps & entity metadata, keeps last-6 ULID suffix for follow-up aidememo_fact_get). Use text for agent prompt injection."},
                    "level": {"type": "string", "enum": ["fact", "entity", "session"], "default": "fact", "description": "Granularity of returned hits. fact = flat snippet list (best for pinpoint lookups: SS-user, abstention). entity = grouped per primary entity, mirroring the bench's session-level pattern that lifted multi-session +20pt and temporal +20pt. session = roll up per tracked-session entity (\"session:\" prefix; auto-created by `aidememo session new`) and emit each session's FULL fact list as one chronological block — restores dialog coherence for cross-turn questions (KU/temporal/SS-pref +10-20pt in our 60q MiniMax measurement). Storage cost: 0 (computed on read via fact_list per unique session entity). Currently only honoured by format=text — format=full / compact stay snippet-flat for stable JSON schema."},
                    "preview_chars": {"type": "number", "default": 200, "description": "Per-snippet content preview length when format=compact. Agent can drill in via aidememo_fact_get for any rank."},
                    "max_chars": {"type": "number", "description": "Hard cap on serialized result size (in chars). When set, the tool drops `related` first then trims previews until under budget. Use to bound agent turn cost."}
                },
                "required": ["topic"]
            }),
        },
        Tool {
            name: "aidememo_entity_describe".into(),
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
            name: "aidememo_fact_add".into(),
            description: "Add a fact to AideMemo.\n\n**SELF-EXTRACTION PATTERN**: Before calling, classify the content yourself \
                from the menu below. AideMemo deliberately does NOT ship a built-in LLM-aided \
                ingest pipeline (cf. Mem0 / Letta) — instead it relies on the calling \
                agent's own reasoning, which is almost always a stronger model than AideMemo \
                could ever embed. Pick the right `fact_type` (preference / lesson / \
                error / decision / convention / claim / note) so the in-pipeline weighting \
                (decay-exempt + 2× boost on personalisation tiers) actually fires on the \
                right facts.\n\nClassification cues (in order of specificity):\n\
                * 'I prefer X' / 'my favorite is Y' / 'I like Z' → preference\n\
                * 'I decided to X' / 'I chose Y' / 'going with Z' → decision\n\
                * 'tried X but Y' / 'turns out' / 'wish I had' → lesson\n\
                * 'avoid X' / 'never again' / 'was a mistake' → error\n\
                * 'always X' / 'every time' / 'I never X' → convention\n\
                * 'X uses Y for Z' / architectural assertion → pattern\n\
                * factual claim with no opinion → claim\n\
                * default catch-all → note\n\n\
                By default the tool runs a BM25 dedup check on the new content first — \
                if a high-overlap existing fact is found it appears as `existing_similar` \
                in the response (the new fact is still added; the agent decides whether \
                to aidememo_fact_supersede the older one). Missing entities are auto-created \
                (default type Unknown) and reported in `auto_created_entities`. If an \
                auto-created name is fuzzily similar to an existing entity (e.g. typo: \
                'Postgrs' vs existing 'Postgres'), the candidates appear as \
                `entity_name_alternatives` so the agent can decide to alias or merge \
                instead of leaving a fragmented graph. Returns {id, content, entity_names, \
                created_at, auto_created_entities, entity_name_alternatives, \
                existing_similar}."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string"},
                    "entities": {"type": "array", "items": {"type": "string"}, "description": "Entity names or aliases. Unknown names are auto-created."},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id. Use with aidememo_search/aidememo_query source_id filters to isolate shared-store reads. If omitted, MCP falls back to AIDEMEMO_SOURCE_ID when set."},
                    "session_id": {"type": "string", "description": "Optional workflow session id returned by aidememo_workflow_start. When set, the fact is attached to that session entity so later level:\"session\" queries can reconstruct the task thread."},
                    "fact_type": {
                        "type": "string",
                        "enum": ["decision", "pattern", "convention", "claim", "note", "question", "preference", "lesson", "error", "unknown"],
                        "description": "Self-classify before calling — see the trigger cues in the tool description. Atomic types (decision/pattern/convention) are mutually exclusive per entity — use aidememo_fact_supersede to retire the old one. Non-atomic (claim/note/question/preference/lesson/error) coexist freely."
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
            name: "aidememo_fact_add_many".into(),
            description: "Add many facts in a single transaction. Dramatically faster than many sequential aidememo_fact_add calls because the disk fsync cost is paid once per batch. Each item has the same shape as aidememo_fact_add's args. Classify each item yourself before calling (decision / lesson / error / preference / convention / claim / note) so aidememo's type-aware ranking can surface the right memories later. Pass top-level session_id from aidememo_workflow_start to attach every item to the current workflow, or item.session_id to override per fact. Returns {count, facts:[{id, entity_names}], auto_created_entities} — the dedup'd auto-created list lets you confirm new entities at a glance."
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
                                "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id. Overrides top-level source_id / AIDEMEMO_SOURCE_ID for this item."},
                                "session_id": {"type": "string", "description": "Optional workflow session id returned by aidememo_workflow_start. Overrides top-level session_id for this item."},
                                "fact_type": {
                                    "type": "string",
                                    "enum": ["decision", "pattern", "convention", "claim", "note", "question", "preference", "lesson", "error", "unknown"]
                                }
                            },
                            "required": ["content"]
                        }
                    },
                    "session_id": {"type": "string", "description": "Optional workflow session id returned by aidememo_workflow_start. Applies to every item unless item.session_id is set."},
                    "source_id": {"type": "string", "description": "Optional source namespace / tenant / upstream id. Applies to every item unless item.source_id is set. If omitted, MCP falls back to AIDEMEMO_SOURCE_ID when set."}
                },
                "required": ["items"]
            }),
        },
        Tool {
            name: "aidememo_fact_supersede".into(),
            description: "Mark an old fact as superseded by a new one. The old \
                fact stays in the store but won't appear in current_only \
                queries (the default for aidememo_search / aidememo_query / aidememo_fact_list). \
                Use this when a decision was overturned or a value \
                changed; for typo fixes use aidememo_fact_edit. The historical \
                timeline is preserved — aidememo_search with `as_of:<date>` \
                replays past state.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "old_id": {"type": "string", "description": "ULID of the fact being replaced"},
                    "new_id": {"type": "string", "description": "ULID of the replacement fact (must already exist; create it first via aidememo_fact_add)"},
                    "dry_run": {"type": "boolean", "default": false, "description": "Validate both ULIDs and return before/after content without writing. Use this to confirm you're about to retire the right fact."}
                },
                "required": ["old_id", "new_id"]
            }),
        },
        Tool {
            name: "aidememo_fact_archive".into(),
            description: "Move facts from the hot store into the cold-tier \
                archive (`<store>.cold.redb`). The hot store shrinks; cold \
                preserves the FactId so `aidememo_fact_get` keeps resolving the \
                archived fact for audit / soft-recovery. Archived facts drop \
                out of `aidememo_search` / `aidememo_query` results by default — pass \
                `include_archive:true` to either tool to merge cold-tier \
                matches back in. Use sparingly, in batch, for facts you've \
                decided are no longer hot context: old conversation logs, \
                deprecated decisions worth keeping for history, etc.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of fact ULIDs to archive."
                    },
                    "dry_run": {
                        "type": "boolean",
                        "default": false,
                        "description": "Count valid candidates but do not move anything."
                    }
                },
                "required": ["ids"]
            }),
        },
        Tool {
            name: "aidememo_fact_edit".into(),
            description: "Edit a fact's content in place. Choose exactly one of \
                append / prepend / find+replace / content. Use this for typo \
                fixes or clarifications — for semantic changes use \
                aidememo_fact_supersede instead so the timeline is preserved.".into(),
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

/// Heuristic-classify a tool error string into a stable category so
/// the agent can decide retry vs fix-input vs give-up without
/// reading the message. Tool functions still return Result<_, String>;
/// classification happens at the dispatcher layer to avoid touching
/// every tool's signature. Future migration can return a typed
/// ToolError directly and skip this step.
///
/// Categories (stable wire contract for agents):
/// * `invalid_input` — required parameter missing, bad enum, malformed
///   input. Agent must fix the call.
/// * `not_found`     — referenced entity / fact / session does not
///   exist. Agent should query first or accept the absence — retry
///   won't help.
/// * `conflict`      — atomic supersede / dedup mismatch. Agent should
///   resolve via aidememo_fact_supersede / aidememo_fact_edit.
/// * `unknown_tool`  — tool name typo. Agent should re-list tools.
/// * `internal`      — fallback. Agent may retry once; persistent
///   internal errors mean a bug worth reporting.
fn classify_error(msg: &str) -> &'static str {
    let lower = msg.to_lowercase();
    if lower.starts_with("unknown tool:") || lower.starts_with("unknown mode:") {
        return "unknown_tool";
    }
    if lower.contains(" required")
        || lower.contains("missing field")
        || lower.contains("invalid params")
        || lower.starts_with("invalid ")
        || lower.contains("must be ")
    {
        return "invalid_input";
    }
    if lower.contains("not found") || lower.contains("does not exist") || lower.contains("no such")
    {
        return "not_found";
    }
    if lower.contains("already exists")
        || lower.contains("conflict")
        || lower.contains("superseded")
        || lower.contains("duplicate")
    {
        return "conflict";
    }
    "internal"
}

fn format_tool_error(tool: &str, msg: &str) -> String {
    let kind = classify_error(msg);
    serde_json::to_string(&json!({
        "error_kind": kind,
        "tool": tool,
        "message": msg,
    }))
    .unwrap_or_else(|_| msg.to_string())
}

fn call_tool(name: &str, args: &Value, wiki: &AideMemo) -> Result<ToolCallResult, String> {
    match name {
        "aidememo_search" => tool_search(args, wiki),
        "aidememo_feedback" => tool_feedback(args, wiki),
        "aidememo_extract" => tool_extract(args, wiki),
        "aidememo_pinned_context" => tool_pinned_context(args, wiki),
        "aidememo_fact_pin" => tool_fact_pin(args, wiki),
        "aidememo_session_start" => tool_session_start(args, wiki),
        "aidememo_entity_get" => tool_entity_get(args, wiki),
        "aidememo_entity_list" => tool_entity_list(args, wiki),
        "aidememo_fact_get" => tool_fact_get(args, wiki),
        "aidememo_fact_list" => tool_fact_list(args, wiki),
        "aidememo_path" => tool_path(args, wiki),
        "aidememo_traverse" => tool_traverse(args, wiki),
        "aidememo_doctor" => tool_doctor(wiki),
        "aidememo_overview" => tool_overview(args, wiki),
        "aidememo_recent" => tool_recent(args, wiki),
        "aidememo_query" => tool_query(args, wiki),
        "aidememo_context" => tool_context(args, wiki),
        "aidememo_workflow_start" => tool_workflow_start(args, wiki),
        "aidememo_entity_describe" => tool_entity_describe(args, wiki),
        "aidememo_fact_add" => tool_fact_add(args, wiki),
        "aidememo_fact_add_many" => tool_fact_add_many(args, wiki),
        "aidememo_fact_supersede" => tool_fact_supersede(args, wiki),
        "aidememo_fact_archive" => tool_fact_archive(args, wiki),
        "aidememo_fact_edit" => tool_fact_edit(args, wiki),
        "aidememo_aggregate" => tool_aggregate(args, wiki),
        _ => Err(format!("Unknown tool: {}", name)),
    }
}

/// Dispatch a single JSON-RPC request to a response.
///
/// Returns `None` for notifications (which have no response).
pub fn dispatch(req: JsonRpcRequest, wiki: &AideMemo) -> Option<JsonRpcResponse> {
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
                        content: vec![ContentBlock::text(format_tool_error(&args.name, &e))],
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
    use aidememo_core::{Config, FactInput, FactType, types::EntityInput};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn open_temp_wiki() -> (TempDir, AideMemo) {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.store.path = dir.path().join("store").to_string_lossy().into_owned();
        let wiki = AideMemo::open(&PathBuf::from(&config.store.path), config).unwrap();
        (dir, wiki)
    }

    fn add_fact(wiki: &AideMemo, content: &str, entity: &str) -> aidememo_core::FactId {
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
        assert!(names.contains(&"aidememo_fact_supersede".to_string()));
        assert!(names.contains(&"aidememo_fact_edit".to_string()));
        assert!(names.contains(&"aidememo_fact_add_many".to_string()));
    }

    #[test]
    fn source_id_roundtrips_through_mcp_add_list_and_search() {
        let (_dir, wiki) = open_temp_wiki();

        for (content, source_id) in [
            ("Redis alpha source cache policy", "alpha"),
            ("Redis beta source cache policy", "beta"),
            ("Redis alpha source eviction policy", "alpha"),
        ] {
            tool_fact_add(
                &json!({
                    "content": content,
                    "entities": ["Redis"],
                    "source_id": source_id,
                    "dedup_check": false
                }),
                &wiki,
            )
            .unwrap();
        }

        let list = tool_fact_list(&json!({"source_id": "alpha", "limit": 10}), &wiki).unwrap();
        let list_text = list
            .content
            .first()
            .and_then(|b| b.text.as_deref())
            .unwrap_or("");
        let list_payload: Value = serde_json::from_str(list_text).expect("list response is JSON");
        let facts = list_payload["facts"].as_array().expect("facts array");
        assert_eq!(facts.len(), 2);
        assert!(
            facts
                .iter()
                .all(|f| f["source_id"].as_str() == Some("alpha"))
        );

        let search =
            tool_search(&json!({"query": "Redis source policy", "source_id": "beta", "limit": 10, "bm25_only": true}), &wiki)
                .unwrap();
        let search_text = search
            .content
            .first()
            .and_then(|b| b.text.as_deref())
            .unwrap_or("");
        let search_payload: Value =
            serde_json::from_str(search_text).expect("search response is JSON");
        let results = search_payload["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["source_id"].as_str(), Some("beta"));
    }

    #[test]
    fn mcp_source_id_uses_env_default_when_argument_is_missing() {
        let source_id = mcp_source_id_with_env(&json!({}), || Some(" agent-env ".to_string()));
        assert_eq!(source_id.as_deref(), Some("agent-env"));
    }

    #[test]
    fn mcp_source_id_argument_overrides_env_default() {
        let source_id = mcp_source_id_with_env(&json!({"source_id": "explicit-agent"}), || {
            Some("agent-env".to_string())
        });
        assert_eq!(source_id.as_deref(), Some("explicit-agent"));
    }

    #[test]
    fn mcp_source_id_ignores_empty_argument_and_empty_env() {
        let source_id =
            mcp_source_id_with_env(&json!({"source_id": "   "}), || Some("  ".to_string()));
        assert_eq!(source_id, None);
    }

    #[test]
    fn context_respects_source_id_scope() {
        let (_dir, wiki) = open_temp_wiki();

        for (content, source_id, fact_type) in [
            (
                "Alpha Redis convention prefers LRU cache policy",
                "alpha",
                "convention",
            ),
            (
                "Alpha Redis lesson: DNS caused worker timeout",
                "alpha",
                "lesson",
            ),
            (
                "Beta Redis convention prefers LFU cache policy",
                "beta",
                "convention",
            ),
            (
                "Beta Redis lesson: pool size caused worker timeout",
                "beta",
                "lesson",
            ),
        ] {
            tool_fact_add(
                &json!({
                    "content": content,
                    "entities": ["Redis"],
                    "source_id": source_id,
                    "fact_type": fact_type,
                    "dedup_check": false
                }),
                &wiki,
            )
            .unwrap();
        }

        let result = tool_context(
            &json!({
                "topic": "Redis worker timeout cache policy",
                "source_id": "alpha",
                "limit": 10,
                "bm25_only": true
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        let serialized = serde_json::to_string(&payload).unwrap();

        assert!(serialized.contains("Alpha Redis convention"));
        assert!(serialized.contains("Alpha Redis lesson"));
        assert!(!serialized.contains("Beta Redis convention"));
        assert!(!serialized.contains("Beta Redis lesson"));
    }

    #[test]
    fn aggregate_respects_source_id_scope() {
        let (_dir, wiki) = open_temp_wiki();

        for (content, source_id) in [
            ("Alpha Redis decision uses LRU cache policy", "alpha"),
            ("Beta Redis decision uses LFU cache policy", "beta"),
        ] {
            tool_fact_add(
                &json!({
                    "content": content,
                    "entities": ["Redis"],
                    "source_id": source_id,
                    "fact_type": "decision",
                    "dedup_check": false
                }),
                &wiki,
            )
            .unwrap();
        }

        let result = tool_aggregate(
            &json!({
                "query": "Redis cache policy decision",
                "op": "enumerate",
                "source_id": "alpha",
                "limit": 10
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        let serialized = serde_json::to_string(&payload).unwrap();

        assert!(serialized.contains("Alpha Redis decision"));
        assert!(!serialized.contains("Beta Redis decision"));
    }

    #[test]
    fn workflow_start_creates_session_ticket_and_scoped_context() {
        let (_dir, wiki) = open_temp_wiki();

        for (content, source_id, fact_type) in [
            (
                "Alpha decision: worker Redis timeout fixes go through the job wrapper",
                "alpha",
                "decision",
            ),
            (
                "Alpha lesson: Redis timeout was DNS, not pool size",
                "alpha",
                "lesson",
            ),
            (
                "Beta lesson: Redis timeout was caused by TLS handshake stalls",
                "beta",
                "lesson",
            ),
        ] {
            tool_fact_add(
                &json!({
                    "content": content,
                    "entities": ["Redis", "Worker"],
                    "source_id": source_id,
                    "fact_type": fact_type,
                    "dedup_check": false
                }),
                &wiki,
            )
            .unwrap();
        }

        let result = tool_workflow_start(
            &json!({
                "title": "Fix Redis timeout in worker",
                "body": "Worker jobs are timing out against Redis",
                "source": "github:org/repo#123",
                "source_id": "alpha"
            }),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();

        assert!(
            payload["session_id"]
                .as_str()
                .unwrap()
                .starts_with("session-")
        );
        assert!(payload["ticket_fact_id"].as_str().is_some());
        let session_id = payload["session_id"].as_str().unwrap();
        tool_fact_add(
            &json!({
                "content": "Implementation note: keep Redis timeout logging on the Worker session thread",
                "entities": ["Redis", "Worker"],
                "session_id": session_id,
                "source_id": "alpha",
                "fact_type": "lesson",
                "dedup_check": false
            }),
            &wiki,
        )
        .unwrap();
        let session_facts =
            tool_fact_list(&json!({"entity": session_id, "limit": 10}), &wiki).unwrap();
        let session_payload: Value =
            serde_json::from_str(session_facts.content[0].text.as_deref().unwrap()).unwrap();
        assert!(
            serde_json::to_string(&session_payload)
                .unwrap()
                .contains("Redis timeout logging")
        );
        let serialized = serde_json::to_string(&payload).unwrap();
        assert!(serialized.contains("job wrapper"));
        assert!(serialized.contains("DNS"));
        assert!(!serialized.contains("TLS handshake"));
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
    fn fact_add_many_attaches_top_level_session_id_to_each_item() {
        let (_dir, wiki) = open_temp_wiki();
        wiki.entity_add(EntityInput {
            name: "session-test".to_string(),
            entity_type: Some(aidememo_core::EntityType::parse("session")),
            ..Default::default()
        })
        .unwrap();

        let result = tool_fact_add_many(
            &json!({
                "session_id": "session-test",
                "items": [
                    {"content": "Decision: use advisory locks for invoice export", "entities": ["BillingExport"], "fact_type": "decision"},
                    {"content": "Lesson: retries duplicate exports without idempotency keys", "entities": ["BillingExport"], "fact_type": "lesson"}
                ]
            }),
            &wiki,
        )
        .unwrap();
        let text = result.content[0].text.as_deref().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        let facts = payload["facts"].as_array().unwrap();
        assert_eq!(facts.len(), 2);
        assert!(facts.iter().all(|fact| {
            fact["entity_names"]
                .as_array()
                .unwrap()
                .iter()
                .any(|name| name.as_str() == Some("session-test"))
        }));

        let session_facts =
            tool_fact_list(&json!({"entity": "session-test", "limit": 10}), &wiki).unwrap();
        let serialized = session_facts.content[0].text.as_deref().unwrap();
        assert!(serialized.contains("advisory locks"));
        assert!(serialized.contains("idempotency keys"));
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
        let id = aidememo_core::ulid::Ulid::from_string(payload["id"].as_str().unwrap()).unwrap();
        let record = wiki.fact_get(&aidememo_core::FactId(id)).unwrap();
        assert_eq!(record.fact_type, aidememo_core::FactType::Decision);
    }

    #[test]
    fn fact_add_rejects_unknown_fact_type_with_helpful_message() {
        let (_dir, wiki) = open_temp_wiki();
        // 'foobar' is not in any alias list — unlike 'decisions'
        // which now resolves to Decision via the central alias table.
        let err =
            tool_fact_add(&json!({"content": "x", "fact_type": "foobar"}), &wiki).unwrap_err();
        assert!(
            err.contains("decision") && err.contains("preference"),
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
        // the session_id from aidememo_search into aidememo_feedback. The latter
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
        assert_eq!(payload["sharing"]["serverless_recommended_writers"], 4);
        assert_eq!(
            payload["sharing"]["recommended_mode"],
            "serverless_fail_fast"
        );
        let sharing_hints = payload["sharing"]["hints"]
            .as_array()
            .expect("sharing hints present");
        assert!(
            sharing_hints
                .iter()
                .any(|h| h["code"] == "sharing_retry_disabled"),
            "MCP doctor should carry retry guidance: {sharing_hints:?}"
        );
    }

    #[test]
    fn fact_add_surfaces_entity_name_alternatives_for_typos() {
        // Live agent test caught this: "Postgres" exists, agent
        // accidentally posts a fact about "Postgrs", auto-create
        // silently splits the graph. Now the response must surface a
        // typo hint pointing at the existing entity so the agent can
        // aidememo_fact_supersede / alias instead of leaving the fragment.
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
    fn extract_returns_ranked_candidates_for_dialog_text() {
        let (_dir, wiki) = open_temp_wiki();
        wiki.entity_add(EntityInput {
            name: "Postgres".into(),
            entity_type: Some(aidememo_core::EntityType::Technology),
            ..Default::default()
        })
        .unwrap();

        let text = "Alice: We decided to use Postgres for hot writes.\n\
                    > the deploy ran fine on staging\n\
                    why does the cache miss after restart?\n\
                    short";
        let result = tool_extract(
            &json!({"text": text, "min_confidence": 0.4, "max_candidates": 10}),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["applied"], false);
        let cands = payload["candidates"].as_array().unwrap();
        assert!(!cands.is_empty(), "expected at least one candidate");
        // The decision sentence with Postgres entity match must rank highest.
        let top = &cands[0];
        assert_eq!(top["suggested_fact_type"], "decision");
        let entities: Vec<&str> = top["suggested_entities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(entities, vec!["Postgres"]);
        // "short" must be filtered out (below MIN_SENTENCE_CHARS).
        let contents: Vec<&str> = cands.iter().filter_map(|c| c["content"].as_str()).collect();
        assert!(contents.iter().all(|c| !c.eq(&"short")));
    }

    #[test]
    fn pinned_context_round_trips_through_pin_and_unpin() {
        let (_dir, wiki) = open_temp_wiki();
        let id = add_fact(&wiki, "use Postgres for hot writes", "Postgres");

        // Empty by default — even though we just added a fact.
        let result = tool_pinned_context(&json!({}), &wiki).unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["count"], 0);

        // Pin via the dedicated tool, then re-query — should appear.
        tool_fact_pin(&json!({"id": id.0.to_string(), "pinned": true}), &wiki).unwrap();
        let result = tool_pinned_context(&json!({}), &wiki).unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["count"], 1);
        assert_eq!(payload["facts"][0]["id"], id.0.to_string());

        // Unpin clears it.
        tool_fact_pin(&json!({"id": id.0.to_string(), "pinned": false}), &wiki).unwrap();
        let result = tool_pinned_context(&json!({}), &wiki).unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["count"], 0);
    }

    #[test]
    fn session_start_returns_all_four_sections() {
        // Build a tiny representative wiki: a pinned decision, a
        // recent note, a typed entity (so top_entities is non-empty),
        // and an orphan to trigger the open_issues path.
        let (_dir, wiki) = open_temp_wiki();
        let pinned = add_fact(&wiki, "use Postgres for hot writes", "Postgres");
        wiki.fact_pin(&pinned, true).unwrap();
        add_fact(&wiki, "deploy ran fine on staging", "Postgres");
        wiki.entity_add(EntityInput {
            name: "Floater".into(),
            ..Default::default()
        })
        .unwrap();

        let result = tool_session_start(&json!({}), &wiki).unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();

        assert!(
            payload["stats"]["fact_count"].as_u64().unwrap_or(0) >= 2,
            "stats present",
        );
        let pinned_arr = payload["pinned"].as_array().unwrap();
        assert_eq!(pinned_arr.len(), 1);
        assert!(
            payload["recent"].as_array().unwrap().len() >= 2,
            "recent present"
        );
        let top: Vec<&str> = payload["top_entities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert!(
            top.contains(&"Postgres"),
            "Postgres in top_entities: {top:?}"
        );
        // open_issues should call out the orphan.
        let by_code: Vec<&str> = payload["open_issues"]["by_code"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["code"].as_str())
            .collect();
        assert!(by_code.contains(&"orphan"), "orphan flagged: {by_code:?}");
    }

    #[test]
    fn pinned_context_excludes_superseded_facts() {
        let (_dir, wiki) = open_temp_wiki();
        let old = add_fact(&wiki, "use Redis 6", "Redis");
        let new = add_fact(&wiki, "use Redis 7", "Redis");
        // Pin the old fact, then supersede it. The pinned list should
        // not surface a retired record even if `pinned=true` is still
        // on it — that's a stale pin the agent can clean up later.
        tool_fact_pin(&json!({"id": old.0.to_string(), "pinned": true}), &wiki).unwrap();
        wiki.fact_supersede(&old, &new).unwrap();

        let result = tool_pinned_context(&json!({}), &wiki).unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["count"], 0);
    }

    #[test]
    fn extract_apply_persists_facts_and_returns_ids() {
        let (_dir, wiki) = open_temp_wiki();
        let text =
            "We decided to use Redis for hot caching. The migration is scheduled for Tuesday.";
        let result = tool_extract(
            &json!({"text": text, "apply": true, "min_confidence": 0.4}),
            &wiki,
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(result.content[0].text.as_deref().unwrap()).unwrap();
        assert_eq!(payload["applied"], true);
        let added = payload["added"].as_array().unwrap();
        assert!(!added.is_empty());
        for entry in added {
            let id_str = entry["id"].as_str().unwrap();
            let id = aidememo_core::ulid::Ulid::from_string(id_str).unwrap();
            // Round-trip: every persisted fact must be retrievable.
            wiki.fact_get(&aidememo_core::FactId(id)).unwrap();
        }
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
        wiki.entity_add(aidememo_core::EntityInput {
            name: "Redis".into(),
            entity_type: Some(aidememo_core::EntityType::Technology),
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
