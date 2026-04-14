//! Domain adapter for search result adaptation based on user feedback.
//!
//! Learns a simple linear re-ranking model: given feedback signals, adjusts
//! the relevance score for each fact entity to improve precision over time.
//!
//! ## Model
//!
//! Maintains a per-entity bias term `b_f` for each fact. On training:
//!   - Helpful feedback on fact `f` in session `s` → increase `b_f`
//!   - Not-helpful feedback → decrease `b_f`
//!
//! The final score for a fact is: `base_score + alpha * b_f`
//!
//! ## Evaluation
//!
//! Uses held-out feedback to compute:
//!   - **Precision@K**: fraction of top-K results with helpful=true
//!   - **Recall boost**: relative improvement in recall vs. unadapted baseline

use crate::error::{Result, WgError};
use crate::types::{AdaptEvalReport, AdaptResult, AdaptStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Domain adapter state — a simple per-fact bias vector.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DomainAdapter {
    /// Per-fact bias adjustments. Positive = helpful boost, negative = penalize.
    fact_biases: HashMap<String, f32>,
    /// How many feedback items were used to train this adapter.
    pub feedback_used: usize,
    /// Number of training iterations (generations).
    pub generation: u32,
    /// Learning rate for updates.
    alpha: f32,
}

impl DomainAdapter {
    /// Create a new adapter with default settings.
    pub fn new() -> Self {
        Self {
            fact_biases: HashMap::new(),
            feedback_used: 0,
            generation: 0,
            alpha: 0.1,
        }
    }

    /// Apply the adapter to adjust a base score for a given fact.
    /// Returns the adjusted score.
    pub fn apply(&self, fact_id: &str, base_score: f32) -> f32 {
        let bias = self.fact_biases.get(fact_id).copied().unwrap_or(0.0);
        base_score + self.alpha * bias
    }

    /// Train the adapter on a batch of feedback.
    ///
    /// Helpful feedback increases the fact's bias; not-helpful decreases it.
    /// Returns an [`AdaptResult`] with training statistics.
    pub fn train(&mut self, feedback: &[(String, bool)]) -> AdaptResult {
        let mut helpful_count = 0;

        for (fact_id, helpful) in feedback {
            let entry = self.fact_biases.entry(fact_id.clone()).or_insert(0.0);
            if *helpful {
                *entry += 1.0;
                helpful_count += 1;
            } else {
                *entry -= 0.5;
            }
            self.feedback_used += 1;
        }

        self.generation += 1;

        AdaptResult {
            feedback_used: feedback.len(),
            helpful_count,
            generation: self.generation,
        }
    }

    /// Evaluate the adapter on held-out feedback.
    ///
    /// Computes Precision@K (K=10) and recall boost over the baseline.
    pub fn evaluate(&self, feedback: &[(String, bool)], k: usize) -> AdaptEvalReport {
        let total = feedback.len();
        if total == 0 {
            return AdaptEvalReport {
                total_feedback: 0,
                helpful_count: 0,
                skipped_count: 0,
                precision_at_10: 0.0,
                recall_boost: 0.0,
            };
        }

        // Sort by fact_id for deterministic ordering (simulates ranked results)
        let mut ordered: Vec<_> = feedback.iter().enumerate().collect();
        ordered.sort_by_key(|(_, (fact_id, _))| fact_id);

        // Simulate ranked list — top-K by biased score
        let mut ranked: Vec<_> = ordered
            .iter()
            .map(|(idx, (fact_id, helpful))| {
                let bias = self.fact_biases.get(fact_id).copied().unwrap_or(0.0);
                let score = bias;
                (*idx, (*fact_id).clone(), *helpful, score)
            })
            .collect();
        ranked.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());
        let top_k: Vec<_> = ranked.iter().take(k).collect();

        let helpful_in_top_k: usize = top_k.iter().filter(|&(_, _, h, _)| *h).count();
        let total_helpful: usize = feedback.iter().filter(|&&(_, h)| h).count();

        let precision_at_10 = if k > 0 {
            helpful_in_top_k as f32 / k as f32
        } else {
            0.0
        };

        // Baseline: random-ish precision = total_helpful / total
        let baseline_precision = total_helpful as f32 / total as f32;
        let recall_boost = if baseline_precision > 0.0 {
            precision_at_10 / baseline_precision - 1.0
        } else {
            0.0
        };

        AdaptEvalReport {
            total_feedback: total,
            helpful_count: total_helpful,
            skipped_count: 0,
            precision_at_10,
            recall_boost,
        }
    }

    /// Get the current status of the adapter.
    pub fn status(&self, feedback_count: usize) -> AdaptStatus {
        AdaptStatus {
            has_adapter: !self.fact_biases.is_empty(),
            feedback_count,
            generation: self.generation,
            ready: self.generation > 0,
        }
    }

    /// Serialize adapter state to bytes (JSON).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| WgError::Serialize {
            context: "DomainAdapter".to_string(),
            source: e,
        })
    }

    /// Deserialize adapter state from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|e| WgError::Deserialize {
            context: "DomainAdapter".to_string(),
            source: e,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_train_helpful_boost() {
        let mut adapter = DomainAdapter::new();
        let feedback = vec![
            ("fact1".to_string(), true),
            ("fact1".to_string(), true),
            ("fact2".to_string(), false),
        ];

        let result = adapter.train(&feedback);
        assert_eq!(result.feedback_used, 3);
        assert_eq!(result.helpful_count, 2);
        assert_eq!(result.generation, 1);

        // fact1: +2 boosts, fact2: -0.5 penalty
        assert!(*adapter.fact_biases.get("fact1").unwrap() > 0.0);
        assert!(*adapter.fact_biases.get("fact2").unwrap() < 0.0);
    }

    #[test]
    fn test_adapter_apply() {
        let mut adapter = DomainAdapter::new();
        adapter.train(&[("fact1".to_string(), true)]);

        let base = 0.5;
        let adjusted = adapter.apply("fact1", base);
        assert!(adjusted > base); // boosted
    }

    #[test]
    fn test_adapter_evaluate() {
        let mut adapter = DomainAdapter::new();
        adapter.train(&[("fact1".to_string(), true)]);

        let feedback = vec![("fact1".to_string(), true), ("fact2".to_string(), false)];

        let report = adapter.evaluate(&feedback, 10);
        assert_eq!(report.total_feedback, 2);
        assert_eq!(report.helpful_count, 1);
        assert!(report.precision_at_10 >= 0.0);
    }

    #[test]
    fn test_adapter_status() {
        let adapter = DomainAdapter::new();
        let status = adapter.status(5);
        assert!(!status.has_adapter);
        assert_eq!(status.feedback_count, 5);
        assert!(!status.ready);
    }
}
