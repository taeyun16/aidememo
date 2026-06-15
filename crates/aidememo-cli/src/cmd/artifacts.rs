//! Derived, read-only artifact renderers shared by CLI and MCP.

use aidememo_core::{
    AideMemo, AideMemoError, EntityRecord, EntitySort, EntitySummary, FactListOpts, FactRecord,
    FactType, ListOpts,
};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub struct SessionCanvasArtifact {
    pub body: String,
    pub session_id: String,
    pub topic: Option<String>,
    pub fact_count: usize,
}

pub struct ProjectProfileArtifact {
    pub body: String,
    pub fact_count: usize,
    pub entity_count: usize,
}

pub fn write_artifact_or_stdout(
    output: Option<&Path>,
    body: String,
    json: bool,
    mut payload: serde_json::Value,
) -> Result<String, AideMemoError> {
    let bytes = body.len();
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("bytes".into(), serde_json::json!(bytes));
    }

    if let Some(path) = output {
        std::fs::write(path, &body)
            .map_err(|e| AideMemoError::Internal(format!("write {}: {e}", path.display())))?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "output".into(),
                serde_json::json!(path.display().to_string()),
            );
        }
        if json {
            return serde_json::to_string_pretty(&payload).map_err(|e| AideMemoError::Serialize {
                context: "artifact metadata".into(),
                source: e,
            });
        }
        return Ok(format!("Wrote {} bytes to {}", bytes, path.display()));
    }

    if json {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("output".into(), serde_json::Value::Null);
            obj.insert("content".into(), serde_json::json!(body));
        }
        return serde_json::to_string_pretty(&payload).map_err(|e| AideMemoError::Serialize {
            context: "artifact content".into(),
            source: e,
        });
    }

    Ok(body)
}

pub fn session_canvas(
    wiki: &AideMemo,
    session: Option<&str>,
    limit: usize,
    include_superseded: bool,
) -> Result<SessionCanvasArtifact, AideMemoError> {
    let session = resolve_session_entity(wiki, session)?;
    let facts = wiki.fact_list(FactListOpts {
        entity_id: Some(session.id),
        limit: Some(limit),
        current_only: !include_superseded,
        ..Default::default()
    })?;
    let body = render_session_canvas(wiki, &session, &facts)?;
    Ok(SessionCanvasArtifact {
        body,
        session_id: session.name,
        topic: session.source_page,
        fact_count: facts.len(),
    })
}

pub fn project_profile(
    wiki: &AideMemo,
    limit: usize,
    source_id: Option<String>,
    include_sessions: bool,
) -> Result<ProjectProfileArtifact, AideMemoError> {
    render_project_profile(wiki, limit, source_id, include_sessions)
}

fn resolve_session_entity(
    wiki: &AideMemo,
    session: Option<&str>,
) -> Result<EntityRecord, AideMemoError> {
    let name = match session.map(str::trim).filter(|s| !s.is_empty()) {
        Some(session) => session.to_string(),
        None => std::env::var("AIDEMEMO_SESSION_ID").map_err(|_| {
            AideMemoError::InvalidInput(
                "pass SESSION or set AIDEMEMO_SESSION_ID before `aidememo session canvas`".into(),
            )
        })?,
    };
    let entity = wiki.entity_get(&name)?;
    if entity.entity_type.to_string() != "session" {
        return Err(AideMemoError::InvalidInput(format!(
            "{name} is a {} entity, not a session",
            entity.entity_type
        )));
    }
    Ok(entity)
}

