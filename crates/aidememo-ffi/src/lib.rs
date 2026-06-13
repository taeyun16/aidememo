//! C-ABI bindings for AideMemo.
//!
//! Style:
//! - All read functions return a heap-allocated, NUL-terminated UTF-8 JSON
//!   string. The caller MUST free it with `aidememo_free_string`.
//! - Errors come back as `{"error": "..."}` JSON, never as null.
//! - `aidememo_open` returns a `aidememo_store_t*` handle (or NULL on failure); free it
//!   with `aidememo_close`.
//! - All input strings are borrowed `const char*` (NUL-terminated, UTF-8).
//!
//! Thread safety: a single `aidememo_store_t*` is safe to share across threads
//! (the underlying graph uses an `RwLock` internally).

// `extern "C"` functions taking `*const c_char` necessarily dereference raw
// pointers, but marking them `unsafe` would change the C header signature
// for no benefit (C callers don't have `unsafe`). The `ptr_to_str` /
// `store_ref` helpers null-check before dereferencing.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use aidememo_core::{
    AideMemo, Config, EntityId, EntityInput, EntityType, FactId, FactInput, FactListOpts, FactType,
    ListOpts, QueryOpts, RelationInput, RelationType, SearchOpts, TraverseDirection, TraverseOpts,
};
use serde_json::json;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Opaque handle
// ---------------------------------------------------------------------------

pub struct AideMemoStore {
    wiki: Arc<AideMemo>,
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

fn ptr_to_optional_str(p: *const c_char) -> Result<Option<&'static str>, String> {
    if p.is_null() {
        return Ok(None);
    }
    let value = ptr_to_str(p)?;
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn store_ref<'a>(p: *const AideMemoStore) -> Result<&'a AideMemoStore, String> {
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
/// Free with `aidememo_close`.
#[unsafe(no_mangle)]
pub extern "C" fn aidememo_open(path: *const c_char) -> *mut AideMemoStore {
    open_with_backend(path, ptr::null())
}

/// Open or create a store at `path` using an explicit backend.
/// `backend` may be NULL or empty for the default SQLite backend.
/// Use "redb" only when this library was built with the `redb` Cargo feature.
#[unsafe(no_mangle)]
pub extern "C" fn aidememo_open_with_backend(
    path: *const c_char,
    backend: *const c_char,
) -> *mut AideMemoStore {
    open_with_backend(path, backend)
}

fn open_with_backend(path: *const c_char, backend: *const c_char) -> *mut AideMemoStore {
    let path = match ptr_to_str(path) {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };
    let backend = match ptr_to_optional_str(backend) {
        Ok(value) => value,
        Err(_) => return ptr::null_mut(),
    };
    let mut config = Config::default();
    if let Some(backend) = backend {
        if config.set("store.backend", backend).is_err() {
            return ptr::null_mut();
        }
    }
    match AideMemo::open(std::path::Path::new(path), config) {
        Ok(wiki) => Box::into_raw(Box::new(AideMemoStore {
            wiki: Arc::new(wiki),
        })),
        Err(_) => ptr::null_mut(),
    }
}

/// Close a store handle. Safe to call with NULL.
#[unsafe(no_mangle)]
pub extern "C" fn aidememo_close(store: *mut AideMemoStore) {
    if !store.is_null() {
        unsafe { drop(Box::from_raw(store)) }
    }
}

/// Free a string returned by any `aidememo_*` read function. Safe to call with NULL.
#[unsafe(no_mangle)]
pub extern "C" fn aidememo_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)) }
    }
}

/// Library version (NUL-terminated). Caller frees with `aidememo_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn aidememo_version() -> *mut c_char {
    return_string(env!("CARGO_PKG_VERSION").to_string())
}

