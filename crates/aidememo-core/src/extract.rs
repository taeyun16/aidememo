//! Heuristic conversation → fact extraction.
//!
//! Takes raw text (a chat transcript, a meeting note, a paragraph from
//! a code review) and returns a ranked list of *candidate* facts the
//! agent can review before committing them with `aidememo_fact_add{,_many}`.
//! Designed to run fully offline — no external LLM needed — so it can
//! ship as a default MCP tool without requiring API keys.
//!
//! Matches the agent-natural workflow that observational-memory
//! systems (Mastra, Supermemory) cover with an LLM extractor: the
//! agent stops being a passive recorder and starts seeing structured
//! suggestions when it dumps recent context. Quality is bounded by
//! the heuristics — agents using a hosted LLM can still call the LLM
//! themselves and feed structured output to `aidememo_fact_add_many`. This
//! module is the always-available baseline.
//!
//! ## Pipeline
//!
//! 1. Split the input into sentence-like chunks on `.`, `!`, `?`, `;`,
//!    or newline boundaries.
//! 2. Drop chunks that are obviously not facts: too short, all
//!    punctuation, dialog markers (`>`, leading `<name>:`).
//! 3. For each survivor, score it as a candidate by:
//!    - Matching entity names from the store via case-insensitive
//!      substring search (covers built-in aliases too).
//!    - Detecting [`FactType`] from keyword cues (e.g. "we decided"
//!      → Decision).
//!    - Combining length normalization, entity-match boost, and
//!      type-detection boost into a `[0.0, 1.0]` confidence.
//! 4. Sort by confidence and return the top `max_candidates`.

use crate::backend::StoreBackend;
use crate::error::Result;
use crate::types::{EntitySort, FactType, ListOpts};

/// One proposed fact the agent can choose to commit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractCandidate {
    /// The raw sentence to be persisted as `fact.content`.
    pub content: String,
    /// Existing entity names matched against the sentence — these
    /// flow into `aidememo_fact_add`'s `entities` arg verbatim, so missing
    /// entities are NOT auto-suggested by the extractor (they would
    /// just create empty fact-less entities).
    pub suggested_entities: Vec<String>,
    /// Heuristic-detected fact type. Falls back to `Note` when no
    /// strong cue matches.
    pub suggested_fact_type: FactType,
    /// Score in `[0.0, 1.0]` capturing how confident the extractor is
    /// that this sentence is worth keeping. Agents typically filter
    /// at `>= 0.5`.
    pub confidence: f32,
}

/// Extract candidate facts from a chunk of text. Pull the existing
/// entity list once (cheap — entity-only metadata, capped at 5000 to
/// bound large wikis) and reuse it across every sentence.
pub fn extract_candidates(
    text: &str,
    store: &(impl StoreBackend + ?Sized),
    max_candidates: usize,
) -> Result<Vec<ExtractCandidate>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }

    // Pull entity names + aliases for substring matching. EntitySummary
    // doesn't carry aliases today, so we resolve them via entity_get on
    // demand — but that's a per-entity read. To keep extraction cheap
    // we settle for matching on canonical names only; agents can
    // afterwards `aidememo_entity_alias_add` if they want fuzzy aliases.
    // Worst-case 5000 names × M sentences is well under a millisecond.
    let entities = store.entity_list(ListOpts {
        entity_type: None,
        min_facts: None,
        sort_by: EntitySort::Name,
        limit: Some(5000),
        offset: 0,
    })?;
    let entity_names: Vec<String> = entities.iter().map(|e| e.name.clone()).collect();

    let mut candidates = Vec::new();
    for sentence in split_sentences(text) {
        if let Some(cand) = score_candidate(&sentence, &entity_names) {
            candidates.push(cand);
        }
    }

    candidates.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(max_candidates);
    Ok(candidates)
}

/// Sentence segmentation. Greedy and regex-free — splits on `.`, `!`,
/// `?`, `;`, or newline. Treats `…` and `?!` as single boundaries.
/// Strips leading dialog markers (`> `, `Alice:`) before returning the
/// trimmed sentence so the candidate content is the agent-relevant
/// fact, not the speaker scaffold.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        match ch {
            '.' | '!' | '?' | ';' | '\n' | '\r' => {
                let trimmed = strip_dialog(buf.trim());
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
                buf.clear();
            }
            _ => buf.push(ch),
        }
    }
    let trimmed = strip_dialog(buf.trim());
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

