//! AideMemo CLI — local-first memory SDK for coding agents.

mod cmd;
mod output;

use aidememo_core::{
    AideMemo, AideMemoError, Config, EntityInput, EntitySort, EntityType, ExportScope, FactInput,
    FactListOpts, FactType, LintReport, ListOpts, QueryOpts, SearchOpts, TraverseDirection,
    TraverseOpts, WorkflowStartOpts,
};
use std::path::{Path, PathBuf};
use std::process::exit;

// Hook into the global allocator so `aidememo bench` can report Rust-heap-only
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
                    "Error: project '{}' not registered. Run `aidememo project list`.",
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
        cmd::Command::Sync(sub) => handle_sync(&store_path, config, sub, json),
        cmd::Command::Config(sub) => handle_config(config, sub),
        cmd::Command::Model(sub) => handle_model(config, sub),
        cmd::Command::Feedback(sub) => cmd::feedback::run_feedback(&store_path, config, sub),
        cmd::Command::Adapt(sub) => cmd::adapt::run_adapt(&store_path, config, sub),
        cmd::Command::Init(sub) => cmd::init::run_init(
            sub.wiki_root,
            sub.no_ingest,
            sub.agent,
            sub.agent_force,
            &store_path,
            config,
            json,
        ),
        cmd::Command::Watch(sub) => cmd::watch::run_watch(sub.wiki_root, sub.interval, sub.search),
        cmd::Command::McpServe(sub) => {
            // Mirror the Mcp arm — honour --store / --project unless
            // the user passed a positional WIKI_ROOT.
            let path = sub.wiki_root.unwrap_or_else(|| store_path.clone());
            cmd::mcp_serve::run_mcp_serve(
                sub.port,
                sub.bind,
                sub.auth_token,
                sub.auth_token_file,
                Some(path),
            )
        }
        cmd::Command::Mcp(sub) => {
            // Honour the global --store / --project resolution if the
            // user didn't pass an explicit positional WIKI_ROOT.
            let path = sub.wiki_root.unwrap_or_else(|| store_path.clone());
            cmd::mcp_stdio::run_mcp(Some(path))
        }
        cmd::Command::McpInstall(sub) => cmd::mcp_install::run_mcp_install(sub, json),
        cmd::Command::Completions(sub) => cmd::completions::run_completions(sub),
        cmd::Command::Pending(sub) => cmd::pending::run_pending(sub, &store_path, config, json),
        cmd::Command::VectorRebuild(sub) => handle_vector_rebuild(&store_path, config, sub),
        cmd::Command::Daemon(sub) => cmd::daemon::run_daemon(sub, store_path.clone()),
        cmd::Command::Extract(sub) => handle_extract(&store_path, config, sub, json),
        cmd::Command::Session(sub) => handle_session(&store_path, config, sub, json),
        cmd::Command::Workflow(sub) => handle_workflow(&store_path, config, sub, json),
        cmd::Command::AutoRelate(sub) => handle_auto_relate(&store_path, config, sub),
        cmd::Command::Overview(sub) => handle_overview(&store_path, config, sub),
        cmd::Command::Consolidate(sub) => handle_consolidate(&store_path, config, sub),
        cmd::Command::Auth(sub) => cmd::auth::run_auth(sub),
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
            // `aidememo daemon start` and then their habitual `aidememo fact add …`
            // hits the lock because the daemon is holding it.
            let msg = e.to_string();
            if msg.contains("Database already open") || msg.contains("Cannot acquire lock") {
                use cmd::daemon::RegistryState;
                match cmd::daemon::registry_state(&store_path) {
                    RegistryState::Healthy(reg) => {
                        eprintln!(
                            "Hint: an AideMemo daemon is running on port {} and is holding this \
                             store's lock. The CLI auto-dispatches read commands + \
                             fact-add through it; commands not yet daemon-aware will \
                             collide. Stop the daemon (`aidememo daemon stop`) if you need \
                             local access, or use the daemon-aware command.",
                            reg.port
                        );
                    }
                    RegistryState::StaleRegistry => {
                        eprintln!(
                            "Hint: ~/.aidememo/daemon.json exists but the daemon isn't \
                             responding. Run `aidememo daemon status` to inspect, or \
                             `aidememo daemon stop` to clear the stale registry."
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
///   2. `AIDEMEMO_LOG` (alias so users don't have to scope `RUST_LOG`)
///   3. default: `aidememo=info,aidememo_core=warn`
///      - `aidememo=info` matches the binary's module path (the bin is
///        named `aidememo` per Cargo.toml's `[[bin]] name = "aidememo"`, so the
///        target is `aidememo::cmd::*` not `aidememo_cli::cmd::*`). Surfaces
///        `aidememo mcp-serve` startup, `aidememo watch` file events.
///      - `aidememo_core=warn` for degraded-path warnings (missing HNSW
///        sidecar, reranker disabled) — quiet otherwise.
///
/// Output goes to stderr so stdout stays clean for `--json` consumers.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = std::env::var("RUST_LOG")
        .or_else(|_| std::env::var("AIDEMEMO_LOG"))
        .ok()
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new("aidememo=info,aidememo_core=warn"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_ansi(false) // structured stderr; downstream tools parse it
        .compact()
        .try_init()
        .ok(); // ignore if already installed (tests / repeated init)
}

fn with_wiki<F>(path: &Path, config: Config, f: F) -> Result<String, AideMemoError>
where
    F: FnOnce(AideMemo) -> Result<String, AideMemoError>,
{
    let wiki = AideMemo::open(path, config)?;
    f(wiki)
}

fn with_wiki_mut<F>(path: &Path, config: Config, f: F) -> Result<String, AideMemoError>
where
    F: FnOnce(&mut AideMemo) -> Result<String, AideMemoError>,
{
    let mut wiki = AideMemo::open(path, config)?;
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
) -> Result<String, AideMemoError> {
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
            // Daemon discovery — aidememo_entity_get tool returns JSON.
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
                    "params": {"name": "aidememo_entity_list", "arguments": args}
                });
                return daemon_tool_call(&url, body, "aidememo_entity_list");
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
                    .map_err(|e| AideMemoError::Internal(format!("stdin read: {}", e)))?;
                buf.trim().to_string()
            } else {
                content.clone().ok_or_else(|| {
                    AideMemoError::InvalidInput(
                        "describe needs CONTENT, --from-stdin, or --clear".into(),
                    )
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
) -> Result<String, AideMemoError> {
    match sub {
        cmd::FactSub::Add {
            content,
            fact_type,
            entities,
            tags,
            source,
            source_id,
            confidence,
            observed_at,
        } => {
            let observed_at_ms = match observed_at.as_deref() {
                Some(s) => Some(parse_iso_to_epoch_ms(s)?),
                None => None,
            };
            // Daemon discovery — if a `aidememo daemon` is running on the
            // same store, dispatch through it. Otherwise we'd hit the
            // single-writer redb lock on the daemon's open handle.
            // The daemon has aidememo_fact_add as a first-class MCP tool so
            // the path is symmetric with read commands.
            if let Some(via) = cmd::daemon::registered_endpoint(path) {
                tracing::debug!(via = %via, "auto-discovered daemon for fact add");
                return run_fact_add_via_daemon(
                    &via,
                    &content,
                    entities.as_ref(),
                    tags.as_ref(),
                    source_id.as_deref(),
                    json,
                );
            }
            with_wiki_mut(path, config, |wiki| {
                let mut auto_created: Vec<String> = Vec::new();
                let mut alternatives: Vec<serde_json::Value> = Vec::new();
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
                                    // Same fuzzy guard as the MCP tool —
                                    // surface near-miss candidates so a
                                    // typo doesn't silently fork the
                                    // graph (live agent test caught
                                    // "Postgres" + "Postgrs" coexisting
                                    // as separate entities).
                                    let suggestions =
                                        wiki.suggest_similar_entities(name).unwrap_or_default();
                                    let new_id = wiki.entity_add(EntityInput {
                                        name: name.clone(),
                                        entity_type: Some(EntityType::Unknown),
                                        ..Default::default()
                                    })?;
                                    auto_created.push(name.clone());
                                    ids.push(new_id);
                                    if !suggestions.is_empty() {
                                        alternatives.push(serde_json::json!({
                                            "requested": name,
                                            "suggestions": suggestions,
                                        }));
                                    }
                                }
                            }
                        }
                        Some(ids)
                    }
                    None => None,
                };

                // AIDEMEMO_SESSION_ID hook — if a tracked session is active,
                // auto-attach its entity. Lets `aidememo session new …; eval`
                // give every subsequent fact the session as a thread.
                let entity_ids = match std::env::var("AIDEMEMO_SESSION_ID") {
                    Ok(sid) if !sid.is_empty() => match wiki.resolve_entity(&sid) {
                        Ok(sess_id) => {
                            let mut ids = entity_ids.unwrap_or_default();
                            if !ids.contains(&sess_id) {
                                ids.push(sess_id);
                            }
                            Some(ids)
                        }
                        Err(_) => {
                            tracing::warn!(
                                "AIDEMEMO_SESSION_ID={sid} doesn't resolve to an entity; skipping session auto-attach"
                            );
                            entity_ids
                        }
                    },
                    _ => entity_ids,
                };

                // Pre-add similarity check so the JSON envelope matches
                // aidememo_fact_add's `existing_similar` field. Mirrors the
                // MCP behaviour: BM25-only (no model load) and
                // non-blocking — we still add the fact, the caller
                // decides whether to aidememo_fact_supersede.
                let existing_similar = wiki
                    .hybrid_search(
                        &content,
                        aidememo_core::SearchOpts {
                            limit: Some(1),
                            bm25_only: true,
                            current_only: true,
                            ..Default::default()
                        },
                    )
                    .ok()
                    .and_then(|hits| hits.into_iter().next())
                    .filter(|hit| hit.score >= 1.0)
                    .map(|hit| {
                        serde_json::json!({
                            "fact_id": hit.fact_id.to_string(),
                            "content": hit.content,
                            "score": hit.score,
                        })
                    });

                let id = wiki.add_fact(FactInput {
                    content: content.clone(),
                    fact_type: parse_fact_type(fact_type),
                    entity_ids,
                    tags: parse_tags(tags),
                    source,
                    source_id,
                    source_confidence: confidence,
                    observed_at: observed_at_ms,
                })?;
                if json {
                    // Match the MCP aidememo_fact_add response shape exactly
                    // so callers (hermes-aidememo plugin, scripts) see a
                    // stable envelope regardless of whether the daemon
                    // was online or the local in-process path was
                    // used.
                    let record = wiki.fact_get(&id)?;
                    let entity_names_resolved: Vec<String> = record
                        .entity_ids
                        .iter()
                        .filter_map(|eid| wiki.entity_get_by_id(*eid).ok())
                        .map(|e| e.name)
                        .collect();
                    let alternatives_field = if alternatives.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::Array(alternatives)
                    };
                    let payload = serde_json::json!({
                        "id": id.to_string(),
                        "content": record.content,
                        "entity_names": entity_names_resolved,
                        "created_at": record.created_at,
                        "auto_created_entities": auto_created,
                        "entity_name_alternatives": alternatives_field,
                        "existing_similar": existing_similar,
                    });
                    return serde_json::to_string_pretty(&payload).map_err(|e| {
                        AideMemoError::Serialize {
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
                if let Some(similar) = &existing_similar {
                    if let (Some(fid), Some(sim_content)) = (
                        similar.get("fact_id").and_then(|v| v.as_str()),
                        similar.get("content").and_then(|v| v.as_str()),
                    ) {
                        msg.push_str(&format!(
                            "\n  similar existing fact: [{fid}] {}",
                            sim_content.chars().take(80).collect::<String>(),
                        ));
                    }
                }
                for alt in &alternatives {
                    if let (Some(req), Some(suggestions)) = (
                        alt.get("requested").and_then(|v| v.as_str()),
                        alt.get("suggestions").and_then(|v| v.as_array()),
                    ) {
                        let preview: Vec<&str> = suggestions
                            .iter()
                            .filter_map(|s| s.as_str())
                            .take(3)
                            .collect();
                        msg.push_str(&format!(
                            "\n  note: '{req}' looks similar to existing entit{}: {}",
                            if preview.len() == 1 { "y" } else { "ies" },
                            preview.join(", "),
                        ));
                    }
                }
                Ok(msg)
            })
        }
        cmd::FactSub::Get { id } => {
            // Same daemon-aware fast path as entity get — aidememo_fact_get
            // tool returns a JSON Fact record we forward verbatim.
            if let Some(via) = cmd::daemon::registered_endpoint(path) {
                tracing::debug!(via = %via, "auto-discovered daemon for fact get");
                return run_fact_get_via_daemon(&via, &id);
            }
            with_wiki(path, config, |wiki| {
                let fact_id =
                    aidememo_core::FactId(aidememo_core::ulid::Ulid::from_string(&id).map_err(
                        |_| AideMemoError::InvalidInput(format!("Invalid fact ID: {}", id)),
                    )?);
                let fact = wiki.fact_get(&fact_id)?;
                output::format_fact(&fact, &wiki, fmt(json))
            })
        }
        cmd::FactSub::List {
            fact_type,
            entity,
            min_confidence,
            source_id,
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
            // filter the aidememo_fact_list tool doesn't expose. Anything
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
                    if let Some(source_id) = source_id.as_ref() {
                        args["source_id"] = serde_json::json!(source_id);
                    }
                    let body = serde_json::json!({
                        "jsonrpc": "2.0", "id": 1,
                        "method": "tools/call",
                        "params": {"name": "aidememo_fact_list", "arguments": args}
                    });
                    return daemon_tool_call(&url, body, "aidememo_fact_list");
                }
            }
            with_wiki(path, config, |wiki| {
                let entity_id = entity.and_then(|n| wiki.resolve_entity(&n).ok());
                let facts = wiki.fact_list(FactListOpts {
                    fact_type: parse_fact_type(fact_type),
                    entity_id,
                    min_confidence,
                    source_id,
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
            let fact_id = aidememo_core::FactId(
                aidememo_core::ulid::Ulid::from_string(&id)
                    .map_err(|_| AideMemoError::InvalidInput(format!("Invalid fact ID: {}", id)))?,
            );
            wiki.fact_delete(&fact_id)?;
            Ok(format!("Deleted fact {}", id))
        }),
        cmd::FactSub::Feedback { id, helpful } => with_wiki_mut(path, config, |wiki| {
            let fact_id = aidememo_core::FactId(
                aidememo_core::ulid::Ulid::from_string(&id)
                    .map_err(|_| AideMemoError::InvalidInput(format!("Invalid fact ID: {}", id)))?,
            );
            wiki.fact_feedback(&fact_id, helpful)?;
            Ok(format!(
                "Recorded {} feedback for fact {}",
                if helpful { "helpful" } else { "not helpful" },
                id
            ))
        }),
        cmd::FactSub::Supersede { old_id, new_id } => {
            // Daemon discovery — aidememo_fact_supersede MCP tool exists.
            if let Some(via) = cmd::daemon::registered_endpoint(path) {
                tracing::debug!(via = %via, "auto-discovered daemon for fact supersede");
                return run_fact_supersede_via_daemon(&via, &old_id, &new_id);
            }
            with_wiki_mut(path, config, |wiki| {
                let parse = |s: &str| {
                    aidememo_core::FactId(
                        aidememo_core::ulid::Ulid::from_string(s)
                            .map_err(|_| {
                                AideMemoError::InvalidInput(format!("Invalid fact ID: {s}"))
                            })
                            .unwrap_or_default(),
                    )
                };
                let old = parse(&old_id);
                let new = parse(&new_id);
                wiki.fact_supersede(&old, &new)?;
                Ok(format!("Superseded {old_id} by {new_id}"))
            })
        }
        cmd::FactSub::Pin { id } => with_wiki(path, config, |wiki| {
            let fact_id = parse_fact_id_str(&id)?;
            wiki.fact_pin(&fact_id, true)?;
            Ok(format!("Pinned fact {id}"))
        }),
        cmd::FactSub::Unpin { id } => with_wiki(path, config, |wiki| {
            let fact_id = parse_fact_id_str(&id)?;
            wiki.fact_pin(&fact_id, false)?;
            Ok(format!("Unpinned fact {id}"))
        }),
        cmd::FactSub::Archive {
            ids,
            older_than,
            fact_type,
            dry_run,
        } => with_wiki_mut(path, config, |wiki| {
            let mut targets: Vec<aidememo_core::FactId> = Vec::new();
            if let Some(spec) = &ids {
                for s in spec.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    targets.push(parse_fact_id_str(s)?);
                }
            }
            if let Some(dur) = &older_than {
                let dur_ms = parse_duration_to_ms(dur)?;
                let cutoff_ms = aidememo_core::time::current_epoch_ms().saturating_sub(dur_ms);
                let mut opts = aidememo_core::FactListOpts::default();
                if let Some(t) = &fact_type {
                    opts.fact_type = Some(aidememo_core::FactType::parse(t));
                }
                let candidates = wiki.fact_list(opts)?;
                for f in candidates {
                    let ts = f.observed_at.unwrap_or(f.created_at);
                    if ts <= cutoff_ms {
                        targets.push(f.id);
                    }
                }
            }
            if targets.is_empty() {
                return Ok("No facts matched (give --ids or --older-than).".into());
            }
            if dry_run {
                let mut out = format!("would archive {} fact(s):", targets.len());
                for id in targets.iter().take(50) {
                    out.push_str(&format!("\n  {id}"));
                }
                if targets.len() > 50 {
                    out.push_str(&format!("\n  ... and {} more", targets.len() - 50));
                }
                return Ok(out);
            }
            let moved = wiki.archive_facts(&targets)?;
            Ok(format!(
                "archived {moved} fact(s) to cold tier ({} candidate(s) considered)",
                targets.len()
            ))
        }),
        cmd::FactSub::Pinned { limit } => with_wiki(path, config, |wiki| {
            let limit = limit.unwrap_or(20);
            let pinned = wiki.pinned_facts(limit)?;
            if json {
                return serde_json::to_string_pretty(&pinned).map_err(|e| {
                    AideMemoError::Serialize {
                        context: "fact pinned (json)".to_string(),
                        source: e,
                    }
                });
            }
            if pinned.is_empty() {
                return Ok("No pinned facts.".to_string());
            }
            let mut out = format!("Pinned facts (top {}):", pinned.len());
            for f in &pinned {
                out.push_str(&format!(
                    "\n  [{}] {} ({})",
                    f.id,
                    f.content.chars().take(80).collect::<String>(),
                    f.fact_type,
                ));
            }
            Ok(out)
        }),
    }
}

fn parse_fact_id_str(s: &str) -> Result<aidememo_core::FactId, AideMemoError> {
    aidememo_core::ulid::Ulid::from_string(s)
        .map(aidememo_core::FactId)
        .map_err(|_| AideMemoError::InvalidInput(format!("Invalid fact ID: {s}")))
}

fn handle_extract(
    store_path: &Path,
    config: Config,
    sub: cmd::ExtractSub,
    json: bool,
) -> Result<String, AideMemoError> {
    let cmd::ExtractSub {
        apply,
        min_confidence,
        max_candidates,
        llm,
        from_stdin,
        text,
    } = sub;
    let raw_text = if from_stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| AideMemoError::InvalidInput(format!("read stdin: {e}")))?;
        buf
    } else {
        text.ok_or_else(|| {
            AideMemoError::InvalidInput("provide TEXT positional or pass --from-stdin".into())
        })?
    };
    let min_confidence = min_confidence.unwrap_or(0.5);
    let max_candidates = max_candidates.unwrap_or(20);

    with_wiki_mut(store_path, config, |wiki| {
        let mut candidates = if llm {
            // Try LLM; on any failure, fall back to heuristic with a
            // tracing warning so the agent still gets a useful result.
            match wiki.extract_candidates_llm(&raw_text, max_candidates) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("LLM extract failed, falling back to heuristic: {e}");
                    wiki.extract_candidates(&raw_text, max_candidates)?
                }
            }
        } else {
            wiki.extract_candidates(&raw_text, max_candidates)?
        };
        candidates.retain(|c| c.confidence >= min_confidence);

        if !apply {
            if json {
                return serde_json::to_string_pretty(
                    &serde_json::json!({"applied": false, "candidates": candidates}),
                )
                .map_err(|e| AideMemoError::Serialize {
                    context: "extract (json)".into(),
                    source: e,
                });
            }
            if candidates.is_empty() {
                return Ok("No candidates above threshold.".into());
            }
            let mut out = format!("{} candidate fact(s):", candidates.len());
            for c in &candidates {
                out.push_str(&format!(
                    "\n  [{:.2}] {:10} {} {}",
                    c.confidence,
                    c.suggested_fact_type,
                    if c.suggested_entities.is_empty() {
                        "—"
                    } else {
                        "→"
                    },
                    if c.suggested_entities.is_empty() {
                        c.content.clone()
                    } else {
                        format!(
                            "{} (entities: {})",
                            c.content,
                            c.suggested_entities.join(", ")
                        )
                    },
                ));
            }
            return Ok(out);
        }

        let mut added: Vec<serde_json::Value> = Vec::new();
        for cand in &candidates {
            let mut entity_ids = Vec::with_capacity(cand.suggested_entities.len());
            for name in &cand.suggested_entities {
                let id = match wiki.resolve_entity(name) {
                    Ok(id) => id,
                    Err(_) => wiki.entity_add(aidememo_core::EntityInput {
                        name: name.clone(),
                        entity_type: Some(aidememo_core::EntityType::Unknown),
                        ..Default::default()
                    })?,
                };
                entity_ids.push(id);
            }
            let id = wiki.add_fact(aidememo_core::FactInput {
                content: cand.content.clone(),
                fact_type: Some(cand.suggested_fact_type),
                entity_ids: if entity_ids.is_empty() {
                    None
                } else {
                    Some(entity_ids)
                },
                tags: None,
                source: None,
                source_id: None,
                source_confidence: Some(cand.confidence),
                observed_at: None,
            })?;
            added.push(serde_json::json!({
                "id": id.to_string(),
                "content": cand.content,
                "fact_type": cand.suggested_fact_type.to_string(),
                "entities": cand.suggested_entities,
                "confidence": cand.confidence,
            }));
        }
        if json {
            return serde_json::to_string_pretty(
                &serde_json::json!({"applied": true, "added": added}),
            )
            .map_err(|e| AideMemoError::Serialize {
                context: "extract (json)".into(),
                source: e,
            });
        }
        Ok(format!("Added {} fact(s) from extraction.", added.len()))
    })
}

