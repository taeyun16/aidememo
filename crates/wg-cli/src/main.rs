//! WikiGraph CLI — Structured index engine for LLM wikis.

mod cmd;
mod output;

use std::path::Path;
use std::process::exit;
use wg_core::{
    Config, EntityInput, EntitySort, EntityType, ExportScope, FactInput, FactListOpts, FactType,
    LintReport, ListOpts, QueryOpts, SearchOpts, TraverseDirection, TraverseOpts, WgError,
    WikiGraph,
};

// Hook into the global allocator so `wg bench` can report Rust-heap-only
// memory use (independent of mmap'd model weights / shared libs that
// dominate RSS). The wrapper is a thin pass-through; overhead is a single
// atomic add per allocation in release builds.
//
// `dhat-heap` feature swaps in dhat::Alloc instead — same #[global_allocator]
// slot, only one allocator per binary, so the two are mutually exclusive.
#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static PEAK_ALLOC: peak_alloc::PeakAlloc = peak_alloc::PeakAlloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static DHAT_ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    // dhat profiler: held for the entire process lifetime. On drop it
    // writes dhat-heap.json to the cwd. Use the online dh_view tool to
    // render allocation site → live-bytes treemap.
    #[cfg(feature = "dhat-heap")]
    let _dhat = dhat::Profiler::new_heap();

    init_tracing();

    let app = cmd::build_cli();

    let args = app.run();

    // Load config
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading config: {}", e);
            exit(1);
        }
    };

    // Resolve store path: --store > --project > default_project > store.path.
    let store_path = if let Some(path) = args.store_path.clone() {
        path
    } else if let Some(project) = &args.project {
        match config.project_path(project) {
            Some(p) => p,
            None => {
                eprintln!(
                    "Error: project '{}' not registered. Run `wg project list`.",
                    project
                );
                exit(1);
            }
        }
    } else {
        config.default_store_path()
    };

    let json = args.json;

    // Handle commands
    let result = match args.command {
        cmd::Command::Entity(sub) => handle_entity(&store_path, config, sub, json),
        cmd::Command::Fact(sub) => handle_fact(&store_path, config, sub, json),
        cmd::Command::Traverse(sub) => handle_traverse(&store_path, config, sub, json),
        cmd::Command::Path(sub) => handle_path(&store_path, config, sub, json),
        cmd::Command::Search(sub) => handle_search(&store_path, config, sub, json),
        cmd::Command::Query(sub) => handle_query(&store_path, config, sub, json),
        cmd::Command::Lint(sub) => handle_lint(&store_path, config, sub, json),
        cmd::Command::Doctor(sub) => cmd::doctor::run_doctor(&store_path, config, sub, json),
        cmd::Command::Recent(sub) => cmd::recent::run_recent(&store_path, config, sub, json),
        cmd::Command::Edit(sub) => cmd::edit::run_edit(&store_path, config, sub, json),
        cmd::Command::Graph(sub) => cmd::graph::run_graph(&store_path, config, sub),
        cmd::Command::Project(sub) => cmd::project::run_project(config, sub),
        cmd::Command::Bench(sub) => cmd::bench::run_bench(&store_path, config, sub, json),
        cmd::Command::Skill(sub) => cmd::skill::run_skill(sub, json),
        cmd::Command::Export(sub) => handle_export(&store_path, config, sub),
        cmd::Command::Import(sub) => handle_import(&store_path, config, sub),
        cmd::Command::Stats(sub) => handle_stats(&store_path, config, sub, json),
        cmd::Command::Ingest(sub) => handle_ingest(&store_path, config, sub),
        cmd::Command::Sync(sub) => handle_sync(&store_path, config, sub),
        cmd::Command::Config(sub) => handle_config(config, sub),
        cmd::Command::Model(sub) => handle_model(config, sub),
        cmd::Command::Feedback(sub) => cmd::feedback::run_feedback(&store_path, config, sub),
        cmd::Command::Adapt(sub) => cmd::adapt::run_adapt(&store_path, config, sub),
        cmd::Command::Init(sub) => cmd::init::run_init(sub.wiki_root, sub.no_ingest),
        cmd::Command::Watch(sub) => cmd::watch::run_watch(sub.wiki_root, sub.interval, sub.search),
        cmd::Command::McpServe(sub) => {
            // Mirror the Mcp arm — honour --store / --project unless
            // the user passed a positional WIKI_ROOT.
            let path = sub.wiki_root.unwrap_or_else(|| store_path.clone());
            cmd::mcp_serve::run_mcp_serve(sub.port, Some(path))
        }
        cmd::Command::Mcp(sub) => {
            // Honour the global --store / --project resolution if the
            // user didn't pass an explicit positional WIKI_ROOT.
            let path = sub.wiki_root.unwrap_or_else(|| store_path.clone());
            cmd::mcp_stdio::run_mcp(Some(path))
        }
        cmd::Command::McpInstall(sub) => cmd::mcp_install::run_mcp_install(sub, json),
        cmd::Command::Completions(sub) => cmd::completions::run_completions(sub),
        cmd::Command::Pending(sub) => cmd::pending::run_pending_review(sub),
        cmd::Command::VectorRebuild(sub) => handle_vector_rebuild(&store_path, config, sub),
        cmd::Command::Daemon(sub) => cmd::daemon::run_daemon(sub, store_path.clone()),
    };

    match result {
        Ok(output) => {
            println!("{}", output);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            // Add a daemon hint when the failure is a redb file-lock
            // collision and the user actually has a daemon registered.
            // Catches the most common foot-gun: user runs
            // `wg daemon start` and then their habitual `wg fact add …`
            // hits the lock because the daemon is holding it.
            let msg = e.to_string();
            if msg.contains("Database already open") || msg.contains("Cannot acquire lock") {
                use cmd::daemon::RegistryState;
                match cmd::daemon::registry_state(&store_path) {
                    RegistryState::Healthy(reg) => {
                        eprintln!(
                            "Hint: a wg daemon is running on port {} and is holding this \
                             store's lock. The CLI auto-dispatches read commands + \
                             fact-add through it; commands not yet daemon-aware will \
                             collide. Stop the daemon (`wg daemon stop`) if you need \
                             local access, or use the daemon-aware command.",
                            reg.port
                        );
                    }
                    RegistryState::StaleRegistry => {
                        eprintln!(
                            "Hint: ~/.wg/daemon.json exists but the daemon isn't \
                             responding. Run `wg daemon status` to inspect, or \
                             `wg daemon stop` to clear the stale registry."
                        );
                    }
                    RegistryState::None => {}
                }
            }
            exit(1);
        }
    }
}

