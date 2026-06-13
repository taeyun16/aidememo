//! Elixir/Erlang NIF bindings for AideMemo (rustler).
//!
//! Read methods that return complex shapes (search results, traversal,
//! query, etc.) return JSON strings. The Elixir host calls `Jason.decode!/1`
//! once to convert. This keeps the Rust surface compact and lets schemas
//! evolve without rebuilding the NIF every time a field is added.

use aidememo_core::{
    AideMemo, Config, EntityId, EntityInput, EntityType, FactId, FactInput, FactListOpts, FactType,
    ListOpts, QueryOpts, RelationInput, RelationType, SearchOpts, TraverseDirection, TraverseOpts,
};
use rustler::{Encoder, Env, NifResult, ResourceArc, Return, Term};
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Resource: NIF handle to the AideMemo
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AideMemoNif {
    wiki: Arc<AideMemo>,
}

#[rustler::resource_impl]
impl rustler::Resource for AideMemoNif {}

fn load(_env: Env, _term: Term) -> bool {
    true
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_json<T: serde::Serialize>(value: &T) -> NifResult<String> {
    serde_json::to_string(value).map_err(|_| rustler::Error::BadArg)
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

fn parse_direction(value: &str) -> TraverseDirection {
    match value.to_lowercase().as_str() {
        "forward" => TraverseDirection::Forward,
        "reverse" => TraverseDirection::Reverse,
        _ => TraverseDirection::Both,
    }
}

fn parse_entity_id(s: &str) -> NifResult<EntityId> {
    EntityId::parse(s).ok_or(rustler::Error::BadArg)
}

fn parse_fact_id(s: &str) -> NifResult<FactId> {
    FactId::parse(s).ok_or(rustler::Error::BadArg)
}

fn opt_str(s: String) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn opt_vec(v: Vec<String>) -> Option<Vec<String>> {
    if v.is_empty() { None } else { Some(v) }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[rustler::nif]
fn open(env: Env, store_path: String) -> Return {
    open_with_config(env, store_path, None)
}

#[rustler::nif]
fn open_with_backend(env: Env, store_path: String, backend: String) -> Return {
    open_with_config(env, store_path, opt_str(backend))
}

fn open_store(
    store_path: &str,
    backend: Option<String>,
) -> std::result::Result<AideMemo, aidememo_core::AideMemoError> {
    let mut config = Config::default();
    if let Some(backend) = backend.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        config.set("store.backend", backend)?;
    }
    AideMemo::open(Path::new(store_path), config)
}

fn open_with_config(env: Env, store_path: String, backend: Option<String>) -> Return {
    match open_store(&store_path, backend) {
        Ok(wiki) => {
            let resource = ResourceArc::new(AideMemoNif {
                wiki: Arc::new(wiki),
            });
            Return::Term(resource.encode(env))
        }
        Err(_) => Return::Error(rustler::Error::BadArg),
    }
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[rustler::nif]
fn search(
    handle: ResourceArc<AideMemoNif>,
    query: String,
    limit: u32,
    current_only: bool,
) -> NifResult<String> {
    let opts = SearchOpts {
        limit: Some(limit as usize),
        current_only,
        ..Default::default()
    };
    let results = handle
        .wiki
        .hybrid_search(&query, opts)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&results)
}

#[rustler::nif]
fn query(
    handle: ResourceArc<AideMemoNif>,
    topic: String,
    limit: u32,
    depth: u32,
    recent_limit: u32,
    current_only: bool,
    mode: String,
) -> NifResult<String> {
    let opts = QueryOpts {
        search_limit: limit as usize,
        depth,
        recent_limit: recent_limit as usize,
        since: None,
        current_only,
        mode: aidememo_core::QueryMode::parse(&mode),
        bm25_only: false,
        source_id: None,
    };
    let result = handle
        .wiki
        .query(&topic, opts)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&result)
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

#[rustler::nif]
fn traverse(
    handle: ResourceArc<AideMemoNif>,
    entity: String,
    depth: u32,
    direction: String,
) -> NifResult<String> {
    let opts = TraverseOpts {
        depth,
        relation_types: None,
        direction: parse_direction(&direction),
    };
    let result = handle
        .wiki
        .traverse(&entity, opts)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&result)
}

#[rustler::nif]
fn path_find(handle: ResourceArc<AideMemoNif>, from: String, to: String) -> NifResult<String> {
    let path = handle
        .wiki
        .path_find(&from, &to)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&path)
}

