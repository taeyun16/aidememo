//! Elixir NIF bindings for WikiGraph.

use rustler::{Encoder, Env, NifResult, NifStruct, ResourceArc, Return, Term};
use std::sync::Arc;
use wg_core::{Config, EntityId, FactInput, FactType, ListOpts, SearchOpts, WikiGraph};

#[derive(NifStruct, Debug, Clone)]
#[module = "Elixir.WgNif"]
pub struct SearchResultNif {
    pub fact_id: String,
    pub content: String,
    pub fact_type: String,
    pub entity_names: Vec<String>,
    pub source: Option<String>,
    pub score: f32,
    pub rank: usize,
}

#[derive(NifStruct, Debug, Clone)]
#[module = "Elixir.WgNif"]
pub struct EntitySummaryNif {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub fact_count: u32,
    pub tags: Vec<String>,
}

#[derive(NifStruct, Debug, Clone)]
#[module = "Elixir.WgNif"]
pub struct LintIssueNif {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub entity_id: Option<String>,
    pub fact_id: Option<String>,
}

fn parse_fact_type(value: &str) -> FactType {
    match value.to_lowercase().as_str() {
        "decision" => FactType::Decision,
        "pattern" => FactType::Pattern,
        "convention" => FactType::Convention,
        "claim" => FactType::Claim,
        "note" => FactType::Note,
        "question" => FactType::Question,
        _ => FactType::Unknown,
    }
}

fn parse_entity_ids(entity_ids: Vec<String>) -> NifResult<Vec<EntityId>> {
    entity_ids
        .into_iter()
        .map(|id| EntityId::parse(&id).ok_or(rustler::Error::BadArg))
        .collect()
}

/// WikiGraph NIF wrapper.
#[derive(Clone)]
pub struct WgNif {
    wiki: Arc<WikiGraph>,
}

#[rustler::resource_impl]
impl rustler::Resource for WgNif {}

impl WgNif {
    pub fn new(store_path: &str) -> Result<Self, wg_core::WgError> {
        let config = Config::default();
        let wiki = WikiGraph::open(std::path::Path::new(store_path), config)?;
        Ok(WgNif {
            wiki: Arc::new(wiki),
        })
    }
}

fn load(_env: Env, _term: Term) -> bool {
    true
}

fn to_search_result(result: wg_core::SearchResult) -> SearchResultNif {
    SearchResultNif {
        fact_id: result.fact_id.to_string(),
        content: result.content,
        fact_type: result.fact_type.to_string(),
        entity_names: result.entity_names,
        source: result.source,
        score: result.score,
        rank: result.rank,
    }
}

fn to_entity_summary(summary: wg_core::EntitySummary) -> EntitySummaryNif {
    EntitySummaryNif {
        id: summary.id.to_string(),
        name: summary.name,
        entity_type: summary.entity_type.to_string(),
        fact_count: summary.fact_count,
        tags: summary.tags,
    }
}

fn to_lint_issue(issue: wg_core::LintIssue) -> LintIssueNif {
    LintIssueNif {
        severity: issue.severity.to_string(),
        code: issue.code,
        message: issue.message,
        entity_id: issue.entity_id.map(|id| id.to_string()),
        fact_id: issue.fact_id.map(|id| id.to_string()),
    }
}

#[rustler::nif]
fn new(env: Env, store_path: String) -> Return {
    match WgNif::new(&store_path) {
        Ok(wrapper) => Return::Term(ResourceArc::new(wrapper).encode(env)),
        Err(_) => Return::Error(rustler::Error::BadArg),
    }
}

#[rustler::nif]
fn search(
    wrapper: ResourceArc<WgNif>,
    query: String,
    limit: u32,
) -> NifResult<Vec<SearchResultNif>> {
    let results = wrapper
        .wiki
        .search(
            &query,
            SearchOpts {
                limit: Some(limit as usize),
                ..Default::default()
            },
        )
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(results.into_iter().map(to_search_result).collect())
}

#[rustler::nif]
fn entity_list(wrapper: ResourceArc<WgNif>) -> NifResult<Vec<EntitySummaryNif>> {
    let entities = wrapper
        .wiki
        .entity_list(ListOpts::default())
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(entities.into_iter().map(to_entity_summary).collect())
}

#[rustler::nif]
fn fact_add(
    wrapper: ResourceArc<WgNif>,
    entity_ids: Vec<String>,
    content: String,
    fact_type: String,
    confidence: f32,
) -> NifResult<String> {
    let entity_ids = parse_entity_ids(entity_ids)?;
    let id = wrapper
        .wiki
        .fact_add(FactInput {
            content,
            fact_type: Some(parse_fact_type(&fact_type)),
            entity_ids: Some(entity_ids),
            tags: None,
            source: None,
            source_confidence: Some(confidence),
            observed_at: None,
        })
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(id.to_string())
}

#[rustler::nif]
fn lint(wrapper: ResourceArc<WgNif>) -> NifResult<Vec<LintIssueNif>> {
    let issues = wrapper.wiki.lint().map_err(|_| rustler::Error::BadArg)?;
    Ok(issues.into_iter().map(to_lint_issue).collect())
}

rustler::init!("Elixir.WgNif", load = load);
