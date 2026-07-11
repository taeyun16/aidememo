//! Python bindings for AideMemo (PyO3).
//!
//! All read methods return native Python objects (dict / list / scalar) via
//! `pythonize`. Write methods accept primitive args and return ULID strings
//! or `None`.
//!
//! Example:
//!
//! ```python
//! import aidememo_python as aidememo
//! g = aidememo.AideMemo("./_meta/wiki.sqlite")
//! results = g.search("rust", limit=5)
//! ctx = g.query("Redis")
//! ```

use aidememo_core::{
    AideMemo, AideMemoError as CoreAideMemoError, Config, EntityId, EntityInput, EntityType,
    FactId, FactInput, FactListOpts, FactType, ListOpts, QueryOpts, RelationInput, RelationType,
    SearchOpts, TraverseDirection, TraverseOpts, WorkflowStartOpts,
};
use pyo3::create_exception;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyModule};
use pythonize::pythonize;
use std::path::Path;
use std::sync::Arc;

create_exception!(
    aidememo_python,
    AideMemoError,
    PyRuntimeError,
    "Base aidememo-python error."
);
create_exception!(
    aidememo_python,
    AideMemoNotFoundError,
    AideMemoError,
    "A requested aidememo entity, fact, relation, or path was not found."
);
create_exception!(
    aidememo_python,
    AideMemoInvalidInputError,
    AideMemoError,
    "The caller passed invalid input to aidememo."
);
create_exception!(
    aidememo_python,
    AideMemoStoreError,
    AideMemoError,
    "The aidememo store could not be opened, read, or written."
);
create_exception!(
    aidememo_python,
    AideMemoSearchError,
    AideMemoError,
    "Search, index, or embedding-model operation failed."
);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn err<E: std::fmt::Display>(e: E) -> PyErr {
    PyErr::new::<AideMemoError, _>(e.to_string())
}

fn map_err(e: CoreAideMemoError) -> PyErr {
    let message = format!("[{}] {e}", e.code());
    match e {
        CoreAideMemoError::EntityNotFound { .. }
        | CoreAideMemoError::EntityIdNotFound(_)
        | CoreAideMemoError::FactNotFound(_)
        | CoreAideMemoError::RelationNotFound { .. }
        | CoreAideMemoError::PathNotFound { .. } => PyErr::new::<AideMemoNotFoundError, _>(message),
        CoreAideMemoError::InvalidInput(_)
        | CoreAideMemoError::EntityAlreadyExists { .. }
        | CoreAideMemoError::ConfigKeyNotFound(_)
        | CoreAideMemoError::FrontmatterParse { .. }
        | CoreAideMemoError::WikilinkParse { .. }
        | CoreAideMemoError::CycleDetected { .. }
        | CoreAideMemoError::SchemaVersionMismatch { .. }
        | CoreAideMemoError::UnsupportedSchemaVersion(_) => {
            PyErr::new::<AideMemoInvalidInputError, _>(message)
        }
        CoreAideMemoError::StoreOpen { .. }
        | CoreAideMemoError::StoreRead { .. }
        | CoreAideMemoError::StoreWrite { .. }
        | CoreAideMemoError::TransactionBegin { .. }
        | CoreAideMemoError::TransactionConflict
        | CoreAideMemoError::FileRead(_, _)
        | CoreAideMemoError::ConfigRead { .. }
        | CoreAideMemoError::ConfigParse { .. } => PyErr::new::<AideMemoStoreError, _>(message),
        CoreAideMemoError::ModelNotFound { .. }
        | CoreAideMemoError::ModelDownloadFailed { .. }
        | CoreAideMemoError::ModelLoadFailed { .. }
        | CoreAideMemoError::ModelInferenceFailed(_)
        | CoreAideMemoError::SearchFailed(_)
        | CoreAideMemoError::IndexCorrupted(_) => PyErr::new::<AideMemoSearchError, _>(message),
        _ => PyErr::new::<AideMemoError, _>(message),
    }
}

fn parse_entity_type(value: &str) -> Option<EntityType> {
    Some(EntityType::parse(value))
}

