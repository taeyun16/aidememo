//! Node.js bindings for AideMemo (napi-rs).
//!
//! Read methods return JSON strings; the JS host calls `JSON.parse()` once.
//! This keeps the Rust surface tiny and lets us evolve schemas without
//! recompiling the bindings every time a field is added.

use aidememo_core::{
    AideMemo, AideMemoError, Config, EntityId, EntityInput, EntityType, FactId, FactInput,
    FactListOpts, FactType, ListOpts, QueryOpts, RelationInput, RelationType, SearchOpts,
    TraverseDirection, TraverseOpts, WorkflowStartOpts,
};
use napi::Status;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn generic_err<E: std::fmt::Display>(e: E) -> Error {
    Error::new(Status::GenericFailure, e.to_string())
}

fn invalid_arg<E: std::fmt::Display>(e: E) -> Error {
    Error::new(Status::InvalidArg, e.to_string())
}

fn map_err(e: AideMemoError) -> Error {
    let reason = format!("[{}] {e}", e.code());
    let status = match e {
        AideMemoError::EntityNotFound { .. }
        | AideMemoError::EntityIdNotFound(_)
        | AideMemoError::FactNotFound(_)
        | AideMemoError::RelationNotFound { .. }
        | AideMemoError::PathNotFound { .. }
        | AideMemoError::InvalidInput(_)
        | AideMemoError::EntityAlreadyExists { .. }
        | AideMemoError::ConfigKeyNotFound(_)
        | AideMemoError::FrontmatterParse { .. }
        | AideMemoError::WikilinkParse { .. }
        | AideMemoError::CycleDetected { .. }
        | AideMemoError::SchemaVersionMismatch { .. }
        | AideMemoError::UnsupportedSchemaVersion(_) => Status::InvalidArg,
        _ => Status::GenericFailure,
    };
    Error::new(status, reason)
}

fn to_json<T: serde::Serialize>(value: &T) -> napi::Result<String> {
    serde_json::to_string(value).map_err(generic_err)
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
    Some(FactType::parse(value))
}

fn parse_direction(value: Option<String>) -> TraverseDirection {
    match value.as_deref().map(|s| s.to_lowercase()).as_deref() {
        Some("forward") => TraverseDirection::Forward,
        Some("reverse") => TraverseDirection::Reverse,
        _ => TraverseDirection::Both,
    }
}

fn parse_entity_id(s: &str) -> napi::Result<EntityId> {
    EntityId::parse(s).ok_or_else(|| invalid_arg(format!("invalid entity id: {s}")))
}

fn parse_fact_id(s: &str) -> napi::Result<FactId> {
    FactId::parse(s).ok_or_else(|| invalid_arg(format!("invalid fact id: {s}")))
}

