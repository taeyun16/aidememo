//! Python bindings for WikiGraph (PyO3).
//!
//! All read methods return native Python objects (dict / list / scalar) via
//! `pythonize`. Write methods accept primitive args and return ULID strings
//! or `None`.
//!
//! Example:
//!
//! ```python
//! import wg_python as wg
//! g = wg.WikiGraph("./_meta/wiki.redb")
//! results = g.search("rust", limit=5)
//! ctx = g.query("Redis")
//! ```

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};
use pythonize::pythonize;
use std::path::Path;
use std::sync::Arc;
use wg_core::{
    Config, EntityId, EntityInput, EntityType, FactId, FactInput, FactListOpts, FactType, ListOpts,
    QueryOpts, RelationInput, RelationType, SearchOpts, TraverseDirection, TraverseOpts, WgError,
    WikiGraph,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn err<E: std::fmt::Display>(e: E) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
}

fn map_err(e: WgError) -> PyErr {
    err(e)
}

fn parse_entity_type(value: &str) -> Option<EntityType> {
    match value.to_lowercase().as_str() {
        "technology" | "tech" => Some(EntityType::Technology),
        "concept" => Some(EntityType::Concept),
        "comparison" | "compare" => Some(EntityType::Comparison),
        "query" | "question" => Some(EntityType::Query),
        "person" => Some(EntityType::Person),
        "team" => Some(EntityType::Team),
        "" | "unknown" => Some(EntityType::Unknown),
        _ => None,
    }
}

fn parse_fact_type(value: &str) -> Option<FactType> {
    match value.to_lowercase().as_str() {
        "decision" | "decide" => Some(FactType::Decision),
        "pattern" => Some(FactType::Pattern),
        "convention" => Some(FactType::Convention),
        "claim" | "assertion" => Some(FactType::Claim),
        "note" | "notes" => Some(FactType::Note),
        "question" | "query" => Some(FactType::Question),
        "" | "unknown" => Some(FactType::Unknown),
        _ => None,
    }
}

fn parse_direction(value: Option<&str>) -> TraverseDirection {
    match value.map(|s| s.to_lowercase()).as_deref() {
        Some("forward") => TraverseDirection::Forward,
        Some("reverse") => TraverseDirection::Reverse,
        _ => TraverseDirection::Both,
    }
}

fn parse_entity_id(s: &str) -> PyResult<EntityId> {
    EntityId::parse(s).ok_or_else(|| err(format!("invalid entity id: {s}")))
}

fn parse_fact_id(s: &str) -> PyResult<FactId> {
    FactId::parse(s).ok_or_else(|| err(format!("invalid fact id: {s}")))
}

fn to_py<T: serde::Serialize>(py: Python<'_>, value: &T) -> PyResult<PyObject> {
    pythonize(py, value)
        .map(|b| b.into())
        .map_err(|e| err(e.to_string()))
}

/// Pull an optional value out of a dict. `None` (Python) and a missing
/// key both collapse to `Ok(None)` so callers can write a single
/// match arm. Returns `Err` on extraction failure (wrong type).
fn dict_opt<'py, T>(item: &Bound<'py, PyDict>, key: &str) -> PyResult<Option<T>>
where
    T: pyo3::FromPyObject<'py>,
{
    match item.get_item(key)? {
        Some(v) if !v.is_none() => Ok(Some(v.extract::<T>()?)),
        _ => Ok(None),
    }
}

/// Build a `FactInput` from a Python dict. Shared between
/// `fact_add_many` (and any future caller that takes per-fact dicts).
/// `content` is required; everything else collapses Python `None` and
/// missing keys to `None`. The `entity_ids` field is normalized from
/// ULID strings to `EntityId`.
fn fact_input_from_dict(item: &Bound<'_, PyDict>) -> PyResult<FactInput> {
    let content: String = item
        .get_item("content")?
        .ok_or_else(|| err("each item needs a 'content' field"))?
        .extract()?;

    let entity_ids = match dict_opt::<Vec<String>>(item, "entity_ids")? {
        Some(names) => Some(
            names
                .iter()
                .map(|s| parse_entity_id(s))
                .collect::<PyResult<Vec<_>>>()?,
        ),
        None => None,
    };

    let fact_type = dict_opt::<String>(item, "fact_type")?
        .as_deref()
        .and_then(parse_fact_type);

    Ok(FactInput {
        content,
        fact_type,
        entity_ids,
        tags: dict_opt::<Vec<String>>(item, "tags")?,
        source: dict_opt::<String>(item, "source")?,
        source_confidence: dict_opt::<f32>(item, "confidence")?,
        observed_at: None,
    })
}

