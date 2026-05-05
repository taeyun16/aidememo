//! HotpotQA distractor-setting retrieval baseline for `wg`.
//!
//! Loads `hotpot_dev_distractor_v1.json` (7,405 multi-hop QA pairs)
//! and benchmarks `wg`'s hybrid search per question. Each question
//! comes with 10 candidate paragraphs (2-3 gold + 7-8 distractors)
//! laid out as `context: [[title, [sentences]]]`. We ingest each
//! paragraph as facts (one fact per sentence so supporting-facts
//! sentence-level recall is measurable), title as `paragraph` entity.
//!
//! Per-question fresh store (mirrors LongMemEval), since each
//! question has its own distractor pool.
//!
//! Outputs (when --emit-retrievals is set): one JSON line per
//! question with `{question_id, query, type, gold_answer,
//! gold_supporting_facts: [[title, sent_idx]], retrievals: [...]}`.
//!
//! Usage:
//!
//! ```bash
//! HOTPOTQA=/tmp/hotpotqa/dev_distractor.json \
//!   cargo run --release -p wg-benchmarks --bin hotpotqa -- \
//!     --hybrid --top-k 5 \
//!     --emit-retrievals /tmp/wg_hotpotqa.jsonl
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wg_core::types::{FactInput, FactType};
use wg_core::{Config, EntityInput, EntityType, SearchOpts, WikiGraph};

#[derive(Debug, Deserialize)]
struct Question {
    #[serde(rename = "_id")]
    id: String,
    question: String,
    answer: Value,
    #[serde(rename = "type")]
    qtype: String,
    level: String,
    context: Vec<(String, Vec<String>)>,
    supporting_facts: Vec<(String, usize)>,
}

#[derive(Debug, Serialize)]
struct RetrievalRow {
    question_id: String,
    question: String,
    qtype: String,
    level: String,
    gold_answer: Value,
    gold_supporting_facts: Vec<(String, usize)>,
    retrievals: Vec<Hit>,
    first_evidence_rank: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Hit {
    rank: usize,
    fact_id: String,
    score: f32,
    content: String,
    paragraph_title: String,
    sentence_idx: usize,
}

struct Args {
    data: PathBuf,
    out: Option<PathBuf>,
    top_k: usize,
    hybrid: bool,
    limit: Option<usize>,
}

fn parse_args() -> Args {
    let data = std::env::var("HOTPOTQA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/hotpotqa/dev_distractor.json"));
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut out = None;
    let mut top_k = 5;
    let mut hybrid = false;
    let mut limit = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--emit-retrievals" if i + 1 < argv.len() => {
                out = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--top-k" if i + 1 < argv.len() => {
                top_k = argv[i + 1].parse().unwrap_or(5);
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
            _ => i += 1,
        }
    }
    Args { data, out, top_k, hybrid, limit }
}

fn build_store_for_question(q: &Question, hybrid: bool) -> Result<(tempfile::TempDir, WikiGraph, HashMap<wg_core::types::FactId, (String, usize)>), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = dir.path().join("hotpot.redb");
    let mut config = Config::default();
    config.store.path = path.to_string_lossy().into_owned();
    if hybrid {
        config.search.semantic_index = "hybrid".into();
        config.model.provider = "model2vec".into();
    }
    let wiki = WikiGraph::open(&path, config).map_err(|e| e.to_string())?;

    let mut inputs = Vec::new();
    let mut sentence_lookup: Vec<(String, usize)> = Vec::new();
    for (title, sentences) in &q.context {
        let entity_id = match wiki.entity_add(EntityInput {
            name: title.clone(),
            entity_type: Some(EntityType::Custom("paragraph".into())),
            ..Default::default()
        }) {
            Ok(id) => id,
            Err(_) => wiki.resolve_entity(title).map_err(|e| e.to_string())?,
        };
        for (sent_idx, sent) in sentences.iter().enumerate() {
            inputs.push(FactInput {
                content: sent.clone(),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![entity_id]),
                tags: Some(vec![format!("title:{title}"), format!("sent_idx:{sent_idx}")]),
                source: Some(title.clone()),
                source_confidence: None,
                observed_at: None,
            });
            sentence_lookup.push((title.clone(), sent_idx));
        }
    }
    let fact_ids = wiki.fact_add_many(inputs).map_err(|e| e.to_string())?;
    let mut id_to_meta = HashMap::new();
    for (fid, meta) in fact_ids.into_iter().zip(sentence_lookup.into_iter()) {
        id_to_meta.insert(fid, meta);
    }
    Ok((dir, wiki, id_to_meta))
}

