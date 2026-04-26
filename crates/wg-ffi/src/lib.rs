//! C-ABI bindings for WikiGraph.
//!
//! Style:
//! - All read functions return a heap-allocated, NUL-terminated UTF-8 JSON
//!   string. The caller MUST free it with `wg_free_string`.
//! - Errors come back as `{"error": "..."}` JSON, never as null.
//! - `wg_open` returns a `wg_store_t*` handle (or NULL on failure); free it
//!   with `wg_close`.
//! - All input strings are borrowed `const char*` (NUL-terminated, UTF-8).
//!
//! Thread safety: a single `wg_store_t*` is safe to share across threads
//! (the underlying graph uses an `RwLock` internally).

use serde_json::json;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::Arc;
use wg_core::{
    Config, EntityId, EntityInput, EntityType, FactId, FactInput, FactListOpts, FactType, ListOpts,
    QueryOpts, RelationInput, RelationType, SearchOpts, TraverseDirection, TraverseOpts, WikiGraph,
};

// ---------------------------------------------------------------------------
// Opaque handle
// ---------------------------------------------------------------------------

pub struct WgStore {
    wiki: Arc<WikiGraph>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn return_json<F>(f: F) -> *mut c_char
where
    F: FnOnce() -> Result<String, String>,
{
    let payload = match f() {
        Ok(s) => s,
        Err(e) => json!({ "error": e }).to_string(),
    };
    match CString::new(payload) {
        Ok(cs) => cs.into_raw(),
        Err(_) => CString::new(r#"{"error":"interior NUL in payload"}"#)
            .unwrap()
            .into_raw(),
    }
}

fn return_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => CString::new(r#"{"error":"interior NUL"}"#)
            .unwrap()
            .into_raw(),
    }
}

fn ptr_to_str(p: *const c_char) -> Result<&'static str, String> {
    if p.is_null() {
        return Err("null pointer".into());
    }
    let cstr = unsafe { CStr::from_ptr(p) };
    cstr.to_str().map_err(|e| e.to_string())
}

fn store_ref<'a>(p: *const WgStore) -> Result<&'a WgStore, String> {
    unsafe { p.as_ref() }.ok_or_else(|| "null store handle".into())
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

fn parse_direction(s: &str) -> TraverseDirection {
    match s.to_lowercase().as_str() {
        "forward" => TraverseDirection::Forward,
        "reverse" => TraverseDirection::Reverse,
        _ => TraverseDirection::Both,
    }
}

fn parse_string_array(json_str: &str) -> Result<Vec<String>, String> {
    if json_str.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(json_str).map_err(|e| format!("invalid JSON array: {e}"))
}

fn json_serialize<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Open or create a store at `path`. Returns NULL on error.
/// Free with `wg_close`.
#[unsafe(no_mangle)]
pub extern "C" fn wg_open(path: *const c_char) -> *mut WgStore {
    let path = match ptr_to_str(path) {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };
    match WikiGraph::open(std::path::Path::new(path), Config::default()) {
        Ok(wiki) => Box::into_raw(Box::new(WgStore {
            wiki: Arc::new(wiki),
        })),
        Err(_) => ptr::null_mut(),
    }
}

/// Close a store handle. Safe to call with NULL.
#[unsafe(no_mangle)]
pub extern "C" fn wg_close(store: *mut WgStore) {
    if !store.is_null() {
        unsafe { drop(Box::from_raw(store)) }
    }
}

/// Free a string returned by any `wg_*` read function. Safe to call with NULL.
#[unsafe(no_mangle)]
pub extern "C" fn wg_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)) }
    }
}

/// Library version (NUL-terminated). Caller frees with `wg_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn wg_version() -> *mut c_char {
    return_string(env!("CARGO_PKG_VERSION").to_string())
}