fn handle_session(
    store_path: &Path,
    config: Config,
    sub: cmd::SessionSub,
    json: bool,
) -> Result<String, AideMemoError> {
    match sub {
        cmd::SessionSub::Start {
            pinned_limit,
            recent_limit,
            recent_days,
            top_entities_limit,
        } => with_wiki(store_path, config, |wiki| {
            let pinned_limit = pinned_limit.unwrap_or(20);
            let recent_limit = recent_limit.unwrap_or(10);
            let recent_days = recent_days.unwrap_or(7);
            let top_entities_limit = top_entities_limit.unwrap_or(10);

            let pinned = wiki.pinned_facts(pinned_limit)?;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let since = Some(now_ms.saturating_sub(recent_days * 24 * 60 * 60 * 1000));
            let recent = wiki.fact_list(aidememo_core::FactListOpts {
                fact_type: None,
                entity_id: None,
                min_confidence: None,
                source_id: None,
                limit: Some(recent_limit),
                offset: 0,
                since,
                until: None,
                current_only: true,
                as_of: None,
            })?;
            let top_entities = wiki.entity_list(aidememo_core::ListOpts {
                entity_type: None,
                min_facts: None,
                sort_by: aidememo_core::EntitySort::FactCount,
                limit: Some(top_entities_limit),
                offset: 0,
            })?;
            let issues = wiki.lint()?;
            let stats = wiki.stats()?;

            if json {
                return serde_json::to_string_pretty(&serde_json::json!({
                    "stats": stats,
                    "pinned": pinned,
                    "recent": recent,
                    "top_entities": top_entities,
                    "open_issues": {
                        "total": issues.len(),
                        "issues": issues,
                    },
                }))
                .map_err(|e| AideMemoError::Serialize {
                    context: "session start (json)".into(),
                    source: e,
                });
            }
            let mut out = String::new();
            out.push_str(&format!(
                "stats: {} entities, {} facts, {} relations",
                stats.entity_count, stats.fact_count, stats.relation_count
            ));
            out.push_str(&format!("\npinned ({}/{}):", pinned.len(), pinned_limit));
            for f in pinned.iter().take(5) {
                out.push_str(&format!(
                    "\n  [{}] {}",
                    f.id,
                    f.content.chars().take(80).collect::<String>()
                ));
            }
            out.push_str(&format!(
                "\nrecent (last {recent_days}d, {}/{}):",
                recent.len(),
                recent_limit
            ));
            for f in recent.iter().take(5) {
                out.push_str(&format!(
                    "\n  [{}] {}",
                    f.id,
                    f.content.chars().take(80).collect::<String>()
                ));
            }
            out.push_str(&format!("\ntop entities ({}):", top_entities.len()));
            for e in &top_entities {
                out.push_str(&format!(
                    "\n  - {} ({}) [{} facts]",
                    e.name, e.entity_type, e.fact_count
                ));
            }
            out.push_str(&format!("\nopen issues: {}", issues.len()));
            Ok(out)
        }),
        cmd::SessionSub::New { topic } => with_wiki(store_path, config, |wiki| {
            // Mint a session entity. Name is `session-<ULID>` so it's
            // sortable by creation time and globally unique even
            // across stores. source_page carries the human topic.
            let session_name = format!("session-{}", aidememo_core::ulid::Ulid::new());
            wiki.entity_add(aidememo_core::EntityInput {
                name: session_name.clone(),
                entity_type: Some(aidememo_core::EntityType::parse("session")),
                source_page: Some(topic.clone()),
                ..Default::default()
            })?;
            if json {
                return serde_json::to_string_pretty(&serde_json::json!({
                    "session_id": session_name,
                    "topic": topic,
                    "export": format!("export AIDEMEMO_SESSION_ID={}", session_name),
                }))
                .map_err(|e| AideMemoError::Serialize {
                    context: "session new (json)".into(),
                    source: e,
                });
            }
            // Stdout is shell-evaluable so users can `eval "$(aidememo session new …)"`.
            // The leading comment lines start with `#` so eval ignores them.
            Ok(format!(
                "# aidememo session: {topic}\n# id: {session_name}\nexport AIDEMEMO_SESSION_ID={session_name}"
            ))
        }),
        cmd::SessionSub::Current => with_wiki(store_path, config, |wiki| {
            let Ok(sid) = std::env::var("AIDEMEMO_SESSION_ID") else {
                if json {
                    return Ok("null".to_string());
                }
                return Ok(
                    "(no current session — set AIDEMEMO_SESSION_ID or run `aidememo session new`)"
                        .into(),
                );
            };
            let entity = wiki.entity_get(&sid)?;
            let fact_count = wiki
                .fact_list(aidememo_core::FactListOpts {
                    fact_type: None,
                    entity_id: Some(entity.id),
                    min_confidence: None,
                    source_id: None,
                    limit: None,
                    offset: 0,
                    since: None,
                    until: None,
                    current_only: true,
                    as_of: None,
                })?
                .len();
            if json {
                return serde_json::to_string_pretty(&serde_json::json!({
                    "session_id": entity.name,
                    "topic": entity.source_page,
                    "fact_count": fact_count,
                    "created_at": entity.created_at,
                }))
                .map_err(|e| AideMemoError::Serialize {
                    context: "session current (json)".into(),
                    source: e,
                });
            }
            Ok(format!(
                "session: {}\ntopic:   {}\nfacts:   {}",
                entity.name,
                entity.source_page.as_deref().unwrap_or("-"),
                fact_count,
            ))
        }),
        cmd::SessionSub::List { limit } => with_wiki(store_path, config, |wiki| {
            let limit = limit.unwrap_or(20);
            let sessions = wiki.entity_list(aidememo_core::ListOpts {
                entity_type: Some(aidememo_core::EntityType::parse("session")),
                min_facts: None,
                sort_by: aidememo_core::EntitySort::UpdatedAt,
                limit: Some(limit),
                offset: 0,
            })?;
            if json {
                return serde_json::to_string_pretty(&sessions).map_err(|e| {
                    AideMemoError::Serialize {
                        context: "session list (json)".into(),
                        source: e,
                    }
                });
            }
            if sessions.is_empty() {
                return Ok("(no tracked sessions yet — run `aidememo session new <topic>`)".into());
            }
            let mut out = format!("{} session(s):\n", sessions.len());
            for s in &sessions {
                out.push_str(&format!("  {} ({} facts)\n", s.name, s.fact_count));
            }
            Ok(out)
        }),
    }
}

