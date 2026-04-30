//! Optional cross-encoder reranking on top of `hybrid_search`.
//!
//! BM25 + semantic + RRF gives a strong ranked candidate list, but
//! cross-encoder rerankers (BGE-reranker, gte-multilingual-reranker,
//! …) score query/text pairs jointly — usually +10-20 % recall@10
//! over RRF alone on real corpora. wg keeps the rerank step opt-in
//! because the model lives in a separate process (typically
//! HuggingFace TEI's `/rerank` endpoint) and the latency cost is
//! workload-dependent.
//!
//! When enabled, the search pipeline adds one final step:
//!
//! ```text
//! BM25 → graph_prefilter → semantic_search → rrf_fusion → rerank top-K
//! ```
//!
//! Failure policy: a reranker that's unreachable, slow, or returns
//! the wrong number of scores does **not** fail the search — we log
//! once to stderr and serve the RRF order instead. The contract is
//! "rerank is best-effort polish on top of the cheap, reliable
//! pipeline below."

use crate::config::Config;
use crate::error::{Result, WgError};
use std::cmp::Ordering;

/// A scorer for (query, candidate-texts) → per-candidate scalar.
///
/// Implementations must return one score per input text, **in input
/// order**. Higher = more relevant. The score *range* is
/// implementation-defined (BGE-reranker emits raw logits; some
/// configs sigmoid them); wg only uses the relative order, so
/// neither absolute range nor monotonicity across providers matter.
pub trait Reranker: Send + Sync {
    /// Short, human-readable name (e.g. `tei(BAAI/bge-reranker-base)`).
    fn name(&self) -> String;

    /// Score `texts` against `query`. Implementations should return
    /// `texts.len()` scores in the same order.
    fn rerank(&self, query: &str, texts: &[String]) -> Result<Vec<f32>>;
}

/// Build a reranker from `config.rerank` (when enabled). Returns
/// `Ok(None)` for the default `provider = ""` (rerank disabled),
/// `Ok(Some(...))` when a provider is configured, `Err(...)` when
/// the provider name is unknown or required fields are missing.
pub fn load_reranker(config: &Config) -> Result<Option<Box<dyn Reranker>>> {
    let provider = config.rerank.provider.trim();
    if provider.is_empty() {
        return Ok(None);
    }
    match provider {
        "tei" | "text-embeddings-inference" => {
            Ok(Some(Box::new(tei::TeiReranker::from_config(config)?)))
        }
        #[cfg(feature = "fastembed")]
        "fastembed" | "bge-reranker" | "onnx-reranker" => Ok(Some(Box::new(
            fastembed_rerank::FastembedReranker::from_config(config)?,
        ))),
        other => {
            let accepted = if cfg!(feature = "fastembed") {
                "[\"tei\", \"fastembed\"]"
            } else {
                "[\"tei\"]"
            };
            Err(WgError::InvalidInput(format!(
                "unknown rerank provider '{other}' — expected one of {accepted}"
            )))
        }
    }
}

/// Reorder the top `top_k` of `results` by reranker score, leaving
/// anything beyond `top_k` in place. The reranker's score replaces
/// the per-row `score` so callers can render confidence consistently
/// (e.g. JSON output, RRF score is preserved separately on the
/// `bm25_score` / `semantic_score` fields). Each row's `rank` is
/// re-indexed at the end so the slot positions stay sequential.
///
/// Errors from the reranker are **not** fatal — we log once and
/// return without changing the order. Returning a non-`Result`
/// would make this even harder to misuse, but we keep `Result` so
/// callers can choose to surface I/O errors during tests.
pub fn apply_rerank(
    results: &mut [crate::types::SearchResult],
    query: &str,
    reranker: &dyn Reranker,
    top_k: usize,
) {
    if results.is_empty() || top_k == 0 {
        return;
    }
    let cap = top_k.min(results.len());
    let texts: Vec<String> = results[..cap].iter().map(|r| r.content.clone()).collect();

    let scores = match reranker.rerank(query, &texts) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                reranker = %reranker.name(),
                "reranker failed — falling back to RRF order ({e})",
            );
            return;
        }
    };
    if scores.len() != cap {
        tracing::warn!(
            reranker = %reranker.name(),
            returned = scores.len(),
            "reranker returned {} scores for {} candidates — ignoring",
            scores.len(),
            cap
        );
        return;
    }

    // Sort the top-K window by score descending. We keep stable
    // ordering on equal scores (preserves original RRF order as the
    // tiebreak) by carrying the original index.
    let mut head: Vec<(f32, usize)> = scores.iter().enumerate().map(|(i, s)| (*s, i)).collect();
    head.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });

    // Build the new top-K slice from `results[..cap]` in the new
    // order, replacing `score` with the rerank score. Tail (anything
    // past `cap`) stays untouched.
    let mut new_head: Vec<crate::types::SearchResult> = Vec::with_capacity(cap);
    for (score, src_idx) in head {
        let mut row = results[src_idx].clone();
        row.score = score;
        new_head.push(row);
    }
    results[..cap].clone_from_slice(&new_head);

    // Re-index rank so renderer output is sequential.
    for (i, r) in results.iter_mut().enumerate() {
        r.rank = i + 1;
    }
}