// ---------------------------------------------------------------------------
// PyWikiGraph
// ---------------------------------------------------------------------------

/// Local knowledge-graph wiki backed by redb.
///
/// Construct with a path to the store file (`.redb`). Optional keyword
/// arguments override the corresponding `Config` fields *before* the
/// store is opened — useful for selecting a different embedding model,
/// flipping HNSW vs BM25, or relaxing durability for high-frequency
/// write workloads. Methods are thread-safe (the underlying graph
/// uses an `RwLock` internally) so a single instance can be shared
/// across threads.
///
/// ```python
/// import wg_python as wg
/// g = wg.WikiGraph(
///     "./_meta/wiki.redb",
///     model="minishlab/potion-base-8M",
///     semantic_index="hnsw",
///     durability="eventual",
/// )
/// ```
#[pyclass(name = "WikiGraph")]
pub struct PyWikiGraph(pub Arc<WikiGraph>);

#[pymethods]
impl PyWikiGraph {
    #[new]
    #[pyo3(signature = (
        store_path,
        *,
        model = None,
        semantic_index = None,
        durability = None,
    ))]
    fn new(
        store_path: String,
        model: Option<String>,
        semantic_index: Option<String>,
        durability: Option<String>,
    ) -> PyResult<Self> {
        let mut config = Config::default();
        if let Some(model) = model {
            config.set("model.name", &model).map_err(map_err)?;
        }
        if let Some(idx) = semantic_index {
            config.set("search.semantic_index", &idx).map_err(map_err)?;
        }
        if let Some(dur) = durability {
            config.set("store.durability", &dur).map_err(map_err)?;
        }
        let wiki = WikiGraph::open(Path::new(&store_path), config).map_err(map_err)?;
        Ok(Self(Arc::new(wiki)))
    }

    // === Search ===

    /// Hybrid search (BM25 + semantic). Returns a list of result dicts.
    /// Set `current_only=True` to exclude superseded facts.
    #[pyo3(signature = (query, limit=None, min_confidence=None, current_only=false))]
    fn search(
        &self,
        py: Python<'_>,
        query: String,
        limit: Option<usize>,
        min_confidence: Option<f32>,
        current_only: bool,
    ) -> PyResult<PyObject> {
        let opts = SearchOpts {
            limit,
            min_confidence,
            current_only,
            ..Default::default()
        };
        let results = self.0.hybrid_search(&query, opts).map_err(map_err)?;
        to_py(py, &results)
    }

    /// Unified context fetch: search + entity resolve + traverse + recent facts.
    /// Returns one dict with keys: topic, entity, search, related, recent_facts.
    /// `mode`: "naive" | "local" | "hybrid" (default) | "global".
    #[pyo3(signature = (topic, limit=10, depth=2, recent_limit=10, current_only=false, mode=None))]
    #[allow(clippy::too_many_arguments)]
    fn query(
        &self,
        py: Python<'_>,
        topic: String,
        limit: usize,
        depth: u32,
        recent_limit: usize,
        current_only: bool,
        mode: Option<String>,
    ) -> PyResult<PyObject> {
        let opts = QueryOpts {
            search_limit: limit,
            depth,
            recent_limit,
            since: None,
            current_only,
            mode: mode
                .as_deref()
                .map(wg_core::QueryMode::parse)
                .unwrap_or_default(),
            bm25_only: false,
        };
        let result = self.0.query(&topic, opts).map_err(map_err)?;
        to_py(py, &result)
    }

    // === Graph ===

    /// Traverse the entity graph from `entity` up to `depth` hops.
    #[pyo3(signature = (entity, depth=2, direction=None))]
    fn traverse(
        &self,
        py: Python<'_>,
        entity: String,
        depth: u32,
        direction: Option<String>,
    ) -> PyResult<PyObject> {
        let opts = TraverseOpts {
            depth,
            relation_types: None,
            direction: parse_direction(direction.as_deref()),
        };
        let result = self.0.traverse(&entity, opts).map_err(map_err)?;
        to_py(py, &result)
    }

    /// Find a path between two entities. Returns a list of steps or `None`.
    fn path_find(&self, py: Python<'_>, from: String, to: String) -> PyResult<PyObject> {
        let result = self.0.path_find(&from, &to).map_err(map_err)?;
        to_py(py, &result)
    }

    // === Entity CRUD ===

    /// Add an entity. Returns the new ULID as a string.
    #[pyo3(signature = (name, entity_type=None, tags=None, aliases=None, source_page=None))]
    fn entity_add(
        &self,
        name: String,
        entity_type: Option<String>,
        tags: Option<Vec<String>>,
        aliases: Option<Vec<String>>,
        source_page: Option<String>,
    ) -> PyResult<String> {
        let input = EntityInput {
            name,
            entity_type: entity_type.as_deref().and_then(parse_entity_type),
            tags,
            aliases,
            source_page,
        };
        let id = self.0.entity_add(input).map_err(map_err)?;
        Ok(id.to_string())
    }

    /// Get a single entity by name (or alias).
    fn entity_get(&self, py: Python<'_>, name: String) -> PyResult<PyObject> {
        let entity = self.0.entity_get(&name).map_err(map_err)?;
        to_py(py, &entity)
    }

    /// List entities. Filters: entity_type, min_facts, limit.
    #[pyo3(signature = (limit=None, entity_type=None, min_facts=None))]
    fn entity_list(
        &self,
        py: Python<'_>,
        limit: Option<usize>,
        entity_type: Option<String>,
        min_facts: Option<u32>,
    ) -> PyResult<PyObject> {
        let opts = ListOpts {
            entity_type: entity_type.as_deref().and_then(parse_entity_type),
            min_facts,
            limit,
            sort_by: Default::default(),
            offset: 0,
        };
        let entities = self.0.entity_list(opts).map_err(map_err)?;
        to_py(py, &entities)
    }

    /// Delete an entity (and unlink it from facts/relations).
    fn entity_delete(&self, name: String) -> PyResult<()> {
        self.0.entity_delete(&name).map_err(map_err)
    }

    /// Set (or clear, with `""`) the entity's compiled-truth summary.
    fn entity_describe(&self, name: String, summary: String) -> PyResult<()> {
        self.0.entity_describe(&name, &summary).map_err(map_err)
    }

    /// Resolve a name or alias to a canonical entity ID.
    fn resolve_entity(&self, name: String) -> PyResult<String> {
        let id = self.0.resolve_entity(&name).map_err(map_err)?;
        Ok(id.to_string())
    }

    // === Fact CRUD ===

    /// Add a fact. `entity_ids` are ULIDs (use `resolve_entity` to convert names).
    #[pyo3(signature = (content, entity_ids=None, fact_type=None, tags=None, source=None, confidence=None))]
    fn fact_add(
        &self,
        content: String,
        entity_ids: Option<Vec<String>>,
        fact_type: Option<String>,
        tags: Option<Vec<String>>,
        source: Option<String>,
        confidence: Option<f32>,
    ) -> PyResult<String> {
        let entity_ids = match entity_ids {
            Some(ids) => Some(
                ids.iter()
                    .map(|s| parse_entity_id(s))
                    .collect::<PyResult<Vec<_>>>()?,
            ),
            None => None,
        };
        let input = FactInput {
            content,
            fact_type: fact_type.as_deref().and_then(parse_fact_type),
            entity_ids,
            tags,
            source,
            source_confidence: confidence,
            observed_at: None,
        };
        let id = self.0.fact_add(input).map_err(map_err)?;
        Ok(id.to_string())
    }

    /// Insert many facts in one redb write transaction.
    ///
    /// Each item is a dict with the same keys `fact_add` accepts:
    /// `content` (required), `entity_ids`, `fact_type`, `tags`,
    /// `source`, `confidence`. Returns the new fact ULIDs in input
    /// order. All-or-nothing — if one item fails to validate, no
    /// facts land.
    fn fact_add_many<'py>(&self, items: Vec<Bound<'py, PyDict>>) -> PyResult<Vec<String>> {
        let inputs: Vec<FactInput> = items
            .iter()
            .map(fact_input_from_dict)
            .collect::<PyResult<_>>()?;
        let ids = self.0.fact_add_many(inputs).map_err(map_err)?;
        Ok(ids.iter().map(|id| id.to_string()).collect())
    }

    /// Get a fact by ID.
    fn fact_get(&self, py: Python<'_>, fact_id: String) -> PyResult<PyObject> {
        let id = parse_fact_id(&fact_id)?;
        let fact = self.0.fact_get(&id).map_err(map_err)?;
        to_py(py, &fact)
    }

    /// List facts. Filters: entity (name), fact_type, min_confidence, limit,
    /// current_only (exclude superseded), since_epoch_ms / until_epoch_ms
    /// (validity-window bounds; pass `None` for either side to leave it open).
    #[pyo3(signature = (
        entity=None,
        fact_type=None,
        min_confidence=None,
        limit=None,
        current_only=false,
        since_epoch_ms=None,
        until_epoch_ms=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn fact_list(
        &self,
        py: Python<'_>,
        entity: Option<String>,
        fact_type: Option<String>,
        min_confidence: Option<f32>,
        limit: Option<usize>,
        current_only: bool,
        since_epoch_ms: Option<u64>,
        until_epoch_ms: Option<u64>,
    ) -> PyResult<PyObject> {
        let entity_id = match entity {
            Some(name) => Some(self.0.resolve_entity(&name).map_err(map_err)?),
            None => None,
        };
        let opts = FactListOpts {
            fact_type: fact_type.as_deref().and_then(parse_fact_type),
            entity_id,
            min_confidence,
            limit,
            offset: 0,
            since: since_epoch_ms,
            until: until_epoch_ms,
            current_only,
            as_of: None,
        };
        let facts = self.0.fact_list(opts).map_err(map_err)?;
        to_py(py, &facts)
    }

    /// Delete a fact by ID.
    fn fact_delete(&self, fact_id: String) -> PyResult<()> {
        let id = parse_fact_id(&fact_id)?;
        self.0.fact_delete(&id).map_err(map_err)
    }

    /// Mark `old_id` as superseded by `new_id` (validity windows).
    fn fact_supersede(&self, old_id: String, new_id: String) -> PyResult<()> {
        let old = parse_fact_id(&old_id)?;
        let new = parse_fact_id(&new_id)?;
        self.0.fact_supersede(&old, &new).map_err(map_err)
    }

    // === Relations ===

    /// Add a relation between two entities (referenced by name or alias).
    fn relation_add(&self, source: String, target: String, rel_type: String) -> PyResult<()> {
        let input = RelationInput {
            source,
            target,
            relation_type: RelationType::new(rel_type),
            weight: None,
            evidence: None,
        };
        self.0.relation_add(input).map_err(map_err)
    }

    /// Remove a relation.
    fn relation_remove(&self, source: String, target: String, rel_type: String) -> PyResult<()> {
        self.0
            .relation_remove(&source, &target, &rel_type)
            .map_err(map_err)
    }

    /// List relations attached to an entity. `direction`: forward / reverse / both.
    #[pyo3(signature = (entity, direction=None))]
    fn relations_get(
        &self,
        py: Python<'_>,
        entity: String,
        direction: Option<String>,
    ) -> PyResult<PyObject> {
        let dir = parse_direction(direction.as_deref());
        let relations = self.0.relations_get(&entity, dir).map_err(map_err)?;
        to_py(py, &relations)
    }

    // === Ingest / Lint / Stats ===

    /// Ingest a markdown wiki into the graph.
    #[pyo3(signature = (wiki_root, incremental=false))]
    fn ingest(&self, py: Python<'_>, wiki_root: String, incremental: bool) -> PyResult<PyObject> {
        let stats = self
            .0
            .ingest(Path::new(&wiki_root), incremental)
            .map_err(map_err)?;
        to_py(py, &stats)
    }

    /// Run lint checks; returns a list of issues.
    fn lint(&self, py: Python<'_>) -> PyResult<PyObject> {
        let issues = self.0.lint().map_err(map_err)?;
        to_py(py, &issues)
    }

    /// Store statistics (entity/fact/relation count, size, last ingest time).
    fn stats(&self, py: Python<'_>) -> PyResult<PyObject> {
        let stats = self.0.stats().map_err(map_err)?;
        to_py(py, &stats)
    }
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

#[pymodule]
fn wg_python(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyWikiGraph>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
