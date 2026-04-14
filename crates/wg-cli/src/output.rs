//! Output formatting for CLI.

use comfy_table::Table;
use wg_core::{
    EntityRecord, EntitySummary, FactRecord, LintReport, Result, SearchResult, StoreStats,
    TraverseResult, WgError, WikiGraph,
};

#[derive(Debug, Clone, Copy)]
pub enum Format {
    Table,
    Json,
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
            Ok(table.to_string())
        }
    }
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

pub fn format_fact(fact: &FactRecord, wiki: &WikiGraph) -> Result<String> {
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

    Ok(table.to_string())
}

pub fn format_fact_list(facts: &[FactRecord], wiki: &WikiGraph) -> Result<String> {
    let mut table = Table::new();
    table.set_header(vec!["ID", "Content", "Type", "Entities", "Confidence"]);

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

        table.add_row(vec![
            &fact.id.to_string()[..8],
            content.as_str(),
            fact.fact_type.to_string().as_str(),
            entity_names.join(", ").as_str(),
            &format!("{:.2}", fact.source_confidence),
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
            table.set_header(vec!["#", "Content", "Type", "Entities", "Score"]);

            for result in results {
                // Truncate content if too long
                let content = if result.content.len() > 50 {
                    format!("{}...", &result.content[..50])
                } else {
                    result.content.clone()
                };

                table.add_row(vec![
                    result.rank.to_string().as_str(),
                    content.as_str(),
                    result.fact_type.to_string().as_str(),
                    result.entity_names.join(", ").as_str(),
                    &format!("{:.3}", result.score),
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
            output.push_str(&format!("Graph Health Report\n"));
            output.push_str(&format!("====================\n\n"));
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

pub fn format_stats(stats: &StoreStats) -> Result<String> {
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
    let naive = chrono::DateTime::from_timestamp(secs as i64, 0).map(|dt| dt.naive_utc()).unwrap_or_default();
    naive.format("%Y-%m-%d %H:%M:%S").to_string()
}
