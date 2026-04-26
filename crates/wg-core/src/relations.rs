//! Typed-relation pattern extraction from markdown prose.
//!
//! Inspired by gbrain's "zero-LLM auto-linking" approach. We scan the body
//! for `[[X]] <verb-phrase> [[Y]]` patterns and emit typed relations
//! (`works_at`, `depends_on`, `supersedes`, …) instead of the catch-all
//! `references` edge.
//!
//! Design choices:
//! - Pattern matching only — no LLM, no embedding.
//! - Each phrase maps to one relation type. Direction is preserved as-written
//!   (e.g. "owns" and "owned by" are *different* relation types, not the
//!   same edge with a flipped flag).
//! - Patterns match on whole-word boundaries inside the gap text between
//!   two consecutive wikilinks. Gaps over `MAX_GAP_CHARS` are skipped.
//! - Longest phrase wins ("invested in" beats "in").
//! - Falls back to `references` (current behavior) when nothing matches —
//!   the caller decides whether to emit that fallback or skip.

use crate::types::{RelationInput, RelationType};

/// A typed relation extracted from prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedRelation {
    /// Source entity (the link that appears first in the prose).
    pub source: String,
    /// Target entity (the link that appears second).
    pub target: String,
    /// Relation type, e.g. `works_at`, `depends_on`. Lowercase snake_case.
    pub relation_type: String,
    /// Source line + the literal gap text, useful for citation/evidence.
    pub evidence: String,
}

impl TypedRelation {
    /// Convert into the `RelationInput` shape used by the store.
    pub fn into_input(self) -> RelationInput {
        RelationInput {
            source: self.source,
            target: self.target,
            relation_type: RelationType::new(self.relation_type),
            weight: None,
            evidence: Some(vec![self.evidence]),
        }
    }
}

const MAX_GAP_CHARS: usize = 80;

/// Built-in phrase → relation_type table. Each phrase is matched with whole-word
/// boundaries; the longest matching phrase wins. Add domain-specific patterns
/// via `extract_typed_relations_with` and a custom slice.
pub const DEFAULT_PATTERNS: &[(&str, &str)] = &[
    // Tech / dependencies
    ("depends on", "depends_on"),
    ("depend on", "depends_on"),
    ("requires", "depends_on"),
    ("uses", "uses"),
    ("use", "uses"),
    ("implements", "implements"),
    ("extends", "extends"),
    ("supersedes", "supersedes"),
    ("replaces", "supersedes"),
    ("alternative to", "alternative_to"),
    ("blocks", "blocks"),
    ("blocked by", "blocked_by"),
    // Ownership / org
    ("owns", "owns"),
    ("owned by", "owned_by"),
    ("works at", "works_at"),
    ("works for", "works_at"),
    ("founded by", "founded_by"),
    ("founded", "founded"),
    ("manages", "manages"),
    ("managed by", "managed_by"),
    ("reports to", "reports_to"),
    // Decision provenance
    ("decided by", "decided_by"),
    ("documented in", "documented_in"),
    ("authored by", "authored_by"),
    // Investment / advisory (people-graph)
    ("invested in", "invested_in"),
    ("invests in", "invested_in"),
    ("advises", "advises"),
    ("advised by", "advised_by"),
    ("attended", "attended"),
    ("attends", "attended"),
    // Hierarchy
    ("parent of", "parent_of"),
    ("child of", "child_of"),
    ("part of", "part_of"),
    ("contains", "contains"),
    // Generic fallback when prose is non-committal but explicit
    ("related to", "related_to"),
    ("see also", "related_to"),
];

/// Span of a `[[wikilink]]` in the source body.
#[derive(Debug)]
struct LinkSpan {
    target: String,
    start: usize,
    end: usize,
    line: usize,
}

