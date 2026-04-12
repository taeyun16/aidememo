//! Python bindings for WikiGraph using PyO3.

use pyo3::prelude::*;
use std::sync::Arc;
use wg_core::{Config, Result, WikiGraph};

/// WikiGraph Python wrapper.
#[pyclass]
pub struct PyWikiGraph(pub Arc<WikiGraph>);

#[pymethods]
impl PyWikiGraph {
    #[new]
    fn new(store_path: String) -> PyResult<Self> {
        let config = Config::default();
        let wiki = WikiGraph::open(std::path::Path::new(&store_path), config)
            .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
        Ok(PyWikiGraph(Arc::new(wiki)))
    }

    fn search(&self, query: String) -> PyResult<String> {
        // Simplified search - full implementation would mirror CLI
        Ok(format!("Search for: {}", query))
    }
}

/// Initialize the wg Python module.
#[pymodule]
fn wg_python(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyWikiGraph>()?;
    Ok(())
}
