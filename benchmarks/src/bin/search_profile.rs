//! Phase breakdown for `AideMemo::hybrid_search` on the
//! BM25-prefilter path (semantic_index = "naive"), where the perf
//! bench shows search_hybrid sitting at ~5-10 ms p95 — within 2× of
//! PLAN.md's target. The HNSW path already passes; this profiler is
//! about understanding the remaining miss on the fallback path.
//!
//! Sets `AIDEMEMO_SEARCH_PROFILE=1` so each phase
//! (bm25_search, graph_prefilter, semantic_search, rrf_fusion) prints
//! its elapsed time per call.
//!
//! Run:
//!   cargo run --release --bin search_profile

use std::time::Instant;

use aidememo_core::{AideMemo, Config, EntityInput, EntityType, FactInput, FactType, SearchOpts};
use tempfile::TempDir;

fn main() {
    // SAFETY: setting an env var at startup before any threads spawn.
    unsafe {
        std::env::set_var("AIDEMEMO_SEARCH_PROFILE", "1");
    }

    const SCALE: usize = 10_000;
    const ENTITY_COUNT: usize = SCALE / 20;

    let dir = TempDir::new().expect("tempdir");
    let store_path = dir.path().join("wiki.redb");

    let mut config = Config::default();
    config.store.path = store_path.to_string_lossy().into_owned();
    config.search.semantic_index = "naive".into();
    let wiki = AideMemo::open(&store_path, config).expect("open");

    eprintln!("=== search profile, scale = {SCALE} (semantic_index=naive) ===");

    eprint!("building wiki... ");
    let t = Instant::now();
    let mut entity_ids = Vec::with_capacity(ENTITY_COUNT);
    for i in 0..ENTITY_COUNT {
        let id = wiki
            .entity_add(EntityInput {
                name: format!("Entity_{i}"),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .expect("entity_add");
        entity_ids.push(id);
    }
    let topics = [
        "cache",
        "persistence",
        "replication",
        "latency",
        "throughput",
        "failover",
        "consistency",
        "sharding",
        "indexing",
        "compression",
    ];
    let inputs: Vec<FactInput> = (0..SCALE)
        .map(|i| {
            let owner = entity_ids[i % ENTITY_COUNT];
            let topic = topics[i % topics.len()];
            FactInput {
                content: format!("Fact {i} on Entity_{} discusses {topic}.", i % ENTITY_COUNT),
                fact_type: Some(if i % 4 == 0 {
                    FactType::Decision
                } else {
                    FactType::Note
                }),
                entity_ids: Some(vec![owner]),
                source_confidence: Some(0.8),
                ..Default::default()
            }
        })
        .collect();
    wiki.fact_add_many(inputs).expect("fact_add_many");
    eprintln!("{:.1}s", t.elapsed().as_secs_f64());

    let queries = [
        "cache replication",
        "failover latency",
        "sharding indexing",
        "consistency throughput",
    ];

    eprintln!();
    eprintln!("--- warmup (loads model + caches first query embedding) ---");
    let t = Instant::now();
    let _ = wiki
        .hybrid_search(
            queries[0],
            SearchOpts {
                limit: Some(10),
                ..Default::default()
            },
        )
        .expect("warmup");
    eprintln!(
        "[search] total: {:.2}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    for (i, q) in queries.iter().enumerate() {
        eprintln!();
        eprintln!("--- run {} : {:?} ---", i + 1, q);
        let t = Instant::now();
        let _ = wiki
            .hybrid_search(
                q,
                SearchOpts {
                    limit: Some(10),
                    ..Default::default()
                },
            )
            .expect("hybrid_search");
        eprintln!(
            "[search] total: {:.2}ms",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}