fn main() -> ExitCode {
    let args = parse_args();
    let started = Instant::now();
    let raw = std::fs::read_to_string(&args.data).unwrap_or_else(|e| {
        eprintln!("error: read {:?}: {e}", args.data);
        std::process::exit(2);
    });
    let mut questions: Vec<Question> = serde_json::from_str(&raw).expect("parse hotpot");
    if let Some(n) = args.limit {
        questions.truncate(n);
    }
    println!("HotpotQA: {} questions, top_k={}, hybrid={}", questions.len(), args.top_k, args.hybrid);

    let mut writer: Option<std::io::BufWriter<std::fs::File>> = args.out.as_ref().map(|p| {
        let f = std::fs::File::create(p).expect("create output");
        std::io::BufWriter::new(f)
    });

    let opts = SearchOpts {
        limit: Some(args.top_k.max(10)),
        ..Default::default()
    };

    let mut hits_at_k: HashMap<usize, usize> = HashMap::new();
    for k in [1usize, 3, 5, 10] {
        hits_at_k.insert(k, 0);
    }
    let mut sup_fact_recall_at_5 = 0usize;
    let mut sup_fact_total = 0usize;
    let mut by_type_ok_at_5: HashMap<String, (usize, usize)> = HashMap::new();
    let mut mrr_sum = 0.0_f64;

    for (qid, q) in questions.iter().enumerate() {
        let (_dir, wiki, id_to_meta) = match build_store_for_question(q, args.hybrid) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  ! ingest fail q={qid}: {e}");
                continue;
            }
        };
        let results = wiki
            .search(&q.question, opts.clone())
            .unwrap_or_else(|e| {
                eprintln!("  ! search fail q={qid}: {e}");
                Vec::new()
            });

        let gold_set: std::collections::HashSet<(String, usize)> = q.supporting_facts.iter().cloned().collect();
        let mut first_rank: Option<usize> = None;
        let mut hits = Vec::new();
        let top5_set: std::collections::HashSet<(String, usize)> = results
            .iter()
            .take(5)
            .filter_map(|r| id_to_meta.get(&r.fact_id).cloned())
            .collect();
        let recall_in_top5 = q.supporting_facts.iter().filter(|sf| top5_set.contains(*sf)).count();
        sup_fact_recall_at_5 += recall_in_top5;
        sup_fact_total += q.supporting_facts.len();

        for (rank, r) in results.iter().enumerate() {
            let rank = rank + 1;
            let (title, sent_idx) = id_to_meta
                .get(&r.fact_id)
                .cloned()
                .unwrap_or_else(|| ("?".into(), 0));
            let is_evidence = gold_set.contains(&(title.clone(), sent_idx));
            if is_evidence && first_rank.is_none() {
                first_rank = Some(rank);
            }
            if rank <= args.top_k {
                hits.push(Hit {
                    rank,
                    fact_id: r.fact_id.to_string(),
                    score: r.score,
                    content: r.content.clone(),
                    paragraph_title: title,
                    sentence_idx: sent_idx,
                });
            }
        }
        for k in [1usize, 3, 5, 10] {
            if let Some(rr) = first_rank {
                if rr <= k { *hits_at_k.entry(k).or_insert(0) += 1; }
            }
        }
        if let Some(rr) = first_rank {
            mrr_sum += 1.0 / rr as f64;
        }
        let entry = by_type_ok_at_5.entry(q.qtype.clone()).or_insert((0, 0));
        entry.1 += 1;
        if first_rank.map(|r| r <= 5).unwrap_or(false) { entry.0 += 1; }

        if let Some(w) = writer.as_mut() {
            let row = RetrievalRow {
                question_id: q.id.clone(),
                question: q.question.clone(),
                qtype: q.qtype.clone(),
                level: q.level.clone(),
                gold_answer: q.answer.clone(),
                gold_supporting_facts: q.supporting_facts.clone(),
                retrievals: hits,
                first_evidence_rank: first_rank,
            };
            use std::io::Write;
            writeln!(w, "{}", serde_json::to_string(&row).unwrap()).unwrap();
        }
        if (qid + 1) % 500 == 0 {
            eprintln!("    [{:>4}/{}] elapsed {:.1?}", qid + 1, questions.len(), started.elapsed());
        }
    }
    if let Some(w) = writer.as_mut() { use std::io::Write; w.flush().unwrap(); }

    let n = questions.len() as f64;
    println!();
    println!("Result: n={}, top_k={}, hybrid={}", questions.len(), args.top_k, args.hybrid);
    for k in [1, 3, 5, 10] {
        let h = *hits_at_k.get(&k).unwrap_or(&0);
        println!("  R@{k} (any sup-fact): {:.3} ({}/{})", h as f64 / n, h, questions.len());
    }
    println!("  MRR: {:.3}", mrr_sum / n);
    println!("  Sentence-level Sup-Fact recall @ 5: {:.3} ({}/{})",
             sup_fact_recall_at_5 as f64 / sup_fact_total.max(1) as f64,
             sup_fact_recall_at_5, sup_fact_total);
    println!();
    println!("By question_type (R@5 first sup-fact):");
    for (qt, (ok, total)) in by_type_ok_at_5.iter() {
        println!("  {qt:<14} {ok}/{total} ({:.3})", *ok as f64 / *total as f64);
    }
    println!("  wall: {:.2?}", started.elapsed());
    ExitCode::SUCCESS
}
