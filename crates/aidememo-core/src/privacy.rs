//! Optional write-time privacy filtering.
//!
//! The default path is disabled and does not call a model. When configured,
//! this module calls a local OpenAI Privacy Filter-compatible sidecar before a
//! fact is persisted, then applies AideMemo's policy locally.

use crate::config::PrivacyConfig;
use crate::error::{AideMemoError, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

#[cfg(feature = "semantic")]
#[derive(Debug, Clone, Deserialize)]
struct SidecarResponse {
    #[serde(default)]
    detected_spans: Vec<DetectedSpan>,
    #[serde(default)]
    spans: Vec<DetectedSpan>,
    #[serde(default, rename = "redacted_text")]
    _redacted_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DetectedSpan {
    pub label: String,
    pub start: usize,
    pub end: usize,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyAction {
    Unchanged,
    Reported,
    Redacted,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyFilterResult {
    pub content: String,
    pub action: PrivacyAction,
    pub span_count: usize,
    pub by_label: BTreeMap<String, usize>,
}

impl PrivacyFilterResult {
    fn unchanged(content: String) -> Self {
        Self {
            content,
            action: PrivacyAction::Unchanged,
            span_count: 0,
            by_label: BTreeMap::new(),
        }
    }
}

pub fn screen_text(content: &str, config: &PrivacyConfig) -> Result<PrivacyFilterResult> {
    if !config.enabled() {
        return Ok(PrivacyFilterResult::unchanged(content.to_string()));
    }
    let mut spans = detect_local_secret_spans(content);
    spans.extend(fetch_spans(content, config)?);
    apply_policy(content, spans, config)
}

pub fn screen_fact_content(content: String, config: &PrivacyConfig) -> Result<String> {
    Ok(screen_text(&content, config)?.content)
}

fn apply_policy(
    content: &str,
    spans: Vec<DetectedSpan>,
    config: &PrivacyConfig,
) -> Result<PrivacyFilterResult> {
    let spans = normalize_spans(spans);
    if spans.is_empty() {
        return Ok(PrivacyFilterResult::unchanged(content.to_string()));
    }

    let by_label = summarize(&spans);
    let mode = config.mode.trim().to_ascii_lowercase();
    let mut blocked_labels: Vec<String> = Vec::new();
    for span in &spans {
        if should_block(&span.label, &mode, config) && !blocked_labels.contains(&span.label) {
            blocked_labels.push(span.label.clone());
        }
    }
    if !blocked_labels.is_empty() {
        tracing::warn!(
            labels = ?blocked_labels,
            span_count = spans.len(),
            "privacy filter blocked fact write"
        );
        return Err(AideMemoError::InvalidInput(format!(
            "privacy filter blocked fact write; labels={}",
            blocked_labels.join(",")
        )));
    }

    match mode.as_str() {
        "report" => {
            tracing::warn!(
                labels = ?by_label,
                span_count = spans.len(),
                "privacy filter reported sensitive spans"
            );
            Ok(PrivacyFilterResult {
                content: content.to_string(),
                action: PrivacyAction::Reported,
                span_count: spans.len(),
                by_label,
            })
        }
        "redact" => {
            let redact_spans: Vec<DetectedSpan> = spans
                .iter()
                .filter(|span| label_in(&span.label, &config.redact_labels))
                .cloned()
                .collect();
            if redact_spans.is_empty() {
                tracing::warn!(
                    labels = ?by_label,
                    span_count = spans.len(),
                    "privacy filter reported review-only spans"
                );
                return Ok(PrivacyFilterResult {
                    content: content.to_string(),
                    action: PrivacyAction::Reported,
                    span_count: spans.len(),
                    by_label,
                });
            }
            let redacted = redact_content(content, &redact_spans)?;
            tracing::warn!(
                labels = ?by_label,
                span_count = spans.len(),
                redacted_count = redact_spans.len(),
                "privacy filter redacted fact content"
            );
            Ok(PrivacyFilterResult {
                content: redacted,
                action: PrivacyAction::Redacted,
                span_count: spans.len(),
                by_label,
            })
        }
        "block" => {
            // Reaching this branch means every detected span was review-only.
            tracing::warn!(
                labels = ?by_label,
                span_count = spans.len(),
                "privacy filter reported review-only spans in block mode"
            );
            Ok(PrivacyFilterResult {
                content: content.to_string(),
                action: PrivacyAction::Reported,
                span_count: spans.len(),
                by_label,
            })
        }
        other => Err(AideMemoError::InvalidInput(format!(
            "privacy.mode must be 'report', 'redact', or 'block', got '{other}'"
        ))),
    }
}

fn should_block(label: &str, mode: &str, config: &PrivacyConfig) -> bool {
    if label_in(label, &config.block_labels) {
        return mode == "redact" || mode == "block";
    }
    mode == "block" && label_in(label, &config.redact_labels)
}

fn label_in(label: &str, labels: &[String]) -> bool {
    labels
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(label))
}

fn normalize_spans(mut spans: Vec<DetectedSpan>) -> Vec<DetectedSpan> {
    spans.retain(|span| span.start < span.end && !span.label.trim().is_empty());
    for span in &mut spans {
        span.label = span.label.trim().to_ascii_lowercase();
    }
    spans.sort_by_key(|span| (span.start, span_priority(&span.label), span.end));
    let mut out: Vec<DetectedSpan> = Vec::new();
    for span in spans {
        if let Some(prev) = out.last_mut()
            && span.start < prev.end
        {
            if span_priority(&span.label) < span_priority(&prev.label) {
                *prev = span;
            }
            continue;
        }
        out.push(span);
    }
    out
}

fn span_priority(label: &str) -> u8 {
    if label.eq_ignore_ascii_case("secret") {
        0
    } else {
        1
    }
}

fn detect_local_secret_spans(content: &str) -> Vec<DetectedSpan> {
    const PREFIXES: &[(&str, usize)] = &[
        ("github_pat_", 20),
        ("sk-proj-", 12),
        ("xoxb-", 12),
        ("ghp_", 12),
        ("gho_", 12),
        ("AKIA", 16),
        ("AIza", 16),
        ("sk-", 12),
    ];

    let bytes = content.as_bytes();
    let mut spans = Vec::new();
    let mut idx = 0usize;
    while idx < bytes.len() {
        let prefix = PREFIXES.iter().find(|(prefix, _)| {
            bytes[idx..].starts_with(prefix.as_bytes())
                && idx
                    .checked_sub(1)
                    .and_then(|prev| bytes.get(prev))
                    .is_none_or(|prev| !is_secret_char(*prev))
        });
        let Some((prefix, min_len)) = prefix else {
            idx += 1;
            continue;
        };
        let mut end = idx + prefix.len();
        while end < bytes.len() && is_secret_char(bytes[end]) {
            end += 1;
        }
        if end - idx >= *min_len {
            spans.push(DetectedSpan {
                label: "secret".to_string(),
                start: byte_to_char_idx(content, idx),
                end: byte_to_char_idx(content, end),
                text: content[idx..end].to_string(),
                placeholder: String::new(),
            });
            idx = end;
        } else {
            idx += prefix.len();
        }
    }
    spans
}

fn is_secret_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn byte_to_char_idx(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx].chars().count()
}

fn summarize(spans: &[DetectedSpan]) -> BTreeMap<String, usize> {
    let mut by_label = BTreeMap::new();
    for span in spans {
        *by_label.entry(span.label.clone()).or_insert(0) += 1;
    }
    by_label
}

fn redact_content(content: &str, spans: &[DetectedSpan]) -> Result<String> {
    let mut out = String::new();
    let mut cursor = 0usize;
    for span in spans {
        let start = char_to_byte(content, span.start).ok_or_else(|| {
            AideMemoError::InvalidInput(format!(
                "privacy filter span start {} is outside content",
                span.start
            ))
        })?;
        let end = char_to_byte(content, span.end).ok_or_else(|| {
            AideMemoError::InvalidInput(format!(
                "privacy filter span end {} is outside content",
                span.end
            ))
        })?;
        if start < cursor || start > end {
            continue;
        }
        out.push_str(&content[cursor..start]);
        out.push_str(&placeholder_for(&span.label));
        cursor = end;
    }
    out.push_str(&content[cursor..]);
    Ok(out)
}

fn placeholder_for(label: &str) -> String {
    format!("[{label}]")
}

fn char_to_byte(s: &str, char_idx: usize) -> Option<usize> {
    if char_idx == s.chars().count() {
        return Some(s.len());
    }
    s.char_indices().nth(char_idx).map(|(idx, _)| idx)
}

#[cfg(feature = "semantic")]
fn spans_from_response(mut response: SidecarResponse) -> Vec<DetectedSpan> {
    if !response.detected_spans.is_empty() {
        return response.detected_spans;
    }
    response.spans.append(&mut response.detected_spans);
    response.spans
}

#[cfg(feature = "semantic")]
fn fetch_spans(content: &str, config: &PrivacyConfig) -> Result<Vec<DetectedSpan>> {
    let endpoint = privacy_endpoint(config)?;
    let body = serde_json::json!({
        "text": content,
        "mode": config.mode,
        "block_labels": config.block_labels,
        "redact_labels": config.redact_labels,
        "review_labels": config.review_labels,
    });
    let mut req = ureq::post(&endpoint).set("Content-Type", "application/json");
    if let Some(token) = privacy_token(config) {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let value: serde_json::Value = req
        .send_json(body)
        .map_err(|e| AideMemoError::Internal(format!("privacy filter POST {endpoint}: {e}")))?
        .into_json()
        .map_err(|e| AideMemoError::Internal(format!("privacy filter response parse: {e}")))?;
    let response: SidecarResponse = serde_json::from_value(value)
        .map_err(|e| AideMemoError::Internal(format!("privacy filter response shape: {e}")))?;
    Ok(spans_from_response(response))
}

#[cfg(not(feature = "semantic"))]
fn fetch_spans(_content: &str, _config: &PrivacyConfig) -> Result<Vec<DetectedSpan>> {
    Err(AideMemoError::InvalidInput(
        "privacy.provider requires a build with the semantic feature".to_string(),
    ))
}

#[cfg(feature = "semantic")]
fn privacy_endpoint(config: &PrivacyConfig) -> Result<String> {
    let endpoint = config.endpoint.trim();
    if endpoint.is_empty() {
        return Err(AideMemoError::InvalidInput(
            "privacy.endpoint is required when privacy.provider is set".to_string(),
        ));
    }
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/filter") || trimmed.ends_with("/redact") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("{trimmed}/filter"))
    }
}

#[cfg(feature = "semantic")]
fn privacy_token(config: &PrivacyConfig) -> Option<String> {
    let env = config.api_key_env.trim();
    if env.is_empty() {
        return None;
    }
    std::env::var(env).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(mode: &str) -> PrivacyConfig {
        PrivacyConfig {
            provider: "openai-privacy-filter".to_string(),
            mode: mode.to_string(),
            endpoint: "http://127.0.0.1:8090".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn redact_mode_masks_configured_labels_and_preserves_review_labels() {
        let spans = vec![
            DetectedSpan {
                label: "private_email".to_string(),
                start: 14,
                end: 31,
                text: "alice@example.com".to_string(),
                placeholder: String::new(),
            },
            DetectedSpan {
                label: "private_person".to_string(),
                start: 0,
                end: 5,
                text: "Alice".to_string(),
                placeholder: String::new(),
            },
        ];
        let result = apply_policy("Alice emailed alice@example.com", spans, &config("redact"))
            .expect("redact");
        assert_eq!(result.action, PrivacyAction::Redacted);
        assert_eq!(result.content, "Alice emailed [private_email]");
        assert_eq!(result.by_label.get("private_email"), Some(&1));
        assert_eq!(result.by_label.get("private_person"), Some(&1));
    }

    #[test]
    fn redact_mode_blocks_secret() {
        let spans = vec![DetectedSpan {
            label: "secret".to_string(),
            start: 6,
            end: 12,
            text: "sk-abc".to_string(),
            placeholder: String::new(),
        }];
        let err = apply_policy("token sk-abc", spans, &config("redact")).expect_err("block");
        assert!(err.to_string().contains("secret"));
    }

    #[test]
    fn local_secret_detector_catches_bare_common_keys() {
        let text = "keep sk-proj-abc123 and github_pat_1234567890abcdef out";
        let spans = detect_local_secret_spans(text);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].label, "secret");
        assert_eq!(spans[0].text, "sk-proj-abc123");
        assert_eq!(spans[1].text, "github_pat_1234567890abcdef");
    }

    #[test]
    fn local_secret_prefers_block_label_over_overlapping_model_span() {
        let text = "keep sk-proj-abc123 out";
        let mut spans = detect_local_secret_spans(text);
        spans.push(DetectedSpan {
            label: "private_person".to_string(),
            start: 5,
            end: 19,
            text: "sk-proj-abc123".to_string(),
            placeholder: String::new(),
        });
        let err = apply_policy(text, spans, &config("redact")).expect_err("block");
        assert!(err.to_string().contains("secret"));
    }

    #[test]
    fn block_mode_rejects_redact_labels() {
        let spans = vec![DetectedSpan {
            label: "private_phone".to_string(),
            start: 5,
            end: 17,
            text: "555-010-9999".to_string(),
            placeholder: String::new(),
        }];
        let err = apply_policy("call 555-010-9999", spans, &config("block")).expect_err("block");
        assert!(err.to_string().contains("private_phone"));
    }

    #[test]
    fn disabled_config_is_noop() {
        let result = screen_text("alice@example.com", &PrivacyConfig::default()).expect("noop");
        assert_eq!(result.action, PrivacyAction::Unchanged);
        assert_eq!(result.content, "alice@example.com");
    }
}
