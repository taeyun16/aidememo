//! Embedding-model sweep on MIRACL/ko.
//!
//! Reuses the aidememo store ingested by `miracl_ingest` (5503 docs).
//! For each (provider, endpoint, model-id) tuple in the matrix,
//! reopens the store with a fresh `Config`, rebuilds the HNSW
//! sidecar with that embedding model, and runs the 213 dev queries.
//! Reports R@10, MRR@10, nDCG@10, build time, and search p50/p95.
//!
//! Pre-reqs:
//!   - `/tmp/aidememo-bench-miracl/_meta/wiki.redb` ingested
//!   - `/tmp/aidememo-tei-bench/miracl_ko_golden.jsonl` written
//!   - For each TEI provider in the matrix, a TEI server reachable
//!     at the configured endpoint with the right model loaded.
//!
//! Pass `--matrix=name1,name2` to limit which configs run; `list`
//! to print the available names without running.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use aidememo_core::{AideMemo, Config, FactListOpts, SearchOpts};
use serde::Deserialize;

const DEFAULT_STORE: &str = "/tmp/aidememo-bench-miracl/_meta/wiki.redb";
const DEFAULT_GOLDEN: &str = "/tmp/aidememo-tei-bench/miracl_ko_golden.jsonl";

fn store_path() -> String {
    std::env::var("AIDEMEMO_BENCH_STORE").unwrap_or_else(|_| DEFAULT_STORE.to_string())
}

fn golden_path() -> String {
    std::env::var("AIDEMEMO_BENCH_GOLDEN").unwrap_or_else(|_| DEFAULT_GOLDEN.to_string())
}

/// Each row: (label, provider, endpoint, model_id, dimension_hint).
/// `dimension_hint` is optional — TEI's tei provider auto-discovers
/// via /info or a probe; pass 0 to defer to that path. For
/// model2vec we set dim=0 too because the loader figures it out.
const MATRIX: &[(&str, &str, &str, &str, usize)] = &[
    (
        "model2vec/potion-multi-128M",
        "model2vec",
        "",
        "minishlab/potion-multilingual-128M",
        0,
    ),
    (
        "tei/multilingual-e5-small",
        "tei",
        "http://localhost:8080",
        "intfloat/multilingual-e5-small",
        0,
    ),
];

#[derive(Deserialize)]
struct GoldenRow {
    query: String,
    expected_sources: Vec<String>,
    #[allow(dead_code)]
    k: usize,
}

fn load_golden() -> Vec<GoldenRow> {
    let body = std::fs::read_to_string(golden_path()).expect("read golden");
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("golden line"))
        .collect()
}