fn parse_fact_type(value: &str) -> Option<FactType> {
    Some(FactType::parse(value))
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

fn attach_session_entity(
    wiki: &AideMemo,
    entity_ids: &mut Vec<EntityId>,
    session_id: Option<&str>,
) -> PyResult<()> {
    let Some(session_id) = session_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let session_entity_id = wiki.resolve_entity(session_id).map_err(map_err)?;
    if !entity_ids.contains(&session_entity_id) {
        entity_ids.push(session_entity_id);
    }
    Ok(())
}

fn to_py<T: serde::Serialize>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>> {
    pythonize(py, value)
        .map(|b| b.into())
        .map_err(|e| err(e.to_string()))
}

/// Pull an optional value out of a dict. `None` (Python) and a missing
/// key both collapse to `Ok(None)` so callers can write a single
/// match arm. Returns `Err` on extraction failure (wrong type).
fn dict_opt<'py, T>(item: &Bound<'py, PyDict>, key: &str) -> PyResult<Option<T>>
where
    T: pyo3::conversion::FromPyObjectOwned<'py>,
    for<'a> <T as pyo3::FromPyObject<'a, 'py>>::Error: std::fmt::Display,
{
    match item.get_item(key)? {
        Some(v) if !v.is_none() => Ok(Some(v.extract::<T>().map_err(|e| err(e.to_string()))?)),
        _ => Ok(None),
    }
}

