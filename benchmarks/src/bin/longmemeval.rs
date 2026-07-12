//! LongMemEval-S retrieval baseline for `aidememo`.
//!
//! Loads the LongMemEval-S JSON file (publicly available from the
//! `xiaowu0162/longmemeval` HF dataset — see `docs/MEASUREMENTS.md`
//! for the curl/hf-hub commands), and for every question:
//!
//! 1. Spins up a fresh, isolated aidememo store under a tempdir so haystacks
//!    from one question can't leak into another.
//! 2. Ingests every chat turn from `haystack_sessions` as a fact —
//!    one fact per turn, tagged with `session:<haystack_session_id>`
//!    so we can later identify whether retrieved hits sit inside an
//!    `answer_session_ids` evidence session.
//! 3. Runs `aidememo_search` (BM25-only via `bm25_only=true` to keep the
//!    baseline portable; semantic adds noise when the dataset is in
//!    English and the default model is multilingual potion-128M).
//! 4. Checks the top-K hits against `answer_session_ids` and records
//!    rank of the first hit, hit-at-1, hit-at-5, hit-at-10.
//!
//! Reports R@1, R@5, R@10, MRR. This is the **retrieval-only** axis
//! of LongMemEval — the official end-to-end metric needs an LLM to
//! generate an answer from the retrieved context. Retrieval recall
//! is the part `aidememo` directly affects, and high recall is necessary
//! for high answer correctness, so this number is a useful proxy and
//! a fair head-to-head against other memory backends evaluated on
//! the same axis.
//!
//! Usage:
//!
//! ```bash
//! # Tiny fixture (committed) — sanity-check the harness without
//! # downloading the 277 MB cleaned dataset.
//! cargo run --release -p aidememo-benchmarks --bin longmemeval -- \
//!     --data benchmarks/fixtures/longmemeval_tiny.json
//!
//! # Full dataset.
//! LONGMEMEVAL_DATA=/tmp/longmemeval_s_cleaned.json \
//!   cargo run --release -p aidememo-benchmarks --bin longmemeval -- --limit 50
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use aidememo_core::types::{FactInput, FactType, GacOpts, VectorRebuildOpts};
use aidememo_core::{AideMemo, Config, EntityInput, EntityType, SearchOpts};
use serde::Deserialize;
use serde_json::Value;