fn scan_link_spans(body: &str) -> Vec<LinkSpan> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            // find matching `]]`
            if let Some(rel_end) = body[i + 2..].find("]]") {
                let end = i + 2 + rel_end + 2;
                let inside = &body[i + 2..end - 2];
                if let Some(target) = inside.split('|').next() {
                    let target = target.trim();
                    if !target.is_empty() && !target.contains('\n') {
                        out.push(LinkSpan {
                            target: target.to_string(),
                            start: i,
                            end,
                            line,
                        });
                    }
                }
                // count newlines we skipped past
                let skipped = &body[i..end];
                line += skipped.matches('\n').count();
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Words that signal a clause/subject change. If one of these appears in the
/// gap *before* the candidate verb, we don't emit a relation for the pair —
/// the second link belongs to a different clause and probably has a different
/// subject than the first link.
const CLAUSE_BREAKERS: &[&str] = &[
    "and", "or", "but", "then", "so", "while", "however", "although", "yet", "because",
];

/// Match a gap (as normalized lowercase words) against the pattern table.
///
/// Rules:
/// - The pattern's words must appear contiguously in the gap.
/// - Anything before the matched window must NOT contain a clause breaker
///   (`and`, `or`, …) — otherwise the second link is in a different clause
///   and we shouldn't emit a relation for this pair.
/// - Longest matching phrase wins.
fn match_pattern<'p>(gap_words: &[&str], patterns: &'p [(&'p str, &'p str)]) -> Option<&'p str> {
    if gap_words.is_empty() {
        return None;
    }

    let mut best: Option<(usize, &'p str)> = None;
    for (phrase, relation) in patterns {
        let phrase_words: Vec<&str> = phrase.split_whitespace().collect();
        if phrase_words.is_empty() || phrase_words.len() > gap_words.len() {
            continue;
        }
        for start in 0..=(gap_words.len() - phrase_words.len()) {
            // Refuse to match across a clause boundary.
            if gap_words[..start]
                .iter()
                .any(|w| CLAUSE_BREAKERS.contains(w))
            {
                break;
            }
            if &gap_words[start..start + phrase_words.len()] == phrase_words.as_slice() {
                let plen = phrase_words.len();
                if best.map_or(true, |(b, _)| plen > b) {
                    best = Some((plen, *relation));
                }
                break;
            }
        }
    }
    best.map(|(_, rel)| rel)
}

