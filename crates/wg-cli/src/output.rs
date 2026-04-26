//! Output formatting for CLI.

use comfy_table::Table;
use wg_core::{
    EntityRecord, EntitySummary, FactRecord, LintReport, QueryResult, Result, SearchResult,
    StoreStats, TraverseResult, WgError, WikiGraph,
};

fn fact_one_line(f: &FactRecord) -> String {
    let snippet: String = f.content.chars().take(70).collect();
    let id_short = &f.id.to_string()[..8];
    let when = format_when(f.observed_at, f.created_at);
    format!("  [{}] {}  ({})", id_short, snippet, when)
}

#[derive(Debug, Clone, Copy)]
pub enum Format {
    Table,
    Json,
}

/// Format an epoch-ms timestamp for human display.
///
/// Returns a date (`2024-03-15`) with a relative hint (`(3d ago)`) when recent.
/// `observed_at` (real-world time) takes precedence over `created_at` (DB insertion).
fn format_when(observed_at: Option<u64>, created_at: u64) -> String {
    use chrono::{DateTime, Utc};
    let (ts, marker) = match observed_at {
        Some(o) => (o, ""),
        None => (created_at, " (added)"),
    };
    let secs = (ts / 1000) as i64;
    let nanos = ((ts % 1000) * 1_000_000) as u32;
    let dt = match DateTime::<Utc>::from_timestamp(secs, nanos) {
        Some(d) => d,
        None => return "-".to_string(),
    };
    let now = Utc::now();
    let date = dt.format("%Y-%m-%d").to_string();
    let delta = now.signed_duration_since(dt);
    let rel = if delta.num_seconds() < 0 {
        // future timestamp — just show date
        String::new()
    } else if delta.num_days() == 0 {
        " (today)".to_string()
    } else if delta.num_days() < 30 {
        format!(" ({}d ago)", delta.num_days())
    } else if delta.num_days() < 365 {
        format!(" ({}mo ago)", delta.num_days() / 30)
    } else {
        format!(" ({}y ago)", delta.num_days() / 365)
    };
    format!("{date}{rel}{marker}")
}

pub fn format_entity(entity: &EntityRecord, format: Format) -> Result<String> {
    match format {
        Format::Json => serde_json::to_string_pretty(entity).map_err(|e| WgError::Serialize {
            context: "entity".to_string(),
            source: e,
        }),
        Format::Table => {
            let mut table = Table::new();
            table.set_header(vec!["Field", "Value"]);
            table.add_row(vec!["ID", &entity.id.to_string()]);
            table.add_row(vec!["Name", &entity.name]);
            table.add_row(vec!["Type", &entity.entity_type.to_string()]);
            table.add_row(vec!["Tags", &entity.tags.join(", ")]);
            table.add_row(vec!["Aliases", &entity.aliases.join(", ")]);
            if let Some(ref source) = entity.source_page {
                table.add_row(vec!["Source", source]);
            }
            if let Some(ref summary) = entity.summary {
                table.add_row(vec!["Summary", summary]);
            }
            Ok(table.to_string())
        }
    }
}

/// Render the "compiled view" of an entity: summary + recent facts.
pub fn format_entity_show(
    entity: &EntityRecord,
    recent: &[FactRecord],
    format: Format,
) -> Result<String> {
    if let Format::Json = format {
        let payload = serde_json::json!({
            "entity": entity,
            "recent_facts": recent,
        });
        return serde_json::to_string_pretty(&payload).map_err(|e| WgError::Serialize {
            context: "entity show".to_string(),
            source: e,
        });
    }
    let mut out = String::new();
    out.push_str(&format!("# {} ({})\n", entity.name, entity.entity_type));
    if !entity.aliases.is_empty() {
        out.push_str(&format!("  aliases: {}\n", entity.aliases.join(", ")));
    }
    if !entity.tags.is_empty() {
        out.push_str(&format!("  tags: {}\n", entity.tags.join(", ")));
    }
    out.push('\n');

    match &entity.summary {
        Some(text) => {
            out.push_str("Summary:\n");
            for line in text.lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
            if let Some(ts) = entity.summary_updated_at {
                out.push_str(&format!("  (updated {})\n", format_when(None, ts)));
            }
        }
        None => {
            out.push_str("Summary: (none — set with `wg entity describe <name> \"...\"`)\n");
        }
    }
    out.push('\n');

    if recent.is_empty() {
        out.push_str("Recent facts: (none)\n");
    } else {
        out.push_str(&format!("Recent facts ({}):\n", recent.len()));
        for f in recent {
            out.push_str(&fact_one_line(f));
            out.push('\n');
        }
    }
    Ok(out)
}