/// Set up the global tracing subscriber.
///
/// Filter precedence:
///   1. `RUST_LOG` if set (standard convention)
///   2. `WG_LOG` (alias so users don't have to scope `RUST_LOG`)
///   3. default: `wg=info,wg_core=warn`
///      - `wg=info` matches the binary's module path (the bin is
///        named `wg` per Cargo.toml's `[[bin]] name = "wg"`, so the
///        target is `wg::cmd::*` not `wg_cli::cmd::*`). Surfaces
///        `wg mcp-serve` startup, `wg watch` file events.
///      - `wg_core=warn` for degraded-path warnings (missing HNSW
///        sidecar, reranker disabled) — quiet otherwise.
///
/// Output goes to stderr so stdout stays clean for `--json` consumers.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = std::env::var("RUST_LOG")
        .or_else(|_| std::env::var("WG_LOG"))
        .ok()
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new("wg=info,wg_core=warn"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_ansi(false) // structured stderr; downstream tools parse it
        .compact()
        .try_init()
        .ok(); // ignore if already installed (tests / repeated init)
}

fn with_wiki<F>(path: &Path, config: Config, f: F) -> Result<String, WgError>
where
    F: FnOnce(WikiGraph) -> Result<String, WgError>,
{
    let wiki = WikiGraph::open(path, config)?;
    f(wiki)
}

fn with_wiki_mut<F>(path: &Path, config: Config, f: F) -> Result<String, WgError>
where
    F: FnOnce(&mut WikiGraph) -> Result<String, WgError>,
{
    let mut wiki = WikiGraph::open(path, config)?;
    f(&mut wiki)
}

fn fmt(json: bool) -> output::Format {
    if json {
        output::Format::Json
    } else {
        output::Format::Table
    }
}

fn handle_entity(
    path: &Path,
    config: Config,
    sub: cmd::EntitySub,
    json: bool,
) -> Result<String, WgError> {
    match sub {
        cmd::EntitySub::Add {
            name,
            entity_type,
            tags,
            aliases,
            source_page,
        } => with_wiki_mut(path, config, |wiki| {
            let id = wiki.entity_add(EntityInput {
                name: name.clone(),
                entity_type: parse_entity_type(entity_type),
                tags: parse_tags(tags),
                aliases: parse_aliases(aliases),
                source_page,
            })?;
            Ok(format!("Added entity '{}' with ID {}", name, id))
        }),
        cmd::EntitySub::Get { name } => {
            // Daemon discovery — wg_entity_get tool returns JSON.
            // We forward the JSON directly so the user gets a single
            // self-contained record; the local table view of an entity
            // doesn't carry meaningfully more than the JSON does.
            if let Some(via) = cmd::daemon::registered_endpoint(path) {
                tracing::debug!(via = %via, "auto-discovered daemon for entity get");
                return run_entity_get_via_daemon(&via, &name);
            }
            with_wiki(path, config, |wiki| {
                let entity = wiki.entity_get(&name)?;
                output::format_entity(&entity, fmt(json))
            })
        }
        cmd::EntitySub::List {
            sort,
            entity_type,
            min_facts,
            limit,
        } => {
            if let Some(via) = cmd::daemon::registered_endpoint(path) {
                tracing::debug!(via = %via, "auto-discovered daemon for entity list");
                let url = format!("{}/mcp", via.trim_end_matches('/'));
                let mut args = serde_json::json!({});
                if let Some(l) = limit {
                    args["limit"] = serde_json::json!(l);
                }
                if let Some(t) = entity_type {
                    args["type"] = serde_json::json!(t);
                }
                let body = serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "method": "tools/call",
                    "params": {"name": "wg_entity_list", "arguments": args}
                });
                return daemon_tool_call(&url, body, "wg_entity_list");
            }
            // Local path — uses sort + min_facts the tool doesn't expose.
            with_wiki(path, config, |wiki| {
                let entities = wiki.entity_list(ListOpts {
                    entity_type: parse_entity_type(entity_type),
                    min_facts,
                    sort_by: parse_entity_sort(sort),
                    limit,
                    offset: 0,
                })?;
                output::format_entity_list(&entities, fmt(json))
            })
        }
        cmd::EntitySub::Rename { old_name, new_name } => with_wiki_mut(path, config, |wiki| {
            wiki.entity_rename(&old_name, &new_name)?;
            Ok(format!("Renamed '{}' to '{}'", old_name, new_name))
        }),
        cmd::EntitySub::Alias {
            name,
            alias,
            action,
        } => with_wiki_mut(path, config, |wiki| match action {
            cmd::AliasAction::Add => {
                wiki.entity_alias_add(&name, &alias)?;
                Ok(format!("Added alias '{}' to entity '{}'", alias, name))
            }
        }),
        cmd::EntitySub::Delete { name } => with_wiki_mut(path, config, |wiki| {
            wiki.entity_delete(&name)?;
            Ok(format!("Deleted entity '{}'", name))
        }),
        cmd::EntitySub::Describe {
            from_stdin,
            clear,
            content,
            name,
        } => with_wiki_mut(path, config, |wiki| {
            let summary = if clear {
                String::new()
            } else if from_stdin {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| WgError::Internal(format!("stdin read: {}", e)))?;
                buf.trim().to_string()
            } else {
                content.clone().ok_or_else(|| {
                    WgError::InvalidInput("describe needs CONTENT, --from-stdin, or --clear".into())
                })?
            };
            wiki.entity_describe(&name, &summary)?;
            if summary.is_empty() {
                Ok(format!("Cleared summary for '{}'", name))
            } else {
                Ok(format!(
                    "Updated summary for '{}' ({} chars)",
                    name,
                    summary.len()
                ))
            }
        }),
        cmd::EntitySub::Show { recent, name } => with_wiki(path, config, |wiki| {
            let entity = wiki.entity_get(&name)?;
            let recent_n = recent.unwrap_or(5);
            let facts = wiki.fact_list(FactListOpts {
                entity_id: Some(entity.id),
                limit: Some(recent_n),
                current_only: false,
                ..Default::default()
            })?;
            output::format_entity_show(&entity, &facts, fmt(json))
        }),
    }
}