fn handle_workflow(
    store_path: &Path,
    config: Config,
    sub: cmd::WorkflowSub,
    json: bool,
) -> Result<String, AideMemoError> {
    match sub {
        cmd::WorkflowSub::Start {
            body,
            body_file,
            from_stdin,
            source,
            source_id,
            limit,
            depth,
            recent_limit,
            bm25_only,
            max_chars,
            title,
        } => {
            let body = read_workflow_body(body, body_file, from_stdin)?;
            with_wiki(store_path, config, |wiki| {
                let pack = workflow_start_pack(
                    &wiki,
                    &title,
                    WorkflowStartOpts {
                        body,
                        source,
                        source_id,
                        limit: limit.unwrap_or(8),
                        depth: depth.unwrap_or(2),
                        recent_limit: recent_limit.unwrap_or(5),
                        bm25_only,
                    },
                )?;
                if json {
                    return serde_json::to_string_pretty(&pack).map_err(|e| {
                        AideMemoError::Serialize {
                            context: "workflow start (json)".into(),
                            source: e,
                        }
                    });
                }
                Ok(render_workflow_start_text(&pack, max_chars.unwrap_or(6000)))
            })
        }
    }
}

fn read_workflow_body(
    body: Option<String>,
    body_file: Option<PathBuf>,
    from_stdin: bool,
) -> Result<Option<String>, AideMemoError> {
    let sources =
        usize::from(body.is_some()) + usize::from(body_file.is_some()) + usize::from(from_stdin);
    if sources > 1 {
        return Err(AideMemoError::InvalidInput(
            "pass only one of --body, --body-file, or --from-stdin".into(),
        ));
    }
    if let Some(body) = body {
        return Ok(Some(body));
    }
    if let Some(path) = body_file {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| AideMemoError::Internal(format!("read {}: {e}", path.display())))?;
        return Ok(Some(text));
    }
    if from_stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| AideMemoError::Internal(format!("stdin read: {e}")))?;
        return Ok(Some(buf));
    }
    Ok(None)
}

