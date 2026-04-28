//! Wiki ingest engine — parses markdown files and extracts entities, relations, and facts.
//!
//! Ingest pipeline:
//! 1. Walk wiki root directory for `.md` files
//! 2. Parse frontmatter (type, tags, aliases)
//! 3. Extract `[[wikilink]]` references → relations
//! 4. Extract heading-based facts (## Decision:, ## Convention:, etc.)
//! 5. Write to store

use memchr::memchr;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::error::WgError;
use crate::relations::{TypedRelation, extract_typed_relations};
use crate::store::Store;
use crate::types::{EntityInput, EntityType, FactInput, FactType, RelationInput, RelationType};

/// Result of a complete ingest operation.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct IngestStats {
    pub entities_added: u64,
    pub entities_updated: u64,
    pub relations_added: u64,
    pub facts_added: u64,
    pub files_scanned: u64,
    pub errors: Vec<String>,
}

/// A parsed wikilink: `[[Target]]` or `[[Target|Display]]`.
#[derive(Debug, Clone)]
pub struct Wikilink {
    /// The link target (e.g. "Redis").
    pub target: String,
    /// The display text, if different (e.g. "레디스"). None if `[[Target]]`.
    pub display: Option<String>,
    /// Line number where this link appears (1-indexed).
    pub line: usize,
}

/// A parsed markdown file.
#[derive(Debug, Clone)]
pub struct ParsedFile {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// Relative path from wiki root (e.g. "entities/redis.md").
    pub rel_path: String,
    /// Frontmatter: entity type (derived from `type:` field or directory).
    pub entity_type: Option<EntityType>,
    /// Frontmatter: tags.
    pub tags: Vec<String>,
    /// Frontmatter: aliases.
    pub aliases: Vec<String>,
    /// Frontmatter: when the fact(s) in this file were actually observed/decided
    /// (epoch ms). Parsed from `date`, `decided_at`, or `observed_at` keys.
    /// Applied to all facts extracted from this file unless overridden.
    pub observed_at: Option<u64>,
    /// Extracted wikilinks.
    pub wikilinks: Vec<Wikilink>,
    /// Typed relations extracted from prose patterns (zero-LLM).
    pub typed_relations: Vec<TypedRelation>,
    /// Heading-anchored sections → fact candidates.
    pub sections: Vec<Section>,
    /// The raw body text (without frontmatter).
    pub body: String,
}

/// A heading-anchored section in a markdown file.
#[derive(Debug, Clone)]
pub struct Section {
    /// Heading text (e.g. "## HA Strategy").
    pub heading: String,
    /// Anchor slug (e.g. "ha-strategy").
    pub anchor: String,
    /// Content under this heading (until next heading or EOF).
    pub content: String,
    /// The fact type inferred from the heading prefix.
    pub fact_type: FactType,
    /// 1-indexed line number where this heading appears.
    pub line: usize,
}

