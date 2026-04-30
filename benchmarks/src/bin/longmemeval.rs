//! LongMemEval-S retrieval baseline for `wg`.
//!
//! Loads the LongMemEval-S JSON file (publicly available from the
//! `xiaowu0162/longmemeval` HF dataset — see `.notes/bench-longmemeval.md`
//! for the curl/hf-hub commands), and for every question:
//!
//! 1. Spins up a fresh, isolated wg store under a tempdir so haystacks
//!    from one question can't leak into another.
//! 2. Ingests every chat turn from `haystack_sessions` as a fact —
//!    one fact per turn, tagged with `session:<haystack_session_id>`
//!    so we can later identify whether retrieved hits sit inside an
//!    `answer_session_ids` evidence session.
//! 3. Runs `wg_search` (BM25-only via `bm25_only=true` to keep the
//!    baseline portable; semantic adds noise when the dataset is in
//!    English and the default model is multilingual potion-128M).
//! 4. Checks the top-K hits against `answer_session_ids` and records
//!    rank of the first hit, hit-at-1, hit-at-5, hit-at-10.
//!
//! Reports R@1, R@5, R@10, MRR. This is the **retrieval-only** axis
//! of LongMemEval — the official end-to-end metric needs an LLM to
//! generate an answer from the retrieved context. Retrieval recall
//! is the part `wg` directly affects, and high recall is necessary
//! for high answer correctness, so this number is a useful proxy and
//! a fair head-to-head against other memory backends evaluated on
//! the same axis.
//!
//! Usage:
//!
//! ```bash
//! # Tiny fixture (committed) — sanity-check the harness without
//! # downloading the 277 MB cleaned dataset.
//! cargo run --release -p wg-benchmarks --bin longmemeval -- \
//!     --data benchmarks/fixtures/longmemeval_tiny.json
//!
//! # Full dataset.
//! LONGMEMEVAL_DATA=/tmp/longmemeval_s_cleaned.json \
//!   cargo run --release -p wg-benchmarks --bin longmemeval -- --limit 50
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use serde::Deserialize;
use serde_json::Value;
use wg_core::types::{FactInput, FactType};
use wg_core::{Config, EntityInput, EntityType, SearchOpts, WikiGraph};