// ---------------------------------------------------------------------------
// HuggingFace text-embeddings-inference /rerank
// ---------------------------------------------------------------------------

mod tei {
    use super::{Config, Reranker, Result, WgError};
    use serde::Deserialize;

    pub struct TeiReranker {
        endpoint: String,
        api_key: Option<String>,
        nice_name: String,
    }

    impl TeiReranker {
        pub fn from_config(config: &Config) -> Result<Self> {
            let raw = config.rerank.endpoint.trim();
            if raw.is_empty() {
                return Err(WgError::InvalidInput(
                    "rerank.endpoint is required for the tei reranker \
                     (e.g. http://localhost:8081 — point at the TEI base URL, \
                     not /rerank)"
                        .into(),
                ));
            }
            // Accept either the bare base URL or the explicit /rerank path —
            // we always normalize to `<base>/rerank` for the actual call.
            let trimmed = raw
                .trim_end_matches('/')
                .trim_end_matches("/rerank")
                .trim_end_matches('/');
            let endpoint = format!("{trimmed}/rerank");

            let api_key = if config.rerank.api_key_env.trim().is_empty() {
                None
            } else {
                std::env::var(&config.rerank.api_key_env).ok()
            };
            let nice_name = if config.rerank.model.trim().is_empty() {
                "tei-rerank".to_string()
            } else {
                format!("tei({})", config.rerank.model)
            };
            Ok(Self {
                endpoint,
                api_key,
                nice_name,
            })
        }
    }

    /// `Vec<Rank>` — TEI returns the candidates *sorted by score* in
    /// the response, but `index` lets us put them back in input order
    /// before scoring against `texts.len()`. Score range depends on
    /// `raw_scores`: with the default `raw_scores=false` the values
    /// are sigmoided to (0, 1); we don't care about absolute range,
    /// just relative ordering.
    #[derive(Debug, Deserialize)]
    struct Rank {
        index: usize,
        score: f32,
        #[serde(default)]
        #[allow(dead_code)]
        text: Option<String>,
    }

    impl Reranker for TeiReranker {
        fn name(&self) -> String {
            self.nice_name.clone()
        }

        fn rerank(&self, query: &str, texts: &[String]) -> Result<Vec<f32>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let body = serde_json::json!({
                "query": query,
                "texts": texts,
                "raw_scores": false,
                "return_text": false,
                "truncate": true,
            });
            let mut req = ureq::post(&self.endpoint).set("Content-Type", "application/json");
            if let Some(key) = &self.api_key {
                req = req.set("Authorization", &format!("Bearer {key}"));
            }
            let resp = req.send_json(body).map_err(|e| {
                WgError::Internal(format!("tei rerank to {} failed: {e}", self.endpoint))
            })?;
            let parsed: Vec<Rank> = resp
                .into_json()
                .map_err(|e| WgError::Internal(format!("tei rerank parse: {e}")))?;
            let mut scores = vec![f32::NEG_INFINITY; texts.len()];
            for r in parsed {
                if r.index < scores.len() {
                    scores[r.index] = r.score;
                }
            }
            Ok(scores)
        }
    }
}

// ---------------------------------------------------------------------------
// fastembed reranker (BGE / Jina cross-encoders via ONNX Runtime)
// ---------------------------------------------------------------------------

/// In-process cross-encoder reranker backed by `fastembed-rs`'s
/// `TextRerank`. No external service required — the BGE / Jina
/// reranker ONNX weights run via `ort` on CPU.
///
/// Why a second reranker provider on top of the existing TEI one?
///
/// - **TEI** wins for production / shared multi-agent setups: one
///   GPU-backed server, every agent reuses the cached model weights.
///   Latency stays consistent regardless of agent count.
/// - **fastembed** wins for local single-user setups: zero infra,
///   first call downloads ~90-300 MB (depending on model), every
///   subsequent call is in-process. Same MCP / CLI / binding surface,
///   no Docker / TEI server.
///
/// Requires the `fastembed` cargo feature on `wg-core`. Default
/// model is `BGERerankerBase` (English+Chinese, ~270 MB). Multilingual
/// callers should set `rerank.model = "bge-reranker-v2-m3"` (or
/// `jina-reranker-v2-base-multilingual`) — both supported by the
/// upstream crate and listed in [`parse_reranker_model`].
#[cfg(feature = "fastembed")]
mod fastembed_rerank {
    use super::{Config, Reranker, Result, WgError};
    use fastembed::{RerankInitOptions, RerankerModel, TextRerank};
    use parking_lot::Mutex;

