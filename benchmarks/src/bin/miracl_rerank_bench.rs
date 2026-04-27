//! Reranker A/B on MIRACL/ko.
//!
//! Runs every dev query through `WikiGraph::hybrid_search` twice —
//! once with `rerank.provider = ""` (off) and once with `rerank.provider
//! = "tei"` pointed at a local TEI `/rerank` endpoint — and compares
//! Recall@10, MRR@10, and nDCG@10 against the MIRACL/ko qrels.
//!
//! The run uses the same wg store rebuilt by `miracl_ingest`. Pre-reqs:
//!   - `/tmp/wg-bench-miracl/_meta/wiki.redb` ingested
//!   - `/tmp/wg-tei-bench/miracl_ko_golden.jsonl` written
//!   - TEI native rerank server reachable at `--rerank-endpoint`
//!     (defaults to http://localhost:8082)
//!
//! Embedding stays on `model2vec`/`potion-multilingual-128M` per
//! prior bench (R@10 = 0.706 baseline). The HNSW sidecar is rebuilt
//! once at the start so the warm-side numbers reflect the cached
//! state every search will see in production.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use serde::Deserialize;
use wg_core::{Config, FactListOpts, SearchOpts, WikiGraph};

const STORE: &str = "/tmp/wg-bench-miracl/_meta/wiki.redb";
const GOLDEN: &str = "/tmp/wg-tei-bench/miracl_ko_golden.jsonl";

#[derive(Deserialize)]
struct GoldenRow {
    query: String,
    expected_sources: Vec<String>,
    #[allow(dead_code)]
    k: usize,
}

fn load_golden() -> Vec<GoldenRow> {
    let body = std::fs::read_to_string(GOLDEN).expect("read golden");
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("golden line"))
        .collect()
}

fn build_source_map(wiki: &WikiGraph) -> HashMap<wg_core::FactId, String> {
    // wg's FactId is a ULID assigned at insert time; we keyed the
    // golden set on the original miracl docid which lives in
    // `source`. Build the reverse map once.
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

#[derive(Default, Clone)]
struct Metrics {
    queries: usize,
    sum_recall_at_10: f64,
    sum_mrr_at_10: f64,
    sum_ndcg_at_10: f64,
    sum_latency_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
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

    // Binary relevance: docid in qrels => 1 else 0
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

fn run(
    label: &str,
    wiki: &WikiGraph,
    golden: &[GoldenRow],
    src_by_id: &HashMap<wg_core::FactId, String>,
) -> Metrics {
    let mut m = Metrics::default();
    let mut latencies = Vec::with_capacity(golden.len());

    for q in golden {
        let expected: HashSet<String> = q.expected_sources.iter().cloned().collect();
        let opts = SearchOpts {
            limit: Some(10),
            ..Default::default()
        };
        let t0 = Instant::now();
        let results = wiki.hybrid_search(&q.query, opts).expect("hybrid_search");
        let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
        latencies.push(dt_ms);

        let sources: Vec<String> = results
            .into_iter()
            .filter_map(|r| src_by_id.get(&r.fact_id).cloned())
            .collect();
        let (r, mrr, ndcg) = score_query(&sources, &expected, 10);
        m.sum_recall_at_10 += r;
        m.sum_mrr_at_10 += mrr;
        m.sum_ndcg_at_10 += ndcg;
        m.sum_latency_ms += dt_ms;
        m.queries += 1;
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    m.p50_ms = percentile(&latencies, 0.50);
    m.p95_ms = percentile(&latencies, 0.95);

    let n = m.queries as f64;
    println!(
        "{:<28}  R@10={:.3}  MRR@10={:.3}  nDCG@10={:.3}  mean={:.0}ms  p50={:.0}ms  p95={:.0}ms",
        label,
        m.sum_recall_at_10 / n,
        m.sum_mrr_at_10 / n,
        m.sum_ndcg_at_10 / n,
        m.sum_latency_ms / n,
        m.p50_ms,
        m.p95_ms,
    );
    m
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rerank_endpoint = args
        .iter()
        .find(|a| a.starts_with("--rerank="))
        .map(|a| a.trim_start_matches("--rerank=").to_string())
        .unwrap_or_else(|| "http://localhost:8082".to_string());
    let top_k_csv = args
        .iter()
        .find(|a| a.starts_with("--top-ks="))
        .map(|a| a.trim_start_matches("--top-ks=").to_string())
        .unwrap_or_else(|| "8,16,32".to_string());
    let top_ks: Vec<usize> = top_k_csv
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    let golden = load_golden();
    eprintln!("golden queries: {}", golden.len());

    // Stage 1: baseline (rerank disabled). Use HNSW path.
    {
        let mut config = Config::default();
        config.store.path = STORE.to_string();
        config.search.semantic_index = "hnsw".into();
        let wiki = WikiGraph::open(Path::new(STORE), config).expect("open wiki");
        let t0 = Instant::now();
        let _ = wiki.vector_index_rebuild().expect("hnsw build");
        eprintln!("HNSW build: {:.1}s", t0.elapsed().as_secs_f64());
        let src_by_id = build_source_map(&wiki);
        eprintln!("source map: {} entries", src_by_id.len());
        run("baseline (no rerank)", &wiki, &golden, &src_by_id);
    }

    // Stage 2: with rerank. New WikiGraph open with rerank config so
    // the lazy reranker loads on first search.
    for top_k in top_ks {
        let mut config = Config::default();
        config.store.path = STORE.to_string();
        config.search.semantic_index = "hnsw".into();
        config.rerank.provider = "tei".into();
        config.rerank.endpoint = rerank_endpoint.clone();
        config.rerank.model = "BAAI/bge-reranker-base".into();
        config.rerank.top_k = top_k;
        let wiki = WikiGraph::open(Path::new(STORE), config).expect("open wiki rerank");
        let _ = wiki.vector_index_rebuild().expect("hnsw build");
        let src_by_id = build_source_map(&wiki);
        let label = format!("rerank=tei top_k={}", top_k);
        run(&label, &wiki, &golden, &src_by_id);
    }
}