#[derive(Debug, Deserialize)]
struct Question {
    question_id: String,
    #[serde(default)]
    question_type: String,
    question: String,
    // Captured for schema fidelity / future answer-correctness eval —
    // current harness only measures retrieval. Type is `Value` (not
    // `String`) because LongMemEval-S contains numeric answers
    // (e.g. "how many" → 3) alongside string answers.
    #[serde(default)]
    #[allow(dead_code)]
    answer: Value,
    /// "2023/02/01 (Wed) 10:20" — when the question is asked. With
    /// `--temporal` we interpret it as the `as_of` boundary; facts
    /// from later sessions are filtered out.
    #[serde(default)]
    question_date: Option<String>,
    /// Per-session date in the same format. With `--temporal` each
    /// fact in a session inherits its session's date as `observed_at`.
    #[serde(default)]
    haystack_dates: Vec<String>,
    haystack_session_ids: Vec<String>,
    haystack_sessions: Vec<Vec<Turn>>,
    answer_session_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Turn {
    role: String,
    content: String,
    // Per-turn evidence flag is captured but not currently scored —
    // future per-turn precision pass can use it.
    #[serde(default)]
    #[allow(dead_code)]
    has_answer: Option<bool>,
}

struct Args {
    data: PathBuf,
    limit: Option<usize>,
    top_k: usize,
    /// Enable temporal-aware retrieval: stamp each fact with its
    /// session's `haystack_date` and pass `until=question_date` so
    /// future-dated noise is filtered. Hard cutoff — see negative
    /// finding in `.notes/bench-longmemeval.md`. Defaults off.
    temporal: bool,
    /// Time-decay tau in days. When set, the harness stamps
    /// `observed_at` from session dates (like `--temporal`) but does
    /// NOT apply a hard `until` cutoff; instead BM25 scores are
    /// multiplied post-hoc by `exp(-age_days_from_question / tau)`
    /// and re-sorted. Soft bias toward sessions near the question
    /// date — won't drop legitimate evidence that's only slightly
    /// future-dated (the dataset noise the negative-result analysis
    /// flagged).
    time_decay_days: Option<f64>,
    /// Restrict the run to a single question_type bucket. Lets you
    /// re-measure just one slice without re-running all 500.
    only_type: Option<String>,
}

/// Parse "2023/02/01 (Wed) 10:20" → epoch ms. Returns None on any
/// parse failure (the harness then falls back to "no temporal info").
fn parse_question_date(raw: &str) -> Option<u64> {
    // Split off the parenthesised weekday: "2023/02/01 (Wed) 10:20"
    let mut parts = raw.split_whitespace();
    let date = parts.next()?; // "2023/02/01"
    let _weekday = parts.next(); // "(Wed)" — discarded
    let time = parts.next()?; // "10:20"
    let mut date_parts = date.split('/');
    let y: i32 = date_parts.next()?.parse().ok()?;
    let mo: u32 = date_parts.next()?.parse().ok()?;
    let d: u32 = date_parts.next()?.parse().ok()?;
    let mut tparts = time.split(':');
    let h: u32 = tparts.next()?.parse().ok()?;
    let m: u32 = tparts.next()?.parse().ok()?;
    // Days-from-epoch via the civil-from-fields algorithm (Howard Hinnant).
    let y_adj = if mo <= 2 { y - 1 } else { y };
    let era = y_adj.div_euclid(400);
    let yoe = (y_adj - era * 400) as u32;
    let mp = if mo <= 2 { mo + 9 } else { mo - 3 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe as i32 - 719_468;
    let seconds_per_day = 86_400_i64;
    let secs = days as i64 * seconds_per_day + (h as i64) * 3600 + (m as i64) * 60;
    Some((secs * 1000) as u64)
}

fn parse_args() -> Args {
    let mut data = std::env::var("LONGMEMEVAL_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("benchmarks/fixtures/longmemeval_tiny.json"));
    let mut limit: Option<usize> = None;
    let mut top_k: usize = 10;
    let mut temporal = false;
    let mut time_decay_days: Option<f64> = None;
    let mut only_type: Option<String> = None;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--data" if i + 1 < argv.len() => {
                data = PathBuf::from(&argv[i + 1]);
                i += 2;
            }
            "--limit" if i + 1 < argv.len() => {
                limit = argv[i + 1].parse().ok();
                i += 2;
            }
            "--top-k" if i + 1 < argv.len() => {
                top_k = argv[i + 1].parse().unwrap_or(10);
                i += 2;
            }
            "--temporal" => {
                temporal = true;
                i += 1;
            }
            "--time-decay-days" if i + 1 < argv.len() => {
                time_decay_days = argv[i + 1].parse().ok();
                i += 2;
            }
            "--only-type" if i + 1 < argv.len() => {
                only_type = Some(argv[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    Args {
        data,
        limit,
        top_k,
        temporal,
        time_decay_days,
        only_type,
    }
}

fn build_store_for_question(
    q: &Question,
    temporal: bool,
    stamp_observed_at: bool,
) -> Result<(tempfile::TempDir, WikiGraph), String> {
    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let mut config = Config::default();
    config.store.path = dir.path().join("store").to_string_lossy().into_owned();
    // Skip the embedding-model load — BM25-only baseline.
    config.search.semantic_index = "bm25".into();
    let store_path = PathBuf::from(&config.store.path);
    let wiki = WikiGraph::open(&store_path, config).map_err(|e| e.to_string())?;

    // Each haystack session becomes an entity so we can tag turns
    // with it via `entity_ids`. That keeps the session linkage in
    // the graph rather than relying on tag-substring lookups.
    // Some questions reference the same session_id twice — dedupe by
    // resolving when entity_add hits "already exists".
    let mut session_eids = Vec::with_capacity(q.haystack_session_ids.len());
    for sid in &q.haystack_session_ids {
        let name = format!("session:{sid}");
        let id = match wiki.entity_add(EntityInput {
            name: name.clone(),
            entity_type: Some(EntityType::Custom("session".into())),
            ..Default::default()
        }) {
            Ok(id) => id,
            Err(_) => wiki.resolve_entity(&name).map_err(|e| e.to_string())?,
        };
        session_eids.push(id);
    }

    // Batch all turns under one fact_add_many — single fsync, fast.
    let mut inputs = Vec::new();
    for (sess_idx, session) in q.haystack_sessions.iter().enumerate() {
        let entity_id = session_eids.get(sess_idx).copied();
        // Stamp observed_at when caller asks (--temporal hard cutoff
        // OR --time-decay soft bias both need session-relative dates).
        let observed_at = if stamp_observed_at {
            q.haystack_dates
                .get(sess_idx)
                .and_then(|d| parse_question_date(d))
        } else {
            None
        };
        let _ = temporal; // disambiguation only — both paths share this stamp
        for turn in session {
            inputs.push(FactInput {
                content: format!("{}: {}", turn.role, turn.content),
                fact_type: Some(FactType::Note),
                entity_ids: entity_id.map(|e| vec![e]),
                tags: Some(vec![format!(
                    "session:{}",
                    q.haystack_session_ids[sess_idx]
                )]),
                source: None,
                source_confidence: None,
                observed_at,
            });
        }
    }
    if !inputs.is_empty() {
        wiki.fact_add_many(inputs).map_err(|e| e.to_string())?;
    }
    Ok((dir, wiki))
}

fn evaluate(
    q: &Question,
    wiki: &WikiGraph,
    top_k: usize,
    temporal: bool,
    time_decay_days: Option<f64>,
) -> Option<usize> {
    // BM25-only baseline. Returns the 1-indexed rank of the first hit
    // whose entity matches one of the answer-session entities, or
    // None if no such hit appears in the top-K.
    //
    // - `temporal=true`: hard `until=question_date` cutoff.
    // - `time_decay_days=Some(tau)`: soft bias — request a wider
    //   candidate slate, multiply each BM25 score by
    //   `exp(-age_days/tau)` where age = |question_date -
    //   observed_at| in days, then re-sort and keep top_k. Works
    //   even when evidence is slightly future-dated (the dataset
    //   noise the previous experiment surfaced).
    let until = if temporal {
        q.question_date.as_deref().and_then(parse_question_date)
    } else {
        None
    };
    // Pull a wider slate when applying decay so post-hoc re-rank can
    // promote a low-BM25-but-recent hit into the top-K.
    let candidate_limit = if time_decay_days.is_some() {
        top_k.saturating_mul(5).max(50)
    } else {
        top_k
    };
    let opts = SearchOpts {
        limit: Some(candidate_limit),
        bm25_only: true,
        current_only: true,
        until,
        ..Default::default()
    };
    let mut results = wiki.hybrid_search(&q.question, opts).ok()?;
    if let Some(tau_days) = time_decay_days {
        let q_date = q
            .question_date
            .as_deref()
            .and_then(parse_question_date)
            .unwrap_or(0);
        // Pre-fetch per-fact observed_at to avoid borrowing wiki twice
        // inside the sort closure.
        let observed: Vec<u64> = results
            .iter()
            .map(|r| {
                wiki.fact_get(&r.fact_id)
                    .ok()
                    .and_then(|f| f.observed_at)
                    .unwrap_or(0)
            })
            .collect();
        for (r, obs) in results.iter_mut().zip(observed.iter()) {
            let age_ms = (q_date as i64 - *obs as i64).unsigned_abs();
            let age_days = age_ms as f64 / 86_400_000.0;
            let decay = (-age_days / tau_days).exp() as f32;
            r.score *= decay;
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(top_k);
    }
    let answer_entity_names: std::collections::HashSet<String> = q
        .answer_session_ids
        .iter()
        .map(|sid| format!("session:{sid}"))
        .collect();
    for (rank, hit) in results.iter().enumerate() {
        // Cheap reverse map: fetch the fact and inspect its
        // session-tagged entity. wg's fact records hold entity_ids;
        // we resolve to names and check membership.
        if let Ok(fact) = wiki.fact_get(&hit.fact_id) {
            for eid in &fact.entity_ids {
                if let Ok(ent) = wiki.entity_get_by_id(*eid)
                    && answer_entity_names.contains(&ent.name)
                {
                    return Some(rank + 1);
                }
            }
        }
    }
    None
}

fn run(args: &Args) -> Result<(), String> {
    let data = args.data.as_path();
    let limit = args.limit;
    let top_k = args.top_k;
    let temporal = args.temporal;
    let raw = std::fs::read_to_string(data).map_err(|e| format!("read {data:?}: {e}"))?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("parse: {e}"))?;
    let mut questions: Vec<Question> = if v.is_array() {
        serde_json::from_value(v).map_err(|e| format!("array parse: {e}"))?
    } else {
        vec![serde_json::from_value(v).map_err(|e| format!("single parse: {e}"))?]
    };
    if let Some(only) = &args.only_type {
        questions.retain(|q| q.question_type == *only);
    }
    let n = limit
        .map(|l| l.min(questions.len()))
        .unwrap_or(questions.len());
    println!("LongMemEval-S retrieval baseline — wg BM25-only");
    println!("dataset:  {data:?}");
    println!("questions: {n} (of {})", questions.len());
    println!("top_k:    {top_k}");
    println!("temporal: {temporal}");
    if let Some(t) = args.time_decay_days {
        println!("decay τ:  {t} days");
    }
    if let Some(only) = &args.only_type {
        println!("only:     {only}");
    }
    println!();

    let mut hit_at_1 = 0usize;
    let mut hit_at_5 = 0usize;
    let mut hit_at_10 = 0usize;
    let mut reciprocal_sum = 0.0_f64;
    let mut by_type: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let started = Instant::now();
    for (i, q) in questions.iter().take(n).enumerate() {
        let stamp_obs = temporal || args.time_decay_days.is_some();
        let (_dir, wiki) = build_store_for_question(q, temporal, stamp_obs)?;
        let rank = evaluate(q, &wiki, top_k, temporal, args.time_decay_days);
        if let Some(r) = rank {
            if r <= 1 {
                hit_at_1 += 1;
            }
            if r <= 5 {
                hit_at_5 += 1;
            }
            if r <= 10 {
                hit_at_10 += 1;
            }
            reciprocal_sum += 1.0 / r as f64;
        }
        let bucket = by_type.entry(q.question_type.clone()).or_insert((0, 0));
        bucket.0 += 1;
        if rank.is_some() {
            bucket.1 += 1;
        }
        if (i + 1) % 10 == 0 {
            eprintln!("[{:>4}/{}] processed (last: {})", i + 1, n, q.question_id);
        }
    }
    let wall = started.elapsed();
    let denom = n as f64;
    println!("R@1:  {:.3}", hit_at_1 as f64 / denom);
    println!("R@5:  {:.3}", hit_at_5 as f64 / denom);
    println!("R@10: {:.3}", hit_at_10 as f64 / denom);
    println!("MRR:  {:.3}", reciprocal_sum / denom);
    println!("wall: {:.2}s", wall.as_secs_f64());
    println!();
    println!("By question_type:");
    for (qt, (total, hit)) in &by_type {
        let qt_disp = if qt.is_empty() {
            "<unknown>"
        } else {
            qt.as_str()
        };
        println!(
            "  {qt_disp:30}  R@{top_k}: {:.3}  ({hit}/{total})",
            *hit as f64 / *total as f64
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = parse_args();
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
