//! Analytics CLI commands.
//!
//! Provides access to DuckDB-backed OLAP queries over canonical memory.

use aidememo_core::AideMemo;
use bpaf::Bpaf;

#[cfg(feature = "analytics")]
#[derive(Debug, Clone, Bpaf)]
pub enum AnalyticsCommand {
    /// Rebuild analytics engine from canonical store
    #[bpaf(command("rebuild"))]
    Rebuild,
    /// Show analytics engine statistics
    #[bpaf(command("stats"))]
    Stats,
    /// Query analytics with SQL
    #[bpaf(command("query"))]
    Query {
        /// SQL query to execute (parameterized with ? placeholders)
        #[bpaf(positional("SQL"))]
        sql: String,
        /// Parameters for SQL query (optional)
        #[bpaf(long, short, argument("PARAM"))]
        params: Vec<String>,
    },
    /// Run predefined analytical queries
    #[bpaf(command("report"))]
    Report {
        /// Report type: growth, sources, centrality, or timeline
        #[bpaf(positional("TYPE"))]
        report_type: String,
        /// Optional limit for results
        #[bpaf(long, short, argument("N"))]
        limit: Option<usize>,
    },
}

#[cfg(feature = "analytics")]
pub fn run_analytics(cmd: AnalyticsCommand, g: &AideMemo) -> anyhow::Result<()> {
    use aidememo_core::analytics::AnalyticsEngine;

    match cmd {
        AnalyticsCommand::Rebuild => {
            println!("Rebuilding analytics engine from canonical store...");
            let mut engine_guard = g.analytics_engine()?;
            if let Some(engine) = engine_guard.as_mut() {
                let store = g.store().read();
                engine.rebuild_from_canonical(&*store)?;
                let stats = engine.stats()?;
                println!("✓ Rebuild complete:");
                println!("  Entities: {}", stats.entity_count);
                println!(
                    "  Facts: {} ({} current)",
                    stats.fact_count, stats.current_fact_count
                );
                println!("  Relations: {}", stats.relation_count);
            } else {
                return Err(anyhow::anyhow!("Analytics engine not initialized"));
            }
            Ok(())
        }
        AnalyticsCommand::Stats => {
            let engine_guard = g.analytics_engine()?;
            if let Some(engine) = engine_guard.as_ref() {
                let stats = engine.stats()?;
                println!("Analytics Engine Statistics:");
                println!("  Entities: {}", stats.entity_count);
                println!(
                    "  Facts: {} ({} current)",
                    stats.fact_count, stats.current_fact_count
                );
                println!("  Relations: {}", stats.relation_count);
                println!("  Watermarks:");
                println!("    Last entity timestamp: {}", stats.last_entity_seq);
                println!("    Last fact timestamp: {}", stats.last_fact_seq);
                println!("    Last relation timestamp: {}", stats.last_relation_seq);
            } else {
                return Err(anyhow::anyhow!("Analytics engine not initialized"));
            }
            Ok(())
        }
        AnalyticsCommand::Query { sql, params } => {
            let engine_guard = g.analytics_engine()?;
            if let Some(engine) = engine_guard.as_ref() {
                let results = engine.query(&sql, &params)?;
                if results.is_empty() {
                    println!("(no results)");
                } else {
                    for row in results {
                        println!("{}", row.join("\t"));
                    }
                }
            } else {
                return Err(anyhow::anyhow!("Analytics engine not initialized"));
            }
            Ok(())
        }
        AnalyticsCommand::Report { report_type, limit } => run_report(&report_type, limit, g),
    }
}

#[cfg(feature = "analytics")]
fn run_report(report_type: &str, limit: Option<usize>, g: &AideMemo) -> anyhow::Result<()> {
    let engine_guard = g.analytics_engine()?;
    let engine = engine_guard
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Analytics engine not initialized"))?;

    let limit = limit.unwrap_or(10);

    match report_type {
        "growth" => {
            // Monthly fact growth
            let results = engine.query(
                "SELECT 
                    created_at_year || '-' || LPAD(CAST(created_at_month AS VARCHAR), 2, '0') as month,
                    COUNT(*) as fact_count
                 FROM facts
                 GROUP BY created_at_year, created_at_month
                 ORDER BY created_at_year DESC, created_at_month DESC
                 LIMIT ?",
                &[limit as i64],
            )?;
            println!("Fact Growth by Month:");
            println!("Month\t\tCount");
            for row in results {
                println!("{}\t{}", row[0], row[1]);
            }
        }
        "sources" => {
            // Top sources by fact count
            let results = engine.query(
                "SELECT 
                    COALESCE(source_id, '(no source)') as source,
                    COUNT(*) as fact_count
                 FROM facts
                 WHERE is_current = true
                 GROUP BY source_id
                 ORDER BY fact_count DESC
                 LIMIT ?",
                &[limit as i64],
            )?;
            println!("Top Sources by Current Facts:");
            println!("Source\t\tCount");
            for row in results {
                println!("{}\t{}", row[0], row[1]);
            }
        }
        "centrality" => {
            // Most connected entities (by relation degree)
            let results = engine.query(
                "SELECT 
                    e.name,
                    COUNT(DISTINCT r.target_entity_id) as out_degree,
                    COUNT(DISTINCT r2.source_entity_id) as in_degree,
                    COUNT(DISTINCT r.target_entity_id) + COUNT(DISTINCT r2.source_entity_id) as total_degree
                 FROM entities e
                 LEFT JOIN relations r ON e.id = r.source_entity_id
                 LEFT JOIN relations r2 ON e.id = r2.target_entity_id
                 GROUP BY e.id, e.name
                 ORDER BY total_degree DESC
                 LIMIT ?",
                &[limit as i64],
            )?;
            println!("Most Connected Entities:");
            println!("Entity\t\tOut\tIn\tTotal");
            for row in results {
                println!("{}\t{}\t{}\t{}", row[0], row[1], row[2], row[3]);
            }
        }
        "timeline" => {
            // Recent facts timeline
            let results = engine.query(
                "SELECT 
                    DATE_TRUNC('day', created_at) as day,
                    fact_type,
                    COUNT(*) as count
                 FROM facts
                 WHERE created_at >= NOW() - INTERVAL '30 days'
                 GROUP BY DATE_TRUNC('day', created_at), fact_type
                 ORDER BY day DESC, count DESC
                 LIMIT ?",
                &[limit as i64],
            )?;
            println!("Recent Facts Timeline (last 30 days):");
            println!("Date\t\tType\t\tCount");
            for row in results {
                println!("{}\t{}\t{}", row[0], row[1], row[2]);
            }
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Unknown report type: {}. Available: growth, sources, centrality, timeline",
                report_type
            ));
        }
    }

    Ok(())
}

#[cfg(not(feature = "analytics"))]
pub fn run_analytics(_cmd: AnalyticsCommand, _g: &AideMemo) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "Analytics feature not enabled. Rebuild with --features analytics"
    ))
}