/// Remove leading `>`-quote markers and `Speaker:` prefixes so the
/// candidate content is the substantive sentence.
fn strip_dialog(s: &str) -> &str {
    let s = s.trim_start_matches('>').trim_start();
    if let Some(idx) = s.find(':') {
        // Only strip when the prefix is short and looks like a name.
        // Bar 1: no spaces (so "Next step:" stays as content).
        // Bar 2: starts with an uppercase letter (so "decision:" stays).
        // Bar 3: no other punctuation in the prefix.
        let prefix = &s[..idx];
        let looks_like_name = !prefix.is_empty()
            && prefix.len() <= 32
            && prefix
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            && prefix.chars().next().is_some_and(|c| c.is_uppercase());
        if looks_like_name {
            return s[idx + 1..].trim_start();
        }
    }
    s
}

const MIN_SENTENCE_CHARS: usize = 10;
const MAX_SENTENCE_CHARS: usize = 600;

/// Score a single sentence. Returns `None` for sentences we don't
/// want to surface (too short, too long, empty after trimming).
fn score_candidate(sentence: &str, entity_names: &[String]) -> Option<ExtractCandidate> {
    let trimmed = sentence.trim();
    if trimmed.len() < MIN_SENTENCE_CHARS || trimmed.len() > MAX_SENTENCE_CHARS {
        return None;
    }
    let lowered = trimmed.to_lowercase();

    // Entity match: case-insensitive substring on whole-name boundary.
    // Skip extremely short entity names (<=2 chars) to avoid false
    // positives (e.g. an entity named "AI" matching "claim").
    let mut matches: Vec<String> = Vec::new();
    for name in entity_names {
        if name.len() < 3 {
            continue;
        }
        if lowered.contains(&name.to_lowercase()) && !matches.contains(name) {
            matches.push(name.clone());
        }
    }

    let fact_type = detect_fact_type(&lowered);

    // Confidence assembly:
    //   base                 0.40
    //   has entity match     +0.25 (×1 if any, capped)
    //   non-default type     +0.20
    //   length normalization +0.15 (favor 30..200 chars)
    let mut confidence = 0.40_f32;
    if !matches.is_empty() {
        confidence += 0.25;
    }
    if fact_type != FactType::Note && fact_type != FactType::Unknown {
        confidence += 0.20;
    }
    let len = trimmed.len();
    if (30..=200).contains(&len) {
        confidence += 0.15;
    } else if len < 30 {
        confidence += 0.05;
    }

    Some(ExtractCandidate {
        content: trimmed.to_string(),
        suggested_entities: matches,
        suggested_fact_type: fact_type,
        confidence: confidence.min(1.0),
    })
}

/// Detect a [`FactType`] from keyword cues. Ordered so that more
/// specific signals (decision / convention / pattern) win over
/// generic ones (claim / note).
fn detect_fact_type(lowered: &str) -> FactType {
    if lowered.ends_with('?') || lowered.contains(" why ") || lowered.starts_with("why ") {
        return FactType::Question;
    }
    // Decisions: explicit "we decided", "decision:", "let's go with",
    // "rolled out". The more declarative the cue, the higher the
    // signal.
    if lowered.contains("decided")
        || lowered.starts_with("decision")
        || lowered.contains("we picked")
        || lowered.contains("we chose")
        || lowered.contains("let's go with")
        || lowered.contains("rolled out")
    {
        return FactType::Decision;
    }
    // Conventions: durable rules — "always", "never", "convention",
    // "rule", "house rule".
    if lowered.contains(" always ")
        || lowered.starts_with("always ")
        || lowered.contains(" never ")
        || lowered.contains("convention")
        || lowered.contains("house rule")
        || lowered.contains(" rule:")
    {
        return FactType::Convention;
    }
    // Patterns / antipatterns.
    if lowered.contains("pattern")
        || lowered.contains("antipattern")
        || lowered.contains("anti-pattern")
    {
        return FactType::Pattern;
    }
    // Claims: "we believe", "we think", "claim".
    if lowered.contains("we believe")
        || lowered.contains("we think")
        || lowered.starts_with("claim:")
        || lowered.contains(" claim ")
    {
        return FactType::Claim;
    }
    FactType::Note
}