// ---------------------------------------------------------------------------
// Entity CRUD
// ---------------------------------------------------------------------------

#[rustler::nif]
fn entity_add(
    handle: ResourceArc<AideMemoNif>,
    name: String,
    entity_type: String,
    tags: Vec<String>,
    aliases: Vec<String>,
    source_page: String,
) -> NifResult<String> {
    let input = EntityInput {
        name,
        entity_type: parse_entity_type(&entity_type),
        tags: opt_vec(tags),
        aliases: opt_vec(aliases),
        source_page: opt_str(source_page),
    };
    let id = handle
        .wiki
        .entity_add(input)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(id.to_string())
}

#[rustler::nif]
fn entity_get(handle: ResourceArc<AideMemoNif>, name: String) -> NifResult<String> {
    let entity = handle
        .wiki
        .entity_get(&name)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&entity)
}

#[rustler::nif]
fn entity_list(
    handle: ResourceArc<AideMemoNif>,
    limit: u32,
    entity_type: String,
) -> NifResult<String> {
    let opts = ListOpts {
        entity_type: if entity_type.is_empty() {
            None
        } else {
            parse_entity_type(&entity_type)
        },
        min_facts: None,
        limit: if limit == 0 {
            None
        } else {
            Some(limit as usize)
        },
        sort_by: Default::default(),
        offset: 0,
    };
    let entities = handle
        .wiki
        .entity_list(opts)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&entities)
}

#[rustler::nif]
fn entity_delete(handle: ResourceArc<AideMemoNif>, name: String) -> NifResult<rustler::Atom> {
    handle
        .wiki
        .entity_delete(&name)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(rustler::types::atom::ok())
}

#[rustler::nif]
fn entity_describe(
    handle: ResourceArc<AideMemoNif>,
    name: String,
    summary: String,
) -> NifResult<rustler::Atom> {
    handle
        .wiki
        .entity_describe(&name, &summary)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(rustler::types::atom::ok())
}

#[rustler::nif]
fn resolve_entity(handle: ResourceArc<AideMemoNif>, name: String) -> NifResult<String> {
    let id = handle
        .wiki
        .resolve_entity(&name)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(id.to_string())
}

// ---------------------------------------------------------------------------
// Fact CRUD
// ---------------------------------------------------------------------------

#[rustler::nif]
fn fact_add(
    handle: ResourceArc<AideMemoNif>,
    content: String,
    entity_ids: Vec<String>,
    fact_type: String,
    tags: Vec<String>,
    source: String,
    confidence: f32,
) -> NifResult<String> {
    let entity_ids = if entity_ids.is_empty() {
        None
    } else {
        Some(
            entity_ids
                .iter()
                .map(|s| parse_entity_id(s))
                .collect::<NifResult<Vec<_>>>()?,
        )
    };
    let input = FactInput {
        content,
        fact_type: parse_fact_type(&fact_type),
        entity_ids,
        tags: opt_vec(tags),
        source: opt_str(source),
        source_id: None,
        source_confidence: if confidence > 0.0 {
            Some(confidence)
        } else {
            None
        },
        observed_at: None,
    };
    let id = handle
        .wiki
        .fact_add(input)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(id.to_string())
}

/// Single item for `fact_add_many`. Pass each item as an Elixir map
/// with these keys; an empty `entity_ids` / `tags` / `source` /
/// `fact_type` skips the corresponding field, and `confidence <= 0`
/// inherits the default.
#[derive(rustler::NifMap)]
struct FactAddManyItem {
    content: String,
    entity_ids: Vec<String>,
    fact_type: String,
    tags: Vec<String>,
    source: String,
    confidence: f32,
}

