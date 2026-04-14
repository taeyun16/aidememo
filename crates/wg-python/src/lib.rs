//! Python bindings for WikiGraph using PyO3.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule};
use std::sync::Arc;
use wg_core::{Config, EntityId, FactInput, FactType, ListOpts, SearchOpts, WikiGraph};

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

fn parse_entity_ids(entity_ids: Vec<String>) -> PyResult<Vec<EntityId>> {
    entity_ids
        .into_iter()
        .map(|id| {
            EntityId::parse(&id).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!("invalid entity id: {id}"))
            })
        })
        .collect()
}

fn entity_to_dict<'py>(py: Python<'py>, entity: &wg_core::EntitySummary) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("id", entity.id.to_string())?;
    dict.set_item("name", &entity.name)?;
    dict.set_item("entity_type", entity.entity_type.to_string())?;
    dict.set_item("fact_count", entity.fact_count)?;
    dict.set_item("tags", &entity.tags)?;
    Ok(dict)
}

fn search_result_to_dict<'py>(py: Python<'py>, result: &wg_core::SearchResult) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("fact_id", result.fact_id.to_string())?;
    dict.set_item("content", &result.content)?;
    dict.set_item("fact_type", result.fact_type.to_string())?;
    dict.set_item("entity_names", &result.entity_names)?;
    dict.set_item("source", &result.source)?;
    dict.set_item("score", result.score)?;
    dict.set_item("rank", result.rank)?;
    Ok(dict)
}

fn lint_issue_to_dict<'py>(py: Python<'py>, issue: &wg_core::LintIssue) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("severity", issue.severity.to_string())?;
    dict.set_item("code", &issue.code)?;
    dict.set_item("message", &issue.message)?;
    dict.set_item("entity_id", issue.entity_id.map(|id| id.to_string()))?;
    dict.set_item("fact_id", issue.fact_id.map(|id| id.to_string()))?;
    Ok(dict)
}

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

    #[pyo3(signature = (query, limit=None))]
    fn search(&self, py: Python<'_>, query: String, limit: Option<usize>) -> PyResult<Py<PyList>> {
        let results = self
            .0
            .search(
                &query,
                SearchOpts {
                    limit,
                    ..Default::default()
                },
            )
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for result in results {
            list.append(search_result_to_dict(py, &result)?)?;
        }
        Ok(list.unbind())
    }

    fn entity_list(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let entities = self
            .0
            .entity_list(ListOpts::default())
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for entity in entities {
            list.append(entity_to_dict(py, &entity)?)?;
        }
        Ok(list.unbind())
    }

    #[pyo3(signature = (entity_ids, content, fact_type, confidence=None))]
    fn fact_add(
        &self,
        entity_ids: Vec<String>,
        content: String,
        fact_type: String,
        confidence: Option<f64>,
    ) -> PyResult<String> {
        let entity_ids = parse_entity_ids(entity_ids)?;
        let id = self
            .0
            .fact_add(FactInput {
                content,
                fact_type: Some(parse_fact_type(&fact_type)),
                entity_ids: Some(entity_ids),
                tags: None,
                source: None,
                source_confidence: confidence.map(|v| v as f32),
            })
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        Ok(id.to_string())
    }

    fn lint(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let issues = self
            .0
            .lint()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for issue in issues {
            list.append(lint_issue_to_dict(py, &issue)?)?;
        }
        Ok(list.unbind())
    }
}

/// Initialize the wg Python module.
#[pymodule]
fn wg_python(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyWikiGraph>()?;
    Ok(())
}
