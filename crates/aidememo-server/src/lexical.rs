//! Rebuildable lexical projection over canonical remote facts.
//!
//! The canonical ledger remains authoritative. This projection can be rebuilt
//! from a project snapshot and advertises only the exact project sequence that
//! snapshot represented.

use aidememo_domain::{
    DomainError, FactRecord, ProjectEpoch, ProjectSequence, ProjectSnapshot, ResourceState,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const FACT_KIND: &str = "fact";
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

#[derive(Debug, Clone)]
struct IndexedFact {
    record: FactRecord,
    terms: HashMap<String, usize>,
    length: usize,
}

/// In-memory lexical index derived only from canonical fact records.
#[derive(Debug, Clone)]
pub(crate) struct LexicalProjection {
    project_epoch: ProjectEpoch,
    index_seq: ProjectSequence,
    documents: Vec<IndexedFact>,
    document_frequency: HashMap<String, usize>,
    average_length: f64,
}

/// One scored canonical fact returned from the lexical projection.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct LexicalHit {
    pub(crate) fact_id: aidememo_domain::FactId,
    pub(crate) session_id: aidememo_domain::SessionId,
    pub(crate) source_id: Option<aidememo_domain::SourceId>,
    pub(crate) actor_id: aidememo_domain::ActorId,
    pub(crate) content: String,
    pub(crate) score: f64,
}

impl LexicalProjection {
    /// Rebuild the projection from one atomic canonical snapshot.
    pub(crate) fn rebuild(snapshot: &ProjectSnapshot) -> Result<Self, DomainError> {
        let mut documents = Vec::new();
        let mut document_frequency = HashMap::<String, usize>::new();
        let mut total_length = 0_usize;

        for resource in &snapshot.resources {
            if resource.resource.kind.as_str() != FACT_KIND {
                continue;
            }
            let ResourceState::Present { body } = &resource.state else {
                continue;
            };
            let record = serde_json::from_slice::<FactRecord>(body).map_err(|error| {
                DomainError::InvalidCommand(format!(
                    "canonical fact {} is not valid typed fact JSON: {error}",
                    resource.resource.id
                ))
            })?;
            let terms = term_counts(&record.content);
            let length = terms.values().copied().sum::<usize>();
            total_length = total_length.saturating_add(length);
            for term in terms.keys() {
                let count = document_frequency.entry(term.clone()).or_default();
                *count = count.saturating_add(1);
            }
            documents.push(IndexedFact {
                record,
                terms,
                length,
            });
        }

        let average_length = if documents.is_empty() {
            0.0
        } else {
            total_length as f64 / documents.len() as f64
        };
        Ok(Self {
            project_epoch: snapshot.project_epoch.clone(),
            index_seq: snapshot.at_seq,
            documents,
            document_frequency,
            average_length,
        })
    }

    #[must_use]
    pub(crate) const fn index_seq(&self) -> ProjectSequence {
        self.index_seq
    }

    #[must_use]
    pub(crate) fn matches_snapshot(&self, snapshot: &ProjectSnapshot) -> bool {
        self.project_epoch == snapshot.project_epoch && self.index_seq == snapshot.at_seq
    }