// ---------------------------------------------------------------------------
// Search / Query
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn wg_search(
    store: *const WgStore,
    query: *const c_char,
    limit: u32,
    current_only: bool,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let q = ptr_to_str(query)?;
        let opts = SearchOpts {
            limit: if limit == 0 {
                None
            } else {
                Some(limit as usize)
            },
            current_only,
            ..Default::default()
        };
        let results = s.wiki.hybrid_search(q, opts).map_err(|e| e.to_string())?;
        json_serialize(&results)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_query(
    store: *const WgStore,
    topic: *const c_char,
    limit: u32,
    depth: u32,
    recent_limit: u32,
    current_only: bool,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let topic = ptr_to_str(topic)?;
        let opts = QueryOpts {
            search_limit: if limit == 0 { 10 } else { limit as usize },
            depth: if depth == 0 { 2 } else { depth },
            recent_limit: if recent_limit == 0 {
                10
            } else {
                recent_limit as usize
            },
            since: None,
            current_only,
        };
        let result = s.wiki.query(topic, opts).map_err(|e| e.to_string())?;
        json_serialize(&result)
    })
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn wg_traverse(
    store: *const WgStore,
    entity: *const c_char,
    depth: u32,
    direction: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let entity = ptr_to_str(entity)?;
        let direction = if direction.is_null() {
            "both"
        } else {
            ptr_to_str(direction)?
        };
        let opts = TraverseOpts {
            depth: if depth == 0 { 2 } else { depth },
            relation_types: None,
            direction: parse_direction(direction),
        };
        let result = s.wiki.traverse(entity, opts).map_err(|e| e.to_string())?;
        json_serialize(&result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_path_find(
    store: *const WgStore,
    from: *const c_char,
    to: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let from = ptr_to_str(from)?;
        let to = ptr_to_str(to)?;
        let path = s.wiki.path_find(from, to).map_err(|e| e.to_string())?;
        json_serialize(&path)
    })
}

// ---------------------------------------------------------------------------
// Entity CRUD
// ---------------------------------------------------------------------------

/// `entity_type`/`tags_json`/`aliases_json`/`source_page` may be NULL or empty.
/// `tags_json` and `aliases_json` are JSON arrays of strings (or empty/null).
#[unsafe(no_mangle)]
pub extern "C" fn wg_entity_add(
    store: *const WgStore,
    name: *const c_char,
    entity_type: *const c_char,
    tags_json: *const c_char,
    aliases_json: *const c_char,
    source_page: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let name = ptr_to_str(name)?.to_string();
        let entity_type = if entity_type.is_null() {
            None
        } else {
            parse_entity_type(ptr_to_str(entity_type)?)
        };
        let tags = if tags_json.is_null() {
            Vec::new()
        } else {
            parse_string_array(ptr_to_str(tags_json)?)?
        };
        let aliases = if aliases_json.is_null() {
            Vec::new()
        } else {
            parse_string_array(ptr_to_str(aliases_json)?)?
        };
        let source_page = if source_page.is_null() {
            None
        } else {
            let v = ptr_to_str(source_page)?;
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        };
        let id = s
            .wiki
            .entity_add(EntityInput {
                name,
                entity_type,
                tags: if tags.is_empty() { None } else { Some(tags) },
                aliases: if aliases.is_empty() {
                    None
                } else {
                    Some(aliases)
                },
                source_page,
            })
            .map_err(|e| e.to_string())?;
        Ok(json!({ "id": id.to_string() }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_entity_get(store: *const WgStore, name: *const c_char) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let name = ptr_to_str(name)?;
        let entity = s.wiki.entity_get(name).map_err(|e| e.to_string())?;
        json_serialize(&entity)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_entity_list(
    store: *const WgStore,
    limit: u32,
    entity_type: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let entity_type = if entity_type.is_null() {
            None
        } else {
            parse_entity_type(ptr_to_str(entity_type)?)
        };
        let opts = ListOpts {
            entity_type,
            min_facts: None,
            limit: if limit == 0 {
                None
            } else {
                Some(limit as usize)
            },
            sort_by: Default::default(),
            offset: 0,
        };
        let entities = s.wiki.entity_list(opts).map_err(|e| e.to_string())?;
        json_serialize(&entities)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_entity_delete(store: *const WgStore, name: *const c_char) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let name = ptr_to_str(name)?;
        s.wiki.entity_delete(name).map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_resolve_entity(store: *const WgStore, name: *const c_char) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let name = ptr_to_str(name)?;
        let id = s.wiki.resolve_entity(name).map_err(|e| e.to_string())?;
        Ok(json!({ "id": id.to_string() }).to_string())
    })
}

// ---------------------------------------------------------------------------
// Fact CRUD
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn wg_fact_add(
    store: *const WgStore,
    content: *const c_char,
    entity_ids_json: *const c_char,
    fact_type: *const c_char,
    tags_json: *const c_char,
    source: *const c_char,
    confidence: f32,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let content = ptr_to_str(content)?.to_string();
        let entity_ids = if entity_ids_json.is_null() {
            None
        } else {
            let arr = parse_string_array(ptr_to_str(entity_ids_json)?)?;
            if arr.is_empty() {
                None
            } else {
                let ids: Result<Vec<EntityId>, String> = arr
                    .iter()
                    .map(|s| EntityId::parse(s).ok_or_else(|| format!("invalid entity id: {s}")))
                    .collect();
                Some(ids?)
            }
        };
        let fact_type = if fact_type.is_null() {
            None
        } else {
            parse_fact_type(ptr_to_str(fact_type)?)
        };
        let tags = if tags_json.is_null() {
            None
        } else {
            let arr = parse_string_array(ptr_to_str(tags_json)?)?;
            if arr.is_empty() { None } else { Some(arr) }
        };
        let source = if source.is_null() {
            None
        } else {
            let v = ptr_to_str(source)?;
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        };
        let input = FactInput {
            content,
            fact_type,
            entity_ids,
            tags,
            source,
            source_confidence: if confidence > 0.0 {
                Some(confidence)
            } else {
                None
            },
            observed_at: None,
        };
        let id = s.wiki.fact_add(input).map_err(|e| e.to_string())?;
        Ok(json!({ "id": id.to_string() }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_fact_get(store: *const WgStore, fact_id: *const c_char) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let id_str = ptr_to_str(fact_id)?;
        let id = FactId::parse(id_str).ok_or_else(|| format!("invalid fact id: {id_str}"))?;
        let fact = s.wiki.fact_get(&id).map_err(|e| e.to_string())?;
        json_serialize(&fact)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_fact_list(
    store: *const WgStore,
    entity: *const c_char,
    fact_type: *const c_char,
    limit: u32,
    current_only: bool,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let entity_id = if entity.is_null() {
            None
        } else {
            let n = ptr_to_str(entity)?;
            if n.is_empty() {
                None
            } else {
                s.wiki.resolve_entity(n).ok()
            }
        };
        let fact_type = if fact_type.is_null() {
            None
        } else {
            parse_fact_type(ptr_to_str(fact_type)?)
        };
        let opts = FactListOpts {
            fact_type,
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
        };
        let facts = s.wiki.fact_list(opts).map_err(|e| e.to_string())?;
        json_serialize(&facts)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_fact_supersede(
    store: *const WgStore,
    old_id: *const c_char,
    new_id: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let old_str = ptr_to_str(old_id)?;
        let new_str = ptr_to_str(new_id)?;
        let old = FactId::parse(old_str).ok_or_else(|| format!("invalid fact id: {old_str}"))?;
        let new = FactId::parse(new_str).ok_or_else(|| format!("invalid fact id: {new_str}"))?;
        s.wiki
            .fact_supersede(&old, &new)
            .map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_fact_delete(store: *const WgStore, fact_id: *const c_char) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let id_str = ptr_to_str(fact_id)?;
        let id = FactId::parse(id_str).ok_or_else(|| format!("invalid fact id: {id_str}"))?;
        s.wiki.fact_delete(&id).map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }).to_string())
    })
}

