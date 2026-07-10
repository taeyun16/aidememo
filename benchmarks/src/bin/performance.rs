//! Experiment 10.4 — performance matrix.
//!
//! Builds a synthetic wiki at multiple scales and measures p50/p95/p99
//! latency for the core read/write operations below. Writes the local result to
//! `benchmarks/results/performance.json`; that environment-sensitive file is
//! ignored until a snapshot is promoted with backend, commit, toolchain, and
//! host provenance in `docs/MEASUREMENTS.md`.
//!
//! Run:
//!   cargo run --release --bin performance
//!   AIDEMEMO_BENCH_LARGE=1 cargo run --release --bin performance   # +50K
//!
//! v1 scope:
//! * Tier 2 (warm, no semantic): traverse_d3, search_bm25, fact_add, lint
//! * Tier 1-ish (cold-ish): startup measured via store-reopen + first traverse
//! * Skips Tier 3 / Tier 4 (semantic) — model load + HNSW build dominate
//!   and confound the BM25 numbers; will be a follow-up bench.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use aidememo_core::{
    AideMemo, Config, EntityInput, EntityType, FactInput, FactType, SearchOpts, TraverseDirection,
    TraverseOpts,
};
use serde::Serialize;
use tempfile::TempDir;

#[derive(Serialize)]
struct Stats {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    mean_ms: f64,
    min_ms: f64,
    max_ms: f64,
    n: usize,
}