    pub(crate) fn search(
        &self,
        query: &str,
        source_id: Option<&str>,
        limit: usize,
    ) -> Vec<LexicalHit> {
        let query_terms = tokenize(query);
        if query_terms.is_empty() || self.documents.is_empty() || limit == 0 {
            return Vec::new();
        }
        let unique_query_terms = query_terms.into_iter().collect::<HashSet<_>>();
        let document_count = self.documents.len() as f64;
        let average_length = self.average_length.max(1.0);
        let mut hits = Vec::new();

        for document in &self.documents {
            if source_id.is_some_and(|expected| {
                document.record.source_id.as_ref().map(|id| id.as_str()) != Some(expected)
            }) {
                continue;
            }
            let mut score = 0.0_f64;
            for term in &unique_query_terms {
                let Some(&term_frequency) = document.terms.get(term) else {
                    continue;
                };
                let document_frequency = self
                    .document_frequency
                    .get(term)
                    .copied()
                    .unwrap_or_default() as f64;
                let idf = (1.0
                    + (document_count - document_frequency + 0.5)
                        / (document_frequency + 0.5))
                    .ln();
                let tf = term_frequency as f64;
                let length_norm = BM25_K1
                    * (1.0 - BM25_B
                        + BM25_B * (document.length as f64 / average_length));
                score += idf * ((tf * (BM25_K1 + 1.0)) / (tf + length_norm));
            }
            if score > 0.0 {
                hits.push(LexicalHit {
                    fact_id: document.record.fact_id.clone(),
                    session_id: document.record.session_id.clone(),
                    source_id: document.record.source_id.clone(),
                    actor_id: document.record.actor_id.clone(),
                    content: document.record.content.clone(),
                    score,
                });
            }
        }

        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.fact_id.as_str().cmp(right.fact_id.as_str()))
        });
        hits.truncate(limit);
        hits
    }
}

fn term_counts(content: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for term in tokenize(content) {
        let count = counts.entry(term).or_default();
        *count = count.saturating_add(1);
    }
    counts
}

fn tokenize(content: &str) -> Vec<String> {
    content
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidememo_domain::{
        ActorId, CanonicalResource, FactId, ProjectId, ProjectScope, ResourceId, ResourceKind,
        ResourceRef, Revision, SessionId, SourceId, TenantId,
    };

    fn snapshot(facts: &[(&str, &str, Option<&str>)]) -> Result<ProjectSnapshot, DomainError> {
        let scope = ProjectScope::new(TenantId::try_from("tenant")?, ProjectId::try_from("project")?);
        let resources = facts
            .iter()
            .enumerate()
            .map(|(index, (id, content, source_id))| {
                let record = FactRecord::new(
                    FactId::try_from(*id)?,
                    SessionId::try_from("session")?,
                    source_id.map(SourceId::try_from).transpose()?,
                    ActorId::try_from("codex")?,
                    (*content).to_owned(),
                )?;
                let body = serde_json::to_vec(&record).map_err(|error| {
                    DomainError::InvalidCommand(format!("encode test fact: {error}"))
                })?;
                Ok(CanonicalResource {
                    scope: scope.clone(),
                    resource: ResourceRef {
                        kind: ResourceKind::try_from(FACT_KIND)?,
                        id: ResourceId::try_from(*id)?,
                    },
                    revision: Revision::new(1)?,
                    state: ResourceState::Present { body },
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        ProjectSnapshot::new(
            scope,
            ProjectEpoch::try_from("epoch")?,
            ProjectSequence::new(facts.len() as u64),
            resources,
        )
    }

    #[test]
    fn bm25_prefers_repeated_query_terms() -> Result<(), DomainError> {
        let snapshot = snapshot(&[
            ("fact-a", "redis cluster timeout", None),
            ("fact-b", "redis redis redis timeout", None),
        ])?;
        let index = LexicalProjection::rebuild(&snapshot)?;
        let hits = index.search("redis", None, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].fact_id.as_str(), "fact-b");
        Ok(())
    }

    #[test]
    fn source_filter_and_sequence_are_explicit() -> Result<(), DomainError> {
        let snapshot = snapshot(&[
            ("fact-a", "redis decision", Some("alpha")),
            ("fact-b", "redis decision", Some("beta")),
        ])?;
        let index = LexicalProjection::rebuild(&snapshot)?;
        assert_eq!(index.index_seq(), ProjectSequence::new(2));
        let hits = index.search("redis", Some("beta"), 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].fact_id.as_str(), "fact-b");
        Ok(())
    }

    #[test]
    fn cjk_terms_are_kept_as_lexical_tokens() -> Result<(), DomainError> {
        let snapshot = snapshot(&[("fact-a", "레디스 장애 복구", None)])?;
        let index = LexicalProjection::rebuild(&snapshot)?;
        let hits = index.search("레디스", None, 10);
        assert_eq!(hits.len(), 1);
        Ok(())
    }
}