fn handle_fact(
    path: &Path,
    config: Config,
    sub: cmd::FactSub,
    json: bool,
) -> Result<String, WgError> {
    match sub {
        cmd::FactSub::Add {
            content,
            fact_type,
            entities,
            tags,
            source,
            confidence,
            observed_at,
        } => {
            let observed_at_ms = match observed_at.as_deref() {
                Some(s) => Some(parse_iso_to_epoch_ms(s)?),
                None => None,
            };
            // Daemon discovery — if a `wg daemon` is running on the
            // same store, dispatch through it. Otherwise we'd hit the
            // single-writer redb lock on the daemon's open handle.
            // The daemon has wg_fact_add as a first-class MCP tool so
            // the path is symmetric with read commands.
            if let Some(via) = cmd::daemon::registered_endpoint(path) {
                tracing::debug!(via = %via, "auto-discovered daemon for fact add");
                return run_fact_add_via_daemon(
                    &via,
                    &content,
                    entities.as_ref(),
                    tags.as_ref(),
                    json,
                );
            }
            with_wiki_mut(path, config, |wiki| {
                let mut auto_created: Vec<String> = Vec::new();
                let entity_ids = match entities {
                    Some(names) => {
                        // Allow comma-separated names in a single --entities flag.
                        let names: Vec<String> = names
                            .into_iter()
                            .flat_map(|raw| {
                                raw.split(',')
                                    .map(|s| s.trim().to_string())
                                    .collect::<Vec<_>>()
                            })
                            .filter(|s| !s.is_empty())
                            .collect();
                        let mut ids = Vec::new();
                        for name in &names {
                            match wiki.resolve_entity(name) {
                                Ok(id) => ids.push(id),
                                Err(_) => {
                                    let new_id = wiki.entity_add(EntityInput {
                                        name: name.clone(),
                                        entity_type: Some(EntityType::Unknown),
                                        ..Default::default()
                                    })?;
                                    auto_created.push(name.clone());
                                    ids.push(new_id);
                                }
                            }
                        }
                        Some(ids)
                    }
                    None => None,
                };

                let id = wiki.add_fact(FactInput {
                    content: content.clone(),
                    fact_type: parse_fact_type(fact_type),
                    entity_ids,
                    tags: parse_tags(tags),
                    source,
                    source_confidence: confidence,
                    observed_at: observed_at_ms,
                })?;
                if json {
                    // Stable shape for programmatic callers (the
                    // hermes-wg plugin and any other client that
                    // would otherwise have to grep for a ULID in
                    // free-form prose).
                    let payload = serde_json::json!({
                        "id": id.to_string(),
                        "auto_created_entities": auto_created,
                    });
                    return serde_json::to_string_pretty(&payload).map_err(|e| {
                        WgError::Serialize {
                            context: "fact add (json)".to_string(),
                            source: e,
                        }
                    });
                }
                let mut msg = format!("Added fact with ID {}", id);
                if !auto_created.is_empty() {
                    let label = if auto_created.len() == 1 {
                        "entity"
                    } else {
                        "entities"
                    };
                    msg.push_str(&format!(
                        "\n  auto-created {}: {}",
                        label,
                        auto_created.join(", ")
                    ));
                }
                Ok(msg)
            })
        }
        cmd::FactSub::Get { id } => {
            // Same daemon-aware fast path as entity get — wg_fact_get
            // tool returns a JSON Fact record we forward verbatim.
            if let Some(via) = cmd::daemon::registered_endpoint(path) {
                tracing::debug!(via = %via, "auto-discovered daemon for fact get");
                return run_fact_get_via_daemon(&via, &id);
            }
            with_wiki(path, config, |wiki| {
                let fact_id = wg_core::FactId(
                    wg_core::ulid::Ulid::from_string(&id)
                        .map_err(|_| WgError::InvalidInput(format!("Invalid fact ID: {}", id)))?,
                );
                let fact = wiki.fact_get(&fact_id)?;
                output::format_fact(&fact, &wiki, fmt(json))
            })
        }
        cmd::FactSub::List {
            fact_type,
            entity,
            min_confidence,
            since,
            until,
            last,
            as_of,
            limit,
        } => {
            let since_ms = resolve_since(since.as_deref(), last.as_deref())?;
            let until_ms = resolve_until(until.as_deref())?;
            let as_of_ms = match as_of.as_deref() {
                Some(d) => Some(parse_iso_to_epoch_ms(d)?),
                None => None,
            };
            // Daemon dispatch only when the user didn't pass any
            // filter the wg_fact_list tool doesn't expose. Anything
            // else falls through to the in-process path so we never
            // silently drop a filter.
            let simple = fact_type.is_none()
                && min_confidence.is_none()
                && since_ms.is_none()
                && until_ms.is_none()
                && as_of_ms.is_none();
            if simple {
                if let Some(via) = cmd::daemon::registered_endpoint(path) {
                    tracing::debug!(via = %via, "auto-discovered daemon for fact list");
                    let url = format!("{}/mcp", via.trim_end_matches('/'));
                    let mut args = serde_json::json!({"limit": limit.unwrap_or(20)});
                    if let Some(e) = entity.as_ref() {
                        args["entity"] = serde_json::json!(e);
                    }
                    let body = serde_json::json!({
                        "jsonrpc": "2.0", "id": 1,
                        "method": "tools/call",
                        "params": {"name": "wg_fact_list", "arguments": args}
                    });
                    return daemon_tool_call(&url, body, "wg_fact_list");
                }
            }
            with_wiki(path, config, |wiki| {
                let entity_id = entity.and_then(|n| wiki.resolve_entity(&n).ok());
                let facts = wiki.fact_list(FactListOpts {
                    fact_type: parse_fact_type(fact_type),
                    entity_id,
                    min_confidence,
                    limit,
                    offset: 0,
                    since: since_ms,
                    until: until_ms,
                    current_only: false,
                    as_of: as_of_ms,
                })?;
                output::format_fact_list(&facts, &wiki, fmt(json))
            })
        }
        cmd::FactSub::Delete { id } => with_wiki_mut(path, config, |wiki| {
            let fact_id = wg_core::FactId(
                wg_core::ulid::Ulid::from_string(&id)
                    .map_err(|_| WgError::InvalidInput(format!("Invalid fact ID: {}", id)))?,
            );
            wiki.fact_delete(&fact_id)?;
            Ok(format!("Deleted fact {}", id))
        }),
        cmd::FactSub::Feedback { id, helpful } => with_wiki_mut(path, config, |wiki| {
            let fact_id = wg_core::FactId(
                wg_core::ulid::Ulid::from_string(&id)
                    .map_err(|_| WgError::InvalidInput(format!("Invalid fact ID: {}", id)))?,
            );
            wiki.fact_feedback(&fact_id, helpful)?;
            Ok(format!(
                "Recorded {} feedback for fact {}",
                if helpful { "helpful" } else { "not helpful" },
                id
            ))
        }),
        cmd::FactSub::Supersede { old_id, new_id } => {
            // Daemon discovery — wg_fact_supersede MCP tool exists.
            if let Some(via) = cmd::daemon::registered_endpoint(path) {
                tracing::debug!(via = %via, "auto-discovered daemon for fact supersede");
                return run_fact_supersede_via_daemon(&via, &old_id, &new_id);
            }
            with_wiki_mut(path, config, |wiki| {
                let parse = |s: &str| {
                    wg_core::FactId(
                        wg_core::ulid::Ulid::from_string(s)
                            .map_err(|_| WgError::InvalidInput(format!("Invalid fact ID: {s}")))
                            .unwrap_or_default(),
                    )
                };
                let old = parse(&old_id);
                let new = parse(&new_id);
                wiki.fact_supersede(&old, &new)?;
                Ok(format!("Superseded {old_id} by {new_id}"))
            })
        }
    }
}