fn workflow_start_pack(
    wiki: &AideMemo,
    title: &str,
    opts: WorkflowStartOpts,
) -> Result<serde_json::Value, AideMemoError> {
    let pack = wiki.workflow_start(title, opts)?;
    serde_json::to_value(pack).map_err(|e| AideMemoError::Serialize {
        context: "workflow start pack".into(),
        source: e,
    })
}

fn render_workflow_start_text(pack: &serde_json::Value, max_chars: usize) -> String {
    fn push_hits(out: &mut String, title: &str, hits: &[serde_json::Value]) {
        out.push_str(&format!("\n## {title} ({})\n", hits.len()));
        for hit in hits.iter().take(5) {
            let id = hit
                .get("fact_id")
                .or_else(|| hit.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let content = hit.get("content").and_then(|v| v.as_str()).unwrap_or("");
            out.push_str(&format!("- [{}] {}\n", id, content));
        }
    }

    let session_id = pack
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let export = pack.get("export").and_then(|v| v.as_str()).unwrap_or("");
    let title = pack.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let mut out = String::new();
    out.push_str(&format!("# workflow start · {title}\n\n"));
    out.push_str(&format!("session: `{session_id}`\n"));
    out.push_str(&format!("eval: `{export}`\n"));
    if let Some(source) = pack.get("source").and_then(|v| v.as_str()) {
        out.push_str(&format!("source: `{source}`\n"));
    }
    if let Some(source_id) = pack.get("source_id").and_then(|v| v.as_str()) {
        out.push_str(&format!("source_id: `{source_id}`\n"));
    }
    if let Some(ticket_fact_id) = pack.get("ticket_fact_id").and_then(|v| v.as_str()) {
        out.push_str(&format!("ticket_fact: `{ticket_fact_id}`\n"));
    }

    if let Some(arr) = pack
        .get("context")
        .and_then(|v| v.get("search"))
        .and_then(|v| v.as_array())
    {
        push_hits(&mut out, "context hits", arr);
    }
    if let Some(arr) = pack.get("relevant_decisions").and_then(|v| v.as_array()) {
        push_hits(&mut out, "relevant decisions", arr);
    }
    if let Some(arr) = pack.get("prior_lessons").and_then(|v| v.as_array()) {
        push_hits(&mut out, "prior lessons", arr);
    }
    if let Some(arr) = pack.get("prior_errors").and_then(|v| v.as_array()) {
        push_hits(&mut out, "prior errors", arr);
    }

    if out.len() > max_chars {
        out.truncate(max_chars.saturating_sub(20));
        out.push_str("\n... [truncated]\n");
    }
    out
}

/// `aidememo entity get NAME` daemon path. aidememo_entity_get returns the
/// entity record as JSON; we forward it verbatim.
fn run_entity_get_via_daemon(base_url: &str, name: &str) -> Result<String, AideMemoError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "aidememo_entity_get", "arguments": {"name": name}}
    });
    daemon_tool_call(&url, body, "aidememo_entity_get")
}

/// `aidememo fact get ID` daemon path. Symmetric with entity get.
fn run_fact_get_via_daemon(base_url: &str, id: &str) -> Result<String, AideMemoError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "aidememo_fact_get", "arguments": {"id": id}}
    });
    daemon_tool_call(&url, body, "aidememo_fact_get")
}

/// `aidememo fact supersede` daemon path. aidememo_fact_supersede tool returns
/// `{"old_id": "...", "new_id": "..."}` JSON; we re-pack the same
/// "Superseded X by Y" line the local path emits.
fn run_fact_supersede_via_daemon(
    base_url: &str,
    old_id: &str,
    new_id: &str,
) -> Result<String, AideMemoError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {
            "name": "aidememo_fact_supersede",
            "arguments": {"old_id": old_id, "new_id": new_id}
        }
    });
    let _ = daemon_tool_call(&url, body, "aidememo_fact_supersede")?;
    Ok(format!("Superseded {old_id} by {new_id}"))
}