#[derive(Serialize)]
struct Row {
    scale: usize,
    op: &'static str,
    stats: Stats,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn stats(times_ms: &[f64]) -> Stats {
    let mut s = times_ms.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
    Stats {
        p50_ms: percentile(&s, 0.50),
        p95_ms: percentile(&s, 0.95),
        p99_ms: percentile(&s, 0.99),
        mean_ms: mean,
        min_ms: *s.first().unwrap_or(&f64::NAN),
        max_ms: *s.last().unwrap_or(&f64::NAN),
        n: times_ms.len(),
    }
}

fn time_n<F: FnMut()>(n: usize, mut f: F) -> Vec<f64> {
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        f();
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times
}

fn build_wiki(path: &Path, scale: usize) -> AideMemo {
    let mut config = Config::default();
    config.store.path = path.to_string_lossy().into_owned();
    // Force the BM25-prefilter path so we benchmark BM25 search alone
    // — switching off the HNSW lookup also silences the per-call
    // "no sidecar" warning that otherwise drowns out timing output.
    config.search.semantic_index = "naive".into();
    let wiki = AideMemo::open(path, config).expect("open store");

    // ~20 facts per entity → entity_count = scale / 20, min 10.
    // Lint's duplicate detector is O(entities²) with trigram similarity,
    // so a higher ratio keeps the bench tractable while still being a
    // realistic shape for a real wiki (1 entity per ~10–50 facts).
    let entity_count = (scale / 20).max(10);
    let mut entity_ids = Vec::with_capacity(entity_count);
    for i in 0..entity_count {
        let id = wiki
            .entity_add(EntityInput {
                name: format!("Entity_{i}"),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .expect("entity_add");
        entity_ids.push(id);
    }

    // Realistic-ish fact bodies — vary topic words so BM25 has real
    // term diversity, otherwise its top-k collapses to a single bucket.
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
    for i in 0..scale {
        let owner = entity_ids[i % entity_count];
        let topic = topics[i % topics.len()];
        wiki.add_fact(FactInput {
            content: format!(
                "Fact {i} on Entity_{} discusses {topic} with notes on tuning, timeouts, and failure modes.",
                i % entity_count
            ),
            fact_type: Some(if i % 4 == 0 {
                FactType::Decision
            } else {
                FactType::Note
            }),
            entity_ids: Some(vec![owner]),
            source_confidence: Some(0.8),
            ..Default::default()
        })
        .expect("add_fact");
    }

    wiki
}

fn run_scale(scale: usize, rows: &mut Vec<Row>) {
    let started = Instant::now();
    eprint!("=== scale = {scale}: building... ");
    let dir = TempDir::new().expect("tempdir");
    let store_path = dir.path().join("wiki.redb");
    let wiki = build_wiki(&store_path, scale);
    eprintln!("{:.1}s", started.elapsed().as_secs_f64());

    // Warmup: BM25 builds lazily on first search; lint walks the graph.
    eprint!("  warmup... ");
    let warm_t0 = Instant::now();
    let _ = wiki
        .hybrid_search(
            "cache",
            SearchOpts {
                limit: Some(5),
                bm25_weight: 1.0,
                semantic_weight: 0.0,
                ..Default::default()
            },
        )
        .expect("warmup search");
    let _ = wiki.lint().expect("warmup lint");
    eprintln!("{:.2}s", warm_t0.elapsed().as_secs_f64());

    eprint!("  traverse_d3... ");
    let op_t0 = Instant::now();
    // traverse_d3 (warm). Entity_0 has ~5 facts and a few neighbors via
    // co-occurrence — depth 3 is enough to fan out non-trivially.
    let traverse_times = time_n(200, || {
        let _ = wiki
            .traverse(
                "Entity_0",
                TraverseOpts {
                    depth: 3,
                    relation_types: None,
                    direction: TraverseDirection::Forward,
                },
            )
            .expect("traverse");
    });
    rows.push(Row {
        scale,
        op: "traverse_d3",
        stats: stats(&traverse_times),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());

    let queries = [
        "cache replication",
        "failover latency",
        "sharding indexing",
        "consistency throughput",
    ];

    eprint!("  search_bm25... ");
    let op_t0 = Instant::now();
    // search_bm25 (warm) — pure BM25 via AideMemo::search. No
    // embedding work, so this is the apples-to-apples view of the
    // BM25 inverted-index lookup against the legacy v0.1 target table below.
    let mut idx = 0usize;
    let search_bm25_times = time_n(200, || {
        let q = queries[idx % queries.len()];
        idx = idx.wrapping_add(1);
        let _ = wiki
            .search(
                q,
                SearchOpts {
                    limit: Some(10),
                    ..Default::default()
                },
            )
            .expect("search_bm25");
    });
    rows.push(Row {
        scale,
        op: "search_bm25",
        stats: stats(&search_bm25_times),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());

    eprint!("  search_hybrid... ");
    let op_t0 = Instant::now();
    // search_hybrid (warm) — production hybrid path. Includes BM25
    // inverted-index lookup, query embedding, semantic re-rank over
    // the prefilter slate, and RRF fusion. We're still in the
    // `semantic_index = "naive"` config so HNSW is skipped, which
    // matches the BM25-prefilter fallback most non-HNSW deployments
    // see. The Tier-3/4 (HNSW + model-warm) path will need its own
    // bench because of the 1–2 s model-load amortization.
    let mut idx = 0usize;
    let search_hybrid_times = time_n(100, || {
        let q = queries[idx % queries.len()];
        idx = idx.wrapping_add(1);
        let _ = wiki
            .hybrid_search(
                q,
                SearchOpts {
                    limit: Some(10),
                    bm25_weight: 1.0,
                    semantic_weight: 1.0,
                    ..Default::default()
                },
            )
            .expect("search_hybrid");
    });
    rows.push(Row {
        scale,
        op: "search_hybrid",
        stats: stats(&search_hybrid_times),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());

    eprint!("  fact_add... ");
    let op_t0 = Instant::now();
    // fact_add (warm) — small N because each call mutates state.
    let entity_count = (scale / 20).max(10);
    let owner = wiki
        .resolve_entity(&format!("Entity_{}", entity_count / 2))
        .unwrap();
    let fact_add_times = time_n(50, || {
        let _ = wiki
            .add_fact(FactInput {
                content: "Synthetic benchmark fact about replication and tuning.".into(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![owner]),
                source_confidence: Some(0.5),
                ..Default::default()
            })
            .expect("add_fact");
    });
    rows.push(Row {
        scale,
        op: "fact_add",
        stats: stats(&fact_add_times),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());

    eprint!("  fact_add_many(100)... ");
    let op_t0 = Instant::now();
    // fact_add_many: amortized cost per fact when batched. Each rep
    // inserts a 100-fact batch and reports the *per-fact* time so the
    // number is directly comparable to the single fact_add row above.
    const BATCH: usize = 100;
    let batch_times_per_fact: Vec<f64> = time_n(20, || {
        let inputs: Vec<FactInput> = (0..BATCH)
            .map(|i| FactInput {
                content: format!("Batch synthetic fact {i} on replication and tuning."),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![owner]),
                source_confidence: Some(0.5),
                ..Default::default()
            })
            .collect();
        let _ = wiki.fact_add_many(inputs).expect("fact_add_many");
    })
    .into_iter()
    .map(|t| t / BATCH as f64)
    .collect();
    rows.push(Row {
        scale,
        op: "fact_add_many",
        stats: stats(&batch_times_per_fact),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());

    eprint!("  lint... ");
    let op_t0 = Instant::now();
    // lint (warm). Duplicate detection is O(entities²) with trigram
    // similarity, so reps are dialed back hard at scale.
    let lint_reps = if scale >= 10_000 {
        3
    } else if scale >= 1_000 {
        10
    } else {
        30
    };
    let lint_times = time_n(lint_reps, || {
        let _ = wiki.lint().expect("lint");
    });
    rows.push(Row {
        scale,
        op: "lint",
        stats: stats(&lint_times),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());

    eprint!("  startup... ");
    let op_t0 = Instant::now();
    // startup (≈ Tier 1): drop + reopen + first traverse measures
    // store-open + index-cold cost. Excludes process spawn / loader,
    // which is invariant across scales.
    drop(wiki);
    let mut startup_times = Vec::with_capacity(20);
    for _ in 0..20 {
        let mut cfg = Config::default();
        cfg.store.path = store_path.to_string_lossy().into_owned();
        cfg.search.semantic_index = "naive".into();
        let t0 = Instant::now();
        let w = AideMemo::open(&store_path, cfg).expect("reopen");
        let _ = w.traverse(
            "Entity_0",
            TraverseOpts {
                depth: 1,
                relation_types: None,
                direction: TraverseDirection::Forward,
            },
        );
        startup_times.push(t0.elapsed().as_secs_f64() * 1000.0);
        drop(w);
    }
    rows.push(Row {
        scale,
        op: "startup",
        stats: stats(&startup_times),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());

    // === HNSW phase: reopen with default config, build sidecar,
    //     measure the HNSW-backed hybrid path. ===
    eprint!("  hnsw_build... ");
    let op_t0 = Instant::now();
    let mut hnsw_cfg = Config::default(); // semantic_index = "hnsw" by default
    hnsw_cfg.store.path = store_path.to_string_lossy().into_owned();
    let hnsw_wiki = AideMemo::open(&store_path, hnsw_cfg).expect("reopen hnsw");
    let hnsw_build_ms = {
        let t0 = Instant::now();
        let _ = hnsw_wiki.vector_index_rebuild().expect("hnsw build");
        t0.elapsed().as_secs_f64() * 1000.0
    };
    rows.push(Row {
        scale,
        op: "hnsw_build",
        stats: stats(&[hnsw_build_ms]),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());

    eprint!("  search_hybrid_hnsw... ");
    let op_t0 = Instant::now();
    let mut idx = 0usize;
    // Warm: first call primes query embedding cache + load model.
    let _ = hnsw_wiki
        .hybrid_search(
            queries[0],
            SearchOpts {
                limit: Some(10),
                ..Default::default()
            },
        )
        .expect("hnsw warmup");
    let hnsw_search_times = time_n(100, || {
        let q = queries[idx % queries.len()];
        idx = idx.wrapping_add(1);
        let _ = hnsw_wiki
            .hybrid_search(
                q,
                SearchOpts {
                    limit: Some(10),
                    bm25_weight: 1.0,
                    semantic_weight: 1.0,
                    ..Default::default()
                },
            )
            .expect("search_hybrid_hnsw");
    });
    rows.push(Row {
        scale,
        op: "search_hybrid_hnsw",
        stats: stats(&hnsw_search_times),
    });
    eprintln!("{:.2}s", op_t0.elapsed().as_secs_f64());
}

fn print_table(rows: &[Row]) {
    println!();
    println!(
        "{:>10}  {:<14}  {:>10}  {:>10}  {:>10}  {:>10}  {:>5}",
        "scale", "op", "p50_ms", "p95_ms", "p99_ms", "mean_ms", "n"
    );
    println!("{}", "-".repeat(80));
    for r in rows {
        println!(
            "{:>10}  {:<14}  {:>10.3}  {:>10.3}  {:>10.3}  {:>10.3}  {:>5}",
            r.scale,
            r.op,
            r.stats.p50_ms,
            r.stats.p95_ms,
            r.stats.p99_ms,
            r.stats.mean_ms,
            r.stats.n,
        );
    }
}

/// Legacy v0.1 p95 latency targets (ms). These constants are retained only as
/// a stable local comparison baseline. Returns None when an `(op, scale)` cell
/// is not represented.
fn target_p95(op: &str, scale: usize) -> Option<f64> {
    let idx = match scale {
        100 => 0,
        1_000 => 1,
        10_000 => 2,
        50_000 => 3,
        100_000 => 4,
        _ => return None,
    };
    let table: &[(&str, [f64; 5])] = &[
        ("startup", [5.0, 10.0, 30.0, 100.0, 200.0]),
        ("traverse_d3", [0.2, 0.5, 1.0, 3.0, 5.0]),
        ("search_bm25", [0.5, 1.0, 3.0, 10.0, 15.0]),
        ("search_hybrid", [1.0, 2.0, 5.0, 15.0, 20.0]),
        ("fact_add", [0.5, 0.5, 1.0, 1.0, 1.0]),
        // fact_add_many is reported per-fact, so same target as
        // single-fact insert. The whole point of the batch path is
        // that one fsync covers many facts, so per-fact cost should
        // sit well under the single-call target.
        ("fact_add_many", [0.5, 0.5, 1.0, 1.0, 1.0]),
        ("lint", [5.0, 10.0, 50.0, 200.0, 500.0]),
    ];
    table.iter().find(|(o, _)| *o == op).map(|(_, t)| t[idx])
}

fn print_target_comparison(rows: &[Row]) {
    println!();
    println!("Comparison vs legacy v0.1 p95 targets:");
    println!(
        "{:>10}  {:<14}  {:>12}  {:>12}  {:>10}",
        "scale", "op", "p95 (ms)", "target (ms)", "ratio"
    );
    println!("{}", "-".repeat(70));
    for r in rows {
        let target = target_p95(r.op, r.scale);
        let (target_str, ratio_str) = match target {
            Some(t) => {
                let ratio = r.stats.p95_ms / t;
                let marker = if ratio <= 1.0 {
                    "OK"
                } else if ratio <= 2.0 {
                    "close"
                } else {
                    "MISS"
                };
                (
                    format!("{:>10.3}", t),
                    format!("{:>5.1}x {}", ratio, marker),
                )
            }
            None => ("—".to_string(), "—".to_string()),
        };
        println!(
            "{:>10}  {:<14}  {:>12.3}  {:>12}  {:>10}",
            r.scale, r.op, r.stats.p95_ms, target_str, ratio_str
        );
    }
}

fn main() {
    let mut scales: Vec<usize> = vec![100, 1_000, 10_000];
    if std::env::var("AIDEMEMO_BENCH_LARGE").is_ok() {
        scales.push(50_000);
    }
    if std::env::var("AIDEMEMO_BENCH_HUGE").is_ok() {
        scales.push(100_000);
    }

    let mut rows = Vec::new();
    for &scale in &scales {
        run_scale(scale, &mut rows);
    }

    print_table(&rows);
    print_target_comparison(&rows);

    let out_dir = Path::new("benchmarks/results");
    std::fs::create_dir_all(out_dir).expect("mkdir results");
    let out_path = out_dir.join("performance.json");
    let json = serde_json::to_string_pretty(&rows).expect("to_json");
    let mut f = std::fs::File::create(&out_path).expect("create json");
    f.write_all(json.as_bytes()).expect("write json");
    eprintln!("\nWrote {}", out_path.display());
}