// ---------------------------------------------------------------------------
// Search / Query
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_search(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_query(
    store: *const AideMemoStore,
    topic: *const c_char,
    limit: u32,
    depth: u32,
    recent_limit: u32,
    current_only: bool,
    mode: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let topic = ptr_to_str(topic)?;
        let mode = if mode.is_null() {
            aidememo_core::QueryMode::default()
        } else {
            aidememo_core::QueryMode::parse(ptr_to_str(mode)?)
        };
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
            mode,
            bm25_only: false,
            source_id: None,
        };
        let result = s.wiki.query(topic, opts).map_err(|e| e.to_string())?;
        json_serialize(&result)
    })
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_traverse(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_path_find(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_entity_add(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_entity_get(
    store: *const AideMemoStore,
    name: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let name = ptr_to_str(name)?;
        let entity = s.wiki.entity_get(name).map_err(|e| e.to_string())?;
        json_serialize(&entity)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_entity_list(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_entity_delete(
    store: *const AideMemoStore,
    name: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let name = ptr_to_str(name)?;
        s.wiki.entity_delete(name).map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_entity_describe(
    store: *const AideMemoStore,
    name: *const c_char,
    summary: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let name = ptr_to_str(name)?;
        let summary = if summary.is_null() {
            ""
        } else {
            ptr_to_str(summary)?
        };
        s.wiki
            .entity_describe(name, summary)
            .map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_resolve_entity(
    store: *const AideMemoStore,
    name: *const c_char,
) -> *mut c_char {
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
pub extern "C" fn aidememo_fact_add(
    store: *const AideMemoStore,
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
            source_id: None,
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

/// Insert many facts in one backend transaction when supported. `items_json` is a
/// JSON array of objects with the same keys as `aidememo_fact_add`'s args:
/// `content` (required), `entity_ids`, `fact_type`, `tags`, `source`,
/// `confidence`. Returns `{"ids":[...]}` on success.
#[unsafe(no_mangle)]
pub extern "C" fn aidememo_fact_add_many(
    store: *const AideMemoStore,
    items_json: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let raw = ptr_to_str(items_json)?;
        let parsed: serde_json::Value =
            serde_json::from_str(raw).map_err(|e| format!("invalid items_json: {e}"))?;
        let arr = parsed
            .as_array()
            .ok_or_else(|| "items_json must be a JSON array".to_string())?;

        let mut inputs = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let obj = item
                .as_object()
                .ok_or_else(|| format!("items_json[{i}] must be a JSON object"))?;
            let content = obj
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("items_json[{i}].content is required"))?
                .to_string();
            let entity_ids = match obj.get("entity_ids") {
                Some(v) if !v.is_null() => {
                    let arr = v
                        .as_array()
                        .ok_or_else(|| format!("items_json[{i}].entity_ids must be an array"))?;
                    let ids: Result<Vec<EntityId>, String> = arr
                        .iter()
                        .map(|x| {
                            let s = x.as_str().ok_or_else(|| {
                                format!("items_json[{i}].entity_ids must be strings")
                            })?;
                            EntityId::parse(s).ok_or_else(|| format!("invalid entity id: {s}"))
                        })
                        .collect();
                    let ids = ids?;
                    if ids.is_empty() { None } else { Some(ids) }
                }
                _ => None,
            };
            let fact_type = obj
                .get("fact_type")
                .and_then(|v| v.as_str())
                .and_then(parse_fact_type);
            let tags = match obj.get("tags") {
                Some(v) if !v.is_null() => {
                    let arr = v
                        .as_array()
                        .ok_or_else(|| format!("items_json[{i}].tags must be an array"))?;
                    let tags: Result<Vec<String>, String> = arr
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .map(|s| s.to_string())
                                .ok_or_else(|| format!("items_json[{i}].tags must be strings"))
                        })
                        .collect();
                    let tags = tags?;
                    if tags.is_empty() { None } else { Some(tags) }
                }
                _ => None,
            };
            let source = obj
                .get("source")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            let confidence = obj
                .get("confidence")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32);
            inputs.push(FactInput {
                content,
                fact_type,
                entity_ids,
                tags,
                source,
                source_id: None,
                source_confidence: confidence,
                observed_at: None,
            });
        }
        let ids = s.wiki.fact_add_many(inputs).map_err(|e| e.to_string())?;
        let id_strs: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        Ok(json!({ "ids": id_strs }).to_string())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_fact_get(
    store: *const AideMemoStore,
    fact_id: *const c_char,
) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let id_str = ptr_to_str(fact_id)?;
        let id = FactId::parse(id_str).ok_or_else(|| format!("invalid fact id: {id_str}"))?;
        let fact = s.wiki.fact_get(&id).map_err(|e| e.to_string())?;
        json_serialize(&fact)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_fact_list(
    store: *const AideMemoStore,
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
            as_of: None,
            source_id: None,
        };
        let facts = s.wiki.fact_list(opts).map_err(|e| e.to_string())?;
        json_serialize(&facts)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_fact_supersede(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_fact_delete(
    store: *const AideMemoStore,
    fact_id: *const c_char,
) -> *mut c_char {
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
pub extern "C" fn aidememo_relation_add(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_relation_remove(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_relations_get(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_ingest(
    store: *const AideMemoStore,
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
pub extern "C" fn aidememo_lint(store: *const AideMemoStore) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let issues = s.wiki.lint().map_err(|e| e.to_string())?;
        json_serialize(&issues)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn aidememo_stats(store: *const AideMemoStore) -> *mut c_char {
    return_json(|| {
        let s = store_ref(store)?;
        let stats = s.wiki.stats().map_err(|e| e.to_string())?;
        json_serialize(&stats)
    })
}

#[cfg(all(test, any(feature = "sqlite", feature = "redb")))]
mod backend_binding_tests {
    use super::*;

    fn temp_store_path(name: &str, suffix: &str) -> std::path::PathBuf {
        let unique = format!(
            "aidememo-ffi-{name}-{}-{}",
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

    fn assert_open_with_backend_opens_backend(name: &str, backend: &str, suffix: &str) {
        let store_path = temp_store_path(name, suffix);
        let path =
            std::ffi::CString::new(store_path.to_string_lossy().as_ref()).expect("path cstring");
        let backend = std::ffi::CString::new(backend).expect("backend cstring");
        let store = aidememo_open_with_backend(path.as_ptr(), backend.as_ptr());
        assert!(!store.is_null());
        let store_ref = unsafe { store.as_ref() }.expect("store ref");
        assert_eq!(
            store_ref.wiki.config().store.backend,
            backend.to_str().expect("backend utf8")
        );

        let stats = aidememo_stats(store);
        assert!(!stats.is_null());
        aidememo_free_string(stats);
        assert_backend_file(&store_path, backend.to_str().expect("backend utf8"));
        aidememo_close(store);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn open_with_backend_opens_sqlite_backend() {
        assert_open_with_backend_opens_backend("sqlite-open", "sqlite", "sqlite");
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn open_with_backend_opens_libsqlite_alias() {
        assert_open_with_backend_opens_backend("libsqlite-open", "libsqlite", "libsqlite");
    }

    #[cfg(feature = "redb")]
    #[test]
    fn open_with_backend_opens_redb_backend() {
        assert_open_with_backend_opens_backend("redb-open", "redb", "redb");
    }
}