fn handle_traverse(
    path: &Path,
    config: Config,
    sub: cmd::TraverseSub,
    json: bool,
) -> Result<String, AideMemoError> {
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon for traverse");
        let url = format!("{}/mcp", via.trim_end_matches('/'));
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {
                "name": "aidememo_traverse",
                "arguments": {"entity": sub.entity, "depth": sub.depth.unwrap_or(2)}
            }
        });
        return daemon_tool_call(&url, body, "aidememo_traverse");
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
) -> Result<String, AideMemoError> {
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon for path");
        let url = format!("{}/mcp", via.trim_end_matches('/'));
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {
                "name": "aidememo_path",
                "arguments": {"from": sub.from, "to": sub.to}
            }
        });
        return daemon_tool_call(&url, body, "aidememo_path");
    }
    with_wiki(path, config, |wiki| {
        let path_steps = wiki.path_find(&sub.from, &sub.to)?;
        if json {
            return serde_json::to_string_pretty(&serde_json::json!({
                "from": &sub.from,
                "to": &sub.to,
                "steps": path_steps,
            }))
            .map_err(|e| AideMemoError::Serialize {
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
) -> Result<String, AideMemoError> {
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
    // Opportunistic discovery: if a `aidememo daemon` is running and serving
    // the same store, dispatch through it so we skip the local redb
    // open + (for hybrid) model load. Set AIDEMEMO_NO_DAEMON=1 to bypass.
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon");
        return run_search_via_daemon(&via, &sub, default_limit, json);
    }

    with_wiki_mut(path, config, |wiki| {
        let session_id = aidememo_core::ulid::Ulid::new().to_string();
        // CLI default = BM25 (fast). `--hybrid` opts into the
        // semantic path; `semantic_weight = 0` in config also forces
        // BM25 even with the flag (caller-on-caller override).
        let bm25_only = !sub.hybrid || semantic_weight == 0.0;
        let opts = SearchOpts {
            limit: sub.limit.or(Some(default_limit)),
            min_confidence: sub.min_confidence,
            source_id: sub.source_id.clone(),
            entity_filter: None,
            bm25_weight,
            semantic_weight,
            since: since_ms,
            until: until_ms,
            session_id: Some(session_id.clone()),
            current_only: false,
            as_of: as_of_ms,
            bm25_only,
            include_archive: sub.include_archive,
        };

        let results = if let Some(ref start) = sub.traverse_from {
            let depth = sub.traverse_depth.unwrap_or(2);
            wiki.search_with_traverse(&sub.query, start, depth, opts)?
        } else {
            wiki.hybrid_search(&sub.query, opts)?
        };

        let search_session = aidememo_core::SearchSession {
            id: session_id,
            query: sub.query.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| AideMemoError::Internal(format!("system clock error: {}", e)))?
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

/// `aidememo search --via http://host:port` — dispatch the search as a
/// JSON-RPC `aidememo_search` tool call against a running `aidememo mcp-serve`
/// daemon. The daemon keeps the redb store and the embedding model
/// warm, so the round-trip is dominated by HTTP latency (typically
/// ~10–20 ms on localhost).
///
/// Output is the daemon's `aidememo_search` content block dumped verbatim —
/// the daemon's tool already formats hits the way agents consume
/// them. We don't try to reformat it as the local table view because
/// the daemon may legitimately have a different store / config than
/// the calling CLI's project.
fn run_search_via_daemon(
    base_url: &str,
    sub: &cmd::SearchSub,
    default_limit: usize,
    _json: bool,
) -> Result<String, AideMemoError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let limit = sub.limit.unwrap_or(default_limit);
    let mut arguments = serde_json::json!({
        "query": sub.query,
        "limit": limit,
        // CLI default = BM25; --hybrid flips it on. Daemon
        // honours the same opt-in semantics.
        "bm25_only": !sub.hybrid,
        // Match the local CLI search path. MCP defaults to current-only
        // for agent "what is true now?" calls, but `aidememo search` has
        // historically searched all facts unless the caller says otherwise.
        "current_only": false,
        "include_archive": sub.include_archive,
    });
    if let Some(since) = &sub.since {
        arguments["since"] = serde_json::Value::String(since.clone());
    }
    if let Some(until) = &sub.until {
        arguments["until"] = serde_json::Value::String(until.clone());
    }
    if let Some(as_of) = &sub.as_of {
        arguments["as_of"] = serde_json::Value::String(as_of.clone());
    }
    if let Some(min_confidence) = sub.min_confidence {
        arguments["min_confidence"] = serde_json::Value::Number(
            serde_json::Number::from_f64(min_confidence as f64).ok_or_else(|| {
                AideMemoError::InvalidInput(format!("invalid min-confidence: {min_confidence}"))
            })?,
        );
    }
    if let Some(source_id) = &sub.source_id {
        arguments["source_id"] = serde_json::Value::String(source_id.clone());
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {
            "name": "aidememo_search",
            "arguments": arguments
        }
    });
    let resp: serde_json::Value = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| AideMemoError::Internal(format!("daemon POST {url} failed: {e}")))?
        .into_json()
        .map_err(|e| AideMemoError::Internal(format!("daemon response parse: {e}")))?;
    if let Some(err) = resp.get("error") {
        return Err(AideMemoError::Internal(format!("daemon error: {err}")));
    }
    let text = resp
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AideMemoError::Internal(format!("unexpected daemon response: {resp}")))?;
    Ok(text.to_string())
}

/// Run a hybrid search across every registered project, tag each hit with
/// its project name, merge by score, and render. Used by `aidememo search
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
) -> Result<String, AideMemoError> {
    if config.projects.is_empty() {
        return Err(AideMemoError::InvalidInput(
            "no projects registered — `aidememo project create` first".into(),
        ));
    }
    let as_of_ms = match sub.as_of.as_deref() {
        Some(d) => Some(parse_iso_to_epoch_ms(d)?),
        None => None,
    };
    let limit = sub.limit.or(Some(default_limit));
    let mut all: Vec<(String, aidememo_core::SearchResult)> = Vec::new();

    for proj_name in config.projects.keys() {
        let store_path = match config.project_path(proj_name) {
            Some(p) => p,
            None => continue,
        };
        let wiki = match AideMemo::open(&store_path, config.clone()) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(
                    project = %proj_name,
                    store = %store_path.display(),
                    "[aidememo search] skipping project: {e}",
                );
                continue;
            }
        };
        let opts = SearchOpts {
            limit,
            min_confidence: sub.min_confidence,
            source_id: sub.source_id.clone(),
            entity_filter: None,
            bm25_weight,
            semantic_weight,
            since: since_ms,
            until: until_ms,
            session_id: None,
            current_only: false,
            as_of: as_of_ms,
            bm25_only: !sub.hybrid || semantic_weight == 0.0,
            include_archive: sub.include_archive,
        };
        match wiki.hybrid_search(&sub.query, opts) {
            Ok(hits) => {
                for h in hits {
                    all.push((proj_name.clone(), h));
                }
            }
            Err(e) => {
                tracing::warn!(project = %proj_name, "[aidememo search] project search failed: {e}")
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
        return serde_json::to_string_pretty(&payload).map_err(|e| AideMemoError::Serialize {
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
) -> Result<String, AideMemoError> {
    let since_ms = resolve_since(None, sub.last.as_deref())?;
    let mode = sub
        .mode
        .as_deref()
        .map(aidememo_core::QueryMode::parse)
        .unwrap_or_default();
    // Same opportunistic-discovery shortcut as `aidememo search`. The aidememo_query
    // tool returns JSON, so the daemon's text content is already in
    // the shape `aidememo query --json` would print; for the table view we
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
            source_id: sub.source_id.clone(),
        };
        let result = wiki.query(&sub.topic, opts)?;
        if json {
            return serde_json::to_string_pretty(&result).map_err(|e| AideMemoError::Serialize {
                context: "query".to_string(),
                source: e,
            });
        }
        output::format_query_result(&result)
    })
}

/// `aidememo query` daemon path. Calls the `aidememo_query` MCP tool and prints
/// its JSON response verbatim (the tool already JSON-encodes the
/// QueryResult; reformatting it as a local table would require
/// deserialising into the `aidememo-core` type, which costs more code than
/// it's worth for the warm-path one-shot.)
fn run_query_via_daemon(base_url: &str, sub: &cmd::QuerySub) -> Result<String, AideMemoError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let mode = sub.mode.clone().unwrap_or_else(|| "hybrid".to_string());
    let mut arguments = serde_json::json!({
        "topic": sub.topic,
        "limit": sub.limit.unwrap_or(10),
        "depth": sub.depth.unwrap_or(2),
        "recent_limit": sub.recent_limit.unwrap_or(10),
        "mode": mode,
    });
    if let Some(source_id) = &sub.source_id {
        arguments["source_id"] = serde_json::Value::String(source_id.clone());
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {
            "name": "aidememo_query",
            "arguments": arguments
        }
    });
    daemon_tool_call(&url, body, "aidememo_query")
}

/// `aidememo fact add` daemon path. Calls the aidememo_fact_add MCP tool which
/// returns `{id, content, entity_names, created_at, auto_created_entities}`.
/// The local CLI surface returns either JSON or a "Added fact with ID …"
/// line; we normalise here so users see the same output whether the
/// daemon is on or off.
fn run_fact_add_via_daemon(
    base_url: &str,
    content: &str,
    entities: Option<&Vec<String>>,
    tags: Option<&Vec<String>>,
    source_id: Option<&str>,
    json: bool,
) -> Result<String, AideMemoError> {
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
    if let Some(source_id) = source_id {
        args["source_id"] = serde_json::json!(source_id);
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "aidememo_fact_add", "arguments": args}
    });
    let raw = daemon_tool_call(&url, body, "aidememo_fact_add")?;
    let payload: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| AideMemoError::Internal(format!("aidememo_fact_add response parse: {e}")))?;
    let id = payload.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
        AideMemoError::Internal(format!("aidememo_fact_add response missing id: {raw}"))
    })?;
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
        // Forward the daemon's full envelope verbatim — daemon-on and
        // daemon-off `aidememo fact add --json` outputs are now identical.
        return Ok(payload.to_string());
    }
    let mut msg = format!("Added fact with ID {id}");
    if !auto_created.is_empty() {
        msg.push_str(&format!(
            "\nAuto-created entities: {}",
            auto_created.join(", ")
        ));
    }
    // Surface dedup / typo hints in the human output too — agents that
    // run via `aidememo` interactively shouldn't have to switch to --json to
    // see them.
    if let Some(similar) = payload.get("existing_similar").and_then(|v| v.as_object()) {
        if let (Some(fid), Some(content)) = (
            similar.get("fact_id").and_then(|v| v.as_str()),
            similar.get("content").and_then(|v| v.as_str()),
        ) {
            msg.push_str(&format!(
                "\nSimilar existing fact: [{fid}] {}",
                content.chars().take(80).collect::<String>(),
            ));
        }
    }
    if let Some(alts) = payload
        .get("entity_name_alternatives")
        .and_then(|v| v.as_array())
    {
        for alt in alts {
            if let (Some(req), Some(suggestions)) = (
                alt.get("requested").and_then(|v| v.as_str()),
                alt.get("suggestions").and_then(|v| v.as_array()),
            ) {
                let preview: Vec<&str> = suggestions
                    .iter()
                    .filter_map(|s| s.as_str())
                    .take(3)
                    .collect();
                msg.push_str(&format!(
                    "\nNote: '{req}' looks similar to existing entit{}: {}",
                    if preview.len() == 1 { "y" } else { "ies" },
                    preview.join(", "),
                ));
            }
        }
    }
    Ok(msg)
}

/// Shared helper for `--via` daemon dispatch: POST a JSON-RPC tool
/// call and unwrap the text content. Used by both aidememo search and
/// aidememo query / aidememo recent. Errors carry the daemon URL so the user
/// can tell whether the daemon, the network, or the tool itself
/// failed.
fn daemon_tool_call(
    url: &str,
    body: serde_json::Value,
    tool: &str,
) -> Result<String, AideMemoError> {
    let resp: serde_json::Value = ureq::post(url)
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| AideMemoError::Internal(format!("daemon POST {url} failed: {e}")))?
        .into_json()
        .map_err(|e| AideMemoError::Internal(format!("daemon response parse: {e}")))?;
    if let Some(err) = resp.get("error") {
        return Err(AideMemoError::Internal(format!(
            "daemon {tool} error: {err}"
        )));
    }
    let text = resp
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AideMemoError::Internal(format!("unexpected daemon {tool} response: {resp}"))
        })?;
    Ok(text.to_string())
}

fn handle_lint(
    path: &Path,
    config: Config,
    sub: cmd::LintSub,
    json: bool,
) -> Result<String, AideMemoError> {
    if let Some(via) = cmd::daemon::registered_endpoint(path) {
        tracing::debug!(via = %via, "auto-discovered daemon for lint");
        let url = format!("{}/mcp", via.trim_end_matches('/'));
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {"name": "aidememo_lint", "arguments": {}}
        });
        return daemon_tool_call(&url, body, "aidememo_lint");
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

