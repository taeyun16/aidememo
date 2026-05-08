//! MultiHop-RAG retrieval baseline for `wg`.
//!
//! Loads the COLM-2024 dataset (yixuantt/MultiHopRAG on HuggingFace)
//! and benchmarks `wg`'s hybrid search across 2556 multi-document
//! queries spread over a 609-article news corpus.
//!
//! Unlike LongMemEval (one isolated store per question), MultiHop-RAG's
//! corpus is shared across all queries — so we ingest the corpus ONCE
//! into a single store and run every query against it. Closer to a
//! real RAG deployment.
//!
//! Usage:
//!
//! ```bash
//! MULTIHOP_DIR=/tmp/multihop_rag \
//!   cargo run --release -p wg-benchmarks --bin multihop_rag -- \
//!     --hybrid --top-k 10 \
//!     --emit-retrievals /tmp/wg_multihop_retrievals.jsonl
//! ```
//!
//! Outputs (when --emit-retrievals is set): one JSON line per query
//! with `{question_id, query, question_type, gold_answer,
//! retrievals: [...]}`. Feed into a Python reader (omega-style) for
//! end-to-end accuracy.
//!
//! Retrieval-only metrics (R@K + MRR) are printed to stderr.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wg_core::types::{FactInput, FactType};
use wg_core::{Config, EntityInput, EntityType, SearchOpts, WikiGraph};

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct Doc {
    title: String,
    body: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct Evidence {
    title: String,
    #[serde(default)]
    fact: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Query {
    query: String,
    answer: Value,
    question_type: String,
    #[serde(default)]
    evidence_list: Vec<Evidence>,
}

#[derive(Debug, Serialize)]
struct RetrievalRow {
    question_id: usize,
    query: String,
    question_type: String,
    gold_answer: Value,
    gold_evidence_titles: Vec<String>,
    retrievals: Vec<Hit>,
    first_evidence_rank: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Hit {
    rank: usize,
    fact_id: String,
    score: f32,
    content: String,
    doc_title: String,
    source: Option<String>,
    published_at: Option<String>,
}

struct Args {
    dir: PathBuf,
    out: Option<PathBuf>,
    top_k: usize,
    hybrid: bool,
    limit: Option<usize>,
    only_type: Option<String>,
    embed_model: Option<String>,
}

fn parse_args() -> Args {
    let dir = std::env::var("MULTIHOP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/multihop_rag"));
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut out = None;
    let mut top_k = 10;
    let mut hybrid = false;
    let mut limit = None;
    let mut only_type = None;
    let mut embed_model: Option<String> = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--emit-retrievals" if i + 1 < argv.len() => {
                out = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--top-k" if i + 1 < argv.len() => {
                top_k = argv[i + 1].parse().unwrap_or(10);
                i += 2;
            }
            "--hybrid" => {
                hybrid = true;
                i += 1;
            }
            "--limit" if i + 1 < argv.len() => {
                limit = argv[i + 1].parse().ok();
                i += 2;
            }
            "--only-type" if i + 1 < argv.len() => {
                only_type = Some(argv[i + 1].clone());
                i += 2;
            }
            "--embed-model" if i + 1 < argv.len() => {
                embed_model = Some(argv[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    Args {
        dir,
        out,
        top_k,
        hybrid,
        limit,
        only_type,
        embed_model,
    }
}

fn parse_iso_to_epoch_ms(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis() as u64)
}

fn build_store(
    corpus: &[Doc],
    hybrid: bool,
    embed_model: Option<&str>,
) -> Result<(tempfile::TempDir, WikiGraph), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = dir.path().join("multihop.redb");
    let mut config = Config::default();
    config.store.path = path.to_string_lossy().into_owned();
    if hybrid {
        // Leave config.search.semantic_index at the wg-core default
        // ("hnsw"). The previous "hybrid" override silently disabled
        // the HNSW path, which made the bench identical for any
        // embed-model — including BGE, which had given a +1.8pt R@5
        // lift on LongMemEval but registered 0pt here. Default-on
        // HNSW restores the apples-to-apples comparison.
        if let Some(name) = embed_model {
            config.model.provider = "fastembed".into();
            config.model.name = name.to_string();
        } else {
            config.model.provider = "model2vec".into();
        }
    }
    let wiki = WikiGraph::open(&path, config).map_err(|e| e.to_string())?;

    // Ingest each doc as one fact, with the doc title as an entity so
    // multi-doc questions can graph-traverse "X mentioned in N docs".
    // Body is chunked at ~500-char boundaries to give BM25 / semantic
    // a finer ranking surface — single-fact-per-doc loses precision
    // when the answer span is one sentence inside a 3KB article.
    let mut inputs = Vec::new();
    for doc in corpus {
        let entity_id = match wiki.entity_add(EntityInput {
            name: doc.title.clone(),
            entity_type: Some(EntityType::Custom("article".into())),
            ..Default::default()
        }) {
            Ok(id) => id,
            Err(_) => wiki.resolve_entity(&doc.title).map_err(|e| e.to_string())?,
        };
        let observed_at = doc.published_at.as_deref().and_then(parse_iso_to_epoch_ms);
        for chunk in chunk_text(&doc.body, 500) {
            inputs.push(FactInput {
                content: chunk,
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![entity_id]),
                tags: doc.source.as_ref().map(|s| vec![format!("source:{s}")]),
                source: doc.source.clone(),
                source_confidence: None,
                observed_at,
            });
        }
    }
    wiki.fact_add_many(inputs).map_err(|e| e.to_string())?;
    Ok((dir, wiki))
}

fn chunk_text(text: &str, target_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for sentence in text.split_inclusive(['.', '!', '?']) {
        if buf.len() + sentence.len() > target_chars && !buf.is_empty() {
            out.push(std::mem::take(&mut buf));
        }
        buf.push_str(sentence);
    }
    if !buf.trim().is_empty() {
        out.push(buf);
    }
    if out.is_empty() && !text.is_empty() {
        out.push(text.to_string());
    }
    out
}

fn main() -> ExitCode {
    let args = parse_args();
    let started = Instant::now();

    let corpus_path = args.dir.join("corpus.json");
    let queries_path = args.dir.join("MultiHopRAG.json");
    let raw_corpus = std::fs::read_to_string(&corpus_path).unwrap_or_else(|e| {
        eprintln!("error: read {corpus_path:?}: {e}");
        std::process::exit(2);
    });
    let raw_queries = std::fs::read_to_string(&queries_path).unwrap_or_else(|e| {
        eprintln!("error: read {queries_path:?}: {e}");
        std::process::exit(2);
    });
    let corpus: Vec<Doc> = serde_json::from_str(&raw_corpus).expect("parse corpus");
    let mut queries: Vec<Query> = serde_json::from_str(&raw_queries).expect("parse queries");
    if let Some(t) = &args.only_type {
        queries.retain(|q| &q.question_type == t);
    }
    if let Some(n) = args.limit {
        queries.truncate(n);
    }
    println!(
        "MultiHop-RAG: corpus={} docs, queries={}",
        corpus.len(),
        queries.len()
    );

    let ingest_start = Instant::now();
    let (_dir, wiki) = build_store(&corpus, args.hybrid, args.embed_model.as_deref()).unwrap_or_else(|e| {
        eprintln!("ingest failed: {e}");
        std::process::exit(1);
    });
    println!(
        "ingest: {} docs, {:.2?}",
        corpus.len(),
        ingest_start.elapsed()
    );

    let mut writer: Option<std::io::BufWriter<std::fs::File>> = args.out.as_ref().map(|p| {
        let f = std::fs::File::create(p).expect("create output");
        std::io::BufWriter::new(f)
    });

    let mut hits_at_k: HashMap<usize, usize> = HashMap::new();
    for k in [1usize, 5, 10, 30] {
        hits_at_k.insert(k, 0);
    }
    let mut mrr_sum = 0.0_f64;
    let mut by_type_ok: HashMap<String, (usize, usize)> = HashMap::new();
    let mut all_evidence_recall: HashMap<String, (usize, usize)> = HashMap::new(); // (hit_total, gold_total)

    let opts = SearchOpts {
        limit: Some(args.top_k.max(30)),
        ..Default::default()
    };

    for (qid, q) in queries.iter().enumerate() {
        let results = wiki.search(&q.query, opts.clone()).unwrap_or_else(|e| {
            eprintln!("  ! search fail q={qid}: {e}");
            Vec::new()
        });
        let gold_titles: Vec<String> = q.evidence_list.iter().map(|e| e.title.clone()).collect();

        // Find first hit whose doc title is in evidence_list.
        let mut first_evidence_rank: Option<usize> = None;
        let mut hits = Vec::new();
        let mut ent_cache: HashMap<wg_core::types::FactId, String> = HashMap::new();
        for (rank, r) in results.iter().enumerate() {
            let rank = rank + 1;
            // Doc title is the primary entity attached to the fact.
            let doc_title = if let Some(t) = ent_cache.get(&r.fact_id) {
                t.clone()
            } else {
                let mut name = String::new();
                if let Ok(fact) = wiki.fact_get(&r.fact_id) {
                    if let Some(eid) = fact.entity_ids.first() {
                        if let Ok(ent) = wiki.entity_get_by_id(*eid) {
                            name = ent.name;
                        }
                    }
                }
                ent_cache.insert(r.fact_id, name.clone());
                name
            };
            let is_evidence = gold_titles.iter().any(|t| t == &doc_title);
            if is_evidence && first_evidence_rank.is_none() {
                first_evidence_rank = Some(rank);
            }
            if rank <= args.top_k {
                hits.push(Hit {
                    rank,
                    fact_id: r.fact_id.to_string(),
                    score: r.score,
                    content: r.content.clone(),
                    doc_title: doc_title.clone(),
                    source: r.source.clone(),
                    published_at: r.observed_at.and_then(|ms| {
                        chrono::DateTime::from_timestamp_millis(ms as i64).map(|dt| dt.to_rfc3339())
                    }),
                });
            }
        }

        // R@K: did any gold-evidence doc appear in top-K?
        for k in [1usize, 5, 10, 30] {
            if let Some(r) = first_evidence_rank {
                if r <= k {
                    *hits_at_k.entry(k).or_insert(0) += 1;
                }
            }
        }
        if let Some(r) = first_evidence_rank {
            mrr_sum += 1.0 / r as f64;
        }
        let entry = by_type_ok.entry(q.question_type.clone()).or_insert((0, 0));
        entry.1 += 1;
        if first_evidence_rank.map(|r| r <= 10).unwrap_or(false) {
            entry.0 += 1;
        }

        // Per-evidence-doc recall (how many of the gold docs landed in top-30)
        if !gold_titles.is_empty() {
            let surfaced_top30: std::collections::HashSet<String> = results
                .iter()
                .take(30)
                .filter_map(|r| ent_cache.get(&r.fact_id).cloned())
                .collect();
            let hit = gold_titles
                .iter()
                .filter(|t| surfaced_top30.contains(*t))
                .count();
            let e = all_evidence_recall
                .entry(q.question_type.clone())
                .or_insert((0, 0));
            e.0 += hit;
            e.1 += gold_titles.len();
        }

        if let Some(w) = writer.as_mut() {
            let row = RetrievalRow {
                question_id: qid,
                query: q.query.clone(),
                question_type: q.question_type.clone(),
                gold_answer: q.answer.clone(),
                gold_evidence_titles: gold_titles,
                retrievals: hits,
                first_evidence_rank,
            };
            use std::io::Write;
            writeln!(w, "{}", serde_json::to_string(&row).unwrap()).unwrap();
        }

        if (qid + 1) % 200 == 0 {
            eprintln!(
                "    [{:>4}/{}] elapsed {:.1?}",
                qid + 1,
                queries.len(),
                started.elapsed()
            );
        }
    }

    if let Some(w) = writer.as_mut() {
        use std::io::Write;
        w.flush().unwrap();
    }

    let n = queries.len() as f64;
    let n_with_evidence = queries
        .iter()
        .filter(|q| !q.evidence_list.is_empty())
        .count() as f64;
    println!();
    println!(
        "Result: {} queries, top_k={}, hybrid={}",
        queries.len(),
        args.top_k,
        args.hybrid
    );
    for k in [1, 5, 10, 30] {
        let hit = *hits_at_k.get(&k).unwrap_or(&0);
        // Only count among queries with evidence (null_query has none).
        let denom = n_with_evidence.max(1.0);
        println!("  R@{k}: {:.3} ({}/{:.0})", hit as f64 / denom, hit, denom);
    }
    println!("  MRR: {:.3}", mrr_sum / n_with_evidence.max(1.0));
    println!("  wall: {:.2?}", started.elapsed());
    println!();
    println!("By question_type (R@10):");
    for (qt, (ok, total)) in by_type_ok.iter() {
        println!(
            "  {qt:<20} {ok}/{total} ({:.3})",
            *ok as f64 / *total as f64
        );
    }
    println!();
    println!("Per-evidence-doc recall (top-30, fraction of gold docs surfaced):");
    for (qt, (hit, total)) in all_evidence_recall.iter() {
        println!(
            "  {qt:<20} {hit}/{total} ({:.3})",
            *hit as f64 / *total as f64
        );
    }

    let _ = n;
    ExitCode::SUCCESS
}