/// Perform a full ingest of the wiki at `wiki_root` into `store`.
///
/// Setting `incremental` to true is currently equivalent to a full ingest
/// (mtime-based incremental logic is planned for a future release).
pub fn ingest_wiki(
    wiki_root: &Path,
    store: &mut Store,
    _incremental: bool,
) -> Result<IngestStats, WgError> {
    let wiki_root = wiki_root.to_path_buf();
    let mut stats = IngestStats::default();

    // Collect all .md files
    let md_files: Vec<PathBuf> = WalkDir::new(&wiki_root)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "md"))
        .map(|e| e.path().to_path_buf())
        .collect();

    stats.files_scanned = md_files.len() as u64;

    for file_path in md_files {
        match process_file(&file_path, &wiki_root) {
            Ok(parsed) => {
                let entity_name = parsed
                    .rel_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&parsed.rel_path)
                    .trim_end_matches(".md")
                    .to_string();

                // --- Entity ---
                let entity_input = EntityInput {
                    name: entity_name.clone(),
                    entity_type: parsed.entity_type,
                    aliases: if parsed.aliases.is_empty() {
                        None
                    } else {
                        Some(parsed.aliases)
                    },
                    tags: if parsed.tags.is_empty() {
                        None
                    } else {
                        Some(parsed.tags)
                    },
                    source_page: Some(parsed.rel_path.clone()),
                };

                let entity_id = match store.entity_get(&entity_name) {
                    Ok(_) => {
                        // entity already exists — skip (don't overwrite user data)
                        stats.entities_updated += 1;
                        // Get the existing entity's ID
                        store.entity_get(&entity_name)?.id
                    }
                    Err(_) => {
                        // create new entity
                        stats.entities_added += 1;
                        store.entity_add(entity_input)?
                    }
                };

                // --- Typed relations from prose patterns ---
                // Track which (source, target) pairs the typed extractor produced
                // so we can skip the catch-all `references` edge for them.
                let mut typed_pairs: std::collections::HashSet<String> =
                    std::collections::HashSet::new();

                // Helper: ensure an entity exists (auto-create as Unknown if missing).
                let ensure_entity = |store: &mut Store, name: &str, stats: &mut IngestStats| {
                    if store.resolve_entity(name).is_err() {
                        let _ = store.entity_add(EntityInput {
                            name: name.to_string(),
                            entity_type: Some(EntityType::Unknown),
                            ..Default::default()
                        });
                        stats.entities_added += 1;
                    }
                };

                for tr in &parsed.typed_relations {
                    ensure_entity(store, &tr.source, &mut stats);
                    ensure_entity(store, &tr.target, &mut stats);
                    let rel = RelationInput {
                        source: tr.source.clone(),
                        target: tr.target.clone(),
                        relation_type: RelationType::new(tr.relation_type.clone()),
                        weight: None,
                        evidence: Some(vec![format!("{}:{}", parsed.rel_path, tr.evidence)]),
                    };
                    if store.relation_add(rel).is_ok() {
                        stats.relations_added += 1;
                        typed_pairs.insert(format!("{}\0{}", tr.source, tr.target));
                    }
                }

                // --- Fallback relations from any wikilink not already typed ---
                for wl in &parsed.wikilinks {
                    let key = format!("{}\0{}", entity_name, wl.target);
                    if typed_pairs.contains(&key) {
                        continue;
                    }
                    let rel = RelationInput {
                        source: entity_name.clone(),
                        target: wl.target.clone(),
                        relation_type: RelationType::new("references"),
                        weight: None,
                        evidence: Some(vec![format!("{}:{}", parsed.rel_path, wl.line)]),
                    };
                    if store.relation_add(rel).is_ok() {
                        stats.relations_added += 1;
                    }
                }

                // --- Facts from sections ---
                for section in &parsed.sections {
                    // Only add sections with a meaningful fact type (skip "Unknown")
                    if section.fact_type == FactType::Unknown {
                        continue;
                    }
                    let source = Some(format!("{}#{}", parsed.rel_path, section.anchor));
                    let fact_input = FactInput {
                        content: section.content.clone(),
                        fact_type: Some(section.fact_type),
                        entity_ids: Some(vec![entity_id]),
                        tags: None,
                        source,
                        source_confidence: Some(0.5), // auto-extracted
                        observed_at: parsed.observed_at,
                    };
                    let _ = store.fact_add(fact_input);
                    stats.facts_added += 1;
                }
            }
            Err(e) => {
                stats.errors.push(format!("{}: {}", file_path.display(), e));
            }
        }
    }

    // Update last_ingest_at in meta (store-level tracking)
    let _ = store.set_last_ingest_at();

    Ok(stats)
}

// ---------------------------------------------------------------------------
// File parsing
// ---------------------------------------------------------------------------

