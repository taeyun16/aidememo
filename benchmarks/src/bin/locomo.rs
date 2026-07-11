//! LoCoMo (Long Conversational Memory) baseline for `aidememo`.
//!
//! Loads `locomo10.json` (snap-research/locomo, ICLR 2024) — 10 long
//! synthetic conversations between speaker_a and speaker_b, each
//! spanning 19 dated sessions / ~18 turns / ~9k tokens. Each
//! conversation comes with ~199 QA pairs whose `evidence` field
//! cites turns by `Dn:k` (Dialog n turn k).
//!
//! Per-conversation fresh aidememo store mirrors LongMemEval. Each turn
//! becomes one fact:
//!   * content      = "{speaker}: {text}"
//!   * entity_ids   = [speaker_entity, session_entity]
//!   * tags         = ["dia_id:Dn:k", "session:Dn"]
//!   * observed_at  = parsed `session_n_date_time` (chrono::NaiveDateTime
//!     of "1:56 pm on 8 May, 2023" → epoch ms UTC)
//!
//! Retrieval grading checks whether any gold `Dn:k` appears in the
//! top-K results' tag list. Reader/judge stage reuses the omega-style
//! Python harness on the emitted JSONL.
//!
//! Usage:
//!
//! ```bash
//! LOCOMO=/tmp/locomo/locomo10.json \
//!   cargo run --release -p aidememo-benchmarks --bin locomo -- \
//!     --hybrid --top-k 10 \
//!     --emit-retrievals /tmp/aidememo_locomo.jsonl
//! ```

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use aidememo_core::types::{FactInput, FactType};
use aidememo_core::{AideMemo, Config, EntityInput, EntityType, SearchOpts};
use serde::{Deserialize, Serialize};
use serde_json::Value;

type TurnMeta = (String, String, String);
type ConversationStore = (
    tempfile::TempDir,
    AideMemo,
    HashMap<aidememo_core::types::FactId, TurnMeta>,
);

#[derive(Debug, Deserialize)]
struct Turn {
    speaker: String,
    dia_id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct QA {
    question: String,
    /// Most QA pairs carry `answer`. Category-5 (open-domain
    /// adversarial) instead carries `adversarial_answer`. We accept
    /// either; the reader/judge stage decides what to grade against.
    #[serde(default)]
    answer: Option<Value>,
    #[serde(default)]
    adversarial_answer: Option<Value>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    category: i32,
}

impl QA {
    fn gold(&self) -> Value {
        self.answer
            .clone()
            .or_else(|| self.adversarial_answer.clone())
            .unwrap_or(Value::Null)
    }
}

#[derive(Debug, Deserialize)]
struct Conversation {
    sample_id: String,
    conversation: Value, // Mixed: session_n (Vec<Turn>) + session_n_date_time (String) + speaker_a/b
    qa: Vec<QA>,
}

#[derive(Debug, Serialize)]
struct RetrievalRow {
    sample_id: String,
    qa_index: usize,
    question: String,
    category: i32,
    gold_answer: Value,
    gold_evidence: Vec<String>,
    retrievals: Vec<Hit>,
    first_evidence_rank: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Hit {
    rank: usize,
    fact_id: String,
    score: f32,
    content: String,
    dia_id: String,
    session: String,
    speaker: String,
}

struct Args {
    data: PathBuf,
    out: Option<PathBuf>,
    top_k: usize,
    hybrid: bool,
    limit: Option<usize>,
}

fn parse_args() -> Args {
    let data = std::env::var("LOCOMO")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/locomo/locomo10.json"));
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut out = None;
    let mut top_k = 10;
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
            _ => i += 1,
        }
    }
    Args {
        data,
        out,
        top_k,
        hybrid,
        limit,
    }
}

/// LoCoMo session timestamps look like `1:56 pm on 8 May, 2023`. Parse
/// to epoch ms UTC (the dataset doesn't carry a timezone, so we treat
/// the wall clock as UTC — fine for relative ordering / decay weights).
fn parse_locomo_dt(s: &str) -> Option<u64> {
    let dt = chrono::NaiveDateTime::parse_from_str(s.trim(), "%I:%M %p on %d %B, %Y").ok()?;
    Some(
        chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc)
            .timestamp_millis() as u64,
    )
}

