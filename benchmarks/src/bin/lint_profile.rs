//! One-shot lint profiler.
//!
//! Builds a 10K-fact synthetic wiki via `fact_add_many` (so the build
//! itself is fast), then runs `lint()` once with `WG_LINT_PROFILE=1`
//! so each phase prints its elapsed time. Sole purpose is to identify
//! the dominant cost inside lint at scale; not a replacement for the
//! main perf bench.
//!
//! Run:
//!   cargo run --release --bin lint_profile

use std::time::Instant;

use tempfile::TempDir;
use wg_core::{Config, EntityInput, EntityType, FactInput, FactType, WikiGraph};

fn main() {
    // Force the profile env var so every `lint()` run emits its
    // per-phase breakdown.
    // SAFETY: setting an env var at startup before any threads spawn.
    unsafe {
        std::env::set_var("WG_LINT_PROFILE", "1");
    }

    const SCALE: usize = 10_000;
    const ENTITY_COUNT: usize = SCALE / 20;

    let dir = TempDir::new().expect("tempdir");
    let store_path = dir.path().join("wiki.redb");

    let mut config = Config::default();
    config.store.path = store_path.to_string_lossy().into_owned();
    config.search.semantic_index = "naive".into();
    let wiki = WikiGraph::open(&store_path, config).expect("open");

    eprintln!("=== lint profile, scale = {SCALE} ===");

    eprint!("building wiki... ");
    let t = Instant::now();
    // Entities first.
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

    eprintln!();
    eprintln!("--- warmup lint (ignore) ---");
    let t = Instant::now();
    let _ = wiki.lint().expect("warmup lint");
    eprintln!("[lint] total: {:.2}ms", t.elapsed().as_secs_f64() * 1000.0);

    eprintln!();
    eprintln!("--- run 1 ---");
    let t = Instant::now();
    let _ = wiki.lint().expect("lint 1");
    eprintln!("[lint] total: {:.2}ms", t.elapsed().as_secs_f64() * 1000.0);

    eprintln!();
    eprintln!("--- run 2 ---");
    let t = Instant::now();
    let _ = wiki.lint().expect("lint 2");
    eprintln!("[lint] total: {:.2}ms", t.elapsed().as_secs_f64() * 1000.0);
}