/// `wg entity get NAME` daemon path. wg_entity_get returns the
/// entity record as JSON; we forward it verbatim.
fn run_entity_get_via_daemon(base_url: &str, name: &str) -> Result<String, WgError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "wg_entity_get", "arguments": {"name": name}}
    });
    daemon_tool_call(&url, body, "wg_entity_get")
}

/// `wg fact get ID` daemon path. Symmetric with entity get.
fn run_fact_get_via_daemon(base_url: &str, id: &str) -> Result<String, WgError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "wg_fact_get", "arguments": {"id": id}}
    });
    daemon_tool_call(&url, body, "wg_fact_get")
}

/// `wg fact supersede` daemon path. wg_fact_supersede tool returns
/// `{"old_id": "...", "new_id": "..."}` JSON; we re-pack the same
/// "Superseded X by Y" line the local path emits.
fn run_fact_supersede_via_daemon(
    base_url: &str,
    old_id: &str,
    new_id: &str,
) -> Result<String, WgError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {
            "name": "wg_fact_supersede",
            "arguments": {"old_id": old_id, "new_id": new_id}
        }
    });
    let _ = daemon_tool_call(&url, body, "wg_fact_supersede")?;
    Ok(format!("Superseded {old_id} by {new_id}"))
}

fn handle_traverse(
    path: &Path,
    config: Config,
    sub: cmd::TraverseSub,
    json: bool,
) -> Result<String, WgError> {
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon for traverse");
        let url = format!("{}/mcp", via.trim_end_matches('/'));
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {
                "name": "wg_traverse",
                "arguments": {"entity": sub.entity, "depth": sub.depth.unwrap_or(2)}
            }
        });
        return daemon_tool_call(&url, body, "wg_traverse");
    }
    with_wiki(path, config, |wiki| {
        let result = wiki.traverse(
            &sub.entity,
            TraverseOpts {
                depth: sub.depth.unwrap_or(2),
                relation_types: None,
                direction: TraverseDirection::Forward,
            },
        )?;
        output::format_traverse(&result, fmt(json))
    })
}

fn handle_path(
    path: &Path,
    config: Config,
    sub: cmd::PathSub,
    json: bool,
) -> Result<String, WgError> {
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon for path");
        let url = format!("{}/mcp", via.trim_end_matches('/'));
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {
                "name": "wg_path",
                "arguments": {"from": sub.from, "to": sub.to}
            }
        });
        return daemon_tool_call(&url, body, "wg_path");
    }
    with_wiki(path, config, |wiki| {
        let path_steps = wiki.path_find(&sub.from, &sub.to)?;
        if json {
            return serde_json::to_string_pretty(&serde_json::json!({
                "from": &sub.from,
                "to": &sub.to,
                "steps": path_steps,
            }))
            .map_err(|e| WgError::Serialize {
                context: "path".to_string(),
                source: e,
            });
        }
        match path_steps {
            Some(steps) => {
                let mut out = String::new();
                out.push_str(&format!("Path from '{}' to '{}':\n", sub.from, sub.to));
                for (i, step) in steps.iter().enumerate() {
                    let from_name = wiki
                        .entity_get_by_id(step.from)
                        .map(|e| e.name)
                        .unwrap_or_default();
                    let to_name = wiki
                        .entity_get_by_id(step.to)
                        .map(|e| e.name)
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "  {}: {} --{}--> {}\n",
                        i + 1,
                        from_name,
                        step.relation_type,
                        to_name
                    ));
                }
                Ok(out)
            }
            None => Ok(format!("No path found from '{}' to '{}'", sub.from, sub.to)),
        }
    })
}