/// Normalize a gap-text into lowercase whitespace-collapsed word list,
/// stripping leading/trailing punctuation per word.
fn normalize_gap(gap: &str) -> Vec<&str> {
    gap.split(|c: char| c.is_whitespace() || (c.is_ascii_punctuation() && c != '_' && c != '-'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Extract typed relations from the body using the default pattern set.
pub fn extract_typed_relations(body: &str) -> Vec<TypedRelation> {
    extract_typed_relations_with(body, DEFAULT_PATTERNS)
}

/// Extract typed relations from the body using a caller-supplied pattern set.
///
/// Pairs each consecutive `[[A]] [[B]]` whose gap is ≤ `MAX_GAP_CHARS` and
/// runs the pattern table over the lowercase normalized gap words.
pub fn extract_typed_relations_with<'p>(
    body: &str,
    patterns: &'p [(&'p str, &'p str)],
) -> Vec<TypedRelation> {
    let spans = scan_link_spans(body);
    if spans.len() < 2 {
        return Vec::new();
    }

    // Build a lowercase shadow once for word matching.
    let body_lower = body.to_lowercase();
    let mut out = Vec::new();
    for pair in spans.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        if b.start <= a.end {
            continue;
        }
        let gap_raw = &body[a.end..b.start];
        if gap_raw.len() > MAX_GAP_CHARS {
            continue;
        }
        // Skip pairs separated by a blank line.
        if gap_raw.contains("\n\n") {
            continue;
        }
        let gap_lower = &body_lower[a.end..b.start];
        let words = normalize_gap(gap_lower);
        if let Some(rel) = match_pattern(&words, patterns) {
            out.push(TypedRelation {
                source: a.target.clone(),
                target: b.target.clone(),
                relation_type: rel.to_string(),
                evidence: format!("line {}: {}", a.line, gap_raw.trim()),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn types(rels: &[TypedRelation]) -> Vec<(&str, &str, &str)> {
        rels.iter()
            .map(|r| {
                (
                    r.source.as_str(),
                    r.relation_type.as_str(),
                    r.target.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn no_links_no_relations() {
        assert!(extract_typed_relations("Plain text, no links.").is_empty());
    }

    #[test]
    fn single_link_no_pair_to_match() {
        assert!(extract_typed_relations("[[Alice]] is interesting.").is_empty());
    }

    #[test]
    fn simple_works_at() {
        let rels = extract_typed_relations("[[Alice]] works at [[Acme]].");
        assert_eq!(types(&rels), vec![("Alice", "works_at", "Acme")]);
    }

    #[test]
    fn distinct_directions_owns_vs_owned_by() {
        let rels = extract_typed_relations(
            "[[Acme]] owns [[BuildingA]]. [[BuildingB]] owned by [[Acme]].",
        );
        assert_eq!(
            types(&rels),
            vec![
                ("Acme", "owns", "BuildingA"),
                ("BuildingB", "owned_by", "Acme"),
            ]
        );
    }

    #[test]
    fn longest_phrase_wins() {
        // "invested in" should beat "in"
        let rels = extract_typed_relations("[[Alice]] invested in [[Acme]].");
        assert_eq!(types(&rels), vec![("Alice", "invested_in", "Acme")]);
    }

    #[test]
    fn intermediate_words_allowed() {
        // "Alice works at the new Acme office" — articles between phrase and link
        let rels = extract_typed_relations("[[Alice]] works at the new [[Acme]] office.");
        assert_eq!(types(&rels), vec![("Alice", "works_at", "Acme")]);
    }

    #[test]
    fn gap_too_large_skipped() {
        // 100+ chars between links — no match emitted
        let big_gap = "x".repeat(120);
        let body = format!("[[Alice]] {} works at [[Acme]].", big_gap);
        assert!(extract_typed_relations(&body).is_empty());
    }

    #[test]
    fn blank_line_breaks_pairing() {
        // Two paragraphs — no cross-paragraph relations
        let body = "[[Alice]] is great.\n\nworks at [[Acme]].";
        assert!(extract_typed_relations(body).is_empty());
    }

    #[test]
    fn no_pattern_no_relation() {
        // Two links with non-pattern prose between → no typed relation
        let rels = extract_typed_relations("[[Alice]] thought about [[Acme]] briefly.");
        assert!(rels.is_empty());
    }

    #[test]
    fn case_insensitive() {
        let rels = extract_typed_relations("[[Alice]] WORKS AT [[Acme]].");
        assert_eq!(types(&rels), vec![("Alice", "works_at", "Acme")]);
    }

    #[test]
    fn three_links_produces_two_pairs() {
        let body = "[[Alice]] works at [[Acme]] which uses [[Redis]].";
        let rels = extract_typed_relations(body);
        assert_eq!(
            types(&rels),
            vec![("Alice", "works_at", "Acme"), ("Acme", "uses", "Redis"),]
        );
    }

    #[test]
    fn clause_boundary_blocks_false_pair() {
        // Acme is not "and reports to Bob"; the conjunction breaks the pair.
        let body = "[[Alice]] works at [[Acme]] and reports to [[Bob]].";
        let rels = extract_typed_relations(body);
        assert_eq!(types(&rels), vec![("Alice", "works_at", "Acme")]);
    }

    #[test]
    fn relative_clause_keeps_pair() {
        // "which uses" — "which" is filler-ish, NOT a clause breaker.
        let body = "[[Acme]] which uses [[Redis]].";
        let rels = extract_typed_relations(body);
        assert_eq!(types(&rels), vec![("Acme", "uses", "Redis")]);
    }

    #[test]
    fn alias_form_target_extraction() {
        // [[Real|Display]] should target "Real", not "Display"
        let rels = extract_typed_relations("[[Alice|Ali]] works at [[Acme Corp|Acme]].");
        assert_eq!(types(&rels), vec![("Alice", "works_at", "Acme Corp")]);
    }

    #[test]
    fn evidence_contains_line_and_gap() {
        let body = "Line 1.\n[[Alice]] works at [[Acme]].";
        let rels = extract_typed_relations(body);
        assert_eq!(rels.len(), 1);
        assert!(rels[0].evidence.contains("line 2"));
        assert!(rels[0].evidence.contains("works at"));
    }

    #[test]
    fn dependency_pattern() {
        let rels = extract_typed_relations("[[Service A]] depends on [[Database]].");
        assert_eq!(types(&rels), vec![("Service A", "depends_on", "Database")]);
    }

    #[test]
    fn supersedes_replaces_alias() {
        let rels = extract_typed_relations(
            "[[Plan B]] supersedes [[Plan A]]. [[Method 2]] replaces [[Method 1]].",
        );
        assert_eq!(
            types(&rels),
            vec![
                ("Plan B", "supersedes", "Plan A"),
                ("Method 2", "supersedes", "Method 1"),
            ]
        );
    }

    #[test]
    fn custom_pattern_table() {
        let custom: &[(&str, &str)] = &[("loves", "loves")];
        let rels = extract_typed_relations_with("[[Alice]] loves [[Bob]].", custom);
        assert_eq!(types(&rels), vec![("Alice", "loves", "Bob")]);
    }

    #[test]
    fn into_input_preserves_data() {
        let tr = TypedRelation {
            source: "A".into(),
            target: "B".into(),
            relation_type: "uses".into(),
            evidence: "line 1: uses".into(),
        };
        let input = tr.into_input();
        assert_eq!(input.source, "A");
        assert_eq!(input.target, "B");
        assert_eq!(input.relation_type.0, "uses");
        assert_eq!(input.evidence, Some(vec!["line 1: uses".to_string()]));
    }
}
