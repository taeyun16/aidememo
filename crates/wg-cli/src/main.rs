//! WikiGraph CLI — Structured index engine for LLM wikis.

mod cmd;
mod output;

use std::path::PathBuf;
use std::process::exit;
use wg_core::{
    Config, EntityInput, EntitySort, EntityType, ExportScope, FactInput, FactListOpts, FactType,
    LintReport, ListOpts, QueryOpts, SearchOpts, TraverseDirection, TraverseOpts, WgError,
    WikiGraph,
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
        cmd::Command::Watch(sub) => cmd::watch::run_watch(sub.wiki_root, sub.interval),
        cmd::Command::McpServe(sub) => cmd::mcp_serve::run_mcp_serve(sub.port, sub.wiki_root),
        cmd::Command::Mcp(sub) => cmd::mcp_stdio::run_mcp(sub.wiki_root),
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

fn fmt(json: bool) -> output::Format {
    if json {
        output::Format::Json
    } else {
        output::Format::Table
    }
}

fn handle_entity(
    path: &PathBuf,
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
        cmd::EntitySub::Get { name } => with_wiki(path, config, |wiki| {
            let entity = wiki.entity_get(&name)?;
            output::format_entity(&entity, fmt(json))
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
            output::format_entity_list(&entities, fmt(json))
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

fn handle_fact(
    path: &PathBuf,
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
            with_wiki_mut(path, config, |wiki| {
                let entity_ids = entities.map(|names| {
                    names
                        .iter()
                        .filter_map(|n| wiki.resolve_entity(n).ok())
                        .collect()
                });

                let id = wiki.add_fact(FactInput {
                    content: content.clone(),
                    fact_type: parse_fact_type(fact_type),
                    entity_ids,
                    tags: parse_tags(tags),
                    source,
                    source_confidence: confidence,
                    observed_at: observed_at_ms,
                })?;
                Ok(format!("Added fact with ID {}", id))
            })
        }
        cmd::FactSub::Get { id } => with_wiki(path, config, |wiki| {
            let fact_id = wg_core::FactId(
                wg_core::ulid::Ulid::from_string(&id)
                    .map_err(|_| WgError::InvalidInput(format!("Invalid fact ID: {}", id)))?,
            );
            let fact = wiki.fact_get(&fact_id)?;
            output::format_fact(&fact, &wiki, fmt(json))
        }),
        cmd::FactSub::List {
            fact_type,
            entity,
            min_confidence,
            since,
            until,
            last,
            limit,
        } => {
            let since_ms = resolve_since(since.as_deref(), last.as_deref())?;
            let until_ms = resolve_until(until.as_deref())?;
            with_wiki(path, config, |wiki| {
                let entity_id = entity.map(|n| wiki.resolve_entity(&n).ok()).flatten();
                let facts = wiki.fact_list(FactListOpts {
                    fact_type: parse_fact_type(fact_type),
                    entity_id,
                    min_confidence,
                    limit,
                    offset: 0,
                    since: since_ms,
                    until: until_ms,
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
    }
}

fn handle_traverse(
    path: &PathBuf,
    config: Config,
    sub: cmd::TraverseSub,
    json: bool,
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
        output::format_traverse(&result, fmt(json))
    })
}

fn handle_path(
    path: &PathBuf,
    config: Config,
    sub: cmd::PathSub,
    json: bool,
) -> Result<String, WgError> {
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
    path: &PathBuf,
    config: Config,
    sub: cmd::SearchSub,
    json: bool,
) -> Result<String, WgError> {
    let default_limit = config.search.default_limit;
    let bm25_weight = config.search.bm25_weight;
    let semantic_weight = config.search.semantic_weight;
    let since_ms = resolve_since(sub.since.as_deref(), sub.last.as_deref())?;
    let until_ms = resolve_until(sub.until.as_deref())?;
    with_wiki_mut(path, config, |wiki| {
        let session_id = wg_core::ulid::Ulid::new().to_string();
        let opts = SearchOpts {
            limit: sub.limit.or(Some(default_limit)),
            min_confidence: sub.min_confidence,
            entity_filter: None,
            bm25_weight,
            semantic_weight,
            since: since_ms,
            until: until_ms,
            session_id: Some(session_id.clone()),
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
        output::format_search_results(&results, &wiki, format)
    })
}

fn handle_query(
    path: &PathBuf,
    config: Config,
    sub: cmd::QuerySub,
    json: bool,
) -> Result<String, WgError> {
    let since_ms = resolve_since(None, sub.last.as_deref())?;
    with_wiki(path, config, |wiki| {
        let opts = QueryOpts {
            search_limit: sub.limit.unwrap_or(10),
            depth: sub.depth.unwrap_or(2),
            recent_limit: sub.recent_limit.unwrap_or(10),
            since: since_ms,
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

fn handle_lint(
    path: &PathBuf,
    config: Config,
    sub: cmd::LintSub,
    json: bool,
) -> Result<String, WgError> {
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

fn handle_stats(
    path: &PathBuf,
    config: Config,
    _sub: cmd::StatsSub,
    json: bool,
) -> Result<String, WgError> {
    with_wiki(path, config, |wiki| {
        let stats = wiki.stats()?;
        output::format_stats(&stats, fmt(json))
    })
}

fn handle_ingest(path: &PathBuf, config: Config, sub: cmd::IngestSub) -> Result<String, WgError> {
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
    cmd::model::run_model(config, sub)
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
fn parse_iso_to_epoch_ms(s: &str) -> Result<u64, WgError> {
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
fn parse_duration_to_ms(s: &str) -> Result<u64, WgError> {
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
fn resolve_since(since: Option<&str>, last: Option<&str>) -> Result<Option<u64>, WgError> {
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