fn build_source_map(wiki: &AideMemo) -> HashMap<aidememo_core::FactId, String> {
    let facts = wiki
        .fact_list(FactListOpts {
            limit: Some(usize::MAX),
            ..Default::default()
        })
        .expect("fact_list");
    facts
        .into_iter()
        .filter_map(|f| f.source.map(|s| (f.id, s)))
        .collect()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn dcg(rel: &[u32]) -> f64 {
    rel.iter()
        .enumerate()
        .map(|(i, r)| (*r as f64) / ((i as f64 + 2.0).log2()))
        .sum()
}

fn score_query(ranked_sources: &[String], expected: &HashSet<String>, k: usize) -> (f64, f64, f64) {
    let head: Vec<&String> = ranked_sources.iter().take(k).collect();
    let rel: Vec<u32> = head
        .iter()
        .map(|s| if expected.contains(*s) { 1 } else { 0 })
        .collect();
    let hits = rel.iter().filter(|r| **r > 0).count() as f64;
    let recall = if expected.is_empty() {
        0.0
    } else {
        hits / expected.len() as f64
    };
    let mrr = rel
        .iter()
        .enumerate()
        .find(|(_, r)| **r > 0)
        .map(|(i, _)| 1.0 / (i as f64 + 1.0))
        .unwrap_or(0.0);
    let ideal_len = expected.len().min(k);
    let ideal: Vec<u32> = (0..ideal_len).map(|_| 1).collect();
    let ndcg = if ideal_len == 0 {
        0.0
    } else {
        dcg(&rel) / dcg(&ideal)
    };
    (recall, mrr, ndcg)
}

fn run_config(
    label: &str,
    provider: &str,
    endpoint: &str,
    model_id: &str,
    dimension: usize,
    golden: &[GoldenRow],
) {
    let mut config = Config::default();
    let store = store_path();
    config.store.path = store.clone();
    config.search.semantic_index = "hnsw".into();
    config.model.provider = provider.into();
    if !endpoint.is_empty() {
        config.model.endpoint = endpoint.into();
    }
    config.model.name = model_id.into();
    if dimension > 0 {
        config.model.dimension = dimension;
    }

    let wiki = AideMemo::open(Path::new(&store), config).expect("open wiki");

    let t0 = Instant::now();
    let count = match wiki.vector_index_rebuild() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("  [{label}] HNSW rebuild failed: {e} — skipping");
            return;
        }
    };
    let build_s = t0.elapsed().as_secs_f64();

    let src_by_id = build_source_map(&wiki);

    let mut sum_r = 0.0;
    let mut sum_mrr = 0.0;
    let mut sum_ndcg = 0.0;
    let mut latencies = Vec::with_capacity(golden.len());
    for q in golden {
        let expected: HashSet<String> = q.expected_sources.iter().cloned().collect();
        let opts = SearchOpts {
            limit: Some(10),
            ..Default::default()
        };
        let t = Instant::now();
        let results = match wiki.hybrid_search(&q.query, opts) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  [{label}] hybrid_search failed: {e}");
                return;
            }
        };
        latencies.push(t.elapsed().as_secs_f64() * 1000.0);
        let sources: Vec<String> = results
            .into_iter()
            .filter_map(|r| src_by_id.get(&r.fact_id).cloned())
            .collect();
        let (r, mrr, ndcg) = score_query(&sources, &expected, 10);
        sum_r += r;
        sum_mrr += mrr;
        sum_ndcg += ndcg;
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = golden.len() as f64;
    println!(
        "{:<32}  R@10={:.3}  MRR@10={:.3}  nDCG@10={:.3}  p50={:5.1}ms  p95={:5.1}ms  build={:.1}s ({} embedded)",
        label,
        sum_r / n,
        sum_mrr / n,
        sum_ndcg / n,
        percentile(&latencies, 0.50),
        percentile(&latencies, 0.95),
        build_s,
        count
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let matrix_filter = args
        .iter()
        .find(|a| a.starts_with("--matrix="))
        .map(|a| a.trim_start_matches("--matrix=").to_string());

    if matrix_filter.as_deref() == Some("list") {
        for (label, provider, endpoint, model_id, _dim) in MATRIX {
            println!(
                "  {:<32}  provider={} endpoint={} model={}",
                label, provider, endpoint, model_id
            );
        }
        return;
    }

    let allow: Option<Vec<String>> = matrix_filter
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(str::to_string).collect());

    let limit: Option<usize> = args
        .iter()
        .find(|a| a.starts_with("--limit="))
        .and_then(|a| a.trim_start_matches("--limit=").parse().ok());

    let mut golden = load_golden();
    if let Some(n) = limit {
        golden.truncate(n);
    }
    eprintln!("golden queries: {}", golden.len());

    println!(
        "{:<32}  R@10     MRR@10   nDCG@10  p50      p95      build",
        "config"
    );
    for (label, provider, endpoint, model_id, dim) in MATRIX {
        if let Some(allow) = &allow {
            if !allow.iter().any(|a| a == label) {
                continue;
            }
        }
        run_config(label, provider, endpoint, model_id, *dim, &golden);
    }
}