fn render_session_canvas(
    wiki: &AideMemo,
    session: &EntityRecord,
    facts: &[FactRecord],
) -> Result<String, AideMemoError> {
    let mut entity_cache = HashMap::new();
    let mut out = String::new();
    out.push_str("# AideMemo Session Canvas\n\n");
    out.push_str(&format!("session: `{}`\n", session.name));
    if let Some(topic) = &session.source_page {
        out.push_str(&format!("topic: {}\n", markdown_inline(topic)));
    }
    out.push_str(&format!("facts: {}\n\n", facts.len()));
    out.push_str("This is a derived, read-only artifact. Verify evidence with `aidememo fact get <fact_id>`.\n\n");

    out.push_str("## Mermaid Canvas\n\n```mermaid\nflowchart TD\n");
    out.push_str(&format!(
        "    S[\"session: {}\"]\n",
        mermaid_label(session.source_page.as_deref().unwrap_or(&session.name), 48)
    ));
    for (idx, fact) in facts.iter().enumerate() {
        let node = format!("F{}", idx + 1);
        let label = format!(
            "{} - {}",
            fact.fact_type,
            short_fact_id(&fact.id.to_string())
        );
        out.push_str(&format!("    {node}[\"{}\"]\n", mermaid_label(&label, 48)));
        out.push_str(&format!("    S --> {node}\n"));
        for entity_name in fact_entity_names(wiki, fact, &mut entity_cache)?
            .into_iter()
            .filter(|name| name != &session.name)
            .take(2)
        {
            let entity_node = format!("E{}_{}", idx + 1, stable_node_suffix(&entity_name));
            out.push_str(&format!(
                "    {entity_node}[\"{}\"]\n",
                mermaid_label(&entity_name, 36)
            ));
            out.push_str(&format!("    {node} --> {entity_node}\n"));
        }
    }
    out.push_str("```\n\n");

    out.push_str("## Evidence Thread\n\n");
    for (idx, fact) in facts.iter().enumerate() {
        let entity_names = fact_entity_names(wiki, fact, &mut entity_cache)?;
        out.push_str(&format!(
            "### {}. {} `{}`\n\n",
            idx + 1,
            fact.fact_type,
            fact.id
        ));
        out.push_str(&format!("- time: {}\n", fact_time(fact)));
        if !entity_names.is_empty() {
            out.push_str(&format!("- entities: {}\n", entity_names.join(", ")));
        }
        if !fact.tags.is_empty() {
            out.push_str(&format!("- tags: {}\n", fact.tags.join(", ")));
        }
        if let Some(source) = &fact.source {
            out.push_str(&format!("- source: {}\n", markdown_inline(source)));
        }
        out.push_str(&format!("- verify: `aidememo fact get {}`\n\n", fact.id));
        out.push_str(&format!("{}\n\n", fact.content.trim()));
    }
    Ok(out)
}