pub fn format_entity_list(entities: &[EntitySummary], format: Format) -> Result<String> {
    match format {
        Format::Json => serde_json::to_string_pretty(entities).map_err(|e| WgError::Serialize {
            context: "entity list".to_string(),
            source: e,
        }),
        Format::Table => {
            let mut table = Table::new();
            table.set_header(vec!["Name", "Type", "Facts", "Tags"]);

            for entity in entities {
                table.add_row(vec![
                    entity.name.as_str(),
                    entity.entity_type.to_string().as_str(),
                    entity.fact_count.to_string().as_str(),
                    entity.tags.join(", ").as_str(),
                ]);
            }

            Ok(table.to_string())
        }
    }
}

pub fn format_fact(fact: &FactRecord, wiki: &WikiGraph, format: Format) -> Result<String> {
    if let Format::Json = format {
        return serde_json::to_string_pretty(fact).map_err(|e| WgError::Serialize {
            context: "fact".to_string(),
            source: e,
        });
    }

    // Get entity names
    let entity_names: Vec<String> = fact
        .entity_ids
        .iter()
        .filter_map(|id| wiki.entity_get_by_id(*id).ok())
        .map(|e| e.name)
        .collect();

    let mut table = Table::new();
    table.set_header(vec!["Field", "Value"]);
    table.add_row(vec!["ID", &fact.id.to_string()]);
    table.add_row(vec!["Content", &fact.content]);
    table.add_row(vec!["Type", &fact.fact_type.to_string()]);
    table.add_row(vec!["Entities", &entity_names.join(", ")]);
    table.add_row(vec!["Tags", &fact.tags.join(", ")]);
    if let Some(ref source) = fact.source {
        table.add_row(vec!["Source", source]);
    }
    table.add_row(vec![
        "Confidence",
        &format!("{:.2}", fact.source_confidence),
    ]);
    table.add_row(vec!["Relevance", &format!("{:.2}", fact.relevance_score)]);
    table.add_row(vec!["Accessed", &fact.access_count.to_string()]);
    table.add_row(vec![
        "When",
        &format_when(fact.observed_at, fact.created_at),
    ]);

    Ok(table.to_string())
}

pub fn format_fact_list(facts: &[FactRecord], wiki: &WikiGraph, format: Format) -> Result<String> {
    if let Format::Json = format {
        return serde_json::to_string_pretty(facts).map_err(|e| WgError::Serialize {
            context: "fact list".to_string(),
            source: e,
        });
    }
    let mut table = Table::new();
    table.set_header(vec![
        "ID",
        "Content",
        "Type",
        "Entities",
        "Confidence",
        "When",
    ]);

    for fact in facts {
        let entity_names: Vec<String> = fact
            .entity_ids
            .iter()
            .filter_map(|id| wiki.entity_get_by_id(*id).ok())
            .map(|e| e.name)
            .collect();

        // Truncate content if too long
        let content = if fact.content.len() > 60 {
            format!("{}...", &fact.content[..60])
        } else {
            fact.content.clone()
        };

        let when = format_when(fact.observed_at, fact.created_at);

        table.add_row(vec![
            &fact.id.to_string()[..8],
            content.as_str(),
            fact.fact_type.to_string().as_str(),
            entity_names.join(", ").as_str(),
            &format!("{:.2}", fact.source_confidence),
            when.as_str(),
        ]);
    }

    Ok(table.to_string())
}

pub fn format_traverse(result: &TraverseResult, format: Format) -> Result<String> {
    match format {
        Format::Json => serde_json::to_string_pretty(result).map_err(|e| WgError::Serialize {
            context: "traverse result".to_string(),
            source: e,
        }),
        Format::Table => {
            let mut table = Table::new();
            table.set_header(vec!["Name", "Type", "Facts", "Tags"]);

            for entity in &result.entities {
                table.add_row(vec![
                    entity.name.as_str(),
                    entity.entity_type.to_string().as_str(),
                    entity.fact_count.to_string().as_str(),
                    entity.tags.join(", ").as_str(),
                ]);
            }

            let mut output = format!(
                "Visited {} entities, {} relations\n\n",
                result.visited_count,
                result.relations.len()
            );
            output.push_str(&table.to_string());
            Ok(output)
        }
    }
}

