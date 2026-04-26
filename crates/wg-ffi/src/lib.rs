//! C-ABI bindings for WikiGraph.

use serde_json::json;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::Arc;
use wg_core::{Config, EntityId, FactInput, FactType, ListOpts, SearchOpts, WikiGraph};

macro_rules! WG_FFI_EXPORT {
    ($expr:expr) => {{
        match $expr {
            Ok(output) => match CString::new(output) {
                Ok(c_string) => c_string.into_raw(),
                Err(_) => CString::new("{\"error\":\"string contains interior NUL\"}")
                    .unwrap()
                    .into_raw(),
            },
            Err(err) => CString::new(json!({"error": err}).to_string())
                .unwrap()
                .into_raw(),
        }
    }};
}

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

fn parse_entity_ids_json(json_ids: &str) -> Result<Vec<EntityId>, String> {
    let values: Vec<String> = serde_json::from_str(json_ids).map_err(|e| e.to_string())?;
    values
        .into_iter()
        .map(|id| EntityId::parse(&id).ok_or_else(|| format!("invalid entity id: {id}")))
        .collect()
}

fn ptr_to_string(value: *const c_char) -> Result<String, String> {
    if value.is_null() {
        return Err("null pointer".to_string());
    }
    let cstr = unsafe { CStr::from_ptr(value) };
    cstr.to_str()
        .map(|s| s.to_string())
        .map_err(|e| e.to_string())
}

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

fn to_json_string<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| json!({"error": e.to_string()}).to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_open(store_path: *const c_char) -> *mut WgFfi {
    let path = match ptr_to_string(store_path) {
        Ok(p) => p,
        Err(_) => return ptr::null_mut(),
    };
    match WgFfi::new(&path) {
        Ok(wrapper) => Box::into_raw(Box::new(wrapper)),
        Err(_) => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_close(ffi: *mut WgFfi) {
    if ffi.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(ffi));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_free_string(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(value));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_search(ffi: *mut WgFfi, query: *const c_char) -> *mut c_char {
    WG_FFI_EXPORT!((|| -> Result<String, String> {
        let wrapper = unsafe { ffi.as_ref() }.ok_or_else(|| "null ffi pointer".to_string())?;
        let query = ptr_to_string(query)?;
        let results = wrapper
            .wiki
            .search(&query, SearchOpts::default())
            .map_err(|e| e.to_string())?;
        Ok(to_json_string(&results))
    })())
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_entity_list(ffi: *mut WgFfi) -> *mut c_char {
    WG_FFI_EXPORT!((|| -> Result<String, String> {
        let wrapper = unsafe { ffi.as_ref() }.ok_or_else(|| "null ffi pointer".to_string())?;
        let entities = wrapper
            .wiki
            .entity_list(ListOpts::default())
            .map_err(|e| e.to_string())?;
        Ok(to_json_string(&entities))
    })())
}

#[unsafe(no_mangle)]
pub extern "C" fn wg_fact_add(
    ffi: *mut WgFfi,
    entity_ids_json: *const c_char,
    content: *const c_char,
    fact_type: *const c_char,
    confidence: f32,
) -> *mut c_char {
    WG_FFI_EXPORT!((|| -> Result<String, String> {
        let wrapper = unsafe { ffi.as_ref() }.ok_or_else(|| "null ffi pointer".to_string())?;
        let entity_ids_json = ptr_to_string(entity_ids_json)?;
        let content = ptr_to_string(content)?;
        let fact_type = ptr_to_string(fact_type)?;
        let entity_ids = parse_entity_ids_json(&entity_ids_json)?;
        let id = wrapper
            .wiki
            .fact_add(FactInput {
                content,
                fact_type: Some(parse_fact_type(&fact_type)),
                entity_ids: Some(entity_ids),
                tags: None,
                source: None,
                source_confidence: Some(confidence),
                observed_at: None,
            })
            .map_err(|e| e.to_string())?;
        Ok(id.to_string())
    })())
}