#[rustler::nif]
fn fact_add_many(
    handle: ResourceArc<AideMemoNif>,
    items: Vec<FactAddManyItem>,
) -> NifResult<Vec<String>> {
    let mut inputs = Vec::with_capacity(items.len());
    for item in items {
        let entity_ids = if item.entity_ids.is_empty() {
            None
        } else {
            Some(
                item.entity_ids
                    .iter()
                    .map(|s| parse_entity_id(s))
                    .collect::<NifResult<Vec<_>>>()?,
            )
        };
        inputs.push(FactInput {
            content: item.content,
            fact_type: parse_fact_type(&item.fact_type),
            entity_ids,
            tags: opt_vec(item.tags),
            source: opt_str(item.source),
            source_id: None,
            source_confidence: if item.confidence > 0.0 {
                Some(item.confidence)
            } else {
                None
            },
            observed_at: None,
        });
    }
    let ids = handle
        .wiki
        .fact_add_many(inputs)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(ids.iter().map(|id| id.to_string()).collect())
}

#[rustler::nif]
fn fact_get(handle: ResourceArc<AideMemoNif>, fact_id: String) -> NifResult<String> {
    let id = parse_fact_id(&fact_id)?;
    let fact = handle
        .wiki
        .fact_get(&id)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&fact)
}

#[rustler::nif]
fn fact_list(
    handle: ResourceArc<AideMemoNif>,
    entity: String,
    fact_type: String,
    limit: u32,
    current_only: bool,
) -> NifResult<String> {
    let entity_id = if entity.is_empty() {
        None
    } else {
        handle.wiki.resolve_entity(&entity).ok()
    };
    let opts = FactListOpts {
        fact_type: if fact_type.is_empty() {
            None
        } else {
            parse_fact_type(&fact_type)
        },
        entity_id,
        min_confidence: None,
        limit: if limit == 0 {
            None
        } else {
            Some(limit as usize)
        },
        offset: 0,
        since: None,
        until: None,
        current_only,
        as_of: None,
        source_id: None,
    };
    let facts = handle
        .wiki
        .fact_list(opts)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&facts)
}

#[rustler::nif]
fn fact_supersede(
    handle: ResourceArc<AideMemoNif>,
    old_id: String,
    new_id: String,
) -> NifResult<rustler::Atom> {
    let old = parse_fact_id(&old_id)?;
    let new = parse_fact_id(&new_id)?;
    handle
        .wiki
        .fact_supersede(&old, &new)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(rustler::types::atom::ok())
}

#[rustler::nif]
fn fact_delete(handle: ResourceArc<AideMemoNif>, fact_id: String) -> NifResult<rustler::Atom> {
    let id = parse_fact_id(&fact_id)?;
    handle
        .wiki
        .fact_delete(&id)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(rustler::types::atom::ok())
}

// ---------------------------------------------------------------------------
// Relations
// ---------------------------------------------------------------------------

#[rustler::nif]
fn relation_add(
    handle: ResourceArc<AideMemoNif>,
    source: String,
    target: String,
    rel_type: String,
) -> NifResult<rustler::Atom> {
    let input = RelationInput {
        source,
        target,
        relation_type: RelationType::new(rel_type),
        weight: None,
        evidence: None,
    };
    handle
        .wiki
        .relation_add(input)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(rustler::types::atom::ok())
}

#[rustler::nif]
fn relation_remove(
    handle: ResourceArc<AideMemoNif>,
    source: String,
    target: String,
    rel_type: String,
) -> NifResult<rustler::Atom> {
    handle
        .wiki
        .relation_remove(&source, &target, &rel_type)
        .map_err(|_| rustler::Error::BadArg)?;
    Ok(rustler::types::atom::ok())
}