fn handle_search(
    path: &Path,
    config: Config,
    sub: cmd::SearchSub,
    json: bool,
) -> Result<String, WgError> {
    let default_limit = config.search.default_limit;
    let bm25_weight = config.search.bm25_weight;
    let semantic_weight = config.search.semantic_weight;
    let since_ms = resolve_since(sub.since.as_deref(), sub.last.as_deref())?;
    let until_ms = resolve_until(sub.until.as_deref())?;
    let as_of_ms = match sub.as_of.as_deref() {
        Some(d) => Some(parse_iso_to_epoch_ms(d)?),
        None => None,
    };

    if sub.all_projects {
        return run_search_all_projects(
            config,
            sub,
            since_ms,
            until_ms,
            default_limit,
            bm25_weight,
            semantic_weight,
            json,
        );
    }

    // Explicit --via wins over discovery.
    if let Some(ref via) = sub.via {
        return run_search_via_daemon(via, &sub, default_limit, json);
    }
    // Opportunistic discovery: if a `wg daemon` is running and serving
    // the same store, dispatch through it so we skip the local redb
    // open + (for hybrid) model load. Set WG_NO_DAEMON=1 to bypass.
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon");
        return run_search_via_daemon(&via, &sub, default_limit, json);
    }

    with_wiki_mut(path, config, |wiki| {
        let session_id = wg_core::ulid::Ulid::new().to_string();
        // CLI default = BM25 (fast). `--hybrid` opts into the
        // semantic path; `semantic_weight = 0` in config also forces
        // BM25 even with the flag (caller-on-caller override).
        let bm25_only = !sub.hybrid || semantic_weight == 0.0;
        let opts = SearchOpts {
            limit: sub.limit.or(Some(default_limit)),
            min_confidence: sub.min_confidence,
            entity_filter: None,
            bm25_weight,
            semantic_weight,
            since: since_ms,
            until: until_ms,
            session_id: Some(session_id.clone()),
            current_only: false,
            as_of: as_of_ms,
            bm25_only,
        };

        let results = if let Some(ref start) = sub.traverse_from {
            let depth = sub.traverse_depth.unwrap_or(2);
            wiki.search_with_traverse(&sub.query, start, depth, opts)?
        } else {
            wiki.hybrid_search(&sub.query, opts)?
        };

        let search_session = wg_core::SearchSession {
            id: session_id,
            query: sub.query.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| WgError::Internal(format!("system clock error: {}", e)))?
                .as_millis() as u64,
            result_count: results.len(),
        };
        wiki.search_session_add(&search_session)?;

        let format = if sub.json || json {
            output::Format::Json
        } else {
            output::Format::Table
        };
        output::format_search_results(&results, wiki, format)
    })
}

/// `wg search --via http://host:port` — dispatch the search as a
/// JSON-RPC `wg_search` tool call against a running `wg mcp-serve`
/// daemon. The daemon keeps the redb store and the embedding model
/// warm, so the round-trip is dominated by HTTP latency (typically
/// ~10–20 ms on localhost).
///
/// Output is the daemon's `wg_search` content block dumped verbatim —
/// the daemon's tool already formats hits the way agents consume
/// them. We don't try to reformat it as the local table view because
/// the daemon may legitimately have a different store / config than
/// the calling CLI's project.
fn run_search_via_daemon(
    base_url: &str,
    sub: &cmd::SearchSub,
    default_limit: usize,
    _json: bool,
) -> Result<String, WgError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let limit = sub.limit.unwrap_or(default_limit);
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {
            "name": "wg_search",
            "arguments": {
                "query": sub.query,
                "limit": limit,
                // CLI default = BM25; --hybrid flips it on. Daemon
                // honours the same opt-in semantics.
                "bm25_only": !sub.hybrid,
            }
        }
    });
    let resp: serde_json::Value = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| WgError::Internal(format!("daemon POST {url} failed: {e}")))?
        .into_json()
        .map_err(|e| WgError::Internal(format!("daemon response parse: {e}")))?;
    if let Some(err) = resp.get("error") {
        return Err(WgError::Internal(format!("daemon error: {err}")));
    }
    let text = resp
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WgError::Internal(format!("unexpected daemon response: {resp}")))?;
    Ok(text.to_string())
}