type TurnTypeMap = std::collections::HashMap<usize, String>;
type SessionTypeMap = std::collections::HashMap<usize, TurnTypeMap>;
type ClassifyMap = std::collections::HashMap<String, SessionTypeMap>;

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
    /// finding in `docs/MEASUREMENTS.md`. Defaults off.
    temporal: bool,
    /// Use hybrid search (BM25 + semantic via in-process model2vec)
    /// instead of BM25-only. Adds the embedding-model load on the
    /// first call per AideMemo (~700-900 ms cold) — in this harness
    /// that's once per question.
    hybrid: bool,
    /// Override the embedding model when --hybrid is on. Accepted:
    /// `model2vec` (default — multilingual potion-128M, 28 MB) or
    /// any fastembed model name (e.g. `bge-small-en-v1.5`,
    /// `multilingual-e5-base`). The fastembed family is English-tuned;
    /// LongMemEval is English; expect a measurable lift.
    embed_model: Option<String>,
    /// Cross-encoder reranker (fastembed). E.g. `bge-reranker-base`
    /// (en+zh, default), `bge-reranker-v2-m3` (multilingual),
    /// `jina-reranker-v2-base-multilingual`. When set, the harness
    /// turns on `rerank.provider = "fastembed"` and reorders the
    /// top-K hybrid result via the cross-encoder. Only meaningful
    /// when `--hybrid` is also on (rerank doesn't apply to BM25-only
    /// searches today).
    reranker: Option<String>,
    /// Override `search.bm25_weight` in RRF fusion. With BM25-strong
    /// corpora (knowledge-update / single-session-user style exact
    /// matches), pushing BM25 above semantic preserves R@1 wins that
    /// pure-semantic embeddings dilute. Defaults to the config value
    /// (1.0).
    bm25_weight: Option<f32>,
    /// Override `search.semantic_weight` in RRF fusion. Lower this
    /// (e.g. 0.5) to back off semantic when it's pulling the wrong
    /// candidates into top-K. Defaults to the config value (1.0).
    semantic_weight: Option<f32>,
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
    /// Take N questions from EACH question_type bucket (instead of
    /// the first N overall, which `--limit` does). Gives every
    /// category equal voice in the aggregate score so a small
    /// sample is representative — critical when --llm-extract
    /// makes full-500 runs impractical. Mutually exclusive with
    /// --only-type.
    balanced_sample: Option<usize>,
    /// If set, append one JSON line per question to this file with
    /// the top-K retrieval records — feed this into the official
    /// LongMemEval evaluator (or the internal `e2e` script) to do
    /// LLM-graded answer correctness.
    emit_retrievals: Option<PathBuf>,
    /// LLM-aided ingestion. When set, each haystack session's turns
    /// are concatenated and fed to `aidememo_extract --llm` (uses
    /// extract.provider — defaults to gpt-4o-mini via OpenAI). The
    /// classified facts (decision / pattern / preference / lesson /
    /// error / …) are added ALONGSIDE the raw turns so retrieval
    /// has both the verbatim evidence AND the distilled signal —
    /// new fact_type weights + decay-exempt rules then fire. This
    /// is the OMEGA / Mastra pattern: ingest-time LLM normalisation
    /// → retrieval gets a richer signal than raw chat dump.
    llm_extract: bool,
    /// Cap candidates per session (default 10). Lower values keep
    /// noise low for long sessions; higher values let the LLM
    /// surface more nuance per session at the cost of more facts
    /// in the BM25 index.
    llm_extract_per_session: usize,
    /// Override the LLM extract provider's base URL. Default
    /// `https://api.openai.com/v1`. Use to swap in MiniMax
    /// (`https://api.minimax.io/v1`), Ollama
    /// (`http://localhost:11434/v1`), or any OpenAI-compatible
    /// endpoint without touching aidememo config.
    llm_extract_base_url: Option<String>,
    /// Env var holding the LLM extract API key. Default
    /// `OPENAI_API_KEY`; pair with `--llm-extract-base-url` for
    /// alt providers (e.g. `MINIMAX_API_KEY`). Empty string ⇒
    /// no Authorization header (Ollama / vLLM local).
    llm_extract_api_key_env: Option<String>,
    /// Override the LLM extract model. Default `gpt-4o-mini`.
    /// Use `MiniMax-M2.7-highspeed`, `qwen2.5:7b`, etc.
    llm_extract_model: Option<String>,
    /// Replace raw turn ingest with classified facts only. Without
    /// this, --llm-extract adds classified facts ALONGSIDE the raw
    /// turns (augment), which the 2026-05-01 measurement showed
    /// confuses readers (-8.3pt mini E2E on 60q balanced — readers
    /// can't tell raw evidence from LLM-distilled fact). With it,
    /// raw turns are dropped and only classified facts represent
    /// each session in the store. Mirrors OMEGA's replace-style
    /// ingestion. Risk: low-quality LLM extracts can drop the
    /// gold-evidence content entirely; pair with a higher-quality
    /// `--llm-extract-model` (gpt-4.1, Claude Opus) when using.
    llm_extract_replace: bool,
    /// OMEGA-style session-level ingest: concatenate every turn in a
    /// session into a single fact ("user: ...\nassistant: ...\n…")
    /// instead of one fact per turn. Mirrors OMEGA's
    /// `store.store(content=format_session_text(turns))` exactly. Each
    /// fact carries `observed_at = haystack_date` so chronological
    /// sort + recency boost work session-level. Drastically reduces
    /// fact count (~40 per question instead of ~1500) and lets the
    /// adaptive max_res cap surface whole sessions, not turn fragments.
    session_level_ingest: bool,
    /// Hybrid ingest: store BOTH turn-level facts AND session-level
    /// facts side-by-side. The reader retrieves from a unified pool;
    /// per-question the adaptive filter picks whatever ranks best —
    /// turn-level for position-sensitive carries (SS-pref / SS-user /
    /// temporal), session-level for cross-snippet aggregation
    /// (KU / multi-session / SS-asst). Trades off store size for
    /// granularity choice. Mutually exclusive with
    /// `--session-level-ingest` (this flag implies session-level
    /// inclusion).
    hybrid_ingest: bool,
    /// Path to a JSON file emitted by
    /// `scripts/longmemeval_classify_sessions.py` that maps each
    /// turn to a `fact_type` label. Layout: `{question_id: {sess_idx:
    /// {turn_idx: type}}}`. When provided, ingest applies the
    /// classified type instead of the default Note. Mirrors the
    /// "self-extraction" pattern aidememo ships in production (the calling
    /// agent does the classification; aidememo stores it). Pure label-only
    /// pass — content is unchanged, so the abstraction-mismatch
    /// failure mode of `--llm-extract` (which rewrites facts) does
    /// not apply.
    classify_from: Option<PathBuf>,
    /// Apply GAC consolidation between ingest and search. Runs
    /// `consolidate_gac { dry_run: false, use_cold_tier: false }`
    /// then `vector_index_rebuild_with_opts { current_only: true }`.
    /// Measures whether geometry-aware compression preserves
    /// retrieval quality on LongMemEval (where representatives are
    /// expected to carry the recall their cluster losers held).
    gac: bool,
    /// θ for `--gac` (retrieval half-angle). Defaults to 0.85.
    gac_theta: f32,
    /// Fact types to protect from GAC clustering. Parsed from
    /// CSV via `--gac-protect preference,lesson,error`. Default
    /// empty.
    gac_protect_types: Vec<FactType>,
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
    let mut hybrid = false;
    let mut embed_model: Option<String> = None;
    let mut reranker: Option<String> = None;
    let mut bm25_weight: Option<f32> = None;
    let mut semantic_weight: Option<f32> = None;
    let mut time_decay_days: Option<f64> = None;
    let mut only_type: Option<String> = None;
    let mut emit_retrievals: Option<PathBuf> = None;
    let mut llm_extract = false;
    let mut llm_extract_per_session: usize = 10;
    let mut llm_extract_base_url: Option<String> = None;
    let mut llm_extract_api_key_env: Option<String> = None;
    let mut llm_extract_model: Option<String> = None;
    let mut llm_extract_replace = false;
    let mut balanced_sample: Option<usize> = None;
    let mut session_level_ingest = false;
    let mut hybrid_ingest = false;
    let mut classify_from: Option<PathBuf> = None;
    let mut gac = false;
    let mut gac_theta: f32 = 0.85;
    let mut gac_protect_types: Vec<FactType> = Vec::new();

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
            "--hybrid" => {
                hybrid = true;
                i += 1;
            }
            "--embed-model" if i + 1 < argv.len() => {
                embed_model = Some(argv[i + 1].clone());
                i += 2;
            }
            "--reranker" if i + 1 < argv.len() => {
                reranker = Some(argv[i + 1].clone());
                i += 2;
            }
            "--bm25-weight" if i + 1 < argv.len() => {
                bm25_weight = argv[i + 1].parse().ok();
                i += 2;
            }
            "--semantic-weight" if i + 1 < argv.len() => {
                semantic_weight = argv[i + 1].parse().ok();
                i += 2;
            }
            "--time-decay-days" if i + 1 < argv.len() => {
                time_decay_days = argv[i + 1].parse().ok();
                i += 2;
            }
            "--only-type" if i + 1 < argv.len() => {
                only_type = Some(argv[i + 1].clone());
                i += 2;
            }
            "--emit-retrievals" if i + 1 < argv.len() => {
                emit_retrievals = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--llm-extract" => {
                llm_extract = true;
                i += 1;
            }
            "--llm-extract-per-session" if i + 1 < argv.len() => {
                llm_extract_per_session = argv[i + 1].parse().unwrap_or(10);
                i += 2;
            }
            "--llm-extract-base-url" if i + 1 < argv.len() => {
                llm_extract_base_url = Some(argv[i + 1].clone());
                i += 2;
            }
            "--llm-extract-api-key-env" if i + 1 < argv.len() => {
                llm_extract_api_key_env = Some(argv[i + 1].clone());
                i += 2;
            }
            "--llm-extract-model" if i + 1 < argv.len() => {
                llm_extract_model = Some(argv[i + 1].clone());
                i += 2;
            }
            "--llm-extract-replace" => {
                llm_extract_replace = true;
                i += 1;
            }
            "--balanced-sample" if i + 1 < argv.len() => {
                balanced_sample = argv[i + 1].parse().ok();
                i += 2;
            }
            "--session-level-ingest" => {
                session_level_ingest = true;
                i += 1;
            }
            "--hybrid-ingest" => {
                hybrid_ingest = true;
                i += 1;
            }
            "--classify-from" if i + 1 < argv.len() => {
                classify_from = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--gac" => {
                gac = true;
                i += 1;
            }
            "--gac-theta" if i + 1 < argv.len() => {
                gac_theta = argv[i + 1].parse().unwrap_or(0.85);
                i += 2;
            }
            "--gac-protect" if i + 1 < argv.len() => {
                gac_protect_types = argv[i + 1]
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(FactType::parse)
                    .collect();
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
        hybrid,
        embed_model,
        reranker,
        bm25_weight,
        semantic_weight,
        time_decay_days,
        only_type,
        emit_retrievals,
        llm_extract,
        llm_extract_per_session,
        llm_extract_base_url,
        llm_extract_api_key_env,
        llm_extract_model,
        llm_extract_replace,
        balanced_sample,
        session_level_ingest,
        hybrid_ingest,
        classify_from,
        gac,
        gac_theta,
        gac_protect_types,
    }
}