fn attach_session_entity(
    wiki: &AideMemo,
    entity_ids: &mut Vec<EntityId>,
    session_id: Option<String>,
) -> napi::Result<()> {
    let Some(session_id) = session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    let session_entity_id = wiki.resolve_entity(session_id).map_err(map_err)?;
    if !entity_ids.contains(&session_entity_id) {
        entity_ids.push(session_entity_id);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Optional argument bags (so JS callers can pass {} or omit fields)
// ---------------------------------------------------------------------------

#[napi(object)]
pub struct SearchArgs {
    pub limit: Option<u32>,
    pub min_confidence: Option<f64>,
    pub current_only: Option<bool>,
    pub source_id: Option<String>,
    pub as_of: Option<f64>,
    pub bm25_only: Option<bool>,
}

#[napi(object)]
pub struct QueryArgs {
    pub limit: Option<u32>,
    pub depth: Option<u32>,
    pub recent_limit: Option<u32>,
    pub current_only: Option<bool>,
    /// Retrieval strategy: "naive" | "local" | "hybrid" (default) | "global"
    pub mode: Option<String>,
    /// Skip the embedding-model load — pure BM25. Cuts cold-start
    /// latency at the cost of semantic recall.
    pub bm25_only: Option<bool>,
    pub source_id: Option<String>,
}

#[napi(object)]
pub struct WorkflowStartArgs {
    pub body: Option<String>,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub actor_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub limit: Option<u32>,
    pub depth: Option<u32>,
    pub recent_limit: Option<u32>,
    pub bm25_only: Option<bool>,
}

#[napi(object)]
pub struct TraverseArgs {
    pub depth: Option<u32>,
    pub direction: Option<String>,
}

#[napi(object)]
pub struct EntityAddArgs {
    pub entity_type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub aliases: Option<Vec<String>>,
    pub source_page: Option<String>,
}

#[napi(object)]
pub struct EntityListArgs {
    pub limit: Option<u32>,
    pub entity_type: Option<String>,
    pub min_facts: Option<u32>,
}

#[napi(object)]
pub struct FactAddArgs {
    pub entity_ids: Option<Vec<String>>,
    pub fact_type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub actor_id: Option<String>,
    pub session_id: Option<String>,
    pub confidence: Option<f64>,
}

/// Single item in a `factAddMany` batch.
#[napi(object)]
pub struct FactAddManyItem {
    pub content: String,
    pub entity_ids: Option<Vec<String>>,
    pub fact_type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub actor_id: Option<String>,
    pub session_id: Option<String>,
    pub confidence: Option<f64>,
}

#[napi(object)]
pub struct FactListArgs {
    pub entity: Option<String>,
    pub fact_type: Option<String>,
    pub min_confidence: Option<f64>,
    pub source_id: Option<String>,
    pub limit: Option<u32>,
    pub current_only: Option<bool>,
}

#[napi(object)]
pub struct StoreOpenArgs {
    /// Storage backend selector: "sqlite" or "libsqlite" in default builds;
    /// "redb" requires the Cargo `redb` feature.
    pub backend: Option<String>,
    pub durability: Option<String>,
}

#[napi(object)]
pub struct BranchPushArgs {
    /// Optional baseline backup directory. When present, only records after
    /// that backup manifest's sync cursor are exported.
    pub base: Option<String>,
}

#[napi(object)]
pub struct BranchMergeArgs {
    /// Merge only this branch id. Omit to merge every branch under source.
    pub branch: Option<String>,
}

// ---------------------------------------------------------------------------
// AideMemoStore — the napi class
// ---------------------------------------------------------------------------

#[napi]
pub struct AideMemoStore {
    wiki: Arc<AideMemo>,
}

#[napi]
impl AideMemoStore {
    #[napi(constructor)]
    pub fn new(store_path: String, args: Option<StoreOpenArgs>) -> napi::Result<Self> {
        let mut config = Config::default();
        if let Some(args) = args {
            if let Some(backend) = args
                .backend
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                config.set("store.backend", backend).map_err(map_err)?;
            }
            if let Some(durability) = args.durability {
                config
                    .set("store.durability", &durability)
                    .map_err(map_err)?;
            }
        }
        let wiki = AideMemo::open(Path::new(&store_path), config).map_err(map_err)?;
        Ok(Self {
            wiki: Arc::new(wiki),
        })
    }

    // === Search ===

    #[napi]
    pub fn search(&self, query: String, args: Option<SearchArgs>) -> napi::Result<String> {
        let args = args.unwrap_or(SearchArgs {
            limit: None,
            min_confidence: None,
            current_only: None,
            source_id: None,
            as_of: None,
            bm25_only: None,
        });
        let opts = SearchOpts {
            limit: args.limit.map(|v| v as usize),
            min_confidence: args.min_confidence.map(|v| v as f32),
            source_id: args.source_id,
            current_only: args.current_only.unwrap_or(false),
            as_of: args.as_of.map(|v| v as u64),
            bm25_only: args.bm25_only.unwrap_or(false),
            ..Default::default()
        };
        let results = self.wiki.hybrid_search(&query, opts).map_err(map_err)?;
        to_json(&results)
    }

    #[napi]
    pub fn query(&self, topic: String, args: Option<QueryArgs>) -> napi::Result<String> {
        let args = args.unwrap_or(QueryArgs {
            limit: None,
            depth: None,
            recent_limit: None,
            current_only: None,
            mode: None,
            bm25_only: None,
            source_id: None,
        });
        let opts = QueryOpts {
            search_limit: args.limit.unwrap_or(10) as usize,
            depth: args.depth.unwrap_or(2),
            recent_limit: args.recent_limit.unwrap_or(10) as usize,
            since: None,
            current_only: args.current_only.unwrap_or(false),
            mode: args
                .mode
                .as_deref()
                .map(aidememo_core::QueryMode::parse)
                .unwrap_or_default(),
            bm25_only: args.bm25_only.unwrap_or(false),
            source_id: args.source_id,
        };
        let result = self.wiki.query(&topic, opts).map_err(map_err)?;
        to_json(&result)
    }

    #[napi]
    pub fn workflow_start(
        &self,
        title: String,
        args: Option<WorkflowStartArgs>,
    ) -> napi::Result<String> {
        let args = args.unwrap_or(WorkflowStartArgs {
            body: None,
            source: None,
            source_id: None,
            actor_id: None,
            parent_session_id: None,
            limit: None,
            depth: None,
            recent_limit: None,
            bm25_only: None,
        });
        let result = self
            .wiki
            .workflow_start(
                &title,
                WorkflowStartOpts {
                    body: args.body,
                    source: args.source,
                    source_id: args.source_id,
                    actor_id: args.actor_id,
                    parent_session_id: args.parent_session_id,
                    limit: args.limit.unwrap_or(8) as usize,
                    depth: args.depth.unwrap_or(2),
                    recent_limit: args.recent_limit.unwrap_or(5) as usize,
                    bm25_only: args.bm25_only.unwrap_or(false),
                },
            )
            .map_err(map_err)?;
        to_json(&result)
    }

    // === Graph ===

    #[napi]
    pub fn traverse(&self, entity: String, args: Option<TraverseArgs>) -> napi::Result<String> {
        let args = args.unwrap_or(TraverseArgs {
            depth: None,
            direction: None,
        });
        let opts = TraverseOpts {
            depth: args.depth.unwrap_or(2),
            relation_types: None,
            direction: parse_direction(args.direction),
        };
        let result = self.wiki.traverse(&entity, opts).map_err(map_err)?;
        to_json(&result)
    }

    #[napi]
    pub fn path_find(&self, from: String, to: String) -> napi::Result<String> {
        let path = self.wiki.path_find(&from, &to).map_err(map_err)?;
        to_json(&path)
    }

    // === Entity CRUD ===

    #[napi]
    pub fn entity_add(&self, name: String, args: Option<EntityAddArgs>) -> napi::Result<String> {
        let args = args.unwrap_or(EntityAddArgs {
            entity_type: None,
            tags: None,
            aliases: None,
            source_page: None,
        });
        let input = EntityInput {
            name,
            entity_type: args.entity_type.as_deref().and_then(parse_entity_type),
            tags: args.tags,
            aliases: args.aliases,
            source_page: args.source_page,
        };
        let id = self.wiki.entity_add(input).map_err(map_err)?;
        Ok(id.to_string())
    }

    #[napi]
    pub fn entity_get(&self, name: String) -> napi::Result<String> {
        let entity = self.wiki.entity_get(&name).map_err(map_err)?;
        to_json(&entity)
    }

    #[napi]
    pub fn entity_list(&self, args: Option<EntityListArgs>) -> napi::Result<String> {
        let args = args.unwrap_or(EntityListArgs {
            limit: None,
            entity_type: None,
            min_facts: None,
        });
        let opts = ListOpts {
            entity_type: args.entity_type.as_deref().and_then(parse_entity_type),
            min_facts: args.min_facts,
            limit: args.limit.map(|v| v as usize),
            sort_by: Default::default(),
            offset: 0,
        };
        let entities = self.wiki.entity_list(opts).map_err(map_err)?;
        to_json(&entities)
    }

    #[napi]
    pub fn entity_delete(&self, name: String) -> napi::Result<()> {
        self.wiki.entity_delete(&name).map_err(map_err)
    }

    /// Set (or clear, with `""`) the entity's compiled-truth summary.
    #[napi]
    pub fn entity_describe(&self, name: String, summary: String) -> napi::Result<()> {
        self.wiki.entity_describe(&name, &summary).map_err(map_err)
    }

    #[napi]
    pub fn resolve_entity(&self, name: String) -> napi::Result<String> {
        let id = self.wiki.resolve_entity(&name).map_err(map_err)?;
        Ok(id.to_string())
    }

    // === Fact CRUD ===

    #[napi]
    pub fn fact_add(&self, content: String, args: Option<FactAddArgs>) -> napi::Result<String> {
        let args = args.unwrap_or(FactAddArgs {
            entity_ids: None,
            fact_type: None,
            tags: None,
            source: None,
            source_id: None,
            actor_id: None,
            session_id: None,
            confidence: None,
        });
        let mut ids = match args.entity_ids {
            Some(ids) => ids
                .iter()
                .map(|s| parse_entity_id(s))
                .collect::<napi::Result<Vec<_>>>()?,
            None => Vec::new(),
        };
        attach_session_entity(&self.wiki, &mut ids, args.session_id)?;
        let entity_ids = if ids.is_empty() { None } else { Some(ids) };
        let input = FactInput {
            content,
            fact_type: args.fact_type.as_deref().and_then(parse_fact_type),
            entity_ids,
            tags: args.tags,
            source: args.source,
            source_id: args.source_id,
            actor_id: args.actor_id,
            source_confidence: args.confidence.map(|v| v as f32),
            observed_at: None,
        };
        let id = self.wiki.fact_add(input).map_err(map_err)?;
        Ok(id.to_string())
    }

    /// Insert many facts in one backend transaction when supported. Returns the
    /// new fact ULIDs in input order. All-or-nothing — if any item
    /// fails to validate, no facts land.
    #[napi]
    pub fn fact_add_many(&self, items: Vec<FactAddManyItem>) -> napi::Result<Vec<String>> {
        let mut inputs = Vec::with_capacity(items.len());
        for item in items {
            let mut ids = match item.entity_ids {
                Some(ids) => ids
                    .iter()
                    .map(|s| parse_entity_id(s))
                    .collect::<napi::Result<Vec<_>>>()?,
                None => Vec::new(),
            };
            attach_session_entity(&self.wiki, &mut ids, item.session_id)?;
            let entity_ids = if ids.is_empty() { None } else { Some(ids) };
            inputs.push(FactInput {
                content: item.content,
                fact_type: item.fact_type.as_deref().and_then(parse_fact_type),
                entity_ids,
                tags: item.tags,
                source: item.source,
                source_id: item.source_id,
                actor_id: item.actor_id,
                source_confidence: item.confidence.map(|v| v as f32),
                observed_at: None,
            });
        }
        let ids = self.wiki.fact_add_many(inputs).map_err(map_err)?;
        Ok(ids.iter().map(|id| id.to_string()).collect())
    }

    #[napi]
    pub fn fact_get(&self, fact_id: String) -> napi::Result<String> {
        let id = parse_fact_id(&fact_id)?;
        let fact = self.wiki.fact_get(&id).map_err(map_err)?;
        to_json(&fact)
    }

    #[napi]
    pub fn pinned_facts(&self, limit: Option<u32>) -> napi::Result<String> {
        let facts = self
            .wiki
            .pinned_facts(limit.unwrap_or(10) as usize)
            .map_err(map_err)?;
        to_json(&facts)
    }

    #[napi]
    pub fn fact_pin(&self, fact_id: String, pinned: bool) -> napi::Result<()> {
        let id = parse_fact_id(&fact_id)?;
        self.wiki.fact_pin(&id, pinned).map_err(map_err)
    }

    #[napi]
    pub fn fact_list(&self, args: Option<FactListArgs>) -> napi::Result<String> {
        let args = args.unwrap_or(FactListArgs {
            entity: None,
            fact_type: None,
            min_confidence: None,
            source_id: None,
            limit: None,
            current_only: None,
        });
        let entity_id = match args.entity {
            Some(name) => Some(self.wiki.resolve_entity(&name).map_err(map_err)?),
            None => None,
        };
        let opts = FactListOpts {
            fact_type: args.fact_type.as_deref().and_then(parse_fact_type),
            entity_id,
            min_confidence: args.min_confidence.map(|v| v as f32),
            source_id: args.source_id,
            limit: args.limit.map(|v| v as usize),
            offset: 0,
            since: None,
            until: None,
            current_only: args.current_only.unwrap_or(false),
            as_of: None,
        };
        let facts = self.wiki.fact_list(opts).map_err(map_err)?;
        to_json(&facts)
    }

    #[napi]
    pub fn fact_supersede(&self, old_id: String, new_id: String) -> napi::Result<()> {
        let old = parse_fact_id(&old_id)?;
        let new = parse_fact_id(&new_id)?;
        self.wiki.fact_supersede(&old, &new).map_err(map_err)
    }

    #[napi]
    pub fn fact_delete(&self, fact_id: String) -> napi::Result<()> {
        let id = parse_fact_id(&fact_id)?;
        self.wiki.fact_delete(&id).map_err(map_err)
    }

    // === Relations ===

    #[napi]
    pub fn relation_add(
        &self,
        source: String,
        target: String,
        rel_type: String,
    ) -> napi::Result<()> {
        let input = RelationInput {
            source,
            target,
            relation_type: RelationType::new(rel_type),
            weight: None,
            evidence: None,
        };
        self.wiki.relation_add(input).map_err(map_err)
    }

    #[napi]
    pub fn relation_remove(
        &self,
        source: String,
        target: String,
        rel_type: String,
    ) -> napi::Result<()> {
        self.wiki
            .relation_remove(&source, &target, &rel_type)
            .map_err(map_err)
    }

    #[napi]
    pub fn relations_get(&self, entity: String, direction: Option<String>) -> napi::Result<String> {
        let dir = parse_direction(direction);
        let relations = self.wiki.relations_get(&entity, dir).map_err(map_err)?;
        to_json(&relations)
    }

    // === Ingest / Lint / Stats ===

    #[napi]
    pub fn ingest(&self, wiki_root: String, incremental: Option<bool>) -> napi::Result<String> {
        let stats = self
            .wiki
            .ingest(Path::new(&wiki_root), incremental.unwrap_or(false))
            .map_err(map_err)?;
        to_json(&stats)
    }

    #[napi]
    pub fn lint(&self) -> napi::Result<String> {
        let issues = self.wiki.lint().map_err(map_err)?;
        to_json(&issues)
    }

    #[napi]
    pub fn stats(&self) -> napi::Result<String> {
        let stats = self.wiki.stats().map_err(map_err)?;
        to_json(&stats)
    }

    // === Branch logs ===

    /// Push a local branch segment for this open store. For S3 branch targets,
    /// use the CLI build with `--features s3`.
    #[napi]
    pub fn branch_push(
        &self,
        branch: String,
        destination: String,
        args: Option<BranchPushArgs>,
    ) -> napi::Result<String> {
        let base = args.and_then(|args| args.base);
        if aidememo_core::backup::is_s3_uri(&destination)
            || base
                .as_deref()
                .is_some_and(aidememo_core::backup::is_s3_uri)
        {
            return Err(invalid_arg(
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
            &self.wiki,
            &branch,
            base_manifest.as_ref(),
            Path::new(&destination),
        )
        .map_err(map_err)?;
        to_json(&report)
    }

    /// Merge local branch segments into this open store. For S3 branch sources,
    /// use the CLI build with `--features s3`.
    #[napi]
    pub fn branch_merge(
        &self,
        source: String,
        args: Option<BranchMergeArgs>,
    ) -> napi::Result<String> {
        if aidememo_core::backup::is_s3_uri(&source) {
            return Err(invalid_arg(
                "S3 branch merge requires the aidememo CLI built with `--features s3`",
            ));
        }
        let branch = args.and_then(|args| args.branch);
        let report = aidememo_core::branch::merge_local_branches_for_wiki(
            &self.wiki,
            Path::new(&source),
            branch.as_deref(),
        )
        .map_err(map_err)?;
        to_json(&report)
    }
}

// ---------------------------------------------------------------------------
// Module version export
// ---------------------------------------------------------------------------

#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(all(test, any(feature = "sqlite", feature = "redb")))]
mod backend_binding_tests {
    use super::*;

    fn temp_store_path(name: &str, suffix: &str) -> std::path::PathBuf {
        let unique = format!(
            "aidememo-napi-{name}-{}-{}",
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

    fn assert_constructor_args_open_backend(
        name: &str,
        args: Option<StoreOpenArgs>,
        expected_backend: &str,
        suffix: &str,
    ) {
        let path = temp_store_path(name, suffix);
        let store =
            AideMemoStore::new(path.to_string_lossy().into_owned(), args).expect("open backend");
        assert_eq!(store.wiki.config().store.backend, expected_backend);

        let entity_id = store
            .wiki
            .entity_add(EntityInput {
                name: format!("Node{expected_backend}"),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .expect("entity add");
        store
            .wiki
            .fact_add(FactInput {
                content: format!("Node binding opened a {expected_backend} backend"),
                entity_ids: Some(vec![entity_id]),
                fact_type: Some(FactType::Note),
                ..Default::default()
            })
            .expect("fact add");

        let stats = store.wiki.stats().expect("stats");
        assert_eq!(stats.entity_count, 1);
        assert_eq!(stats.fact_count, 1);
        assert_backend_file(&path, expected_backend);
    }

    fn assert_constructor_opens_backend(name: &str, backend: &str, suffix: &str) {
        assert_constructor_args_open_backend(
            name,
            Some(StoreOpenArgs {
                backend: Some(backend.to_string()),
                durability: None,
            }),
            backend,
            suffix,
        );
    }

    #[test]
    fn constructor_without_backend_opens_default_backend() {
        let expected = expected_default_backend();
        assert_constructor_args_open_backend("default-open", None, expected, expected);
    }

    #[test]
    fn constructor_with_empty_backend_opens_default_backend() {
        let expected = expected_default_backend();
        assert_constructor_args_open_backend(
            "empty-backend-open",
            Some(StoreOpenArgs {
                backend: Some(String::new()),
                durability: None,
            }),
            expected,
            expected,
        );
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn constructor_opens_sqlite_backend() {
        assert_constructor_opens_backend("sqlite-open", "sqlite", "sqlite");
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn constructor_opens_libsqlite_alias() {
        assert_constructor_opens_backend("libsqlite-open", "libsqlite", "libsqlite");
    }

    #[cfg(feature = "redb")]
    #[test]
    fn constructor_opens_redb_backend() {
        assert_constructor_opens_backend("redb-open", "redb", "redb");
    }

    #[test]
    fn branch_push_merge_round_trips_with_open_handles() {
        let backend = expected_default_backend();
        let source_path = temp_store_path("branch-source", backend);
        let target_path = temp_store_path("branch-target", backend);
        let branches_dir = source_path
            .parent()
            .expect("source parent")
            .join("branch-log");
        let source = AideMemoStore::new(source_path.to_string_lossy().into_owned(), None)
            .expect("open source");
        let target = AideMemoStore::new(target_path.to_string_lossy().into_owned(), None)
            .expect("open target");

        let entity_id = source
            .wiki
            .entity_add(EntityInput {
                name: "NodeBranch".to_string(),
                entity_type: Some(EntityType::Concept),
                ..Default::default()
            })
            .expect("entity add");
        source
            .wiki
            .fact_add(FactInput {
                content: "Node binding branch fact".to_string(),
                entity_ids: Some(vec![entity_id]),
                fact_type: Some(FactType::Lesson),
                ..Default::default()
            })
            .expect("fact add");

        let push_json = source
            .branch_push(
                "node-smoke".to_string(),
                branches_dir.to_string_lossy().into_owned(),
                None,
            )
            .expect("branch push");
        let push: serde_json::Value = serde_json::from_str(&push_json).expect("push json");
        assert_eq!(push["branch_id"], "node-smoke");
        assert!(push["records_exported"].as_u64().expect("records") >= 2);

        let merge_json = target
            .branch_merge(
                branches_dir.to_string_lossy().into_owned(),
                Some(BranchMergeArgs {
                    branch: Some("node-smoke".to_string()),
                }),
            )
            .expect("branch merge");
        let merge: serde_json::Value = serde_json::from_str(&merge_json).expect("merge json");
        assert_eq!(merge["segments_merged"], 1);
        assert_eq!(merge["facts_inserted"], 1);
        let stats = target.wiki.stats().expect("target stats");
        assert_eq!(stats.entity_count, 1);
        assert_eq!(stats.fact_count, 1);

        let repeat_json = target
            .branch_merge(
                branches_dir.to_string_lossy().into_owned(),
                Some(BranchMergeArgs {
                    branch: Some("node-smoke".to_string()),
                }),
            )
            .expect("repeat branch merge");
        let repeat: serde_json::Value = serde_json::from_str(&repeat_json).expect("repeat json");
        assert_eq!(repeat["facts_inserted"], 0);
    }
}