// ─────────────────────────────────────────────────────────── LLM extractor

/// LLM-aided fact extraction. Posts the input text plus the existing
/// entity name list to a chat-completions endpoint
/// (`<endpoint>/chat/completions`) and parses a JSON-object response
/// into [`ExtractCandidate`]s. Opt-in via `extract.provider = "openai"`
/// in the config; when the provider is empty, callers should fall back
/// to [`extract_candidates`] (the heuristic baseline).
///
/// The LLM is asked to:
///   - emit at most `max_candidates` durable facts;
///   - reuse existing entity names verbatim when they appear;
///   - assign one of `decision / pattern / convention / claim / note /
///     question` as `fact_type`;
///   - score each fact with a `[0.0, 1.0]` confidence.
///
/// On any HTTP / JSON error this function returns the underlying
/// error — aidememo's CLI/MCP handlers should fall back to the heuristic
/// extractor with a warning rather than failing the whole call.
///
/// Requires the `semantic` feature (which already pulls `ureq`); if
/// you compile aidememo-core bare, only `extract_candidates` is available.
#[cfg(feature = "semantic")]
pub fn extract_candidates_llm(
    text: &str,
    store: &(impl StoreBackend + ?Sized),
    cfg: &crate::config::ExtractConfig,
    max_candidates: usize,
) -> Result<Vec<ExtractCandidate>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    if cfg.provider.is_empty() {
        return Err(crate::error::AideMemoError::invalid_input(
            "extract.provider not set — call extract_candidates instead",
        ));
    }

    let entities = store.entity_list(ListOpts {
        entity_type: None,
        min_facts: None,
        sort_by: EntitySort::Name,
        limit: Some(5000),
        offset: 0,
    })?;
    let entity_names: Vec<String> = entities.iter().map(|e| e.name.clone()).collect();

    let api_key = if cfg.api_key_env.is_empty() {
        String::new()
    } else {
        std::env::var(&cfg.api_key_env).unwrap_or_default()
    };
    let endpoint = if cfg.endpoint.is_empty() {
        "https://api.openai.com/v1".to_string()
    } else {
        cfg.endpoint.trim_end_matches('/').to_string()
    };
    let url = format!("{}/chat/completions", endpoint);
    let cap = max_candidates.min(cfg.max_candidates).max(1);

    let body = build_llm_request(&cfg.model, &entity_names, text, cap, cfg.max_tokens);
    let raw = post_chat_completion(&url, &api_key, &body)?;
    let candidates = parse_llm_response(&raw, &entity_names, cap)?;
    Ok(candidates)
}

/// Build the OpenAI chat-completions request body.
#[cfg(feature = "semantic")]
fn build_llm_request(
    model: &str,
    entity_names: &[String],
    text: &str,
    cap: usize,
    max_tokens: u32,
) -> serde_json::Value {
    // System prompt is verbose on purpose — the model otherwise tends
    // to emit greetings as facts and to invent entities not in the
    // wiki. Both are major retrieval-noise sources.
    let entity_list = if entity_names.is_empty() {
        "(no existing entities — invent only when explicitly named in the text)".to_string()
    } else {
        // Cap the list at ~200 names so the prompt stays bounded on
        // larger wikis. The LLM only needs them for normalisation.
        let take = entity_names.iter().take(200).cloned().collect::<Vec<_>>();
        take.join(", ")
    };
    let system = format!(
        "You extract durable facts from chat / notes / docs for a knowledge wiki. \
         Output ONLY a JSON object: {{\"facts\":[{{\"content\":\"<sentence>\",\"fact_type\":\"<one of: decision|pattern|convention|claim|note|question>\",\"entities\":[\"X\",\"Y\"],\"confidence\":<0.0-1.0>}}]}}. \
         At most {cap} facts. \
         Reuse these entity names verbatim when they appear in the text: {entity_list}. \
         Only invent NEW entities when they are explicitly named in the text and missing from that list. \
         Drop greetings, dialog scaffolding (Speaker:, > quotes), acknowledgements. \
         fact_type rules: decision = 'we will / decided / chose'; pattern = 'X uses Y for Z' / 'X is implemented with Y'; convention = 'always / never / format X as'; claim = factual assertion about external state; note = passive observation / FYI; question = ends with '?' or is an open investigation. \
         confidence: 0.9 for explicit decisions / clear claims, 0.6 for inferences from context, 0.3 for vague / fragmentary mentions."
    );
    serde_json::json!({
        "model": model,
        "response_format": {"type": "json_object"},
        "max_completion_tokens": max_tokens,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": text},
        ],
    })
}