/// Run a hybrid search across every registered project, tag each hit with
/// its project name, merge by score, and render. Used by `wg search
/// --all-projects` (Tier 5-A.3 — Basic Memory issue #123).
#[allow(clippy::too_many_arguments)]
fn run_search_all_projects(
    config: Config,
    sub: cmd::SearchSub,
    since_ms: Option<u64>,
    until_ms: Option<u64>,
    default_limit: usize,
    bm25_weight: f32,
    semantic_weight: f32,
    json: bool,
) -> Result<String, WgError> {
    if config.projects.is_empty() {
        return Err(WgError::InvalidInput(
            "no projects registered — `wg project create` first".into(),
        ));
    }
    let as_of_ms = match sub.as_of.as_deref() {
        Some(d) => Some(parse_iso_to_epoch_ms(d)?),
        None => None,
    };
    let limit = sub.limit.or(Some(default_limit));
    let mut all: Vec<(String, wg_core::SearchResult)> = Vec::new();

    for proj_name in config.projects.keys() {
        let store_path = match config.project_path(proj_name) {
            Some(p) => p,
            None => continue,
        };
        let wiki = match WikiGraph::open(&store_path, config.clone()) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(
                    project = %proj_name,
                    store = %store_path.display(),
                    "[wg search] skipping project: {e}",
                );
                continue;
            }
        };
        let opts = SearchOpts {
            limit,
            min_confidence: sub.min_confidence,
            entity_filter: None,
            bm25_weight,
            semantic_weight,
            since: since_ms,
            until: until_ms,
            session_id: None,
            current_only: false,
            as_of: as_of_ms,
            bm25_only: !sub.hybrid || semantic_weight == 0.0,
        };
        match wiki.hybrid_search(&sub.query, opts) {
            Ok(hits) => {
                for h in hits {
                    all.push((proj_name.clone(), h));
                }
            }
            Err(e) => {
                tracing::warn!(project = %proj_name, "[wg search] project search failed: {e}")
            }
        }
    }

    // Sort by score descending across the merged set, then re-rank.
    all.sort_by(|a, b| {
        b.1.score
            .partial_cmp(&a.1.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if let Some(l) = limit {
        all.truncate(l);
    }
    for (i, (_p, r)) in all.iter_mut().enumerate() {
        r.rank = i + 1;
    }

    if sub.json || json {
        let payload: Vec<_> = all
            .iter()
            .map(|(p, r)| {
                serde_json::json!({
                    "project": p,
                    "result": r,
                })
            })
            .collect();
        return serde_json::to_string_pretty(&payload).map_err(|e| WgError::Serialize {
            context: "all-projects search".to_string(),
            source: e,
        });
    }

    let mut out = String::new();
    out.push_str(&format!(
        "Search '{}' across {} project(s) — {} hit(s)\n\n",
        sub.query,
        config.projects.len(),
        all.len()
    ));
    for (proj, r) in &all {
        let snippet: String = r.content.chars().take(60).collect();
        out.push_str(&format!(
            "  [{}] [{}] {}  (score={:.3})\n",
            proj, r.fact_id, snippet, r.score
        ));
    }
    Ok(out)
}

fn handle_query(
    path: &Path,
    config: Config,
    sub: cmd::QuerySub,
    json: bool,
) -> Result<String, WgError> {
    let since_ms = resolve_since(None, sub.last.as_deref())?;
    let mode = sub
        .mode
        .as_deref()
        .map(wg_core::QueryMode::parse)
        .unwrap_or_default();
    // Same opportunistic-discovery shortcut as `wg search`. The wg_query
    // tool returns JSON, so the daemon's text content is already in
    // the shape `wg query --json` would print; for the table view we
    // pretty-print whatever the daemon returned (the formatter expects
    // an in-memory QueryResult, but a JSON dump is the next-best thing
    // for a one-shot CLI use of `--via`).
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon for query");
        return run_query_via_daemon(&via, &sub);
    }
    with_wiki(path, config, |wiki| {
        let opts = QueryOpts {
            search_limit: sub.limit.unwrap_or(10),
            depth: sub.depth.unwrap_or(2),
            recent_limit: sub.recent_limit.unwrap_or(10),
            since: since_ms,
            current_only: false,
            mode,
            bm25_only: false,
        };
        let result = wiki.query(&sub.topic, opts)?;
        if json {
            return serde_json::to_string_pretty(&result).map_err(|e| WgError::Serialize {
                context: "query".to_string(),
                source: e,
            });
        }
        output::format_query_result(&result)
    })
}

/// `wg query` daemon path. Calls the `wg_query` MCP tool and prints
/// its JSON response verbatim (the tool already JSON-encodes the
/// QueryResult; reformatting it as a local table would require
/// deserialising into the wg-core type, which costs more code than
/// it's worth for the warm-path one-shot.)
fn run_query_via_daemon(base_url: &str, sub: &cmd::QuerySub) -> Result<String, WgError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let mode = sub.mode.clone().unwrap_or_else(|| "hybrid".to_string());
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {
            "name": "wg_query",
            "arguments": {
                "topic": sub.topic,
                "limit": sub.limit.unwrap_or(10),
                "depth": sub.depth.unwrap_or(2),
                "recent_limit": sub.recent_limit.unwrap_or(10),
                "mode": mode,
            }
        }
    });
    daemon_tool_call(&url, body, "wg_query")
}

/// `wg fact add` daemon path. Calls the wg_fact_add MCP tool which
/// returns `{id, content, entity_names, created_at, auto_created_entities}`.
/// The local CLI surface returns either JSON or a "Added fact with ID …"
/// line; we normalise here so users see the same output whether the
/// daemon is on or off.
fn run_fact_add_via_daemon(
    base_url: &str,
    content: &str,
    entities: Option<&Vec<String>>,
    tags: Option<&Vec<String>>,
    json: bool,
) -> Result<String, WgError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    // Mirror the CLI's "comma-separated names in a single flag" shape.
    let entity_names: Vec<String> = entities
        .map(|v| {
            v.iter()
                .flat_map(|raw| raw.split(',').map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let tag_list: Vec<String> = tags
        .map(|v| {
            v.iter()
                .flat_map(|raw| raw.split(',').map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut args = serde_json::json!({ "content": content });
    if !entity_names.is_empty() {
        args["entities"] = serde_json::json!(entity_names);
    }
    if !tag_list.is_empty() {
        args["tags"] = serde_json::json!(tag_list);
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "wg_fact_add", "arguments": args}
    });
    let raw = daemon_tool_call(&url, body, "wg_fact_add")?;
    let payload: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| WgError::Internal(format!("wg_fact_add response parse: {e}")))?;
    let id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WgError::Internal(format!("wg_fact_add response missing id: {raw}")))?;
    let auto_created: Vec<String> = payload
        .get("auto_created_entities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if json {
        return Ok(serde_json::json!({
            "id": id,
            "auto_created_entities": auto_created,
        })
        .to_string());
    }
    let mut msg = format!("Added fact with ID {id}");
    if !auto_created.is_empty() {
        msg.push_str(&format!(
            "\nAuto-created entities: {}",
            auto_created.join(", ")
        ));
    }
    Ok(msg)
}

/// Shared helper for `--via` daemon dispatch: POST a JSON-RPC tool
/// call and unwrap the text content. Used by both wg search and
/// wg query / wg recent. Errors carry the daemon URL so the user
/// can tell whether the daemon, the network, or the tool itself
/// failed.
fn daemon_tool_call(url: &str, body: serde_json::Value, tool: &str) -> Result<String, WgError> {
    let resp: serde_json::Value = ureq::post(url)
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| WgError::Internal(format!("daemon POST {url} failed: {e}")))?
        .into_json()
        .map_err(|e| WgError::Internal(format!("daemon response parse: {e}")))?;
    if let Some(err) = resp.get("error") {
        return Err(WgError::Internal(format!("daemon {tool} error: {err}")));
    }
    let text = resp
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WgError::Internal(format!("unexpected daemon {tool} response: {resp}")))?;
    Ok(text.to_string())
}