fn process_file(file_path: &Path, wiki_root: &Path) -> Result<ParsedFile, WgError> {
    let content = std::fs::read_to_string(file_path)
        .map_err(|e| WgError::FileRead(file_path.to_path_buf(), e.to_string()))?;

    let rel_path = file_path
        .strip_prefix(wiki_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .replace('\\', "/");

    // Split frontmatter + body
    let (frontmatter, body) = extract_frontmatter(&content);

    // Parse frontmatter
    let (entity_type, tags, aliases, observed_at) = parse_frontmatter(&frontmatter, &rel_path);

    // Infer entity_type from directory if not set in frontmatter
    let entity_type = entity_type.or_else(|| infer_entity_type_from_path(&rel_path));

    // Parse wikilinks, typed relations, and sections
    let wikilinks = extract_wikilinks(&body);
    let typed_relations = extract_typed_relations(&body);
    let sections = extract_sections(&body);

    Ok(ParsedFile {
        path: file_path.to_path_buf(),
        rel_path,
        entity_type,
        tags,
        aliases,
        observed_at,
        wikilinks,
        typed_relations,
        sections,
        body,
    })
}

// ---------------------------------------------------------------------------
// Frontmatter
// ---------------------------------------------------------------------------

/// Split frontmatter from body. Returns (frontmatter_text, body_text).
/// If no frontmatter, returns (empty, original_content).
fn extract_frontmatter(content: &str) -> (String, String) {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return (String::new(), content.to_string());
    }

    // Find the closing `---`
    if let Some(end) = content[3..].find("\n---") {
        let fm = content[3..end + 3].to_string();
        let body = content[end + 6..].trim_start().to_string();
        (fm, body)
    } else {
        (String::new(), content.to_string())
    }
}

/// Parse frontmatter key:value pairs.
/// Supports: type, tags, aliases, date/decided_at/observed_at.
///
/// Date keys are parsed as ISO 8601 (YYYY-MM-DD or RFC3339) and converted to epoch ms.
/// First non-empty date key wins, in priority order: `observed_at`, `decided_at`, `date`.
fn parse_frontmatter(
    frontmatter: &str,
    _rel_path: &str,
) -> (Option<EntityType>, Vec<String>, Vec<String>, Option<u64>) {
    let mut entity_type = None;
    let mut tags = Vec::new();
    let mut aliases = Vec::new();
    let mut date: Option<u64> = None;
    let mut decided_at: Option<u64> = None;
    let mut observed_at: Option<u64> = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim();
            // Strip surrounding quotes that YAML often adds for date strings.
            let val = val.trim().trim_matches('"').trim_matches('\'').trim();

            match key {
                "type" => {
                    entity_type = parse_entity_type(val);
                }
                "tags" => {
                    tags = parse_list_field(val);
                }
                "aliases" => {
                    aliases = parse_list_field(val);
                }
                "date" => {
                    date = parse_iso8601_to_epoch_ms(val);
                }
                "decided_at" => {
                    decided_at = parse_iso8601_to_epoch_ms(val);
                }
                "observed_at" => {
                    observed_at = parse_iso8601_to_epoch_ms(val);
                }
                _ => {}
            }
        }
    }

    let observed = observed_at.or(decided_at).or(date);

    (entity_type, tags, aliases, observed)
}

/// Parse a frontmatter date string into epoch milliseconds.
///
/// Accepts:
/// - `YYYY-MM-DD` (interpreted as 00:00:00 UTC)
/// - RFC3339 (`2024-03-15T10:00:00Z`, `2024-03-15T10:00:00+09:00`)
///
/// Returns `None` if the string can't be parsed; ingest should skip the field
/// rather than fail the file.
fn parse_iso8601_to_epoch_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Try RFC3339 first (most specific).
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        let ms = dt.timestamp_millis();
        return u64::try_from(ms).ok();
    }

    // Fall back to date-only.
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0)?.and_utc();
        let ms = dt.timestamp_millis();
        return u64::try_from(ms).ok();
    }

    None
}

fn parse_entity_type(s: &str) -> Option<EntityType> {
    match s.to_lowercase().as_str() {
        "entity" | "technology" => Some(EntityType::Technology),
        "concept" => Some(EntityType::Concept),
        "comparison" => Some(EntityType::Comparison),
        "query" => Some(EntityType::Query),
        "person" => Some(EntityType::Person),
        "team" => Some(EntityType::Team),
        _ => None,
    }
}

