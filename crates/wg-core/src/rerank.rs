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
        other => Err(WgError::InvalidInput(format!(
            "unknown rerank provider '{other}' — expected one of [\"tei\"]"
        ))),
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
