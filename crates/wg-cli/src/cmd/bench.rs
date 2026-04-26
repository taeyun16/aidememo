//! `wg bench` — search-relevance benchmark.
//!
//! Reads a JSONL "golden set" of queries with expected fact IDs and reports
//! Precision@K, Recall@K, and search latency (p50 / p95). Inspired by
//! gbrain's published P@5 / R@5 numbers — turning subjective "feels good"
//! into reproducible measurements.
//!
//! Golden file format (one JSON object per line):
//!
//! ```jsonl
//! {"query": "high availability", "expected": ["01KP...", "01KQ..."]}
//! {"query": "rust async runtime", "expected": ["01KR..."], "k": 3}
//! ```
//!
//! Optional `k` overrides the per-query top-K cutoff; otherwise the global
//! `--k` flag applies.

use bpaf::*;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Instant;
use wg_core::{Config, SearchOpts, WgError, WikiGraph};

use cpu_time::ProcessTime;
use memory_stats::memory_stats;

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub struct BenchSub {
    pub k: Option<usize>,
    pub limit: Option<usize>,
    pub json: bool,
    pub golden: PathBuf,
}

pub fn bench_command() -> impl Parser<Command> {
    let k = long("k")
        .help("Top-K cutoff for P@K and R@K (default 5)")
        .argument::<usize>("K")
        .optional();
    let limit = long("limit")
        .short('l')
        .help("Max hits per query (default 20)")
        .argument::<usize>("LIMIT")
        .optional();
    let json = long("json").short('j').help("Output as JSON").switch();
    let golden = positional::<PathBuf>("GOLDEN")
        .help("Path to a JSONL file: {\"query\": \"...\", \"expected\": [\"id\", ...]}");

    construct!(BenchSub {
        k,
        limit,
        json,
        golden
    })
    .map(Command::Bench)
    .to_options()
    .command("bench")
    .help("Benchmark search relevance against a golden JSONL set")
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GoldenRow {
    query: String,
    #[serde(default)]
    expected: Vec<String>,
    #[serde(default)]
    k: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
struct PerQuery {
    query: String,
    found: usize,
    expected: usize,
    p_at_k: f64,
    r_at_k: f64,
    latency_ms: f64,
}

#[derive(Debug, serde::Serialize)]
struct Summary {
    total_queries: usize,
    queries_with_expected: usize,
    k: usize,
    mean_p_at_k: f64,
    mean_r_at_k: f64,
    p50_latency_ms: f64,
    p95_latency_ms: f64,
    /// Tier 6-A: process-level resource use across the benchmark run.
    /// `None` when measurement isn't available (memory_stats can fail in
    /// containers / sandboxes where /proc isn't readable).
    profile: Option<ProfileMetrics>,
    per_query: Vec<PerQuery>,
}

#[derive(Debug, serde::Serialize)]
struct ProfileMetrics {
    /// Process resident set size before the benchmark started.
    rss_baseline_mb: f64,
    /// Peak RSS sampled across queries.
    rss_peak_mb: f64,
    /// Delta peak − baseline.
    rss_delta_mb: f64,
    /// CPU user time spent inside this process for the whole bench run.
    cpu_user_ms: f64,
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

pub fn run_bench(
    store_path: &PathBuf,
    config: Config,
    sub: BenchSub,
    global_json: bool,
) -> Result<String, WgError> {
    let default_k = sub.k.unwrap_or(5);
    let limit = sub.limit.unwrap_or(20);
    let json_mode = sub.json || global_json;

    let raw = std::fs::read_to_string(&sub.golden)
        .map_err(|e| WgError::FileRead(sub.golden.clone(), e.to_string()))?;

    let mut rows: Vec<GoldenRow> = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let row: GoldenRow = serde_json::from_str(line)
            .map_err(|e| WgError::InvalidInput(format!("golden line {}: {}", i + 1, e)))?;
        rows.push(row);
    }

    if rows.is_empty() {
        return Err(WgError::InvalidInput(
            "golden file has no usable rows".into(),
        ));
    }

    let wiki = WikiGraph::open(store_path, config)?;
    let mut per_query: Vec<PerQuery> = Vec::with_capacity(rows.len());
    let mut latencies_us: Vec<u64> = Vec::with_capacity(rows.len());
    let mut p_sum = 0f64;
    let mut r_sum = 0f64;
    let mut counted = 0usize;

    // Tier 6-A: snapshot baseline RSS + start CPU-time clock so we can
    // attribute resource cost to this whole bench run.
    let baseline_rss = memory_stats().map(|s| s.physical_mem);
    let cpu_start = ProcessTime::now();
    let mut peak_rss: Option<usize> = baseline_rss;

    for row in &rows {
        let row_k = row.k.unwrap_or(default_k);

        let started = Instant::now();
        let results = wiki
            .hybrid_search(
                &row.query,
                SearchOpts {
                    limit: Some(limit),
                    ..Default::default()
                },
            )
            .unwrap_or_default();
        let elapsed = started.elapsed();
        latencies_us.push(elapsed.as_micros() as u64);

        let top: Vec<String> = results
            .iter()
            .take(row_k)
            .map(|r| r.fact_id.to_string())
            .collect();
        let expected_set: std::collections::HashSet<&str> =
            row.expected.iter().map(|s| s.as_str()).collect();
        let found = top
            .iter()
            .filter(|id| expected_set.contains(id.as_str()))
            .count();

        let (p, r) = if row.expected.is_empty() {
            (f64::NAN, f64::NAN)
        } else {
            let p = found as f64 / row_k as f64;
            let r = found as f64 / row.expected.len() as f64;
            counted += 1;
            p_sum += p;
            r_sum += r;
            (p, r)
        };
        per_query.push(PerQuery {
            query: row.query.clone(),
            found,
            expected: row.expected.len(),
            p_at_k: p,
            r_at_k: r,
            latency_ms: elapsed.as_secs_f64() * 1000.0,
        });

        // Sample RSS after each query. memory_stats is a syscall; doing it
        // per-query (vs. inside a thread) keeps the code dead simple at the
        // cost of slightly understating true peak between samples.
        if let Some(s) = memory_stats() {
            peak_rss = Some(peak_rss.map_or(s.physical_mem, |p| p.max(s.physical_mem)));
        }
    }

    let cpu_user = cpu_start.elapsed();

    latencies_us.sort_unstable();
    let p50 = percentile(&latencies_us, 50);
    let p95 = percentile(&latencies_us, 95);

    let mean_p = if counted > 0 {
        p_sum / counted as f64
    } else {
        f64::NAN
    };
    let mean_r = if counted > 0 {
        r_sum / counted as f64
    } else {
        f64::NAN
    };

    let profile = match (baseline_rss, peak_rss) {
        (Some(base), Some(peak)) => Some(ProfileMetrics {
            rss_baseline_mb: base as f64 / 1_048_576.0,
            rss_peak_mb: peak as f64 / 1_048_576.0,
            rss_delta_mb: (peak.saturating_sub(base)) as f64 / 1_048_576.0,
            cpu_user_ms: cpu_user.as_secs_f64() * 1000.0,
        }),
        _ => None,
    };

    let summary = Summary {
        total_queries: rows.len(),
        queries_with_expected: counted,
        k: default_k,
        mean_p_at_k: mean_p,
        mean_r_at_k: mean_r,
        p50_latency_ms: p50,
        p95_latency_ms: p95,
        profile,
        per_query,
    };

    if json_mode {
        return serde_json::to_string_pretty(&summary).map_err(|e| WgError::Serialize {
            context: "bench".to_string(),
            source: e,
        });
    }

    Ok(format_human(&summary))
}

fn percentile(sorted_us: &[u64], pct: usize) -> f64 {
    if sorted_us.is_empty() {
        return 0.0;
    }
    let idx = ((pct as f64 / 100.0) * (sorted_us.len() - 1) as f64).round() as usize;
    sorted_us[idx.min(sorted_us.len() - 1)] as f64 / 1000.0
}

fn format_human(s: &Summary) -> String {
    let mut out = String::new();
    out.push_str(&format!("Bench: {} queries (K={})\n", s.total_queries, s.k));
    if s.queries_with_expected == 0 {
        out.push_str(
            "  (no queries had `expected` IDs — relevance not measured, latency only)\n\n",
        );
    } else {
        out.push_str(&format!("  Mean P@{}:  {:.3}\n", s.k, s.mean_p_at_k));
        out.push_str(&format!("  Mean R@{}:  {:.3}\n", s.k, s.mean_r_at_k));
        out.push_str(&format!(
            "  ({} queries had expected IDs)\n",
            s.queries_with_expected
        ));
    }
    out.push_str(&format!(
        "  Latency:  p50 = {:.2} ms,  p95 = {:.2} ms\n",
        s.p50_latency_ms, s.p95_latency_ms
    ));

    if let Some(p) = &s.profile {
        out.push_str(&format!("  CPU:      user = {:.1} ms\n", p.cpu_user_ms));
        out.push_str(&format!(
            "  RSS:      baseline = {:.1} MB,  peak = {:.1} MB,  delta = +{:.1} MB\n",
            p.rss_baseline_mb, p.rss_peak_mb, p.rss_delta_mb
        ));
    }
    out.push('\n');

    out.push_str("Per-query:\n");
    for q in &s.per_query {
        let snippet: String = q.query.chars().take(50).collect();
        if q.expected == 0 {
            out.push_str(&format!(
                "  [{:>5.1}ms] {} — found {} (no gold)\n",
                q.latency_ms, snippet, q.found
            ));
        } else {
            out.push_str(&format!(
                "  [{:>5.1}ms] {} — P={:.2}  R={:.2}  ({}/{})\n",
                q.latency_ms, snippet, q.p_at_k, q.r_at_k, q.found, q.expected
            ));
        }
    }
    out
}