// ---------------------------------------------------------------------------
// Relations
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn wg_relation_add(
    store: *const WgStore,
    source: *const c_char,
    target: *const c_char,
    rel_type: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let source = ptr_to_str(source)?.to_string();
        let target = ptr_to_str(target)?.to_string();
        let rel_type = ptr_to_str(rel_type)?.to_string();
        let input = RelationInput {
            source,
            target,
            relation_type: RelationType::new(rel_type),
            weight: None,
            evidence: None,
        };
        s.wiki.relation_add(input).map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_relation_remove(
    store: *const WgStore,
    source: *const c_char,
    target: *const c_char,
    rel_type: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let source = ptr_to_str(source)?;
        let target = ptr_to_str(target)?;
        let rel_type = ptr_to_str(rel_type)?;
        s.wiki
            .relation_remove(source, target, rel_type)
            .map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_relations_get(
    store: *const WgStore,
    entity: *const c_char,
    direction: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let entity = ptr_to_str(entity)?;
        let direction = if direction.is_null() {
            "both"
        } else {
            ptr_to_str(direction)?
        };
        let relations = s
            .wiki
            .relations_get(entity, parse_direction(direction))
            .map_err(|e| e.to_string())?;
        json_serialize(&relations)
    })
}

// ---------------------------------------------------------------------------
// Ingest / Lint / Stats
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn wg_ingest(
    store: *const WgStore,
    wiki_root: *const c_char,
    incremental: bool,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let root = ptr_to_str(wiki_root)?;
        let stats = s
            .wiki
            .ingest(std::path::Path::new(root), incremental)
            .map_err(|e| e.to_string())?;
        json_serialize(&stats)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_lint(store: *const WgStore) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let issues = s.wiki.lint().map_err(|e| e.to_string())?;
        json_serialize(&issues)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_stats(store: *const WgStore) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let stats = s.wiki.stats().map_err(|e| e.to_string())?;
        json_serialize(&stats)
    })
}