fn render_project_profile(
    wiki: &AideMemo,
    limit: usize,
    source_id: Option<String>,
    include_sessions: bool,
) -> Result<ProjectProfileArtifact, AideMemoError> {
    let mut facts = wiki.fact_list(FactListOpts {
        source_id,
        limit: Some(limit),
        current_only: true,
        ..Default::default()
    })?;
    facts.retain(|fact| include_sessions || fact.fact_type != FactType::Question);

    let mut entity_cache = HashMap::new();
    if !include_sessions {
        let mut retained = Vec::new();
        for fact in facts {
            let names = fact_entity_names(wiki, &fact, &mut entity_cache)?;
            if names.iter().any(|name| !is_session_entity_name(name)) {
                retained.push(fact);
            }
        }
        facts = retained;
    }

    let mut top_entities = wiki.entity_list(ListOpts {
        sort_by: EntitySort::FactCount,
        limit: Some(24),
        ..Default::default()
    })?;
    if !include_sessions {
        top_entities.retain(|entity| {
            entity.entity_type.to_string() != "session" && !is_session_entity_name(&entity.name)
        });
    }
    top_entities.truncate(12);
    let entity_count = top_entities.len();

    let mut by_type: BTreeMap<String, Vec<&FactRecord>> = BTreeMap::new();
    for fact in &facts {
        by_type
            .entry(fact.fact_type.to_string())
            .or_default()
            .push(fact);
    }

    let mut out = String::new();
    out.push_str("# AideMemo Project Profile\n\n");
    out.push_str("This is a derived, read-only artifact generated from current typed facts. It is not a replacement for the fact trail.\n\n");
    out.push_str("## Evidence Contract\n\n");
    out.push_str("- Source of truth: typed facts in the AideMemo store.\n");
    out.push_str(
        "- Drill-down: every bullet keeps its fact id for `aidememo fact get <fact_id>`.\n",
    );
    out.push_str("- Regeneration: rerun `aidememo profile export --output project_profile.md` after memory changes.\n\n");

    out.push_str("## Top Entities\n\n");
    for entity in top_entities {
        push_entity_profile_line(wiki, &mut out, &entity)?;
    }
    out.push('\n');

    for section in [
        "decision",
        "convention",
        "pattern",
        "preference",
        "lesson",
        "error",
        "claim",
        "note",
        "unknown",
    ] {
        if let Some(section_facts) = by_type.get(section) {
            if section_facts.is_empty() {
                continue;
            }
            out.push_str(&format!("## {}\n\n", title_case(section)));
            for fact in section_facts.iter().take(20) {
                let mut entity_names = fact_entity_names(wiki, fact, &mut entity_cache)?;
                if !include_sessions {
                    entity_names.retain(|name| !is_session_entity_name(name));
                }
                let suffix = if entity_names.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", entity_names.join(", "))
                };
                out.push_str(&format!(
                    "- `{}`{} {} (verify: `aidememo fact get {}`)\n",
                    fact.id,
                    suffix,
                    one_line(&fact.content, 180),
                    fact.id
                ));
            }
            out.push('\n');
        }
    }

    Ok(ProjectProfileArtifact {
        body: out,
        fact_count: facts.len(),
        entity_count,
    })
}

fn push_entity_profile_line(
    wiki: &AideMemo,
    out: &mut String,
    entity: &EntitySummary,
) -> Result<(), AideMemoError> {
    let record = wiki.entity_get_by_id(entity.id)?;
    let summary = record
        .summary
        .as_deref()
        .map(|s| one_line(s, 160))
        .unwrap_or_else(|| "no compiled summary".to_string());
    out.push_str(&format!(
        "- {} ({}, {} facts): {}\n",
        entity.name, entity.entity_type, entity.fact_count, summary
    ));
    Ok(())
}

fn fact_entity_names(
    wiki: &AideMemo,
    fact: &FactRecord,
    cache: &mut HashMap<aidememo_core::EntityId, String>,
) -> Result<Vec<String>, AideMemoError> {
    let mut names = Vec::new();
    for entity_id in &fact.entity_ids {
        if let Some(name) = cache.get(entity_id) {
            names.push(name.clone());
            continue;
        }
        let entity = wiki.entity_get_by_id(*entity_id)?;
        cache.insert(*entity_id, entity.name.clone());
        names.push(entity.name);
    }
    Ok(names)
}

fn fact_time(fact: &FactRecord) -> String {
    let ms = fact.observed_at.unwrap_or(fact.created_at);
    format_epoch_ms(ms)
}

fn format_epoch_ms(ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms as i64)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| ms.to_string())
}

fn short_fact_id(id: &str) -> String {
    id.chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn stable_node_suffix(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        }
        if out.len() >= 12 {
            break;
        }
    }
    if out.is_empty() {
        "entity".to_string()
    } else {
        out
    }
}

fn is_session_entity_name(name: &str) -> bool {
    name.starts_with("session-")
}

fn mermaid_label(text: &str, max_chars: usize) -> String {
    one_line(text, max_chars)
        .replace('\\', "\\\\")
        .replace('"', "'")
}

fn markdown_inline(text: &str) -> String {
    text.replace('\n', " ").trim().to_string()
}

fn one_line(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
    {
        if out.chars().count() >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn title_case(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}
