//! Node.js/NAPI bindings for WikiGraph.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::path::Path;
use std::sync::Arc;
use wg_core::{
    Config, EntityId, FactInput, FactType, ListOpts, SearchOpts, WikiGraph,
};

fn to_json<T: serde::Serialize>(value: &T) -> napi::Result<String> {
    serde_json::to_string(value).map_err(|e| Error::from_reason(e.to_string()))
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

fn parse_entity_ids(entity_ids: Vec<String>) -> napi::Result<Vec<EntityId>> {
    entity_ids
        .into_iter()
        .map(|id| {
            EntityId::parse(&id)
                .ok_or_else(|| Error::from_reason(format!("invalid entity id: {id}")))
        })
        .collect()
}

/// WikiGraph NAPI wrapper.
#[napi]
pub struct WgStore {
    wiki: Arc<WikiGraph>,
}

#[napi]
impl WgStore {
    #[napi(constructor)]
    pub fn new(store_path: String) -> napi::Result<Self> {
        let config = Config::default();
        let wiki = WikiGraph::open(Path::new(&store_path), config)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(WgStore {
            wiki: Arc::new(wiki),
        })
    }

    #[napi]
    pub fn search(
        &self,
        query: String,
        limit: Option<u32>,
        bm25_weight: Option<f64>,
        semantic_weight: Option<f64>,
    ) -> napi::Result<String> {
        let opts = SearchOpts {
            limit: limit.map(|v| v as usize),
            bm25_weight: bm25_weight.unwrap_or(0.0) as f32,
            semantic_weight: semantic_weight.unwrap_or(0.0) as f32,
            ..Default::default()
        };
        let results = self
            .wiki
            .search(&query, opts)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        to_json(&results)
    }

    #[napi]
    pub fn entity_list(&self) -> napi::Result<String> {
        let entities = self
            .wiki
            .entity_list(ListOpts::default())
            .map_err(|e| Error::from_reason(e.to_string()))?;
        to_json(&entities)
    }

    #[napi]
    pub fn fact_add(
        &self,
        entity_ids: Vec<String>,
        content: String,
        fact_type: String,
        confidence: Option<f64>,
    ) -> napi::Result<String> {
        let entity_ids = parse_entity_ids(entity_ids)?;
        let input = FactInput {
            content,
            fact_type: Some(parse_fact_type(&fact_type)),
            entity_ids: Some(entity_ids),
            tags: None,
            source: None,
            source_confidence: confidence.map(|v| v as f32),
        };
        let id = self
            .wiki
            .fact_add(input)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(id.to_string())
    }

    #[napi]
    pub fn lint(&self) -> napi::Result<String> {
        let report = self
            .wiki
            .lint()
            .map_err(|e| Error::from_reason(e.to_string()))?;
        to_json(&report)
    }
}