fn parse_list_field(s: &str) -> Vec<String> {
    // Handles: [tag1, tag2, tag3] or tag1, tag2, tag3
    let s = s.trim();
    if s.starts_with('[') {
        s.trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\''))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    } else {
        s.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Infer entity type from directory path.
/// e.g. "entities/redis.md" → Technology
///      "concepts/cache.md"  → Concept
fn infer_entity_type_from_path(rel_path: &str) -> Option<EntityType> {
    let first = rel_path.split('/').next()?;
    match first {
        "entities" => Some(EntityType::Technology),
        "concepts" => Some(EntityType::Concept),
        "comparisons" => Some(EntityType::Comparison),
        "people" => Some(EntityType::Person),
        "teams" => Some(EntityType::Team),
        _ => Some(EntityType::Unknown),
    }
}

// ---------------------------------------------------------------------------
// Wikilinks
// ---------------------------------------------------------------------------

/// Extract all [[wikilink]] references from text.
fn extract_wikilinks(text: &str) -> Vec<Wikilink> {
    let mut wikilinks = Vec::new();

    for (line_num, line) in text.lines().enumerate() {
        let line_num = line_num + 1; // 1-indexed

        // Find all [[ ... ]] spans
        let mut start = 0;
        while let Some(open) = memchr(b'[', &line.as_bytes()[start..]) {
            let abs = start + open;
            if line[abs..].starts_with("[[") {
                if let Some(close) = memchr(b']', &line.as_bytes()[abs + 2..]) {
                    let abs_close = abs + 2 + close;
                    let inner = &line[abs + 2..abs_close];

                    // Check for display text separator: [[Target|Display]]
                    let (target, display) = if let Some(pipe) = inner.find('|') {
                        (
                            inner[..pipe].trim().to_string(),
                            Some(inner[pipe + 1..].trim().to_string()),
                        )
                    } else {
                        (inner.trim().to_string(), None)
                    };

                    if !target.is_empty() {
                        wikilinks.push(Wikilink {
                            target,
                            display,
                            line: line_num,
                        });
                    }

                    start = abs_close + 1;
                } else {
                    break;
                }
            } else {
                start = abs + 1;
            }
        }
    }

    wikilinks
}

// ---------------------------------------------------------------------------
// Sections / heading facts
// ---------------------------------------------------------------------------

/// Fact type inference from heading text.
fn infer_fact_type(heading: &str) -> FactType {
    let h = heading.to_lowercase();

    if h.contains("decision")
        || h.contains("결정")
        || h.contains("전략")
        || h.contains("strategy")
        || h.contains("strategic")
    {
        FactType::Decision
    } else if h.contains("convention") || h.contains("규칙") || h.contains("표준") {
        FactType::Convention
    } else if h.contains("pattern") || h.contains("패턴") || h.contains("방법") {
        FactType::Pattern
    } else if h.contains("note") || h.contains("메모") || h.contains("참고") {
        FactType::Note
    } else if h.contains("question") || h.contains("질문") || h.contains("?") {
        FactType::Question
    } else if h.contains("claim") || h.contains("주장") {
        FactType::Claim
    } else {
        FactType::Unknown
    }
}

/// Normalize a heading to an anchor slug.
/// "## HA Strategy" → "ha-strategy"
fn heading_to_anchor(heading: &str) -> String {
    heading
        .trim_start_matches('#')
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

/// Extract heading-anchored sections from markdown body.
fn extract_sections(body: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut current_heading = String::new();
    let mut current_anchor = String::new();
    let mut current_fact_type = FactType::Unknown;
    let mut current_content = String::new();
    let mut current_line = 0;
    let mut line_num = 0;

    for line in body.lines() {
        line_num += 1;

        let trimmed = line.trim();

        // Detect ATX heading (## Text)
        if trimmed.starts_with("##") {
            // Emit previous section
            if !current_heading.is_empty() && !current_content.trim().is_empty() {
                sections.push(Section {
                    heading: current_heading.clone(),
                    anchor: current_anchor.clone(),
                    content: current_content.trim().to_string(),
                    fact_type: current_fact_type,
                    line: current_line,
                });
            }

            // Start new section
            current_heading = trimmed.trim_start_matches('#').trim().to_string();
            current_anchor = heading_to_anchor(&current_heading);
            current_fact_type = infer_fact_type(&current_heading);
            current_content.clear();
            current_line = line_num;
        } else if !current_heading.is_empty() {
            if !current_content.is_empty() {
                current_content.push('\n');
            }
            current_content.push_str(line);
        }
    }

    if !current_heading.is_empty() && !current_content.trim().is_empty() {
        sections.push(Section {
            heading: current_heading,
            anchor: current_anchor,
            content: current_content.trim().to_string(),
            fact_type: current_fact_type,
            line: current_line,
        });
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_frontmatter() {
        let content = r#"---
type: technology
tags: [infra, cache]
aliases: [redis-server]
---
# Redis
Some body text.
"#;
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.contains("type:"));
        assert!(body.contains("Redis"));
    }

    #[test]
    fn test_extract_wikilinks() {
        let text = "See [[Redis]] and [[Redis Sentinel|Sentinel]] for details.";
        let links = extract_wikilinks(text);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "Redis");
        assert_eq!(links[1].target, "Redis Sentinel");
        assert_eq!(links[1].display, Some("Sentinel".to_string()));
    }

    #[test]
    fn test_extract_sections() {
        let body = r#"# Redis

Some intro.

## HA Strategy

We use Sentinel for failover.

## Configuration

port: 6379
"#;
        let sections = extract_sections(body);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "HA Strategy");
        assert_eq!(sections[0].fact_type, FactType::Decision);
        assert!(sections[0].content.contains("Sentinel"));
        assert_eq!(sections[1].heading, "Configuration");
    }

    #[test]
    fn test_heading_to_anchor() {
        assert_eq!(heading_to_anchor("## HA Strategy"), "ha-strategy");
        assert_eq!(heading_to_anchor("# Redis Cache"), "redis-cache");
    }

    #[test]
    fn test_parse_frontmatter_tags() {
        let fm = r#"type: concept
tags: [caching, performance]
aliases: cache, memory-cache"#;
        let (et, tags, aliases, observed) = parse_frontmatter(fm, "");
        assert!(matches!(et, Some(EntityType::Concept)));
        assert_eq!(tags, vec!["caching", "performance"]);
        assert_eq!(aliases, vec!["cache", "memory-cache"]);
        assert!(observed.is_none());
    }

    #[test]
    fn test_parse_frontmatter_date_only() {
        let fm = "type: technology\ndate: 2024-03-15";
        let (_, _, _, observed) = parse_frontmatter(fm, "");
        // 2024-03-15T00:00:00Z = 1710460800 seconds = 1710460800000 ms
        assert_eq!(observed, Some(1_710_460_800_000));
    }

    #[test]
    fn test_parse_frontmatter_decided_at_overrides_date() {
        let fm = "date: 2024-01-01\ndecided_at: 2024-06-15";
        let (_, _, _, observed) = parse_frontmatter(fm, "");
        // decided_at wins over date
        let decided = parse_iso8601_to_epoch_ms("2024-06-15");
        assert_eq!(observed, decided);
    }

    #[test]
    fn test_parse_frontmatter_observed_at_wins() {
        let fm = "date: 2024-01-01\ndecided_at: 2024-06-15\nobserved_at: 2024-12-31";
        let (_, _, _, observed) = parse_frontmatter(fm, "");
        let expected = parse_iso8601_to_epoch_ms("2024-12-31");
        assert_eq!(observed, expected);
    }

    #[test]
    fn test_parse_frontmatter_quoted_date() {
        let fm = "date: \"2024-03-15\"";
        let (_, _, _, observed) = parse_frontmatter(fm, "");
        assert_eq!(observed, Some(1_710_460_800_000));
    }

    #[test]
    fn test_parse_frontmatter_rfc3339() {
        let fm = "observed_at: 2024-03-15T10:30:00Z";
        let (_, _, _, observed) = parse_frontmatter(fm, "");
        // 2024-03-15T10:30:00Z = 1710498600 seconds
        assert_eq!(observed, Some(1_710_498_600_000));
    }

    #[test]
    fn test_parse_frontmatter_invalid_date_yields_none() {
        let fm = "date: not-a-date";
        let (_, _, _, observed) = parse_frontmatter(fm, "");
        assert!(observed.is_none());
    }

    #[test]
    fn test_parse_iso8601_handles_timezone() {
        // KST (+09:00) — 2024-03-15T09:00:00+09:00 == 2024-03-15T00:00:00Z
        let ms = parse_iso8601_to_epoch_ms("2024-03-15T09:00:00+09:00").unwrap();
        assert_eq!(ms, 1_710_460_800_000);
    }
}