    pub struct FastembedReranker {
        inner: Mutex<TextRerank>,
        model_name: String,
    }

    impl FastembedReranker {
        pub fn from_config(config: &Config) -> Result<Self> {
            let model_id = if config.rerank.model.is_empty() {
                "bge-reranker-base".to_string()
            } else {
                config.rerank.model.clone()
            };
            let model_enum = parse_reranker_model(&model_id)?;
            let mut opts = RerankInitOptions::new(model_enum);
            // Honour the user's wg cache_dir (same convention as
            // `model.cache_dir` for embeddings) so reranker weights
            // co-locate with everything else under one root.
            if !config.model.cache_dir.is_empty() {
                let cache = expand_tilde(&config.model.cache_dir);
                if !cache.as_os_str().is_empty() {
                    opts = opts.with_cache_dir(cache);
                }
            }
            let inner = TextRerank::try_new(opts).map_err(|e| WgError::ModelLoadFailed {
                path: std::path::PathBuf::from(&model_id),
                source: Box::new(std::io::Error::other(e.to_string())),
            })?;
            Ok(Self {
                inner: Mutex::new(inner),
                model_name: model_id,
            })
        }
    }

    impl Reranker for FastembedReranker {
        fn name(&self) -> String {
            format!("fastembed({})", self.model_name)
        }
        fn rerank(&self, query: &str, texts: &[String]) -> Result<Vec<f32>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let mut guard = self.inner.lock();
            // The fastembed crate's generic constraint requires query
            // and documents to share `S: AsRef<str>`. Project &[String]
            // to a Vec<&str> to satisfy it without reallocating the
            // strings themselves.
            let docs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            // `return_documents=false` — we only want scores; the
            // caller already has the original texts and we look them
            // up by input position, not by the reranker's echo.
            let results = guard
                .rerank(query, docs, false, None)
                .map_err(|e| WgError::Internal(format!("fastembed rerank: {e}")))?;
            // `TextRerank::rerank` returns results sorted by score
            // descending, NOT in input order. wg's Reranker contract
            // requires same-length, input-order scores. Re-sort by
            // `index` to recover the original order.
            let mut scored: Vec<(usize, f32)> =
                results.into_iter().map(|r| (r.index, r.score)).collect();
            scored.sort_by_key(|(idx, _)| *idx);
            Ok(scored.into_iter().map(|(_, s)| s).collect())
        }
    }

    /// Map a config string to the `RerankerModel` enum. Accepts
    /// kebab/lowercase forms ("bge-reranker-base"), HF hub paths
    /// ("BAAI/bge-reranker-base"), and PascalCase enum names
    /// ("BGERerankerBase"). Unknown names error rather than picking
    /// a default, since the wrong model means a 100-300 MB wasted
    /// download.
    fn parse_reranker_model(s: &str) -> Result<RerankerModel> {
        let canon = s
            .to_lowercase()
            .replace(['_', ' '], "-")
            .replace("baai/", "")
            .replace("rozgo/", "")
            .replace("jinaai/", "");
        Ok(match canon.as_str() {
            "bge-reranker-base" | "bgererankerbase" => RerankerModel::BGERerankerBase,
            "bge-reranker-v2-m3" | "bgererankerv2m3" => RerankerModel::BGERerankerV2M3,
            "jina-reranker-v1-turbo-en" | "jinarerankerv1turboen" => {
                RerankerModel::JINARerankerV1TurboEn
            }
            "jina-reranker-v2-base-multilingual"
            | "jinarerankerv2basemultiligual"
            | "jinarerankerv2basemultilingual" => RerankerModel::JINARerankerV2BaseMultiligual,
            other => {
                return Err(WgError::InvalidInput(format!(
                    "unknown fastembed reranker '{other}' — accepted: \
                     bge-reranker-base (default, en+zh), \
                     bge-reranker-v2-m3 (multilingual), \
                     jina-reranker-v1-turbo-en, \
                     jina-reranker-v2-base-multilingual"
                )));
            }
        })
    }

    fn expand_tilde(s: &str) -> std::path::PathBuf {
        if let Some(rest) = s.strip_prefix("~/")
            && let Some(home) = std::env::var_os("HOME")
        {
            return std::path::PathBuf::from(home).join(rest);
        }
        std::path::PathBuf::from(s)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parser_canonical_forms() {
            assert_eq!(
                parse_reranker_model("bge-reranker-base").unwrap(),
                RerankerModel::BGERerankerBase
            );
            assert_eq!(
                parse_reranker_model("BGERerankerBase").unwrap(),
                RerankerModel::BGERerankerBase
            );
            assert_eq!(
                parse_reranker_model("BAAI/bge-reranker-v2-m3").unwrap(),
                RerankerModel::BGERerankerV2M3
            );
            assert_eq!(
                parse_reranker_model("jina-reranker-v2-base-multilingual").unwrap(),
                RerankerModel::JINARerankerV2BaseMultiligual
            );
        }

        #[test]
        fn parser_rejects_unknown_with_helpful_message() {
            let err = parse_reranker_model("bge-reranker-large")
                .unwrap_err()
                .to_string();
            assert!(err.contains("unknown fastembed reranker"));
            assert!(err.contains("bge-reranker-base"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FactId, FactType, SearchResult};

    /// In-process reranker that scores by string length (longer ⇒
    /// higher). Lets us exercise `apply_rerank` without an HTTP
    /// dependency; what we're testing is wg's *plumbing*, not the
    /// upstream model.
    struct LengthReranker;
    impl Reranker for LengthReranker {
        fn name(&self) -> String {
            "length".into()
        }
        fn rerank(&self, _query: &str, texts: &[String]) -> Result<Vec<f32>> {
            Ok(texts.iter().map(|t| t.len() as f32).collect())
        }
    }

    fn fake_result(content: &str, rank: usize, score: f32) -> SearchResult {
        SearchResult {
            fact_id: FactId::new(),
            content: content.to_string(),
            fact_type: FactType::Note,
            entity_names: vec![],
            source: None,
            score,
            rank,
            created_at: 0,
            observed_at: None,
            #[cfg(feature = "semantic")]
            session_id: None,
        }
    }

    #[test]
    fn apply_rerank_orders_top_k_by_score() {
        let mut results = vec![
            fake_result("aa", 1, 0.9),
            fake_result("bbbb", 2, 0.8),
            fake_result("c", 3, 0.7),
            fake_result("dddddd", 4, 0.6),
        ];
        // top_k=3 reranks the first three; "dddddd" stays in slot 4
        // (untouched by the rerank pass).
        apply_rerank(&mut results, "ignored", &LengthReranker, 3);

        let names: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
        // Top-3 sorted by length desc: bbbb (4) > aa (2) > c (1).
        // Slot 4 untouched.
        assert_eq!(names, vec!["bbbb", "aa", "c", "dddddd"]);
        // Rank is re-indexed.
        assert_eq!(results[0].rank, 1);
        assert_eq!(results[1].rank, 2);
        // Scores were replaced with rerank scores for the head only.
        assert!((results[0].score - 4.0).abs() < 1e-3);
        assert!((results[3].score - 0.6).abs() < 1e-3); // untouched tail
    }

    #[test]
    fn apply_rerank_top_k_zero_is_a_noop() {
        let mut results = vec![fake_result("x", 1, 0.9), fake_result("y", 2, 0.5)];
        let before: Vec<f32> = results.iter().map(|r| r.score).collect();
        apply_rerank(&mut results, "q", &LengthReranker, 0);
        let after: Vec<f32> = results.iter().map(|r| r.score).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn apply_rerank_empty_results_is_a_noop() {
        let mut results: Vec<SearchResult> = vec![];
        apply_rerank(&mut results, "q", &LengthReranker, 10);
        assert!(results.is_empty());
    }

    /// A reranker that returns the wrong number of scores must NOT
    /// reorder the results — we log once and serve RRF.
    struct WrongCountReranker;
    impl Reranker for WrongCountReranker {
        fn name(&self) -> String {
            "wrong-count".into()
        }
        fn rerank(&self, _query: &str, _texts: &[String]) -> Result<Vec<f32>> {
            Ok(vec![1.0]) // always returns exactly one score
        }
    }

    #[test]
    fn apply_rerank_falls_back_when_score_count_mismatches() {
        let mut results = vec![
            fake_result("aa", 1, 0.9),
            fake_result("bbbb", 2, 0.8),
            fake_result("c", 3, 0.7),
        ];
        let before: Vec<String> = results.iter().map(|r| r.content.clone()).collect();
        apply_rerank(&mut results, "q", &WrongCountReranker, 3);
        let after: Vec<String> = results.iter().map(|r| r.content.clone()).collect();
        assert_eq!(before, after, "mismatched score count must not reorder");
    }
}