fn handle_lint(
    path: &Path,
    config: Config,
    sub: cmd::LintSub,
    json: bool,
) -> Result<String, WgError> {
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon for lint");
        let url = format!("{}/mcp", via.trim_end_matches('/'));
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {"name": "wg_lint", "arguments": {}}
        });
        return daemon_tool_call(&url, body, "wg_lint");
    }
    with_wiki(path, config, |wiki| {
        let issues = wiki.lint()?;
        let stats = wiki.stats()?;
        let report = LintReport {
            issues,
            entity_count: stats.entity_count,
            fact_count: stats.fact_count,
            relation_count: stats.relation_count,
        };
        let format = if sub.json || json {
            output::Format::Json
        } else {
            output::Format::Table
        };
        output::format_lint_report(&report, format)
    })
}

fn handle_export(path: &Path, config: Config, sub: cmd::ExportSub) -> Result<String, WgError> {
    with_wiki(path, config, |wiki| {
        let scope = match sub.scope.as_deref() {
            Some("entities") => ExportScope::Entities,
            Some("relations") => ExportScope::Relations,
            Some("facts") => ExportScope::Facts,
            _ => ExportScope::All,
        };

        let mut output = Vec::new();
        let stats = wiki.export_jsonl(&mut output, scope)?;
        let content = String::from_utf8(output).map_err(|e| WgError::Internal(e.to_string()))?;

        if sub.output.is_some() {
            if let Some(path) = sub.output {
                std::fs::write(&path, &content)
                    .map_err(|e| WgError::Internal(format!("Failed to write file: {}", e)))?;
                Ok(format!(
                    "Exported to {}: {} entities, {} relations, {} facts",
                    path.display(),
                    stats.entities_exported,
                    stats.relations_exported,
                    stats.facts_exported
                ))
            } else {
                Ok(content)
            }
        } else {
            Ok(content)
        }
    })
}

fn handle_import(path: &Path, config: Config, sub: cmd::ImportSub) -> Result<String, WgError> {
    with_wiki_mut(path, config, |wiki| {
        let content = if let Some(p) = sub.path.as_ref() {
            std::fs::read_to_string(p)
                .map_err(|e| WgError::Internal(format!("Failed to read file: {}", e)))?
        } else {
            // Read from stdin
            use std::io::Read;
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .map_err(|e| WgError::Internal(format!("Failed to read stdin: {}", e)))?;
            input
        };

        let mut reader = std::io::Cursor::new(content);
        let stats = wiki.import_jsonl(&mut reader)?;
        Ok(format!(
            "Imported: {} entities, {} relations, {} facts ({} errors)",
            stats.entities_imported, stats.relations_imported, stats.facts_imported, stats.errors
        ))
    })
}

fn handle_stats(
    path: &Path,
    config: Config,
    _sub: cmd::StatsSub,
    json: bool,
) -> Result<String, WgError> {
    with_wiki(path, config, |wiki| {
        let stats = wiki.stats()?;
        output::format_stats(&stats, fmt(json))
    })
}

fn handle_vector_rebuild(
    path: &Path,
    config: Config,
    sub: cmd::VectorRebuildSub,
) -> Result<String, WgError> {
    with_wiki(path, config, |wiki| {
        let started = std::time::Instant::now();
        let count = wiki.vector_index_rebuild()?;
        let elapsed_ms = started.elapsed().as_millis();

        if sub.json {
            let payload = serde_json::json!({
                "facts_indexed": count,
                "elapsed_ms": elapsed_ms,
            });
            return serde_json::to_string_pretty(&payload).map_err(|e| WgError::Serialize {
                context: "vector-rebuild".to_string(),
                source: e,
            });
        }
        if count == 0 {
            Ok(format!(
                "No facts to index — sidecar removed (took {} ms).",
                elapsed_ms
            ))
        } else {
            Ok(format!(
                "Rebuilt HNSW index over {} facts in {} ms.",
                count, elapsed_ms
            ))
        }
    })
}

fn handle_ingest(path: &Path, config: Config, sub: cmd::IngestSub) -> Result<String, WgError> {
    let wiki = WikiGraph::open(path, config)?;
    let stats = wiki.ingest(&sub.wiki_root, sub.incremental)?;

    let mut lines = vec![format!(
        "Ingest complete: {} files scanned, +{} entities, +{} relations, +{} facts",
        stats.files_scanned, stats.entities_added, stats.relations_added, stats.facts_added
    )];
    if stats.entities_updated > 0 {
        lines.push(format!(
            "  ({} entities refreshed from frontmatter)",
            stats.entities_updated
        ));
    }
    if !stats.errors.is_empty() {
        lines.push(format!("  {} errors (see logs)", stats.errors.len()));
        for e in stats.errors.iter().take(5) {
            lines.push(format!("    - {}", e));
        }
    }
    Ok(lines.join("\n"))
}

fn handle_sync(path: &Path, config: Config, sub: cmd::SyncSub) -> Result<String, WgError> {
    // sync is an alias for ingest --incremental
    handle_ingest(
        path,
        config,
        cmd::IngestSub {
            wiki_root: sub.wiki_root,
            incremental: true,
        },
    )
}

fn handle_config(config: Config, sub: cmd::ConfigSub) -> Result<String, WgError> {
    match sub {
        cmd::ConfigSub::List => {
            let json = serde_json::to_string_pretty(&config).map_err(|e| WgError::Serialize {
                context: "config".to_string(),
                source: e,
            })?;
            Ok(json)
        }
        cmd::ConfigSub::Get { key } => match config.get(&key) {
            Some(value) => Ok(value),
            None => Err(WgError::ConfigKeyNotFound(key)),
        },
        cmd::ConfigSub::Set { key, value } => {
            let mut config = config;
            config.set(&key, &value)?;
            config.save()?;
            Ok(format!("Set {} = {}", key, value))
        }
    }
}