/// Bundle of retrieval-side knobs the harness flips per measurement.
/// Keeping them in one struct rather than as a long argument list
/// makes call sites readable and keeps clippy happy at the
/// 7-argument bar.
struct BuildOpts<'a> {
    temporal: bool,
    stamp_observed_at: bool,
    hybrid: bool,
    embed_model: Option<&'a str>,
    reranker: Option<&'a str>,
    bm25_weight: Option<f32>,
    semantic_weight: Option<f32>,
    decay_days_for_hybrid: Option<f64>,
    llm_extract: bool,
    llm_extract_per_session: usize,
    llm_extract_base_url: Option<&'a str>,
    llm_extract_api_key_env: Option<&'a str>,
    llm_extract_model: Option<&'a str>,
    llm_extract_replace: bool,
    /// One fact per session (concat all turns) instead of one per turn.
    session_level_ingest: bool,
    /// Both turn-level facts AND session-level facts side-by-side.
    /// Mutually exclusive with `session_level_ingest` semantically
    /// (this flag wins when both are set).
    hybrid_ingest: bool,
    /// Optional per-turn fact_type classification, keyed by
    /// (sess_idx, turn_idx). Loaded from `--classify-from FILE` and
    /// scoped to the current question by main(). When None, every
    /// turn-level fact ingests as `Note` (the legacy default).
    classify_for_question:
        Option<&'a std::collections::HashMap<usize, std::collections::HashMap<usize, String>>>,
}

