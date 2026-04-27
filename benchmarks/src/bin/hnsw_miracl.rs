//! HNSW ROI prototype on MIRACL/ko 5503-doc corpus.
//!
//! Loads the wg store at /tmp/wg-bench-miracl, embeds every fact once
//! with the configured model2vec model, builds an in-memory HNSW
//! index, then runs the 213 dev queries against:
//!
//!   - brute force: scan all 5503 cosines per query (recall ceiling)
//!   - HNSW (ef=20, 50, 100): ANN top-10 via instant-distance
//!
//! Reports P@10, R@10, build time, query time. We compare to the
//! P@10=0.175 / R@10=0.706 numbers the in-tree wg bench already
//! reported with the BM25→cosine pipeline.

use std::path::Path;
use std::time::Instant;

use instant_distance::{Builder, Point as IDPoint, Search};
use serde::Deserialize;
use simsimd::SpatialSimilarity;

use wg_core::Config;
use wg_core::FactListOpts;
use wg_core::WikiGraph;
use wg_core::embedding::load_provider;

const STORE: &str = "/tmp/wg-bench-miracl/_meta/wiki.redb";
const GOLDEN: &str = "/tmp/bench_miracl_ko.jsonl";

/// L2-normalized vector with cosine distance for instant-distance.
/// Pre-normalize at insert time so distance is `1 - dot(a, b)` —
/// matches the cosine metric every other layer of wg uses.
#[derive(Clone, Debug)]
struct Vec256(Vec<f32>);

impl IDPoint for Vec256 {
    fn distance(&self, other: &Self) -> f32 {
        // Both vectors are unit-normalized at insert; dot == cosine sim.
        let sim = match f32::dot(&self.0, &other.0) {
            Some(d) => d as f32,
            None => 0.0,
        };
        // Distance, not similarity. instant-distance's "smaller = nearer".
        1.0 - sim
    }
}

fn normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    for x in v.iter_mut() {
        *x /= norm;
    }
}

#[derive(Deserialize)]
struct GoldenRow {
    query: String,
    expected: Vec<String>,
}

fn main() {
    let config = Config::default();
    let wiki = WikiGraph::open(Path::new(STORE), config.clone()).expect("open store");

    println!("loading facts…");
    let facts = wiki
        .fact_list(FactListOpts {
            limit: Some(10_000),
            ..Default::default()
        })
        .expect("list facts");
    println!("  {} facts", facts.len());

    println!("loading provider…");
    let provider = load_provider(wiki.config()).expect("load provider");
    println!("  {} (dim={})", provider.name(), provider.dimension());

    println!("embedding {} facts…", facts.len());
    let t0 = Instant::now();
    let texts: Vec<String> = facts.iter().map(|f| f.content.clone()).collect();
    let mut embeddings = provider.embed_batch(&texts).expect("embed batch");
    for v in embeddings.iter_mut() {
        normalize(v);
    }
    println!("  done in {:?}", t0.elapsed());

    // Keep an owned copy in fact-order for brute-force scoring.
    // instant-distance reorders points internally for graph layout,
    // so the points returned by `map.iter()` aren't aligned with
    // `facts[i]` anymore.
    let bf_embeddings: Vec<Vec256> = embeddings.iter().cloned().map(Vec256).collect();

    // Build HNSW. We try a few ef_search values to map the
    // recall-vs-latency curve.
    println!("building HNSW…");
    let t0 = Instant::now();
    let points: Vec<Vec256> = embeddings.into_iter().map(Vec256).collect();
    let values: Vec<usize> = (0..facts.len()).collect();
    let map = Builder::default()
        .ef_construction(200)
        .seed(42)
        .build(points, values);
    println!("  build took {:?}", t0.elapsed());

    // Build a docid (source) → fact_id string lookup so we can compare
    // against the golden expected list.
    let fid_strings: Vec<String> = facts.iter().map(|f| f.id.to_string()).collect();

    // Load goldens
    println!("loading goldens…");
    let goldens: Vec<GoldenRow> = std::fs::read_to_string(GOLDEN)
        .expect("read golden")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse"))
        .collect();
    println!("  {} queries", goldens.len());

    let k = 10usize;

    // Helper: brute force top-k by cosine sim
    fn brute_topk(q: &[f32], embeddings: &[Vec256], k: usize) -> Vec<usize> {
        let mut scored: Vec<(usize, f32)> = embeddings
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let s = match f32::dot(q, &e.0) {
                    Some(d) => d as f32,
                    None => 0.0,
                };
                (i, s)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(k).map(|(i, _)| i).collect()
    }

    // For each method, compute sum of P@k, R@k.
    let methods = [
        ("brute_force", 0usize), // 0 = brute (skip HNSW)
        ("hnsw_ef_20", 20),
        ("hnsw_ef_50", 50),
        ("hnsw_ef_100", 100),
        ("hnsw_ef_200", 200),
    ];

    for (label, ef) in methods {
        let t_total = Instant::now();
        let mut p_sum = 0.0f64;
        let mut r_sum = 0.0f64;
        let mut latencies = Vec::with_capacity(goldens.len());

        for g in &goldens {
            let mut qv = provider.embed(&g.query).expect("embed query");
            normalize(&mut qv);

            let started = Instant::now();
            let result_ids: Vec<usize> = if ef == 0 {
                brute_topk(&qv, &bf_embeddings, k)
            } else {
                let mut search = Search::default();
                let qpoint = Vec256(qv);
                map.search(&qpoint, &mut search)
                    .take(k)
                    .map(|item| *item.value)
                    .collect()
            };
            latencies.push(started.elapsed());

            // Convert to fact_id strings, compare to expected
            let top_fids: Vec<&str> = result_ids
                .iter()
                .map(|&i| fid_strings[i].as_str())
                .collect();
            let exp: std::collections::HashSet<&str> =
                g.expected.iter().map(|s| s.as_str()).collect();
            let hits = top_fids.iter().filter(|id| exp.contains(*id)).count();

            let p = hits as f64 / k as f64;
            let r = hits as f64 / g.expected.len() as f64;
            p_sum += p;
            r_sum += r;
        }

        let n = goldens.len() as f64;
        let mean_p = p_sum / n;
        let mean_r = r_sum / n;
        latencies.sort();
        let p50 = latencies[latencies.len() / 2];
        let p95 = latencies[(latencies.len() * 95) / 100];
        let total = t_total.elapsed();

        println!(
            "  {label:14}  P@{k}={mean_p:.3}  R@{k}={mean_r:.3}  p50={:>5}us  p95={:>5}us  total={:?}",
            p50.as_micros(),
            p95.as_micros(),
            total
        );
    }
}
