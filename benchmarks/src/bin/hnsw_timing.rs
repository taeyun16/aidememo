//! Measure where vector_index_rebuild spends its time.
//!
//! Calls each stage of the rebuild path manually with timing
//! instrumentation. Useful for answering "why does ingest take
//! N seconds?" without sprinkling println!s through wg-core.

use std::path::Path;
use std::time::Instant;
use wg_core::FactListOpts;
use wg_core::WikiGraph;
use wg_core::embedding::load_provider;
use wg_core::vector_index::HnswIndex;

fn main() {
    let store_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/wg-bench-miracl/_meta/wiki.redb".to_string());
    println!("store: {store_path}");

    let config = wg_core::Config::default();
    let wiki = WikiGraph::open(Path::new(&store_path), config).expect("open");

    let t0 = Instant::now();
    let provider = load_provider(wiki.config()).expect("load provider");
    println!(
        "  {:>8.2?}  load_provider({})",
        t0.elapsed(),
        provider.name()
    );

    let t = Instant::now();
    let facts = wiki
        .fact_list(FactListOpts {
            limit: None,
            ..Default::default()
        })
        .expect("list");
    println!("  {:>8.2?}  fact_list (n={})", t.elapsed(), facts.len());

    let t = Instant::now();
    let texts: Vec<String> = facts.iter().map(|f| f.content.clone()).collect();
    println!("  {:>8.2?}  collect texts", t.elapsed());

    let t = Instant::now();
    let embeddings = provider.embed_batch(&texts).expect("embed");
    println!(
        "  {:>8.2?}  embed_batch (avg {:.2}ms/fact)",
        t.elapsed(),
        t.elapsed().as_secs_f64() * 1000.0 / texts.len() as f64
    );

    let t = Instant::now();
    let entries: Vec<(_, _)> = facts
        .into_iter()
        .zip(embeddings)
        .map(|(f, v)| (f.id, v))
        .collect();
    println!("  {:>8.2?}  build entries", t.elapsed());

    let t = Instant::now();
    let idx = HnswIndex::build(&provider.name(), provider.dimension(), entries);
    println!("  {:>8.2?}  HnswIndex::build", t.elapsed());

    let t = Instant::now();
    let path = std::path::Path::new(&store_path).with_extension("hnsw.bin");
    idx.save_to(&path).expect("save");
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "  {:>8.2?}  save_to ({:.1} MB)",
        t.elapsed(),
        bytes as f64 / 1_048_576.0
    );

    println!("\n  total: {:?}", t0.elapsed());
}
