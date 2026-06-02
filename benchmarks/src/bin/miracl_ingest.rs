//! Bulk-load the prepared MIRACL/ko subset into a aidememo store.
//!
//! Reads `/tmp/aidememo-tei-bench/miracl_ko_facts.jsonl` (one
//! {content, source, fact_type, tags, source_confidence} object per
//! line; produced by `prep_miracl.py`) and pushes everything in via
//! `AideMemo::fact_add_many`. Faster than shelling out to
//! `aidememo fact add` per row.

use std::path::Path;
use std::time::Instant;

use aidememo_core::{AideMemo, Config, FactInput, FactType};
use serde::Deserialize;

const DEFAULT_STORE: &str = "/tmp/aidememo-bench-miracl/_meta/wiki.redb";
const DEFAULT_INPUT: &str = "/tmp/aidememo-tei-bench/miracl_ko_facts.jsonl";
const BATCH: usize = 500;

#[derive(Deserialize)]
struct Row {
    content: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    source_confidence: Option<f32>,
}

fn main() {
    let store = std::env::var("AIDEMEMO_BENCH_STORE").unwrap_or_else(|_| DEFAULT_STORE.to_string());
    let input = std::env::var("AIDEMEMO_BENCH_INPUT").unwrap_or_else(|_| DEFAULT_INPUT.to_string());
    let store_path = Path::new(&store);
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir store dir");
    }

    let mut config = Config::default();
    config.store.path = store_path.to_string_lossy().into_owned();
    // Pure offline ingest — no need for HNSW or HTTP embed.
    // Keep semantic_index = "naive" so we don't auto-rebuild HNSW.
    config.search.semantic_index = "naive".into();
    let wiki = AideMemo::open(store_path, config).expect("open aidememo store");

    eprintln!("=== MIRACL/ko ingest into {store} ===");
    let body = std::fs::read_to_string(&input).expect("read facts file");
    let lines: Vec<&str> = body.lines().collect();
    eprintln!("input: {} rows", lines.len());

    let mut total = 0usize;
    let mut batch: Vec<FactInput> = Vec::with_capacity(BATCH);
    let started = Instant::now();
    for (i, line) in lines.iter().enumerate() {
        let row: Row = serde_json::from_str(line).expect("row parse");
        batch.push(FactInput {
            content: row.content,
            fact_type: Some(FactType::Note),
            entity_ids: None,
            tags: if row.tags.is_empty() {
                None
            } else {
                Some(row.tags)
            },
            source: if row.source.is_empty() {
                None
            } else {
                Some(row.source)
            },
            source_id: None,
            source_confidence: row.source_confidence,
            observed_at: None,
        });
        if batch.len() >= BATCH || i + 1 == lines.len() {
            let n = batch.len();
            wiki.fact_add_many(std::mem::take(&mut batch))
                .expect("fact_add_many");
            total += n;
            eprintln!(
                "  ingested {total}/{} ({:.1}s elapsed)",
                lines.len(),
                started.elapsed().as_secs_f64()
            );
        }
    }
    eprintln!(
        "done. {total} facts in {:.1}s ({:.0} facts/sec)",
        started.elapsed().as_secs_f64(),
        total as f64 / started.elapsed().as_secs_f64()
    );
}
