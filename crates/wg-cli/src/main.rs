//! WikiGraph CLI — Structured index engine for LLM wikis.

mod cmd;
mod output;

use std::path::PathBuf;
use std::process::exit;
use wg_core::{
    Config, EntityInput, EntitySort, EntityType, EntityUpdate, ExportScope, FactInput,
    FactListOpts, FactType, FactUpdate, IngestStats, ListOpts, RelationType, SearchOpts,
    TraverseDirection, TraverseOpts, WgError, WikiGraph,
};

fn main() {
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

    // Get store path
    let store_path = args
        .store_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(&config.store.path));

    // Handle commands
    let result = match args.command {
        cmd::Command::Entity(sub) => handle_entity(&store_path, config, sub),
        cmd::Command::Fact(sub) => handle_fact(&store_path, config, sub),
        cmd::Command::Traverse(sub) => handle_traverse(&store_path, config, sub),
        cmd::Command::Path(sub) => handle_path(&store_path, config, sub),
        cmd::Command::Search(sub) => handle_search(&store_path, config, sub),
        cmd::Command::Lint(sub) => handle_lint(&store_path, config, sub),
        cmd::Command::Export(sub) => handle_export(&store_path, config, sub),
        cmd::Command::Import(sub) => handle_import(&store_path, config, sub),
        cmd::Command::Stats(sub) => handle_stats(&store_path, config, sub),
        cmd::Command::Ingest(sub) => handle_ingest(&store_path, config, sub),
        cmd::Command::Sync(sub) => handle_sync(&store_path, config, sub),
        cmd::Command::Config(sub) => handle_config(config, sub),
        cmd::Command::Model(sub) => handle_model(config, sub),
        cmd::Command::Init(sub) => cmd::init::run_init(sub.wiki_root, sub.no_ingest),
        cmd::Command::Watch(sub) => cmd::watch::run_watch(sub.wiki_root, sub.interval),
    };

    match result {
        Ok(output) => {
            println!("{}", output);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            exit(1);
        }
    }
}

fn with_wiki<F>(path: &PathBuf, config: Config, f: F) -> Result<String, WgError>
where
    F: FnOnce(WikiGraph) -> Result<String, WgError>,
{
    let wiki = WikiGraph::open(path, config)?;
    f(wiki)
}

fn with_wiki_mut<F>(path: &PathBuf, config: Config, f: F) -> Result<String, WgError>
where
    F: FnOnce(&mut WikiGraph) -> Result<String, WgError>,
{
    let mut wiki = WikiGraph::open(path, config)?;
    f(&mut wiki)
}

fn handle_entity(path: &PathBuf, config: Config, sub: cmd::EntitySub) -> Result<String, WgError> {
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
        cmd::EntitySub::Get { name } => with_wiki(path, config, |wiki| {
            let entity = wiki.entity_get(&name)?;
            output::format_entity(&entity, output::Format::Table)
        }),
        cmd::EntitySub::List {
            sort,
            entity_type,
            min_facts,
            limit,
        } => with_wiki(path, config, |wiki| {
            let entities = wiki.entity_list(ListOpts {
                entity_type: parse_entity_type(entity_type),
                min_facts,
                sort_by: parse_entity_sort(sort),
                limit,
                offset: 0,
            })?;
            output::format_entity_list(&entities, output::Format::Table)
        }),
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
    }
}

fn handle_fact(path: &PathBuf, config: Config, sub: cmd::FactSub) -> Result<String, WgError> {
    match sub {
        cmd::FactSub::Add {
            content,
            fact_type,
            entities,
            tags,
            source,
            confidence,
        } => with_wiki_mut(path, config, |wiki| {
            let entity_ids = entities.map(|names| {
                names
                    .iter()
                    .filter_map(|n| wiki.resolve_entity(n).ok())
                    .collect()
            });

            let id = wiki.fact_add(FactInput {
                content: content.clone(),
                fact_type: parse_fact_type(fact_type),
                entity_ids,
                tags: parse_tags(tags),
                source,
                source_confidence: confidence,
            })?;
            Ok(format!("Added fact with ID {}", id))
        }),
        cmd::FactSub::Get { id } => with_wiki(path, config, |wiki| {
            let fact_id = wg_core::FactId(
                wg_core::ulid::Ulid::from_string(&id)
                    .map_err(|_| WgError::InvalidInput(format!("Invalid fact ID: {}", id)))?,
            );
            let fact = wiki.fact_get(&fact_id)?;
            output::format_fact(&fact, &wiki)
        }),
        cmd::FactSub::List {
            fact_type,
            entity,
            min_confidence,
            limit,
        } => with_wiki(path, config, |wiki| {
            let entity_id = entity.map(|n| wiki.resolve_entity(&n).ok()).flatten();
            let facts = wiki.fact_list(FactListOpts {
                fact_type: parse_fact_type(fact_type),
                entity_id,
                min_confidence,
                limit,
                offset: 0,
            })?;
            output::format_fact_list(&facts, &wiki)
        }),
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
    }
}

