//! Node.js/NAPI bindings for WikiGraph.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::Arc;
use wg_core::{Config, WikiGraph};

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
        let wiki = WikiGraph::open(std::path::Path::new(&store_path), config)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(WgStore {
            wiki: Arc::new(wiki),
        })
    }

    #[napi]
    pub fn search(&self, query: String) -> String {
        format!("Search for: {}", query)
    }
}