fn build_store_for_conv(conv: &Conversation, hybrid: bool) -> Result<ConversationStore, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = dir.path().join("locomo.redb");
    let mut config = Config::default();
    config.store.path = path.to_string_lossy().into_owned();
    if hybrid {
        config.search.semantic_index = "hybrid".into();
        config.model.provider = "model2vec".into();
    }
    let wiki = AideMemo::open(&path, config).map_err(|e| e.to_string())?;

    // Resolve speakers up front — every fact carries one speaker entity.
    let speaker_a = conv
        .conversation
        .get("speaker_a")
        .and_then(|v| v.as_str())
        .unwrap_or("speaker_a")
        .to_string();
    let speaker_b = conv
        .conversation
        .get("speaker_b")
        .and_then(|v| v.as_str())
        .unwrap_or("speaker_b")
        .to_string();
    let mut speaker_id: HashMap<String, aidememo_core::types::EntityId> = HashMap::new();
    for name in [&speaker_a, &speaker_b] {
        let id = match wiki.entity_add(EntityInput {
            name: name.clone(),
            entity_type: Some(EntityType::Custom("speaker".into())),
            ..Default::default()
        }) {
            Ok(id) => id,
            Err(_) => wiki.resolve_entity(name).map_err(|e| e.to_string())?,
        };
        speaker_id.insert(name.clone(), id);
    }

    let mut inputs: Vec<FactInput> = Vec::new();
    let mut id_to_meta_seq: Vec<(String, String, String)> = Vec::new(); // (dia_id, session, speaker)
    for n in 1..=35usize {
        let session_key = format!("session_{n}");
        let dt_key = format!("session_{n}_date_time");
        let Some(turns) = conv
            .conversation
            .get(&session_key)
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        let observed_at = conv
            .conversation
            .get(&dt_key)
            .and_then(|v| v.as_str())
            .and_then(parse_locomo_dt);
        let session_label = format!("D{n}");
        // Session-as-entity so retrieval can group / traverse.
        let session_entity_id = match wiki.entity_add(EntityInput {
            name: session_label.clone(),
            entity_type: Some(EntityType::Custom("session".into())),
            ..Default::default()
        }) {
            Ok(id) => id,
            Err(_) => wiki
                .resolve_entity(&session_label)
                .map_err(|e| e.to_string())?,
        };
        for raw in turns {
            let turn: Turn = match serde_json::from_value(raw.clone()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let speaker_eid = speaker_id
                .get(&turn.speaker)
                .copied()
                .unwrap_or(session_entity_id);
            inputs.push(FactInput {
                content: format!("{}: {}", turn.speaker, turn.text),
                fact_type: Some(FactType::Note),
                entity_ids: Some(vec![speaker_eid, session_entity_id]),
                tags: Some(vec![
                    format!("dia_id:{}", turn.dia_id),
                    format!("session:{}", session_label),
                ]),
                source: Some("raw-chat".into()),
                source_id: None,
                actor_id: None,
                source_confidence: None,
                observed_at,
            });
            id_to_meta_seq.push((
                turn.dia_id.clone(),
                session_label.clone(),
                turn.speaker.clone(),
            ));
        }
    }
    let fact_ids = wiki.fact_add_many(inputs).map_err(|e| e.to_string())?;
    let mut id_to_meta: HashMap<aidememo_core::types::FactId, (String, String, String)> =
        HashMap::new();
    for (fid, meta) in fact_ids.into_iter().zip(id_to_meta_seq) {
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
    let convs: Vec<Conversation> = serde_json::from_str(&raw).expect("parse locomo");
    println!(
        "LoCoMo: {} conversations, {} QA total",
        convs.len(),
        convs.iter().map(|c| c.qa.len()).sum::<usize>()
    );

    let mut writer: Option<std::io::BufWriter<std::fs::File>> = args.out.as_ref().map(|p| {
        let f = std::fs::File::create(p).expect("create output");
        std::io::BufWriter::new(f)
    });

    let opts = SearchOpts {
        limit: Some(args.top_k.max(30)),
        ..Default::default()
    };

    let mut hits_at_k: HashMap<usize, usize> = HashMap::new();
    for k in [1usize, 5, 10, 30] {
        hits_at_k.insert(k, 0);
    }
    let mut total_with_evidence = 0usize;
    let mut mrr_sum = 0.0_f64;
    let mut by_cat_ok_at_5: HashMap<i32, (usize, usize)> = HashMap::new();
    let mut q_emitted = 0usize;

    for conv in &convs {
        let (_dir, wiki, id_to_meta) = match build_store_for_conv(conv, args.hybrid) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  ! ingest fail conv={}: {e}", conv.sample_id);
                continue;
            }
        };

        for (qi, qa) in conv.qa.iter().enumerate() {
            if let Some(n) = args.limit {
                if q_emitted >= n {
                    break;
                }
            }
            let results = wiki.search(&qa.question, opts.clone()).unwrap_or_else(|e| {
                eprintln!("  ! search fail conv={} q={qi}: {e}", conv.sample_id);
                Vec::new()
            });
            let gold: HashSet<String> = qa.evidence.iter().cloned().collect();
            let mut first_rank: Option<usize> = None;
            let mut hits = Vec::new();
            for (rank, r) in results.iter().enumerate() {
                let rank = rank + 1;
                let (dia, sess, speaker) = id_to_meta
                    .get(&r.fact_id)
                    .cloned()
                    .unwrap_or_else(|| ("?".into(), "?".into(), "?".into()));
                let is_evidence = gold.contains(&dia);
                if is_evidence && first_rank.is_none() {
                    first_rank = Some(rank);
                }
                if rank <= args.top_k {
                    hits.push(Hit {
                        rank,
                        fact_id: r.fact_id.to_string(),
                        score: r.score,
                        content: r.content.clone(),
                        dia_id: dia,
                        session: sess,
                        speaker,
                    });
                }
            }
            if !gold.is_empty() {
                total_with_evidence += 1;
                for k in [1usize, 5, 10, 30] {
                    if let Some(rr) = first_rank {
                        if rr <= k {
                            *hits_at_k.entry(k).or_insert(0) += 1;
                        }
                    }
                }
                if let Some(rr) = first_rank {
                    mrr_sum += 1.0 / rr as f64;
                }
                let entry = by_cat_ok_at_5.entry(qa.category).or_insert((0, 0));
                entry.1 += 1;
                if first_rank.map(|r| r <= 5).unwrap_or(false) {
                    entry.0 += 1;
                }
            }

            if let Some(w) = writer.as_mut() {
                let row = RetrievalRow {
                    sample_id: conv.sample_id.clone(),
                    qa_index: qi,
                    question: qa.question.clone(),
                    category: qa.category,
                    gold_answer: qa.gold(),
                    gold_evidence: qa.evidence.clone(),
                    retrievals: hits,
                    first_evidence_rank: first_rank,
                };
                use std::io::Write;
                writeln!(w, "{}", serde_json::to_string(&row).unwrap()).unwrap();
            }
            q_emitted += 1;
        }
        eprintln!(
            "    conv {} done — {:.1?} elapsed",
            conv.sample_id,
            started.elapsed()
        );
        if args.limit.map(|n| q_emitted >= n).unwrap_or(false) {
            break;
        }
    }
    if let Some(w) = writer.as_mut() {
        use std::io::Write;
        w.flush().unwrap();
    }

    let denom = total_with_evidence.max(1) as f64;
    println!();
    println!(
        "Result: {} questions emitted ({} with evidence), top_k={}, hybrid={}",
        q_emitted, total_with_evidence, args.top_k, args.hybrid
    );
    for k in [1, 5, 10, 30] {
        let h = *hits_at_k.get(&k).unwrap_or(&0);
        println!(
            "  R@{k}: {:.3} ({}/{})",
            h as f64 / denom,
            h,
            total_with_evidence
        );
    }
    println!("  MRR: {:.3}", mrr_sum / denom);
    println!("  wall: {:.2?}", started.elapsed());
    println!();
    println!("By question category (R@5 first-evidence):");
    let mut cats: Vec<i32> = by_cat_ok_at_5.keys().copied().collect();
    cats.sort();
    for c in cats {
        let (ok, total) = by_cat_ok_at_5[&c];
        println!(
            "  cat {c:<2} {ok}/{total} ({:.3})",
            ok as f64 / total as f64
        );
    }
    ExitCode::SUCCESS
}