fn handle_export(
    path: &Path,
    config: Config,
    sub: cmd::ExportSub,
) -> Result<String, AideMemoError> {
    with_wiki(path, config, |wiki| {
        let scope = match sub.scope.as_deref() {
            Some("entities") => ExportScope::Entities,
            Some("relations") => ExportScope::Relations,
            Some("facts") => ExportScope::Facts,
            _ => ExportScope::All,
        };

        let mut output = Vec::new();
        let stats = wiki.export_jsonl(&mut output, scope)?;
        let content =
            String::from_utf8(output).map_err(|e| AideMemoError::Internal(e.to_string()))?;

        if sub.output.is_some() {
            if let Some(path) = sub.output {
                std::fs::write(&path, &content)
                    .map_err(|e| AideMemoError::Internal(format!("Failed to write file: {}", e)))?;
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

fn handle_import(
    path: &Path,
    config: Config,
    sub: cmd::ImportSub,
) -> Result<String, AideMemoError> {
    with_wiki_mut(path, config, |wiki| {
        let content = if let Some(p) = sub.path.as_ref() {
            std::fs::read_to_string(p)
                .map_err(|e| AideMemoError::Internal(format!("Failed to read file: {}", e)))?
        } else {
            // Read from stdin
            use std::io::Read;
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .map_err(|e| AideMemoError::Internal(format!("Failed to read stdin: {}", e)))?;
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
) -> Result<String, AideMemoError> {
    with_wiki(path, config, |wiki| {
        let stats = wiki.stats()?;
        output::format_stats(&stats, fmt(json))
    })
}

fn handle_vector_rebuild(
    path: &Path,
    config: Config,
    sub: cmd::VectorRebuildSub,
) -> Result<String, AideMemoError> {
    with_wiki(path, config, |wiki| {
        let started = std::time::Instant::now();
        let stats =
            wiki.vector_index_rebuild_with_opts(aidememo_core::types::VectorRebuildOpts {
                current_only: sub.current_only,
            })?;
        let elapsed_ms = started.elapsed().as_millis();

        if sub.json {
            let payload = serde_json::json!({
                "facts_indexed": stats.facts_indexed,
                "superseded_skipped": stats.superseded_skipped,
                "current_only": sub.current_only,
                "elapsed_ms": elapsed_ms,
            });
            return serde_json::to_string_pretty(&payload).map_err(|e| AideMemoError::Serialize {
                context: "vector-rebuild".to_string(),
                source: e,
            });
        }
        if stats.facts_indexed == 0 {
            Ok(format!(
                "No facts to index — sidecar removed (took {} ms).",
                elapsed_ms
            ))
        } else if sub.current_only {
            Ok(format!(
                "Rebuilt HNSW index over {} current facts ({} superseded skipped) in {} ms.",
                stats.facts_indexed, stats.superseded_skipped, elapsed_ms
            ))
        } else {
            Ok(format!(
                "Rebuilt HNSW index over {} facts in {} ms.",
                stats.facts_indexed, elapsed_ms
            ))
        }
    })
}

fn handle_auto_relate(
    path: &Path,
    config: Config,
    sub: cmd::AutoRelateSub,
) -> Result<String, AideMemoError> {
    with_wiki(path, config, |wiki| {
        let started = std::time::Instant::now();
        let mut opts = aidememo_core::AutoRelateOpts::default();
        if let Some(t) = sub.threshold {
            opts.threshold = t;
        }
        if let Some(k) = sub.top_k {
            opts.top_k = k;
        }
        opts.dry_run = sub.dry_run;
        let stats = wiki.auto_relate(opts)?;
        let elapsed_ms = started.elapsed().as_millis();

        if sub.json {
            let payload = serde_json::json!({
                "facts_processed": stats.facts_processed,
                "pairs_evaluated": stats.pairs_evaluated,
                "edges_created": stats.edges_created,
                "edges_skipped_same_entity": stats.edges_skipped_same_entity,
                "edges_skipped_existing": stats.edges_skipped_existing,
                "dry_run": sub.dry_run,
                "elapsed_ms": elapsed_ms,
            });
            return serde_json::to_string_pretty(&payload).map_err(|e| AideMemoError::Serialize {
                context: "auto-relate".to_string(),
                source: e,
            });
        }

        let header = if sub.dry_run {
            "auto-relate (dry-run)"
        } else {
            "auto-relate"
        };
        Ok(format!(
            "{} — {} facts processed, {} pairs evaluated, {} edges {} ({} same-entity, {} pre-existing skipped) in {} ms.",
            header,
            stats.facts_processed,
            stats.pairs_evaluated,
            stats.edges_created,
            if sub.dry_run {
                "would create"
            } else {
                "created"
            },
            stats.edges_skipped_same_entity,
            stats.edges_skipped_existing,
            elapsed_ms,
        ))
    })
}

fn handle_consolidate(
    path: &Path,
    config: Config,
    sub: cmd::ConsolidateSub,
) -> Result<String, AideMemoError> {
    with_wiki(path, config, |wiki| {
        let started = std::time::Instant::now();
        // GAC strategy short-circuit. Stage 2a is analysis-only and
        // ignores --semantic-threshold / --ttl entirely.
        if sub.gac {
            let theta = sub.gac_theta.unwrap_or(0.85);
            let protected_types: Vec<aidememo_core::FactType> = sub
                .gac_protect
                .as_deref()
                .map(|s| {
                    s.split(',')
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map(aidememo_core::FactType::parse)
                        .collect()
                })
                .unwrap_or_default();
            let gac_opts = aidememo_core::types::GacOpts {
                theta,
                dry_run: sub.dry_run,
                spread_residual_budget: sub.gac_spread_budget.unwrap_or(0),
                use_cold_tier: sub.gac_cold_tier,
                protected_types,
            };
            let stats = wiki.consolidate_gac(gac_opts.clone())?;
            let elapsed_ms = started.elapsed().as_millis();
            if sub.json {
                let payload = serde_json::json!({
                    "strategy": "gac",
                    "theta": stats.theta,
                    "dry_run": sub.dry_run,
                    "use_cold_tier": gac_opts.use_cold_tier,
                    "spread_residual_budget": gac_opts.spread_residual_budget,
                    "facts_processed": stats.facts_processed,
                    "n_clusters": stats.n_clusters,
                    "n_singletons": stats.n_singletons,
                    "n_multi_clusters": stats.n_multi_clusters,
                    "tight_clusters": stats.tight_clusters,
                    "spread_clusters": stats.spread_clusters,
                    "tight_facts": stats.tight_facts,
                    "spread_facts": stats.spread_facts,
                    "max_cluster_size": stats.max_cluster_size,
                    "max_dbar": stats.max_dbar,
                    "tight_collapsed": stats.tight_collapsed,
                    "spread_archived": stats.spread_archived,
                    "archived_to_cold": stats.archived_to_cold,
                    "protected_skipped": stats.protected_skipped,
                    "elapsed_ms": elapsed_ms,
                });
                return serde_json::to_string_pretty(&payload).map_err(|e| {
                    AideMemoError::Serialize {
                        context: "consolidate-gac".into(),
                        source: e,
                    }
                });
            }
            let mode = if sub.dry_run {
                "dry-run".to_string()
            } else if gac_opts.use_cold_tier {
                "applied (cold-tier)".to_string()
            } else {
                "applied (supersede)".to_string()
            };
            let protect_suffix = if stats.protected_skipped > 0 {
                format!(", protected_skipped {}", stats.protected_skipped)
            } else {
                String::new()
            };
            return Ok(format!(
                "GAC {} (θ={:.2}, θ'={:.2}, budget={}) — {} facts, {} clusters \
                 ({} singletons + {} multi), {} tight ({} facts) / {} spread \
                 ({} facts), max cluster size {}, max d̄={:.3}, \
                 collapsed {} tight + {} spread, archived_to_cold {}{}, in {} ms.",
                mode,
                stats.theta,
                1.0 - stats.theta,
                gac_opts.spread_residual_budget,
                stats.facts_processed,
                stats.n_clusters,
                stats.n_singletons,
                stats.n_multi_clusters,
                stats.tight_clusters,
                stats.tight_facts,
                stats.spread_clusters,
                stats.spread_facts,
                stats.max_cluster_size,
                stats.max_dbar,
                stats.tight_collapsed,
                stats.spread_archived,
                stats.archived_to_cold,
                protect_suffix,
                elapsed_ms,
            ));
        }
        let mut opts = aidememo_core::ConsolidateOpts::default();
        if let Some(t) = sub.semantic_threshold {
            opts.semantic_threshold = t;
        }
        // Parse repeated --ttl TYPE=DAYS into the BTreeMap.
        for spec in &sub.ttl {
            let Some((t, d)) = spec.split_once('=') else {
                return Err(AideMemoError::InvalidInput(format!(
                    "--ttl expects TYPE=DAYS, got '{spec}'"
                )));
            };
            let days: u64 = d
                .parse()
                .map_err(|e| AideMemoError::InvalidInput(format!("--ttl '{spec}' DAYS: {e}")))?;
            opts.ttl_days_by_type.insert(t.to_lowercase(), days);
        }
        opts.dry_run = sub.dry_run;
        let stats = wiki.consolidate_semantic(opts.clone())?;
        let elapsed_ms = started.elapsed().as_millis();

        if sub.json {
            let payload = serde_json::json!({
                "facts_processed": stats.facts_processed,
                "pairs_found": stats.pairs_found,
                "supersedes_applied": stats.supersedes_applied,
                "expired_applied": stats.expired_applied,
                "max_cosine": stats.max_cosine,
                "threshold": opts.semantic_threshold,
                "ttl_days_by_type": opts.ttl_days_by_type,
                "dry_run": sub.dry_run,
                "elapsed_ms": elapsed_ms,
            });
            return serde_json::to_string_pretty(&payload).map_err(|e| AideMemoError::Serialize {
                context: "consolidate".to_string(),
                source: e,
            });
        }

        let verb = if sub.dry_run {
            "would apply"
        } else {
            "applied"
        };
        let ttl_summary = if opts.ttl_days_by_type.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = opts
                .ttl_days_by_type
                .iter()
                .map(|(k, v)| format!("{k}={v}d"))
                .collect();
            format!(
                ", {} expired ({}) {}",
                stats.expired_applied,
                parts.join(","),
                verb
            )
        };
        Ok(format!(
            "consolidate{} — {} facts processed, {} pair(s) ≥ {:.2} cosine, {} supersede(s) {}{} (max cosine seen: {:.3}) in {} ms.",
            if sub.dry_run { " (dry-run)" } else { "" },
            stats.facts_processed,
            stats.pairs_found,
            opts.semantic_threshold,
            stats.supersedes_applied,
            verb,
            ttl_summary,
            stats.max_cosine,
            elapsed_ms,
        ))
    })
}

fn handle_overview(
    path: &Path,
    config: Config,
    sub: cmd::OverviewSub,
) -> Result<String, AideMemoError> {
    with_wiki(path, config, |wiki| {
        let mut opts = aidememo_core::OverviewOpts::default();
        if let Some(n) = sub.top_n {
            opts.top_n_entities = n;
        }
        if let Some(d) = sub.recent_days {
            opts.recent_days = d;
        }
        let result = wiki.overview(opts.clone())?;

        if sub.json {
            return serde_json::to_string_pretty(&result).map_err(|e| AideMemoError::Serialize {
                context: "overview".to_string(),
                source: e,
            });
        }

        let mut lines = Vec::new();
        lines.push(format!(
            "Wiki overview: {} entities ({} orphan), {} facts ({} current, {} pinned), {} relations.",
            result.stats.entity_count,
            result.orphan_entity_count,
            result.stats.fact_count,
            result.current_fact_count,
            result.pinned_fact_count,
            result.stats.relation_count,
        ));
        lines.push(format!(
            "Recent activity: {} facts in the last {} day(s).",
            result.recent_fact_count, opts.recent_days,
        ));

        if !result.entity_types.is_empty() {
            lines.push(String::new());
            lines.push("Entity types:".to_string());
            for bucket in &result.entity_types {
                let examples: Vec<String> = bucket
                    .top_examples
                    .iter()
                    .take(3)
                    .map(|e| format!("{} ({})", e.name, e.fact_count))
                    .collect();
                let suffix = if examples.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", examples.join(", "))
                };
                lines.push(format!(
                    "  {} × {}{}",
                    bucket.count, bucket.entity_type, suffix
                ));
            }
        }

        if !result.fact_types.is_empty() {
            lines.push(String::new());
            lines.push("Fact types:".to_string());
            let total = result
                .fact_types
                .iter()
                .map(|b| b.count)
                .sum::<u64>()
                .max(1);
            for bucket in &result.fact_types {
                let pct = (bucket.count as f64 / total as f64) * 100.0;
                lines.push(format!(
                    "  {} × {} ({:.0}%)",
                    bucket.count, bucket.fact_type, pct
                ));
            }
        }

        if !result.top_entities.is_empty() {
            lines.push(String::new());
            lines.push(format!(
                "Top {} entities by fact count:",
                opts.top_n_entities
            ));
            for e in &result.top_entities {
                lines.push(format!(
                    "  {:<3} {} ({})",
                    e.fact_count, e.name, e.entity_type
                ));
            }
        }

        Ok(lines.join("\n"))
    })
}

fn handle_ingest(
    path: &Path,
    config: Config,
    sub: cmd::IngestSub,
) -> Result<String, AideMemoError> {
    let wiki = AideMemo::open(path, config)?;
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

fn handle_sync(
    path: &Path,
    config: Config,
    sub: cmd::SyncSub,
    global_json: bool,
) -> Result<String, AideMemoError> {
    match sub {
        cmd::SyncSub::Ingest { wiki_root } => handle_ingest(
            path,
            config,
            cmd::IngestSub {
                wiki_root,
                incremental: true,
            },
        ),
        cmd::SyncSub::Pull {
            url,
            token,
            token_file,
            limit,
            watch,
            json,
        } => handle_sync_pull(
            path,
            config,
            url,
            token,
            token_file,
            limit,
            watch,
            json || global_json,
        ),
        cmd::SyncSub::Status { url, json } => handle_sync_status(path, url, json || global_json),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_sync_pull(
    store_path: &Path,
    config: Config,
    url: String,
    token: Option<String>,
    token_file: Option<PathBuf>,
    limit: Option<usize>,
    watch: Option<u64>,
    json: bool,
) -> Result<String, AideMemoError> {
    let key = url.trim_end_matches('/').to_string();
    let resolved_token = token
        .or_else(|| {
            token_file
                .as_ref()
                .and_then(|p| cmd::mcp_serve::read_token_file(p).ok())
        })
        .or_else(|| std::env::var("AIDEMEMO_MCP_AUTH_TOKEN").ok())
        .or_else(|| cmd::auth::load_token_for(&key));
    let batch_limit = limit.unwrap_or(5000);

    if let Some(interval_sec) = watch {
        if interval_sec == 0 {
            return Err(AideMemoError::InvalidInput(
                "--watch SEC must be positive".into(),
            ));
        }
        // Long-running mode. Each iteration runs one full drain
        // (auto-pagination) then sleeps. SIGINT is delivered to the
        // process by the runtime; the std::thread::sleep wakes via
        // signal so the loop ends naturally on Ctrl-C.
        loop {
            match drain_pull(
                store_path,
                &config,
                &key,
                resolved_token.as_deref(),
                batch_limit,
            ) {
                Ok(summary) => render_pull_summary(&summary, json),
                Err(e) => {
                    if json {
                        let payload = serde_json::json!({"url": key, "error": e.to_string()});
                        println!("{}", payload);
                    } else {
                        eprintln!("error pulling {key}: {e}");
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(interval_sec));
        }
    }

    let summary = drain_pull(
        store_path,
        &config,
        &key,
        resolved_token.as_deref(),
        batch_limit,
    )?;
    Ok(render_pull_summary_string(&summary, json))
}

#[derive(Debug, Default, Clone, serde::Serialize)]
struct PullSummary {
    url: String,
    iterations: usize,
    entities_inserted: usize,
    entities_skipped: usize,
    facts_inserted: usize,
    facts_skipped: usize,
    relations_inserted: usize,
    relations_skipped: usize,
    errors: usize,
    cursor_entity: Option<String>,
    cursor_fact: Option<String>,
    cursor_entity_updated_at: Option<u64>,
    cursor_fact_updated_at: Option<u64>,
    elapsed_ms: u128,
}

fn drain_pull(
    store_path: &Path,
    config: &Config,
    key: &str,
    token: Option<&str>,
    batch_limit: usize,
) -> Result<PullSummary, AideMemoError> {
    let started = std::time::Instant::now();
    let cursor_path = sync_cursor_path(store_path);
    let mut summary = PullSummary {
        url: key.to_string(),
        ..Default::default()
    };

    // Cap iterations to protect against pathological loops where the
    // upstream keeps returning records but the cursor doesn't advance
    // (would indicate a server bug, not a normal condition).
    let max_iter = 1024;
    for _ in 0..max_iter {
        // Re-load on every iteration so concurrent writers (rare but
        // possible during local-tier writes) see the freshest cursor.
        let cursor_file = load_sync_cursor(&cursor_path);
        let prev = cursor_file.remotes.get(key).cloned().unwrap_or_default();

        let body = pull_one_batch(key, token, &prev, batch_limit)?;
        let wiki = AideMemo::open(store_path, config.clone())?;
        let stats = wiki.sync_import(&body)?;
        drop(wiki); // release redb lock before saving cursor + next loop

        let cycle_records = stats.entities_inserted
            + stats.entities_skipped
            + stats.facts_inserted
            + stats.facts_skipped
            + stats.relations_inserted
            + stats.relations_skipped;

        let entry = StoredCursor {
            entity: stats
                .new_cursor
                .entity
                .map(|e| e.0.to_string())
                .or_else(|| prev.entity.clone()),
            fact: stats
                .new_cursor
                .fact
                .map(|f| f.0.to_string())
                .or_else(|| prev.fact.clone()),
            entity_updated_at: stats
                .new_cursor
                .entity_updated_at
                .or(prev.entity_updated_at),
            fact_updated_at: stats.new_cursor.fact_updated_at.or(prev.fact_updated_at),
            last_pulled_at: aidememo_core::time::current_epoch_ms(),
        };
        let mut cursor_file_now = load_sync_cursor(&cursor_path);
        cursor_file_now
            .remotes
            .insert(key.to_string(), entry.clone());
        save_sync_cursor(&cursor_path, &cursor_file_now)?;

        summary.iterations += 1;
        summary.entities_inserted += stats.entities_inserted;
        summary.entities_skipped += stats.entities_skipped;
        summary.facts_inserted += stats.facts_inserted;
        summary.facts_skipped += stats.facts_skipped;
        summary.relations_inserted += stats.relations_inserted;
        summary.relations_skipped += stats.relations_skipped;
        summary.errors += stats.errors;
        summary.cursor_entity = entry.entity;
        summary.cursor_fact = entry.fact;
        summary.cursor_entity_updated_at = entry.entity_updated_at;
        summary.cursor_fact_updated_at = entry.fact_updated_at;

        // Stop conditions:
        //  - Upstream returned nothing (steady state — fully drained)
        //  - Server didn't move any cursor AND emitted no records
        //    (upstream has nothing newer than what we sent)
        if cycle_records == 0 {
            break;
        }
        // If the upstream returned strictly fewer records than the
        // batch limit, the next request would only return zero — skip
        // the extra round-trip.
        if cycle_records < batch_limit {
            break;
        }
    }

    summary.elapsed_ms = started.elapsed().as_millis();
    Ok(summary)
}

fn pull_one_batch(
    key: &str,
    token: Option<&str>,
    prev: &StoredCursor,
    batch_limit: usize,
) -> Result<String, AideMemoError> {
    use std::io::Read;
    let mut endpoint = format!("{}/sync/since?limit={}", key, batch_limit);
    if let Some(e) = &prev.entity {
        endpoint.push_str(&format!("&entity={}", e));
    }
    if let Some(f) = &prev.fact {
        endpoint.push_str(&format!("&fact={}", f));
    }
    if let Some(t) = prev.entity_updated_at {
        endpoint.push_str(&format!("&entity_updated_at={}", t));
    }
    if let Some(t) = prev.fact_updated_at {
        endpoint.push_str(&format!("&fact_updated_at={}", t));
    }

    tracing::debug!(target: "aidememo::sync", "GET {}", endpoint);
    let mut req = ureq::get(&endpoint);
    if let Some(t) = token {
        if !t.is_empty() {
            req = req.set("Authorization", &format!("Bearer {}", t));
        }
    }
    let resp = req
        .call()
        .map_err(|e| AideMemoError::Internal(format!("sync pull HTTP failed: {e}")))?;
    if resp.status() != 200 {
        return Err(AideMemoError::Internal(format!(
            "sync pull returned HTTP {}: {}",
            resp.status(),
            resp.into_string().unwrap_or_default()
        )));
    }
    let mut body = String::new();
    resp.into_reader()
        .read_to_string(&mut body)
        .map_err(|e| AideMemoError::Internal(format!("sync pull body read: {e}")))?;
    Ok(body)
}

fn render_pull_summary_string(s: &PullSummary, json: bool) -> String {
    if json {
        serde_json::to_string_pretty(s).unwrap_or_else(|_| "{}".to_string())
    } else {
        format!(
            "pulled from {} in {} iter, {} ms: +{} entities, +{} facts, \
             +{} relations ({} skipped, {} errors); cursor → entity={:?} fact={:?}",
            s.url,
            s.iterations,
            s.elapsed_ms,
            s.entities_inserted,
            s.facts_inserted,
            s.relations_inserted,
            s.entities_skipped + s.facts_skipped + s.relations_skipped,
            s.errors,
            s.cursor_entity,
            s.cursor_fact,
        )
    }
}

fn render_pull_summary(s: &PullSummary, json: bool) {
    println!("{}", render_pull_summary_string(s, json));
}

fn handle_sync_status(
    store_path: &Path,
    url: Option<String>,
    json: bool,
) -> Result<String, AideMemoError> {
    let cursor_path = sync_cursor_path(store_path);
    let cursor = load_sync_cursor(&cursor_path);
    let now = aidememo_core::time::current_epoch_ms();

    if json {
        // Always emit a stable shape regardless of which subset.
        let mut entries: Vec<serde_json::Value> = Vec::new();
        let iter: Box<dyn Iterator<Item = (&String, &StoredCursor)>> = match &url {
            Some(u) => {
                let key = u.trim_end_matches('/').to_string();
                Box::new(cursor.remotes.iter().filter(move |(k, _)| **k == key))
            }
            None => Box::new(cursor.remotes.iter()),
        };
        for (k, e) in iter {
            entries.push(serde_json::json!({
                "url": k,
                "entity": e.entity,
                "fact": e.fact,
                "entity_updated_at": e.entity_updated_at,
                "fact_updated_at": e.fact_updated_at,
                "last_pulled_at": e.last_pulled_at,
                "age_ms": now.saturating_sub(e.last_pulled_at),
            }));
        }
        return Ok(serde_json::json!({
            "store": store_path.display().to_string(),
            "cursor_file": cursor_path.display().to_string(),
            "remotes": entries,
        })
        .to_string());
    }

    if cursor.remotes.is_empty() {
        return Ok(format!(
            "no remotes recorded (cursor file: {})",
            cursor_path.display()
        ));
    }
    let mut out = format!("sync status — {}\n", cursor_path.display());
    let filter = url.as_ref().map(|u| u.trim_end_matches('/').to_string());
    for (k, e) in &cursor.remotes {
        if let Some(f) = &filter {
            if k != f {
                continue;
            }
        }
        let age_sec = now.saturating_sub(e.last_pulled_at) / 1000;
        out.push_str(&format!(
            "  {}\n    last_pulled_at: {} ({} sec ago)\n    \
             cursor: entity={:?} fact={:?}\n    \
             updated_at: entity={:?} fact={:?}\n",
            k, e.last_pulled_at, age_sec, e.entity, e.fact, e.entity_updated_at, e.fact_updated_at
        ));
    }
    Ok(out)
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize, Clone)]
struct StoredCursor {
    entity: Option<String>,
    fact: Option<String>,
    /// Phase 2.5 — high-water `updated_at` watermarks. `#[serde(default)]`
    /// so existing cursor files (Phase 2 era) keep loading without
    /// resetting the ULID watermark.
    #[serde(default)]
    entity_updated_at: Option<u64>,
    #[serde(default)]
    fact_updated_at: Option<u64>,
    last_pulled_at: u64,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct SyncCursorFile {
    /// Keyed by the upstream base URL, trimmed of trailing slash.
    remotes: std::collections::HashMap<String, StoredCursor>,
}

fn sync_cursor_path(store_path: &Path) -> std::path::PathBuf {
    let mut p = store_path.to_path_buf();
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wiki".into());
    p.set_file_name(format!("{}.sync.json", stem));
    p
}

fn load_sync_cursor(path: &Path) -> SyncCursorFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_sync_cursor(path: &Path, cursor: &SyncCursorFile) -> Result<(), AideMemoError> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(cursor).map_err(|e| AideMemoError::Serialize {
        context: "sync cursor".to_string(),
        source: e,
    })?;
    std::fs::write(&tmp, bytes)
        .map_err(|e| AideMemoError::Internal(format!("write sync cursor: {e}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| AideMemoError::Internal(format!("rename sync cursor: {e}")))?;
    Ok(())
}

fn handle_config(config: Config, sub: cmd::ConfigSub) -> Result<String, AideMemoError> {
    match sub {
        cmd::ConfigSub::List => {
            let json =
                serde_json::to_string_pretty(&config).map_err(|e| AideMemoError::Serialize {
                    context: "config".to_string(),
                    source: e,
                })?;
            Ok(json)
        }
        cmd::ConfigSub::Get { key } => match config.get(&key) {
            Some(value) => Ok(value),
            None => Err(AideMemoError::ConfigKeyNotFound(key)),
        },
        cmd::ConfigSub::Set { key, value } => {
            let mut config = config;
            config.set(&key, &value)?;
            config.save()?;
            Ok(format!("Set {} = {}", key, value))
        }
    }
}

fn handle_model(config: Config, sub: cmd::ModelSub) -> Result<String, AideMemoError> {
    cmd::model::run_model(config, sub)
}

// Helper functions

fn parse_entity_type(s: Option<String>) -> Option<EntityType> {
    s.map(|t| EntityType::parse(&t))
}

pub(crate) fn parse_fact_type(s: Option<String>) -> Option<FactType> {
    // Delegates to FactType::parse so all CLI/MCP/binding sites
    // share one alias table (decision/decide, preference/pref/
    // preferences, lesson/lessons/learning, error/err/mistake/
    // failure, etc.). New types added in aidememo-core land here free.
    s.map(|t| FactType::parse(&t))
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
pub(crate) fn parse_iso_to_epoch_ms(s: &str) -> Result<u64, AideMemoError> {
    let s = s.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return u64::try_from(dt.timestamp_millis())
            .map_err(|_| AideMemoError::InvalidInput(format!("date out of range: {s}")));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| AideMemoError::InvalidInput(format!("invalid date: {s}")))?
            .and_utc();
        return u64::try_from(dt.timestamp_millis())
            .map_err(|_| AideMemoError::InvalidInput(format!("date out of range: {s}")));
    }
    Err(AideMemoError::InvalidInput(format!(
        "expected YYYY-MM-DD or RFC3339 date, got: {s}"
    )))
}

/// Parse a relative-duration string like `30d`, `12h`, `4w`, `1y`, `45m` into milliseconds.
pub(crate) fn parse_duration_to_ms(s: &str) -> Result<u64, AideMemoError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(AideMemoError::InvalidInput("empty duration".to_string()));
    }
    // Split numeric prefix from unit suffix (last char).
    let (num_part, unit) = match s.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&s[..i], c.to_ascii_lowercase()),
        _ => {
            return Err(AideMemoError::InvalidInput(format!(
                "duration needs a unit suffix (s/m/h/d/w/y), got: {s}"
            )));
        }
    };
    let n: u64 = num_part
        .parse()
        .map_err(|_| AideMemoError::InvalidInput(format!("invalid number in duration: {s}")))?;
    let ms = match unit {
        's' => n * 1_000,
        'm' => n * 60 * 1_000,
        'h' => n * 60 * 60 * 1_000,
        'd' => n * 24 * 60 * 60 * 1_000,
        'w' => n * 7 * 24 * 60 * 60 * 1_000,
        'y' => n * 365 * 24 * 60 * 60 * 1_000,
        _ => {
            return Err(AideMemoError::InvalidInput(format!(
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
) -> Result<Option<u64>, AideMemoError> {
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

fn resolve_until(until: Option<&str>) -> Result<Option<u64>, AideMemoError> {
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