#[rustler::nif]
fn relations_get(
    handle: ResourceArc<AideMemoNif>,
    entity: String,
    direction: String,
) -> NifResult<String> {
    let dir = parse_direction(&direction);
    let relations = handle
        .wiki
        .relations_get(&entity, dir)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&relations)
}

// ---------------------------------------------------------------------------
// Ingest / Lint / Stats
// ---------------------------------------------------------------------------

#[rustler::nif]
fn ingest(
    handle: ResourceArc<AideMemoNif>,
    wiki_root: String,
    incremental: bool,
) -> NifResult<String> {
    let stats = handle
        .wiki
        .ingest(Path::new(&wiki_root), incremental)
        .map_err(|_| rustler::Error::BadArg)?;
    to_json(&stats)
}

#[rustler::nif]
fn lint(handle: ResourceArc<AideMemoNif>) -> NifResult<String> {
    let issues = handle.wiki.lint().map_err(|_| rustler::Error::BadArg)?;
    to_json(&issues)
}

#[rustler::nif]
fn stats(handle: ResourceArc<AideMemoNif>) -> NifResult<String> {
    let stats = handle.wiki.stats().map_err(|_| rustler::Error::BadArg)?;
    to_json(&stats)
}

#[rustler::nif]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

rustler::init!("Elixir.AideMemoNif.Native", load = load);

#[cfg(all(test, any(feature = "sqlite", feature = "redb")))]
mod backend_binding_tests {
    use super::*;

    fn temp_store_path(name: &str, suffix: &str) -> std::path::PathBuf {
        let unique = format!(
            "aidememo-nif-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join(format!("wiki.{suffix}"))
    }

    fn assert_backend_file(path: &std::path::Path, backend: &str) {
        let header = std::fs::read(path).expect("read backend file");
        match backend {
            "sqlite" | "libsqlite" => assert_eq!(&header[..16], b"SQLite format 3\0"),
            "redb" => assert_ne!(&header[..16], b"SQLite format 3\0"),
            other => panic!("unsupported backend in test: {other}"),
        }
    }

    fn expected_default_backend() -> &'static str {
        if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
            "redb"
        } else {
            "sqlite"
        }
    }

    fn assert_open_store_opens_backend(name: &str, backend: Option<&str>, expected_backend: &str) {
        let path = temp_store_path(name, expected_backend);
        let store = open_store(path.to_string_lossy().as_ref(), backend.map(str::to_string))
            .expect("open backend");

        assert_eq!(store.config().store.backend, expected_backend);
        let entity_id = store
            .entity_add(EntityInput {
                name: format!("Elixir{expected_backend}"),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .expect("entity add");
        store
            .fact_add(FactInput {
                content: format!("Elixir NIF opened a {expected_backend} backend"),
                entity_ids: Some(vec![entity_id]),
                fact_type: Some(FactType::Note),
                ..Default::default()
            })
            .expect("fact add");

        let stats = store.stats().expect("stats");
        assert_eq!(stats.entity_count, 1);
        assert_eq!(stats.fact_count, 1);
        assert_backend_file(&path, expected_backend);
    }

    #[test]
    fn open_without_backend_opens_default_backend() {
        let expected = expected_default_backend();
        assert_open_store_opens_backend("default-open", None, expected);
    }

    #[test]
    fn open_with_empty_backend_opens_default_backend() {
        let expected = expected_default_backend();
        assert_open_store_opens_backend("empty-backend-open", Some(""), expected);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn open_with_backend_opens_sqlite_backend() {
        assert_open_store_opens_backend("sqlite-open", Some("sqlite"), "sqlite");
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn open_with_backend_opens_libsqlite_alias() {
        assert_open_store_opens_backend("libsqlite-open", Some("libsqlite"), "libsqlite");
    }

    #[cfg(feature = "redb")]
    #[test]
    fn open_with_backend_opens_redb_backend() {
        assert_open_store_opens_backend("redb-open", Some("redb"), "redb");
    }
}
