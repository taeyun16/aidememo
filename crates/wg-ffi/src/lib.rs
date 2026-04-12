//! C-ABI bindings for WikiGraph.

use std::sync::Arc;
use wg_core::{Config, WikiGraph};

/// WikiGraph FFI wrapper.
pub struct WgFfi {
    wiki: Arc<WikiGraph>,
}

impl WgFfi {
    pub fn new(store_path: &str) -> Result<Self, wg_core::WgError> {
        let config = Config::default();
        let wiki = WikiGraph::open(std::path::Path::new(store_path), config)?;
        Ok(WgFfi {
            wiki: Arc::new(wiki),
        })
    }
}