pub fn format_search_results(
    results: &[SearchResult],
    _wiki: &WikiGraph,
    format: Format,
) -> Result<String> {
    match format {
        Format::Json => serde_json::to_string_pretty(results).map_err(|e| WgError::Serialize {
            context: "search results".to_string(),
            source: e,
        }),
        Format::Table => {
            let mut table = Table::new();
            table.set_header(vec![
                "#", "Content", "Type", "Entities", "Score", "When", "Source",
            ]);

            for result in results {
                // Truncate content if too long
                let content = if result.content.len() > 50 {
                    format!("{}...", &result.content[..50])
                } else {
                    result.content.clone()
                };

                let when = format_when(result.observed_at, result.created_at);
                let src = result.source.as_deref().unwrap_or("");

                table.add_row(vec![
                    result.rank.to_string().as_str(),
                    content.as_str(),
                    result.fact_type.to_string().as_str(),
                    result.entity_names.join(", ").as_str(),
                    &format!("{:.3}", result.score),
                    when.as_str(),
                    src,
                ]);
            }

            Ok(table.to_string())
        }
    }
}

pub fn format_lint_report(report: &LintReport, format: Format) -> Result<String> {
    match format {
        Format::Json => serde_json::to_string_pretty(report).map_err(|e| WgError::Serialize {
            context: "lint report".to_string(),
            source: e,
        }),
        Format::Table => {
            let mut output = String::new();
            output.push_str("Graph Health Report\n");
            output.push_str("====================\n\n");
            output.push_str(&format!("Entities: {}\n", report.entity_count));
            output.push_str(&format!("Facts: {}\n", report.fact_count));
            output.push_str(&format!("Relations: {}\n\n", report.relation_count));

            if report.issues.is_empty() {
                output.push_str("No issues found.\n");
            } else {
                let mut table = Table::new();
                table.set_header(vec!["Severity", "Code", "Message"]);

                for issue in &report.issues {
                    table.add_row(vec![
                        issue.severity.to_string().as_str(),
                        issue.code.as_str(),
                        issue.message.as_str(),
                    ]);
                }

                output.push_str(&table.to_string());
            }

            Ok(output)
        }
    }
}

pub fn format_query_result(result: &QueryResult) -> Result<String> {
    let mut out = String::new();
    out.push_str(&format!("Query: {}\n", result.topic));

    if let Some(ref e) = result.entity {
        out.push_str(&format!(
            "Entity: {} ({}) — aliases: [{}], tags: [{}]\n",
            e.name,
            e.entity_type,
            e.aliases.join(", "),
            e.tags.join(", ")
        ));
    } else {
        out.push_str("Entity: (no exact match — search-only result)\n");
    }
    out.push('\n');

    if result.search.is_empty() {
        out.push_str("Search: no matches.\n\n");
    } else {
        out.push_str(&format!("Search ({} hits):\n", result.search.len()));
        for r in &result.search {
            let snippet: String = r.content.chars().take(80).collect();
            out.push_str(&format!(
                "  {}. [{}] {}  (score={:.3})\n",
                r.rank, r.fact_id, snippet, r.score
            ));
        }
        out.push('\n');
    }

    if !result.related.is_empty() {
        out.push_str(&format!("Related entities ({}):\n", result.related.len()));
        for e in &result.related {
            out.push_str(&format!(
                "  - {} ({}) [{} facts]\n",
                e.name, e.entity_type, e.fact_count
            ));
        }
        out.push('\n');
    }

    if !result.recent_facts.is_empty() {
        out.push_str(&format!("Recent facts ({}):\n", result.recent_facts.len()));
        for f in &result.recent_facts {
            let snippet: String = f.content.chars().take(80).collect();
            let when = format_when(f.observed_at, f.created_at);
            out.push_str(&format!(
                "  [{}] {}  ({})\n",
                &f.id.to_string()[..8],
                snippet,
                when
            ));
        }
    }

    Ok(out)
}

pub fn format_stats(stats: &StoreStats, format: Format) -> Result<String> {
    if let Format::Json = format {
        return serde_json::to_string_pretty(stats).map_err(|e| WgError::Serialize {
            context: "stats".to_string(),
            source: e,
        });
    }

    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);

    table.add_row(vec!["Entities", &stats.entity_count.to_string()]);
    table.add_row(vec!["Facts", &stats.fact_count.to_string()]);
    table.add_row(vec!["Relations", &stats.relation_count.to_string()]);
    table.add_row(vec!["Store Size", &format_size(stats.total_size_bytes)]);

    if let Some(last_ingest) = stats.last_ingest_at {
        let date = chrono_from_ms(last_ingest);
        table.add_row(vec!["Last Ingest", &date]);
    }

    Ok(table.to_string())
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn chrono_from_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let naive = chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|dt| dt.naive_utc())
        .unwrap_or_default();
    naive.format("%Y-%m-%d %H:%M:%S").to_string()
}