fn handle_model(config: Config, sub: cmd::ModelSub) -> Result<String, WgError> {
    cmd::model::run_model(config, sub)
}

// Helper functions

fn parse_entity_type(s: Option<String>) -> Option<EntityType> {
    s.map(|t| EntityType::parse(&t))
}

pub(crate) fn parse_fact_type(s: Option<String>) -> Option<FactType> {
    s.map(|t| match t.to_lowercase().as_str() {
        "decision" | "decide" => FactType::Decision,
        "pattern" => FactType::Pattern,
        "convention" => FactType::Convention,
        "claim" | "assertion" => FactType::Claim,
        "note" | "notes" => FactType::Note,
        "question" | "query" => FactType::Question,
        _ => FactType::Unknown,
    })
}

fn parse_tags(s: Option<Vec<String>>) -> Option<Vec<String>> {
    s.map(|tags| {
        tags.into_iter()
            .flat_map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    })
}

fn parse_aliases(s: Option<Vec<String>>) -> Option<Vec<String>> {
    s.map(|aliases| {
        aliases
            .into_iter()
            .flat_map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    })
}

fn parse_entity_sort(s: Option<String>) -> EntitySort {
    match s.as_deref() {
        Some("name") => EntitySort::Name,
        Some("fact-count") | Some("facts") => EntitySort::FactCount,
        _ => EntitySort::UpdatedAt,
    }
}

// === Time argument parsing ===

/// Parse an ISO 8601 date or RFC3339 timestamp into epoch milliseconds.
/// Accepts: `YYYY-MM-DD` (UTC midnight) or `2024-03-15T10:00:00Z` etc.
pub(crate) fn parse_iso_to_epoch_ms(s: &str) -> Result<u64, WgError> {
    let s = s.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return u64::try_from(dt.timestamp_millis())
            .map_err(|_| WgError::InvalidInput(format!("date out of range: {s}")));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| WgError::InvalidInput(format!("invalid date: {s}")))?
            .and_utc();
        return u64::try_from(dt.timestamp_millis())
            .map_err(|_| WgError::InvalidInput(format!("date out of range: {s}")));
    }
    Err(WgError::InvalidInput(format!(
        "expected YYYY-MM-DD or RFC3339 date, got: {s}"
    )))
}

/// Parse a relative-duration string like `30d`, `12h`, `4w`, `1y`, `45m` into milliseconds.
pub(crate) fn parse_duration_to_ms(s: &str) -> Result<u64, WgError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(WgError::InvalidInput("empty duration".to_string()));
    }
    // Split numeric prefix from unit suffix (last char).
    let (num_part, unit) = match s.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&s[..i], c.to_ascii_lowercase()),
        _ => {
            return Err(WgError::InvalidInput(format!(
                "duration needs a unit suffix (s/m/h/d/w/y), got: {s}"
            )));
        }
    };
    let n: u64 = num_part
        .parse()
        .map_err(|_| WgError::InvalidInput(format!("invalid number in duration: {s}")))?;
    let ms = match unit {
        's' => n * 1_000,
        'm' => n * 60 * 1_000,
        'h' => n * 60 * 60 * 1_000,
        'd' => n * 24 * 60 * 60 * 1_000,
        'w' => n * 7 * 24 * 60 * 60 * 1_000,
        'y' => n * 365 * 24 * 60 * 60 * 1_000,
        _ => {
            return Err(WgError::InvalidInput(format!(
                "unknown duration unit '{unit}' in: {s}"
            )));
        }
    };
    Ok(ms)
}

/// Resolve `--since` and `--last` into a single lower-bound epoch ms.
/// `--last` takes precedence; if both are set, `--last` wins (it's the more direct intent).
pub(crate) fn resolve_since(
    since: Option<&str>,
    last: Option<&str>,
) -> Result<Option<u64>, WgError> {
    if let Some(last) = last {
        let delta_ms = parse_duration_to_ms(last)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        return Ok(Some(now.saturating_sub(delta_ms)));
    }
    if let Some(s) = since {
        return Ok(Some(parse_iso_to_epoch_ms(s)?));
    }
    Ok(None)
}

fn resolve_until(until: Option<&str>) -> Result<Option<u64>, WgError> {
    match until {
        Some(s) => Ok(Some(parse_iso_to_epoch_ms(s)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod time_tests {
    use super::*;

    #[test]
    fn parse_iso_date_only() {
        assert_eq!(
            parse_iso_to_epoch_ms("2024-03-15").unwrap(),
            1_710_460_800_000
        );
    }

    #[test]
    fn parse_iso_rfc3339() {
        assert_eq!(
            parse_iso_to_epoch_ms("2024-03-15T10:30:00Z").unwrap(),
            1_710_498_600_000
        );
    }

    #[test]
    fn parse_iso_invalid() {
        assert!(parse_iso_to_epoch_ms("garbage").is_err());
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration_to_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_duration_to_ms("5m").unwrap(), 300_000);
        assert_eq!(parse_duration_to_ms("2h").unwrap(), 7_200_000);
        assert_eq!(parse_duration_to_ms("3d").unwrap(), 259_200_000);
        assert_eq!(parse_duration_to_ms("1w").unwrap(), 604_800_000);
        assert_eq!(parse_duration_to_ms("1y").unwrap(), 31_536_000_000);
    }

    #[test]
    fn parse_duration_rejects_no_unit() {
        assert!(parse_duration_to_ms("30").is_err());
    }

    #[test]
    fn parse_duration_rejects_bad_unit() {
        assert!(parse_duration_to_ms("30x").is_err());
    }

    #[test]
    fn resolve_since_prefers_last() {
        let s = resolve_since(Some("2024-01-01"), Some("1d"))
            .unwrap()
            .unwrap();
        // last=1d, so since = now - 1d; should be much greater than 2024-01-01
        assert!(s > 1_704_067_200_000);
    }
}