/// Build a `FactInput` from a Python dict. Shared between
/// `fact_add_many` (and any future caller that takes per-fact dicts).
/// `content` is required; everything else collapses Python `None` and
/// missing keys to `None`. The `entity_ids` field is normalized from
/// ULID strings to `EntityId`. `session_id`, when present, is resolved
/// as a session entity and attached to the fact's entity list.
fn fact_input_from_dict(
    wiki: &AideMemo,
    item: &Bound<'_, PyDict>,
    default_session_id: Option<&str>,
) -> PyResult<FactInput> {
    let content: String = item
        .get_item("content")?
        .ok_or_else(|| err("each item needs a 'content' field"))?
        .extract()?;

    let mut entity_ids = match dict_opt::<Vec<String>>(item, "entity_ids")? {
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
    let item_session_id = dict_opt::<String>(item, "session_id")?;
    let session_id = item_session_id.as_deref().or(default_session_id);
    let mut ids = entity_ids.take().unwrap_or_default();
    attach_session_entity(wiki, &mut ids, session_id)?;
    let entity_ids = if ids.is_empty() { None } else { Some(ids) };

    Ok(FactInput {
        content,
        fact_type,
        entity_ids,
        tags: dict_opt::<Vec<String>>(item, "tags")?,
        source: dict_opt::<String>(item, "source")?,
        source_id: dict_opt::<String>(item, "source_id")?,
        actor_id: dict_opt::<String>(item, "actor_id")?,
        source_confidence: dict_opt::<f32>(item, "confidence")?,
        observed_at: None,
    })
}

// ---------------------------------------------------------------------------
// PyAideMemo
// ---------------------------------------------------------------------------

/// Local knowledge-graph wiki backed by SQLite by default.
///
/// Construct with a path to the store file. Optional keyword
/// arguments override the corresponding `Config` fields *before* the
/// store is opened — useful for selecting a different embedding model,
/// flipping HNSW vs BM25, or relaxing durability for high-frequency
/// write workloads. Methods are thread-safe (the underlying graph
/// uses an `RwLock` internally) so a single instance can be shared
/// across threads.
///
/// ```python
/// import aidememo_python as aidememo
/// g = aidememo.AideMemo(
///     "./_meta/wiki.sqlite",
///     model="minishlab/potion-base-8M",
///     semantic_index="hnsw",
///     durability="eventual",
/// )
/// ```
#[pyclass(name = "AideMemo")]
pub struct PyAideMemo(pub Arc<AideMemo>);

#[pymethods]
impl PyAideMemo {
    #[new]
    #[pyo3(signature = (
        store_path,
        *,
        model = None,
        semantic_index = None,
        durability = None,
        backend = None,
    ))]
    fn new(
        store_path: String,
        model: Option<String>,
        semantic_index: Option<String>,
        durability: Option<String>,
        backend: Option<String>,
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
        if let Some(backend) = backend.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            config.set("store.backend", backend).map_err(map_err)?;
        }
        let wiki = AideMemo::open(Path::new(&store_path), config).map_err(map_err)?;
        Ok(Self(Arc::new(wiki)))
    }

    // === Search ===

    /// Hybrid search (BM25 + semantic). Returns a list of result dicts.
    /// Set `current_only=True` to exclude superseded facts.
    #[pyo3(signature = (query, limit=None, min_confidence=None, current_only=false, source_id=None))]
    fn search(
        &self,
        py: Python<'_>,
        query: String,
        limit: Option<usize>,
        min_confidence: Option<f32>,
        current_only: bool,
        source_id: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let opts = SearchOpts {
            limit,
            min_confidence,
            current_only,
            source_id,
            ..Default::default()
        };
        let results = self.0.hybrid_search(&query, opts).map_err(map_err)?;
        to_py(py, &results)
    }

    /// Unified context fetch: search + entity resolve + traverse + recent facts.
    /// Returns one dict with keys: topic, entity, search, related, recent_facts.
    /// `mode`: "naive" | "local" | "hybrid" (default) | "global".
    #[pyo3(signature = (topic, limit=10, depth=2, recent_limit=10, current_only=false, mode=None, source_id=None))]
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
        source_id: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let opts = QueryOpts {
            search_limit: limit,
            depth,
            recent_limit,
            since: None,
            current_only,
            mode: mode
                .as_deref()
                .map(aidememo_core::QueryMode::parse)
                .unwrap_or_default(),
            bm25_only: false,
            source_id,
        };
        let result = self.0.query(&topic, opts).map_err(map_err)?;
        to_py(py, &result)
    }

    /// Start a workflow from a sparse issue/ticket and return the context pack.
    #[pyo3(signature = (title, body=None, source=None, source_id=None, actor_id=None, parent_session_id=None, limit=8, depth=2, recent_limit=5, bm25_only=false))]
    #[allow(clippy::too_many_arguments)]
    fn workflow_start(
        &self,
        py: Python<'_>,
        title: String,
        body: Option<String>,
        source: Option<String>,
        source_id: Option<String>,
        actor_id: Option<String>,
        parent_session_id: Option<String>,
        limit: usize,
        depth: u32,
        recent_limit: usize,
        bm25_only: bool,
    ) -> PyResult<Py<PyAny>> {
        let result = self
            .0
            .workflow_start(
                &title,
                WorkflowStartOpts {
                    body,
                    source,
                    source_id,
                    actor_id,
                    parent_session_id,
                    limit,
                    depth,
                    recent_limit,
                    bm25_only,
                },
            )
            .map_err(map_err)?;
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
    ) -> PyResult<Py<PyAny>> {
        let opts = TraverseOpts {
            depth,
            relation_types: None,
            direction: parse_direction(direction.as_deref()),
        };
        let result = self.0.traverse(&entity, opts).map_err(map_err)?;
        to_py(py, &result)
    }

    /// Find a path between two entities. Returns a list of steps or `None`.
    fn path_find(&self, py: Python<'_>, from: String, to: String) -> PyResult<Py<PyAny>> {
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
    fn entity_get(&self, py: Python<'_>, name: String) -> PyResult<Py<PyAny>> {
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
    ) -> PyResult<Py<PyAny>> {
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
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (content, entity_ids=None, fact_type=None, tags=None, source=None, confidence=None, source_id=None, actor_id=None, session_id=None))]
    fn fact_add(
        &self,
        content: String,
        entity_ids: Option<Vec<String>>,
        fact_type: Option<String>,
        tags: Option<Vec<String>>,
        source: Option<String>,
        confidence: Option<f32>,
        source_id: Option<String>,
        actor_id: Option<String>,
        session_id: Option<String>,
    ) -> PyResult<String> {
        let mut ids = match entity_ids {
            Some(ids) => ids
                .iter()
                .map(|s| parse_entity_id(s))
                .collect::<PyResult<Vec<_>>>()?,
            None => Vec::new(),
        };
        attach_session_entity(&self.0, &mut ids, session_id.as_deref())?;
        let entity_ids = if ids.is_empty() { None } else { Some(ids) };
        let input = FactInput {
            content,
            fact_type: fact_type.as_deref().and_then(parse_fact_type),
            entity_ids,
            tags,
            source,
            source_id,
            actor_id,
            source_confidence: confidence,
            observed_at: None,
        };
        let id = self.0.fact_add(input).map_err(map_err)?;
        Ok(id.to_string())
    }

    /// Insert many facts in one backend transaction when supported.
    ///
    /// Each item is a dict with the same keys `fact_add` accepts:
    /// `content` (required), `entity_ids`, `fact_type`, `tags`,
    /// `source`, `source_id`, `confidence`, `session_id`. Returns the new fact
    /// ULIDs in input order. All-or-nothing — if one item fails to validate,
    /// no facts land.
    #[pyo3(signature = (items, session_id=None))]
    fn fact_add_many<'py>(
        &self,
        items: Vec<Bound<'py, PyDict>>,
        session_id: Option<String>,
    ) -> PyResult<Vec<String>> {
        let inputs: Vec<FactInput> = items
            .iter()
            .map(|item| fact_input_from_dict(&self.0, item, session_id.as_deref()))
            .collect::<PyResult<_>>()?;
        let ids = self.0.fact_add_many(inputs).map_err(map_err)?;
        Ok(ids.iter().map(|id| id.to_string()).collect())
    }

    /// Get a fact by ID.
    fn fact_get(&self, py: Python<'_>, fact_id: String) -> PyResult<Py<PyAny>> {
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
        source_id=None,
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
        source_id: Option<String>,
    ) -> PyResult<Py<PyAny>> {
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
            source_id,
        };
        let facts = self.0.fact_list(opts).map_err(map_err)?;
        to_py(py, &facts)
    }

    /// Delete a fact by ID.
    fn fact_delete(&self, fact_id: String) -> PyResult<()> {
        let id = parse_fact_id(&fact_id)?;
        self.0.fact_delete(&id).map_err(map_err)
    }

    /// Return currently pinned facts, sorted by recency.
    #[pyo3(signature = (limit=10))]
    fn pinned_facts(&self, py: Python<'_>, limit: usize) -> PyResult<Py<PyAny>> {
        let facts = self.0.pinned_facts(limit).map_err(map_err)?;
        to_py(py, &facts)
    }

    /// Pin or unpin a fact in the always-loaded tier.
    fn fact_pin(&self, fact_id: String, pinned: bool) -> PyResult<()> {
        let id = parse_fact_id(&fact_id)?;
        self.0.fact_pin(&id, pinned).map_err(map_err)
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
    ) -> PyResult<Py<PyAny>> {
        let dir = parse_direction(direction.as_deref());
        let relations = self.0.relations_get(&entity, dir).map_err(map_err)?;
        to_py(py, &relations)
    }

    // === Ingest / Lint / Stats ===

    /// Ingest a markdown wiki into the graph.
    #[pyo3(signature = (wiki_root, incremental=false))]
    fn ingest(&self, py: Python<'_>, wiki_root: String, incremental: bool) -> PyResult<Py<PyAny>> {
        let stats = self
            .0
            .ingest(Path::new(&wiki_root), incremental)
            .map_err(map_err)?;
        to_py(py, &stats)
    }

    /// Run lint checks; returns a list of issues.
    fn lint(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let issues = self.0.lint().map_err(map_err)?;
        to_py(py, &issues)
    }

    /// Store statistics (entity/fact/relation count, size, last ingest time).
    fn stats(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let stats = self.0.stats().map_err(map_err)?;
        to_py(py, &stats)
    }

    // === Branch logs ===

    /// Push a local branch segment for this open store. For S3 branch targets,
    /// use the CLI build with `--features s3`.
    #[pyo3(signature = (branch, destination, base=None))]
    fn branch_push(
        &self,
        py: Python<'_>,
        branch: String,
        destination: String,
        base: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        if aidememo_core::backup::is_s3_uri(&destination)
            || base
                .as_deref()
                .is_some_and(aidememo_core::backup::is_s3_uri)
        {
            return Err(PyErr::new::<AideMemoInvalidInputError, _>(
                "S3 branch push requires the aidememo CLI built with `--features s3`",
            ));
        }
        let base_manifest = match base.as_deref() {
            Some(path) => Some(
                aidememo_core::backup::read_local_backup_manifest(Path::new(path))
                    .map_err(map_err)?,
            ),
            None => None,
        };
        let report = aidememo_core::branch::push_local_branch_for_wiki(
            &self.0,
            &branch,
            base_manifest.as_ref(),
            Path::new(&destination),
        )
        .map_err(map_err)?;
        to_py(py, &report)
    }

    /// Merge local branch segments into this open store. For S3 branch sources,
    /// use the CLI build with `--features s3`.
    #[pyo3(signature = (source, branch=None))]
    fn branch_merge(
        &self,
        py: Python<'_>,
        source: String,
        branch: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        if aidememo_core::backup::is_s3_uri(&source) {
            return Err(PyErr::new::<AideMemoInvalidInputError, _>(
                "S3 branch merge requires the aidememo CLI built with `--features s3`",
            ));
        }
        let report = aidememo_core::branch::merge_local_branches_for_wiki(
            &self.0,
            Path::new(&source),
            branch.as_deref(),
        )
        .map_err(map_err)?;
        to_py(py, &report)
    }
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

#[pymodule]
fn aidememo_python(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAideMemo>()?;
    m.add("AideMemoError", _py.get_type::<AideMemoError>())?;
    m.add(
        "AideMemoNotFoundError",
        _py.get_type::<AideMemoNotFoundError>(),
    )?;
    m.add(
        "AideMemoInvalidInputError",
        _py.get_type::<AideMemoInvalidInputError>(),
    )?;
    m.add("AideMemoStoreError", _py.get_type::<AideMemoStoreError>())?;
    m.add("AideMemoSearchError", _py.get_type::<AideMemoSearchError>())?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
