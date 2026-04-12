//! Elixir NIF bindings for WikiGraph.

use rustler::{Encoder, Env, Term};
use std::sync::Arc;
use wg_core::{Config, WikiGraph};

/// WikiGraph NIF wrapper.
pub struct WgNif {
    wiki: Arc<WikiGraph>,
}

impl WgNif {
    pub fn new(store_path: &str) -> Result<Self, wg_core::WgError> {
        let config = Config::default();
        let wiki = WikiGraph::open(std::path::Path::new(store_path), config)?;
        Ok(WgNif {
            wiki: Arc::new(wiki),
        })
    }
}