fn handle_traverse(
    path: &PathBuf,
    config: Config,
    sub: cmd::TraverseSub,
) -> Result<String, WgError> {
    with_wiki(path, config, |wiki| {
        let result = wiki.traverse(
            &sub.entity,
            TraverseOpts {
                depth: sub.depth.unwrap_or(2),
                relation_types: None,
                direction: TraverseDirection::Forward,
            },
        )?;
        output::format_traverse(&result, output::Format::Table)
    })
}

fn handle_path(path: &PathBuf, config: Config, sub: cmd::PathSub) -> Result<String, WgError> {
    with_wiki(path, config, |wiki| {
        match wiki.path_find(&sub.from, &sub.to)? {
            Some(path_steps) => {
                let mut output = String::new();
                output.push_str(&format!("Path from '{}' to '{}':\n", sub.from, sub.to));
                for (i, step) in path_steps.iter().enumerate() {
                    let from_name = wiki
                        .entity_get_by_id(step.from)
                        .map(|e| e.name)
                        .unwrap_or_default();
                    let to_name = wiki
                        .entity_get_by_id(step.to)
                        .map(|e| e.name)
                        .unwrap_or_default();
                    output.push_str(&format!(
                        "  {}: {} --{}--> {}\n",
                        i + 1,
                        from_name,
                        step.relation_type,
                        to_name
                    ));
                }
                Ok(output)
            }
            None => Ok(format!("No path found from '{}' to '{}'", sub.from, sub.to)),
        }
    })
}

fn handle_search(path: &PathBuf, config: Config, sub: cmd::SearchSub) -> Result<String, WgError> {
    let default_limit = config.search.default_limit;
    let bm25_weight = config.search.bm25_weight;
    let semantic_weight = config.search.semantic_weight;
    with_wiki(path, config, |wiki| {
        let opts = SearchOpts {
            limit: sub.limit.or(Some(default_limit)),
            min_confidence: sub.min_confidence,
            entity_filter: None,
            bm25_weight,
            semantic_weight,
        };

        let results = if let Some(ref start) = sub.traverse_from {
            let depth = sub.traverse_depth.unwrap_or(2);
            wiki.search_with_traverse(&sub.query, start, depth, opts)?
        } else {
            wiki.search(&sub.query, opts)?
        };

        let format = if sub.json {
            output::Format::Json
        } else {
            output::Format::Table
        };
        output::format_search_results(&results, &wiki, format)
    })
}

fn handle_lint(path: &PathBuf, config: Config, sub: cmd::LintSub) -> Result<String, WgError> {
    with_wiki(path, config, |wiki| {
        let report = wiki.lint()?;
        let format = if sub.json {
            output::Format::Json
        } else {
            output::Format::Table
        };
        output::format_lint_report(&report, format)
    })
}

fn handle_export(path: &PathBuf, config: Config, sub: cmd::ExportSub) -> Result<String, WgError> {
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

fn handle_import(path: &PathBuf, config: Config, sub: cmd::ImportSub) -> Result<String, WgError> {
    with_wiki_mut(path, config, |wiki| {
        let content = if sub.path.is_some() {
            std::fs::read_to_string(sub.path.as_ref().unwrap())
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

fn handle_stats(path: &PathBuf, config: Config, _sub: cmd::StatsSub) -> Result<String, WgError> {
    with_wiki(path, config, |wiki| {
        let stats = wiki.stats()?;
        output::format_stats(&stats)
    })
}

fn handle_ingest(path: &PathBuf, config: Config, sub: cmd::IngestSub) -> Result<String, WgError> {
    let mut wiki = WikiGraph::open(path, config)?;
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

fn handle_sync(path: &PathBuf, config: Config, sub: cmd::SyncSub) -> Result<String, WgError> {
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
    match sub {
        cmd::ModelSub::Status => Ok(format!(
            "Model: {} (not yet loaded)\nCache dir: {}",
            config.model.name, config.model.cache_dir
        )),
        cmd::ModelSub::Download { name } => Err(WgError::InvalidInput(
            "Model download not yet implemented".to_string(),
        )),
        cmd::ModelSub::RebuildVectors => Err(WgError::InvalidInput(
            "Vector rebuild not yet implemented".to_string(),
        )),
    }
}

// Helper functions

fn parse_entity_type(s: Option<String>) -> Option<EntityType> {
    s.map(|t| match t.to_lowercase().as_str() {
        "technology" | "tech" => EntityType::Technology,
        "concept" => EntityType::Concept,
        "comparison" | "compare" => EntityType::Comparison,
        "query" | "question" => EntityType::Query,
        "person" => EntityType::Person,
        "team" => EntityType::Team,
        _ => EntityType::Unknown,
    })
}

fn parse_fact_type(s: Option<String>) -> Option<FactType> {
    s.map(|t| match t.to_lowercase().as_str() {
        "decision" | "decide" => FactType::Decision,
        "pattern" => FactType::Pattern,
        "convention" | "convention" => FactType::Convention,
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