fn build_store_for_question(
    q: &Question,
    opts: BuildOpts<'_>,
) -> Result<(tempfile::TempDir, AideMemo), String> {
    let BuildOpts {
        temporal,
        stamp_observed_at,
        hybrid,
        embed_model,
        reranker,
        bm25_weight,
        semantic_weight,
        decay_days_for_hybrid,
        llm_extract,
        llm_extract_per_session,
        llm_extract_base_url,
        llm_extract_api_key_env,
        llm_extract_model,
        llm_extract_replace,
        session_level_ingest,
        hybrid_ingest,
        classify_for_question,
    } = opts;
    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let mut config = Config::default();
    config.store.path = dir.path().join("store").to_string_lossy().into_owned();
    if llm_extract {
        // Wire aidememo-core's LLM extractor. Defaults aim at OpenAI
        // (gpt-4o-mini) to match the published baselines, but every
        // knob is overridable so the harness can exercise the same
        // path against MiniMax / Ollama / Kimi / OpenRouter without
        // touching aidememo config.
        config.extract.provider = "openai".into();
        config.extract.model = llm_extract_model
            .map(str::to_string)
            .or_else(|| std::env::var("AIDEMEMO_EXTRACT_MODEL").ok())
            .unwrap_or_else(|| "gpt-4o-mini".into());
        if let Some(url) = llm_extract_base_url {
            config.extract.endpoint = url.to_string();
        }
        if let Some(env_var) = llm_extract_api_key_env {
            config.extract.api_key_env = env_var.to_string();
        }
        config.extract.max_candidates = llm_extract_per_session;
    }
    // BM25-only by default; --hybrid flips to the in-process semantic
    // path (model2vec embeddings + HNSW) at the cost of a one-time
    // model load per AideMemo instance.
    if !hybrid {
        config.search.semantic_index = "bm25".into();
    } else if let Some(m) = embed_model {
        // --embed-model overrides the model2vec default with any
        // fastembed family member (bge-small-en-v1.5 by default for
        // English benchmarks). The fastembed feature must be on the
        // aidememo-core build (it is via benchmarks/Cargo.toml).
        config.model.provider = "fastembed".into();
        config.model.name = m.to_string();
    }
    if let Some(r) = reranker {
        // Cross-encoder rerank wired via fastembed (in-process ONNX,
        // no TEI server). Top-K rerank window matches the harness
        // top_k so every returned slot is candidate for promotion.
        config.rerank.provider = "fastembed".into();
        config.rerank.model = r.to_string();
        config.rerank.top_k = 20;
    }
    if let Some(w) = bm25_weight {
        config.search.bm25_weight = w;
    }
    if let Some(w) = semantic_weight {
        config.search.semantic_weight = w;
    }
    // Decay disabled by default in this harness — aidememo-core's
    // SearchConfig::default sets time_decay_tau_ms=90 days, but the
    // LongMemEval-S dataset has dating noise (74 evidence sessions
    // future-dated past the question; see docs/MEASUREMENTS.md)
    // and per-category measurements showed bge embeddings are better
    // off WITHOUT decay (knowledge-update R@1 0.692 → 0.987 by
    // turning decay off). The `--time-decay-days` flag opts the user
    // back in: in hybrid mode it routes through aidememo-core's
    // in-pipeline decay (so it composes correctly with the cross-
    // encoder reranker); in bm25_only mode the harness still applies
    // a post-hoc multiplier (aidememo-core's BM25 path doesn't apply decay).
    config.search.time_decay_tau_ms = 0;
    if hybrid && let Some(days) = decay_days_for_hybrid {
        config.search.time_decay_tau_ms = (days * 86_400_000.0) as u64;
    }
    let store_path = PathBuf::from(&config.store.path);
    let wiki = AideMemo::open(&store_path, config).map_err(|e| e.to_string())?;

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
    // Skip when --llm-extract-replace is set: the LLM-extracted facts
    // below take the place of raw turns (OMEGA-style replace mode,
    // no augment). Reader sees only distilled signal, not raw chat.
    if !llm_extract_replace {
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
            // Three modes: hybrid (both), session-only, turn-only (default).
            let want_session = session_level_ingest || hybrid_ingest;
            let want_turns = hybrid_ingest || !session_level_ingest;
            if want_session {
                // OMEGA-style: one fact per session, all turns concatenated.
                // Reader retrieves whole conversational blocks for cross-snippet
                // aggregation tasks (KU / multi-session / SS-asst).
                let content: String = session
                    .iter()
                    .map(|t| format!("{}: {}", t.role, t.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                inputs.push(FactInput {
                    content,
                    fact_type: Some(FactType::Note),
                    entity_ids: entity_id.map(|e| vec![e]),
                    tags: Some(vec![format!(
                        "session:{}",
                        q.haystack_session_ids[sess_idx]
                    )]),
                    source: Some("session".into()),
                    source_id: None,
                    actor_id: None,
                    source_confidence: None,
                    observed_at,
                });
            }
            if want_turns {
                let session_classify = classify_for_question.and_then(|m| m.get(&sess_idx));
                for (turn_idx, turn) in session.iter().enumerate() {
                    let ft = session_classify
                        .and_then(|c| c.get(&turn_idx))
                        .map(|s| FactType::parse(s))
                        .unwrap_or(FactType::Note);
                    inputs.push(FactInput {
                        content: format!("{}: {}", turn.role, turn.content),
                        fact_type: Some(ft),
                        entity_ids: entity_id.map(|e| vec![e]),
                        tags: Some(vec![format!(
                            "session:{}",
                            q.haystack_session_ids[sess_idx]
                        )]),
                        // Layer label for the hybrid-retrieval reader prompt.
                        // Lets the e2e script render '[raw chat]' vs
                        // '[distilled fact]' so the reader knows which
                        // snippet preserves verbatim detail.
                        source: Some("raw-chat".into()),
                        source_id: None,
                        actor_id: None,
                        source_confidence: None,
                        observed_at,
                    });
                }
            }
        }
        if !inputs.is_empty() {
            wiki.fact_add_many(inputs).map_err(|e| e.to_string())?;
        }
    } else {
        // Surface a clear error if the user asked for replace mode
        // without enabling --llm-extract — otherwise the store
        // would end up empty and every search would miss.
        if !llm_extract {
            return Err("--llm-extract-replace requires --llm-extract".into());
        }
    }

    // ── LLM-aided extraction (opt-in) ─────────────────────────────
    // For each session, concatenate its turns and ask the LLM to
    // surface up to `llm_extract_per_session` durable facts with
    // proper fact_type / entity classification. These distilled
    // facts are added ALONGSIDE the raw turns so retrieval has both
    // the verbatim evidence (matches the gold session_id) AND the
    // type-classified signal that fires the new fact_type weights +
    // decay-exempt rules. Mirrors OMEGA's ingest-time LLM
    // normalisation pass.
    //
    // Cost: one chat-completion per haystack session. Skip on any
    // failure (rate limit, transient error) — the raw-turn ingest
    // is already in place, so the question still scores correctly,
    // just without the extra signal.
    if llm_extract {
        // Concurrent extraction: one HTTP call per session in
        // parallel via rayon's global pool (aidememo-core's
        // extract_candidates_llm only reads the store, no write
        // lock — safe for `&wiki` from multiple threads). Sessions
        // with empty turns are filtered upstream so the par_iter
        // closure can stay infallible. Per-session results then
        // collapse into a single fact_add_many serially (write txn
        // is exclusive, but it's <10 ms vs ~3 s per LLM call).
        use rayon::prelude::*;
        let observed_at_per_sess: Vec<Option<u64>> = q
            .haystack_sessions
            .iter()
            .enumerate()
            .map(|(idx, _)| {
                if stamp_observed_at {
                    q.haystack_dates
                        .get(idx)
                        .and_then(|d| parse_question_date(d))
                } else {
                    None
                }
            })
            .collect();

        let session_facts: Vec<Vec<FactInput>> = q
            .haystack_sessions
            .par_iter()
            .enumerate()
            .map(|(sess_idx, session)| -> Vec<FactInput> {
                if session.is_empty() {
                    return Vec::new();
                }
                let session_text = session
                    .iter()
                    .map(|t| format!("{}: {}", t.role, t.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                let candidates =
                    match wiki.extract_candidates_llm(&session_text, llm_extract_per_session) {
                        Ok(c) => c,
                        Err(_) => return Vec::new(),
                    };
                let entity_id = session_eids.get(sess_idx).copied();
                let observed_at = observed_at_per_sess.get(sess_idx).copied().flatten();
                let mut classified: Vec<FactInput> = Vec::with_capacity(candidates.len());
                for c in candidates {
                    if c.confidence < 0.5 {
                        continue;
                    }
                    classified.push(FactInput {
                        content: c.content,
                        fact_type: Some(c.suggested_fact_type),
                        entity_ids: entity_id.map(|e| vec![e]),
                        tags: Some(vec![format!(
                            "session:{}",
                            q.haystack_session_ids[sess_idx]
                        )]),
                        source: Some("llm-extract".into()),
                        source_id: None,
                        actor_id: None,
                        source_confidence: Some(c.confidence),
                        observed_at,
                    });
                }
                classified
            })
            .collect();

        // Serial write: one fact_add_many per session. The redb
        // single-writer lock makes parallel writes a non-starter,
        // but write txns are short (~5-10 ms) so the bottleneck is
        // safely on the network side.
        for facts in session_facts {
            if !facts.is_empty() {
                let _ = wiki.fact_add_many(facts);
            }
        }
    }
    Ok((dir, wiki))
}

/// One retrieval row, intended for the `--emit-retrievals` JSONL.
/// Carries enough for a downstream LLM reader (content + which
/// session it came from) plus the rank/score the harness saw so
/// E2E judging can sanity-check ordering.
///
/// `source` distinguishes the two retrieval layers when
/// `--llm-extract` is on: `"raw-chat"` = verbatim user/assistant
/// turn, `"llm-extract"` = LLM-distilled classified fact. Reader
/// prompts can label hits accordingly so the model knows which
/// snippet preserves verbatim detail vs which is summarised.
#[derive(serde::Serialize)]
struct RetrievalRecord {
    rank: usize,
    fact_id: String,
    content: String,
    score: f32,
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// epoch-ms of the session this fact came from; lets the
    /// downstream Python harness sort retrievals chronologically and
    /// apply OMEGA-style recency boosts (esp. for knowledge-update).
    #[serde(skip_serializing_if = "Option::is_none")]
    referenced_date: Option<u64>,
    /// Layer-1 deterministic structured values pulled out of `content`
    /// at retrieval time (currency / duration / event_date / count).
    /// Empty vec when the fact has no extractable typed slots — gives
    /// the Python aggregation layer a concrete signal to compute
    /// sums/counts/timelines without asking the reader to do
    /// arithmetic. Anchored to `referenced_date` for relative-date
    /// resolution.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    structured: Vec<aidememo_core::extract_structured::StructuredValue>,
    /// Cosine similarity (0.0..1.0) between the question's embedding
    /// and this fact's content embedding, computed via the same
    /// model aidememo uses for hybrid_search. Lets the Python aggregation
    /// layer filter structured values by per-fact semantic
    /// relevance — solves the "non-bike $40 leaks into bike-expense
    /// sum" failure mode where BM25 surfaces topically similar but
    /// off-target facts.
    ///
    /// Skipped when the `semantic` feature is off or the embedder
    /// fails to load. Cheap to compute when hybrid_search already
    /// loaded the model (one extra `embed(text)` per top-K hit, ~1ms
    /// each with model2vec).
    #[serde(skip_serializing_if = "Option::is_none")]
    relevance: Option<f32>,
}

/// Compute cosine similarity between the question's embedding and a
/// fact's content embedding. Caches the question embedding via
/// thread-local since `evaluate` calls this per top-K hit. Returns
/// None when the embedder fails to load (no semantic feature, or
/// model file missing) — the field then serialises as null and the
/// Python harness falls back to BM25 score.
fn relevance_score(wiki: &AideMemo, question: &str, fact_content: &str) -> Option<f32> {
    use std::cell::RefCell;
    thread_local! {
        // (question_text, embedding) — invalidates when question
        // changes. Per-thread because rayon's session-extract loop
        // can run concurrently; embedder is internally Sync.
        static Q_EMBED: RefCell<Option<(String, Vec<f32>)>> = const { RefCell::new(None) };
    }
    let q_vec = Q_EMBED.with(|cell| -> Option<Vec<f32>> {
        let mut slot = cell.borrow_mut();
        if let Some((cached_q, vec)) = slot.as_ref()
            && cached_q == question
        {
            return Some(vec.clone());
        }
        match wiki.embed(question) {
            Ok(v) => {
                *slot = Some((question.to_string(), v.clone()));
                Some(v)
            }
            Err(_) => None,
        }
    })?;
    let f_vec = wiki.embed(fact_content).ok()?;
    Some(AideMemo::cosine_similarity(&q_vec, &f_vec))
}

fn evaluate(
    q: &Question,
    wiki: &AideMemo,
    top_k: usize,
    temporal: bool,
    hybrid: bool,
    time_decay_days: Option<f64>,
) -> (Option<usize>, Vec<RetrievalRecord>) {
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
    // Pull a wider slate when applying post-hoc decay (BM25-only mode)
    // so the post-multiplier can promote a low-BM25-but-recent hit
    // into the top-K. Hybrid mode handles this inside aidememo-core, so
    // the wider slate is unnecessary there.
    let candidate_limit = if time_decay_days.is_some() && !hybrid {
        top_k.saturating_mul(5).max(50)
    } else {
        top_k
    };
    let opts = SearchOpts {
        limit: Some(candidate_limit),
        bm25_only: !hybrid,
        current_only: true,
        until,
        ..Default::default()
    };
    let mut results = match wiki.hybrid_search(&q.question, opts) {
        Ok(r) => r,
        Err(_) => return (None, Vec::new()),
    };
    // Post-hoc decay only applies to bm25_only mode — in hybrid mode
    // aidememo-core's rrf_fusion already applies the decay in-pipeline (set
    // by build_store_for_question), so doubling here would corrupt
    // both ordering and any reranker step that ran after fusion.
    if let Some(tau_days) = time_decay_days
        && !hybrid
    {
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
    // Single pass: build retrieval records (for emission) AND find
    // the rank of the first evidence-session hit. Resolve the
    // session entity per hit so the JSONL row carries it for the
    // downstream LLM reader.
    let mut records: Vec<RetrievalRecord> = Vec::with_capacity(results.len().min(top_k));
    let mut hit_rank: Option<usize> = None;
    for (idx, hit) in results.iter().enumerate() {
        if records.len() >= top_k {
            break;
        }
        let rank = idx + 1;
        let mut session_id: Option<String> = None;
        let mut is_evidence = false;
        if let Ok(fact) = wiki.fact_get(&hit.fact_id) {
            for eid in &fact.entity_ids {
                if let Ok(ent) = wiki.entity_get_by_id(*eid)
                    && let Some(stripped) = ent.name.strip_prefix("session:")
                {
                    session_id = Some(stripped.to_string());
                    if answer_entity_names.contains(&ent.name) {
                        is_evidence = true;
                    }
                    break;
                }
            }
            // Layer-1 deterministic structured-value extraction.
            // Anchor on the fact's referenced date so relative phrases
            // ("yesterday", "two weeks ago") resolve to the right
            // absolute date for the conversation that produced this
            // fact. Skip extraction when no anchor is available — the
            // structured field stays empty and the Python aggregation
            // layer falls back to text-only reading.
            let anchor_dt = fact
                .observed_at
                .and_then(|ms| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms as i64));
            let structured = aidememo_core::extract_structured::extract(&fact.content, anchor_dt);
            // Per-fact semantic relevance to the QUESTION (not the
            // search query — sometimes they differ when query was
            // expanded). Same embedder hybrid_search uses; the
            // model is already loaded, so each call is ~1ms with
            // model2vec. Lets the downstream Python harness filter
            // structured aggregations to only on-topic facts.
            let relevance = relevance_score(wiki, &q.question, &fact.content);
            records.push(RetrievalRecord {
                rank,
                fact_id: hit.fact_id.to_string(),
                content: fact.content,
                score: hit.score,
                session_id,
                source: fact.source,
                referenced_date: fact.observed_at,
                structured,
                relevance,
            });
        }
        if is_evidence && hit_rank.is_none() {
            hit_rank = Some(rank);
        }
    }
    (hit_rank, records)
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
    // Balanced sampling: take the first `n` of each question_type
    // bucket so the aggregate isn't dominated by whichever category
    // appears first in the JSON. Critical for small samples (e.g.
    // when --llm-extract makes 500-question runs impractical) — the
    // first 5 questions of LongMemEval-S are all single-session-user,
    // so a flat --limit 5 measures one category while pretending to
    // measure six.
    if let Some(per_type) = args.balanced_sample {
        let mut by_type: std::collections::BTreeMap<String, Vec<Question>> = Default::default();
        for q in questions.drain(..) {
            by_type.entry(q.question_type.clone()).or_default().push(q);
        }
        for (_t, bucket) in by_type {
            for q in bucket.into_iter().take(per_type) {
                questions.push(q);
            }
        }
    }
    let n = limit
        .map(|l| l.min(questions.len()))
        .unwrap_or(questions.len());
    println!("LongMemEval-S retrieval baseline — aidememo BM25-only");
    println!("dataset:  {data:?}");
    println!("questions: {n} (of {})", questions.len());
    println!("top_k:    {top_k}");
    println!("temporal: {temporal}");
    println!("hybrid:   {}", args.hybrid);
    if let Some(m) = &args.embed_model {
        println!("embed:    {m}");
    }
    if let Some(r) = &args.reranker {
        println!("reranker: {r}");
    }
    if let Some(w) = args.bm25_weight {
        println!("bm25_w:   {w}");
    }
    if let Some(w) = args.semantic_weight {
        println!("sem_w:    {w}");
    }
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
    let mut emit_writer = if let Some(path) = &args.emit_retrievals {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {parent:?}: {e}"))?;
        }
        let f = std::fs::File::create(path).map_err(|e| format!("open {path:?}: {e}"))?;
        Some(std::io::BufWriter::new(f))
    } else {
        None
    };
    // Load classify-from JSON once. Layout: {qid: {sess_idx: {turn_idx: type}}}.
    // The file is small (~30k turns × ~10 char label) so a single
    // up-front parse is fine.
    let classify_map: Option<ClassifyMap> = if let Some(path) = &args.classify_from {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
        let raw: std::collections::HashMap<
            String,
            std::collections::HashMap<String, std::collections::HashMap<String, String>>,
        > = serde_json::from_str(&raw).map_err(|e| format!("parse classify-from: {e}"))?;
        // Convert the inner String keys (sess_idx / turn_idx) to usize
        // up front so the per-question lookup is a plain HashMap fetch.
        let mut out = std::collections::HashMap::new();
        for (qid, sess_map) in raw {
            let mut s_out = std::collections::HashMap::new();
            for (sk, t_map) in sess_map {
                let sk_u: usize = match sk.parse() {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let mut t_out = std::collections::HashMap::new();
                for (tk, ty) in t_map {
                    let tk_u: usize = match tk.parse() {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    t_out.insert(tk_u, ty);
                }
                s_out.insert(sk_u, t_out);
            }
            out.insert(qid, s_out);
        }
        println!("classify: loaded {} questions from {path:?}", out.len());
        Some(out)
    } else {
        None
    };

    let started = Instant::now();
    let mut gac_total_collapsed: u64 = 0;
    let mut gac_total_facts: u64 = 0;
    for (i, q) in questions.iter().take(n).enumerate() {
        // Always stamp observed_at from the LongMemEval session date.
        // Cost is zero when readers don't use it; without it, readers
        // see "Date: Unknown" and refuse temporal questions (a 60→5%
        // drop on the temporal-reasoning category we caught after
        // measuring with --hybrid alone). --temporal stays as the
        // search-time hard cutoff (excludes facts after question_date).
        let stamp_obs = true;
        let _ = (temporal, args.time_decay_days.is_some());
        let (_dir, wiki) = build_store_for_question(
            q,
            BuildOpts {
                temporal,
                stamp_observed_at: stamp_obs,
                hybrid: args.hybrid,
                embed_model: args.embed_model.as_deref(),
                reranker: args.reranker.as_deref(),
                bm25_weight: args.bm25_weight,
                semantic_weight: args.semantic_weight,
                // Hybrid path: route decay into aidememo-core's in-pipeline
                // rrf_fusion (composes with rerank). bm25_only path:
                // keep the post-hoc multiplier inside `evaluate`.
                decay_days_for_hybrid: if args.hybrid {
                    args.time_decay_days
                } else {
                    None
                },
                llm_extract: args.llm_extract,
                llm_extract_per_session: args.llm_extract_per_session,
                llm_extract_base_url: args.llm_extract_base_url.as_deref(),
                llm_extract_api_key_env: args.llm_extract_api_key_env.as_deref(),
                llm_extract_model: args.llm_extract_model.as_deref(),
                llm_extract_replace: args.llm_extract_replace,
                session_level_ingest: args.session_level_ingest,
                hybrid_ingest: args.hybrid_ingest,
                classify_for_question: classify_map.as_ref().and_then(|m| m.get(&q.question_id)),
            },
        )?;
        if args.gac {
            let stats = wiki
                .consolidate_gac(GacOpts {
                    theta: args.gac_theta,
                    dry_run: false,
                    spread_residual_budget: 0,
                    use_cold_tier: false,
                    protected_types: args.gac_protect_types.clone(),
                })
                .map_err(|e| format!("consolidate_gac q{i}: {e}"))?;
            gac_total_facts += stats.facts_processed as u64;
            gac_total_collapsed += (stats.tight_collapsed + stats.spread_archived) as u64;
            // Rebuild HNSW with current_only so the supersede pass is
            // visible to the search loop. Pure BM25 path doesn't need
            // this but the call is cheap when no sidecar exists.
            if args.hybrid {
                wiki.vector_index_rebuild_with_opts(VectorRebuildOpts { current_only: true })
                    .map_err(|e| format!("vector_index_rebuild q{i}: {e}"))?;
            }
        }
        let (rank, retrievals) =
            evaluate(q, &wiki, top_k, temporal, args.hybrid, args.time_decay_days);
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
        if let Some(w) = emit_writer.as_mut() {
            use std::io::Write;
            let row = serde_json::json!({
                "question_id": q.question_id,
                "question_type": q.question_type,
                "question": q.question,
                "question_date": q.question_date,
                "answer_session_ids": q.answer_session_ids,
                "retrievals": retrievals,
                "first_evidence_rank": rank,
            });
            writeln!(w, "{row}").map_err(|e| format!("write retrievals row: {e}"))?;
        }
        if (i + 1) % 10 == 0 {
            eprintln!("[{:>4}/{}] processed (last: {})", i + 1, n, q.question_id);
        }
    }
    if let Some(mut w) = emit_writer {
        use std::io::Write;
        w.flush().map_err(|e| format!("flush retrievals: {e}"))?;
    }
    let wall = started.elapsed();
    let denom = n as f64;
    println!("R@1:  {:.3}", hit_at_1 as f64 / denom);
    println!("R@5:  {:.3}", hit_at_5 as f64 / denom);
    println!("R@10: {:.3}", hit_at_10 as f64 / denom);
    println!("MRR:  {:.3}", reciprocal_sum / denom);
    println!("wall: {:.2}s", wall.as_secs_f64());
    if args.gac {
        let pct = if gac_total_facts == 0 {
            0.0
        } else {
            100.0 * gac_total_collapsed as f64 / gac_total_facts as f64
        };
        println!(
            "gac:  θ={:.2}  collapsed {} / {} facts ({:.1}% across {} questions)",
            args.gac_theta, gac_total_collapsed, gac_total_facts, pct, n
        );
    }
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