/// Minimal HTTP POST. We use `ureq` indirectly via the existing `aidememo-cli`
/// dep tree if compiled together, but `aidememo-core` cannot pull a heavy
/// HTTP client just for this opt-in path — so we reach for the
/// std-library + reqwest-blocking-style pattern through `ureq`'s
/// transitive presence in the workspace. Without `ureq`, we fall back
/// to the std `std::net::TcpStream` route via the same trick used in
/// `rerank.rs`.
#[cfg(feature = "semantic")]
fn post_chat_completion(url: &str, api_key: &str, body: &serde_json::Value) -> Result<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .build();
    let mut req = agent.post(url);
    if !api_key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", api_key));
    }
    req.set("Content-Type", "application/json")
        .send_json(body.clone())
        .map_err(|e| {
            crate::error::AideMemoError::invalid_input(format!("LLM extract POST failed: {e}"))
        })?
        .into_string()
        .map_err(|e| {
            crate::error::AideMemoError::invalid_input(format!("LLM extract read failed: {e}"))
        })
}

/// Parse the OpenAI response envelope and the embedded JSON object.
#[cfg(feature = "semantic")]
fn parse_llm_response(
    raw: &str,
    entity_names: &[String],
    cap: usize,
) -> Result<Vec<ExtractCandidate>> {
    let env: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        crate::error::AideMemoError::invalid_input(format!(
            "LLM extract envelope parse failed: {e}; raw={}",
            &raw[..raw.len().min(200)]
        ))
    })?;
    let content = env["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            crate::error::AideMemoError::invalid_input(format!(
                "LLM extract: missing choices[0].message.content; raw={}",
                &raw[..raw.len().min(200)]
            ))
        })?;
    // Reasoning models (MiniMax / DeepSeek-R1 / Qwen-thinking) prefix the
    // answer with a <think>…</think> block. Strip it so json parsing
    // sees only the final JSON payload.
    let content_stripped = if let Some(idx) = content.find("</think>") {
        content[idx + "</think>".len()..].trim()
    } else {
        content.trim()
    };
    let payload: serde_json::Value = serde_json::from_str(content_stripped).map_err(|e| {
        crate::error::AideMemoError::invalid_input(format!(
            "LLM extract content not valid JSON: {e}; content={}",
            &content_stripped[..content_stripped.len().min(200)]
        ))
    })?;
    let arr = payload["facts"].as_array().cloned().unwrap_or_default();

    // Lower-case set for entity normalisation — agent-supplied names
    // come back from the LLM in mixed case.
    let lc: std::collections::HashSet<String> =
        entity_names.iter().map(|n| n.to_lowercase()).collect();
    let mut out = Vec::with_capacity(arr.len().min(cap));
    for item in arr.into_iter().take(cap) {
        let content = item["content"].as_str().unwrap_or("").trim().to_string();
        if content.is_empty() {
            continue;
        }
        let fact_type = match FactType::parse(item["fact_type"].as_str().unwrap_or("note")) {
            FactType::Unknown => FactType::Note,
            t => t,
        };
        let entities_raw = item["entities"].as_array().cloned().unwrap_or_default();
        // Filter to only entities the wiki already knows OR that the
        // LLM clearly extracted from the text. Casing comes back from
        // the model normalised, but we map back to the canonical
        // entity_names spelling when available.
        let mut entities: Vec<String> = Vec::new();
        for e in entities_raw {
            let Some(name) = e.as_str() else { continue };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if let Some(canonical) = entity_names.iter().find(|n| n.eq_ignore_ascii_case(name)) {
                if !entities.contains(canonical) {
                    entities.push(canonical.clone());
                }
            } else if !lc.contains(&name.to_lowercase()) {
                // Net-new entity from the text; keep the LLM's spelling.
                if !entities.iter().any(|x| x.eq_ignore_ascii_case(name)) {
                    entities.push(name.to_string());
                }
            }
        }
        let confidence = item["confidence"]
            .as_f64()
            .map(|f: f64| f.clamp(0.0, 1.0) as f32)
            .unwrap_or(0.5_f32);
        out.push(ExtractCandidate {
            content,
            suggested_entities: entities,
            suggested_fact_type: fact_type,
            confidence,
        });
    }
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sentences_handles_punctuation_and_newlines() {
        let text = "We decided to use Redis. It's faster.\nNext step: deploy.";
        let sentences = split_sentences(text);
        assert_eq!(sentences.len(), 3);
        assert!(sentences[0].contains("Redis"));
        assert!(sentences[2].contains("Next step"));
    }

    #[test]
    fn split_sentences_strips_dialog_markers() {
        let text = "> we decided to use Postgres\nAlice: Postgres has VACUUM";
        let sentences = split_sentences(text);
        assert_eq!(sentences[0], "we decided to use Postgres");
        // "Alice:" stripped, leading "Postgres ..." remains.
        assert!(sentences[1].starts_with("Postgres has"));
    }

    #[test]
    fn split_sentences_keeps_colon_prefixed_keywords() {
        // "decision:" is content, not speaker — must not be stripped.
        let text = "decision: use Postgres";
        let sentences = split_sentences(text);
        assert_eq!(sentences[0], "decision: use Postgres");
    }

    #[test]
    fn detect_fact_type_finds_decisions_and_conventions() {
        assert_eq!(
            detect_fact_type("we decided to use redis for hot caching"),
            FactType::Decision
        );
        assert_eq!(
            detect_fact_type("we always run lints before merging"),
            FactType::Convention
        );
        assert_eq!(
            detect_fact_type("the antipattern here is reading after writing"),
            FactType::Pattern
        );
        assert_eq!(
            detect_fact_type("why does the cache miss after restart?"),
            FactType::Question
        );
        assert_eq!(
            detect_fact_type("we believe latency comes from network hops"),
            FactType::Claim
        );
        assert_eq!(
            detect_fact_type("the deploy ran fine on staging"),
            FactType::Note
        );
    }

    #[test]
    fn score_candidate_drops_short_garbage() {
        let entities: Vec<String> = vec![];
        assert!(score_candidate("ok", &entities).is_none());
        assert!(score_candidate("", &entities).is_none());
    }

    #[test]
    fn score_candidate_boosts_with_entity_match() {
        let entities = vec!["Redis".to_string()];
        let with = score_candidate("we decided to use Redis for caching", &entities).unwrap();
        let without = score_candidate(
            "we decided to use the in-memory store for caching",
            &entities,
        )
        .unwrap();
        assert!(with.confidence > without.confidence);
        assert_eq!(with.suggested_entities, vec!["Redis"]);
        assert_eq!(with.suggested_fact_type, FactType::Decision);
    }

    #[cfg(feature = "redb")]
    #[test]
    fn extract_candidates_sorts_by_confidence() {
        // Use a temp store so we can populate it with one entity then
        // run extraction. Top result should be the decision sentence
        // mentioning the entity.
        use crate::config::Config;
        use crate::store::Store;
        use crate::types::{EntityInput, EntityType};
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.store.path = dir.path().join("store").to_string_lossy().into_owned();
        let path: std::path::PathBuf = (&config.store.path).into();
        let mut store = Store::open(&path, config).unwrap();
        store
            .entity_add(EntityInput {
                name: "Postgres".into(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        let text = "We decided to use Postgres for hot writes.\n\
                    Some other unrelated chatter.\n\
                    The deploy ran on staging.";
        let candidates = extract_candidates(text, &store, 5).unwrap();
        assert!(!candidates.is_empty());
        // Highest-confidence candidate must be the decision-with-entity one.
        let top = &candidates[0];
        assert!(top.suggested_entities.contains(&"Postgres".to_string()));
        assert_eq!(top.suggested_fact_type, FactType::Decision);
    }
}
