//! Rebuildable semantic/HNSW projection over canonical remote facts.
//!
//! This module deliberately owns no canonical state. Every vector and HNSW
//! node is derived from a validated [`ProjectSnapshot`], carries that
//! snapshot's exact sequence watermark, and can be discarded/rebuilt at any
//! time. The production provider is OpenAI-compatible HTTP so the SSOT server
//! does not depend on the embedded `aidememo-core` model/runtime stack.

use aidememo_domain::{
    DomainError, FactRecord, ProjectEpoch, ProjectSequence, ProjectSnapshot, ResourceState,
};
use instant_distance::{Builder, HnswMap, Point, Search};
use serde::Deserialize;
use std::sync::Arc;

const FACT_KIND: &str = "fact";
const HNSW_EF_CONSTRUCTION: usize = 200;
const HNSW_SEED: u64 = 42;

/// Text embedding source used by the server-side semantic projection.
///
/// Providers are intentionally independent from canonical storage. A provider
/// switch invalidates the projection because vectors from different model
/// identities or dimensions are never mixed.
pub trait EmbeddingProvider: Send + Sync {
    /// Stable human-readable model/provider identity.
    fn name(&self) -> String;

    /// Output vector dimension.
    fn dimension(&self) -> usize;

    /// Embed one query string.
    ///
    /// # Errors
    ///
    /// Returns a domain/storage error when the provider cannot produce a
    /// finite vector with the configured dimension.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, DomainError>;

    /// Embed canonical fact contents in input order.
    ///
    /// # Errors
    ///
    /// Returns a domain/storage error when the provider request fails or the
    /// result shape is invalid.
    fn embed_document_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, DomainError>;
}

/// OpenAI-compatible HTTP embedding provider for the SSOT server.
///
/// The endpoint may be OpenAI, Ollama, vLLM, LocalAI, OpenRouter, or another
/// service that accepts `{model,input}` and returns `data[].embedding`.
pub struct HttpEmbeddingProvider {
    endpoint: String,
    model: String,
    api_key: Option<String>,
    dimension: usize,
    agent: ureq::Agent,
}

impl HttpEmbeddingProvider {
    /// Build a validated HTTP provider.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidCommand`] for an invalid endpoint, empty
    /// model, or zero vector dimension.
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        dimension: usize,
        api_key: Option<String>,
    ) -> Result<Self, DomainError> {
        let endpoint = endpoint.into().trim().to_owned();
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            return Err(DomainError::InvalidCommand(
                "embedding endpoint must use http:// or https://".to_owned(),
            ));
        }
        let model = model.into().trim().to_owned();
        if model.is_empty() {
            return Err(DomainError::InvalidCommand(
                "embedding model must not be empty".to_owned(),
            ));
        }
        if dimension == 0 {
            return Err(DomainError::InvalidCommand(
                "embedding dimension must be greater than zero".to_owned(),
            ));
        }
        Ok(Self {
            endpoint,
            model,
            api_key: api_key.filter(|key| !key.trim().is_empty()),
            dimension,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(5))
                .timeout_read(std::time::Duration::from_secs(30))
                .timeout_write(std::time::Duration::from_secs(30))
                .build(),
        })
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, DomainError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });
        let mut request = self
            .agent
            .post(&self.endpoint)
            .set("Content-Type", "application/json");
        if let Some(api_key) = &self.api_key {
            request = request.set("Authorization", &format!("Bearer {api_key}"));
        }
        let response = request.send_json(body).map_err(|error| {
            DomainError::StorageFailure {
                operation: "embedding_request",
                detail: error.to_string(),
            }
        })?;
        let response = response.into_json::<EmbeddingResponse>().map_err(|error| {
            DomainError::StorageFailure {
                operation: "embedding_decode",
                detail: error.to_string(),
            }
        })?;
        let vectors = response
            .data
            .into_iter()
            .map(|item| item.embedding)
            .collect::<Vec<_>>();
        validate_vectors(&vectors, texts.len(), self.dimension)?;
        Ok(vectors)
    }
}

impl EmbeddingProvider for HttpEmbeddingProvider {
    fn name(&self) -> String {
        format!("openai-compatible({})", self.model)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, DomainError> {
        let mut vectors = self.embed_batch(&[text.to_owned()])?;
        vectors.pop().ok_or_else(|| DomainError::StorageFailure {
            operation: "embedding_decode",
            detail: "embedding endpoint returned no query vector".to_owned(),
        })
    }

    fn embed_document_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, DomainError> {
        self.embed_batch(texts)
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
}

#[derive(Clone, Debug)]
struct SemanticPoint {
    vector: Vec<f32>,
}

impl Point for SemanticPoint {
    fn distance(&self, other: &Self) -> f32 {
        1.0 - dot(&self.vector, &other.vector)
    }
}

#[derive(Clone, Debug)]
struct SemanticDocument {
    record: FactRecord,
    vector: Vec<f32>,
}

/// One semantic candidate returned from the HNSW projection.
#[derive(Clone, Debug)]
pub(crate) struct SemanticHit {
    pub(crate) fact_id: aidememo_domain::FactId,
    pub(crate) session_id: aidememo_domain::SessionId,
    pub(crate) source_id: Option<aidememo_domain::SourceId>,
    pub(crate) actor_id: aidememo_domain::ActorId,
    pub(crate) content: String,
    pub(crate) score: f64,
}

/// HNSW projection for one exact canonical project snapshot and model identity.
pub(crate) struct SemanticProjection {
    project_epoch: ProjectEpoch,
    index_seq: ProjectSequence,
    model: String,
    dimension: usize,
    documents: Vec<SemanticDocument>,
    index: Option<HnswMap<SemanticPoint, usize>>,
}

impl SemanticProjection {
    /// Rebuild the semantic projection from one atomic canonical snapshot.
    ///
    /// # Errors
    ///
    /// Returns a provider or canonical-decoding error. No partially built
    /// projection is returned.
    pub(crate) fn rebuild(
        snapshot: &ProjectSnapshot,
        provider: &dyn EmbeddingProvider,
    ) -> Result<Self, DomainError> {
        let dimension = provider.dimension();
        if dimension == 0 {
            return Err(DomainError::InvalidCommand(
                "embedding provider reported zero dimension".to_owned(),
            ));
        }
        let mut records = Vec::new();
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
            records.push(record);
        }
        let texts = records
            .iter()
            .map(|record| record.content.clone())
            .collect::<Vec<_>>();
        let mut vectors = provider.embed_document_batch(&texts)?;
        validate_vectors(&vectors, records.len(), dimension)?;

        let mut documents = Vec::with_capacity(records.len());
        let mut points = Vec::with_capacity(records.len());
        for (record, vector) in records.into_iter().zip(vectors.iter_mut()) {
            normalize(vector);
            points.push(SemanticPoint {
                vector: vector.clone(),
            });
            documents.push(SemanticDocument {
                record,
                vector: vector.clone(),
            });
        }
        let index = if points.is_empty() {
            None
        } else {
            let ids = (0..points.len()).collect::<Vec<_>>();
            Some(
                Builder::default()
                    .ef_construction(HNSW_EF_CONSTRUCTION)
                    .seed(HNSW_SEED)
                    .build(points, ids),
            )
        };
        Ok(Self {
            project_epoch: snapshot.project_epoch.clone(),
            index_seq: snapshot.at_seq,
            model: provider.name(),
            dimension,
            documents,
            index,
        })
    }

    #[must_use]
    pub(crate) const fn project_epoch(&self) -> &ProjectEpoch {
        &self.project_epoch
    }

    #[must_use]
    pub(crate) const fn index_seq(&self) -> ProjectSequence {
        self.index_seq
    }

    #[must_use]
    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    #[must_use]
    pub(crate) fn matches(
        &self,
        snapshot: &ProjectSnapshot,
        provider: &dyn EmbeddingProvider,
    ) -> bool {
        self.project_epoch == snapshot.project_epoch
            && self.index_seq == snapshot.at_seq
            && self.model == provider.name()
            && self.dimension == provider.dimension()
    }

    /// Search the HNSW projection and recompute exact cosine scores for the
    /// returned candidates.
    ///
    /// # Errors
    ///
    /// Returns a provider/model shape error when the query embedding cannot be
    /// compared with this projection.
    pub(crate) fn search(
        &self,
        provider: &dyn EmbeddingProvider,
        query: &str,
        source_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SemanticHit>, DomainError> {
        if self.model != provider.name() || self.dimension != provider.dimension() {
            return Err(DomainError::InvalidCommand(
                "semantic projection model changed; rebuild required".to_owned(),
            ));
        }
        let Some(index) = &self.index else {
            return Ok(Vec::new());
        };
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut query_vector = provider.embed_query(query)?;
        validate_vectors(std::slice::from_ref(&query_vector), 1, self.dimension)?;
        normalize(&mut query_vector);
        let candidate_limit = if source_id.is_some() {
            self.documents.len()
        } else {
            limit.saturating_mul(8).max(limit).min(self.documents.len())
        };
        let point = SemanticPoint {
            vector: query_vector.clone(),
        };
        let mut scratch = Search::default();
        let mut hits = index
            .search(&point, &mut scratch)
            .take(candidate_limit)
            .filter_map(|item| {
                let document = self.documents.get(*item.value)?;
                if source_id.is_some_and(|expected| {
                    document.record.source_id.as_ref().map(|id| id.as_str()) != Some(expected)
                }) {
                    return None;
                }
                Some(SemanticHit {
                    fact_id: document.record.fact_id.clone(),
                    session_id: document.record.session_id.clone(),
                    source_id: document.record.source_id.clone(),
                    actor_id: document.record.actor_id.clone(),
                    content: document.record.content.clone(),
                    score: f64::from(dot(&query_vector, &document.vector)),
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.fact_id.as_str().cmp(right.fact_id.as_str()))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

fn validate_vectors(
    vectors: &[Vec<f32>],
    expected_count: usize,
    dimension: usize,
) -> Result<(), DomainError> {
    if vectors.len() != expected_count {
        return Err(DomainError::StorageFailure {
            operation: "embedding_shape",
            detail: format!(
                "provider returned {} vectors for {expected_count} inputs",
                vectors.len()
            ),
        });
    }
    if vectors
        .iter()
        .any(|vector| vector.len() != dimension || vector.iter().any(|value| !value.is_finite()))
    {
        return Err(DomainError::StorageFailure {
            operation: "embedding_shape",
            detail: format!("provider returned a non-finite or non-{dimension}-dimensional vector"),
        });
    }
    Ok(())
}

fn normalize(vector: &mut [f32]) {
    let norm = vector
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt()
        .max(1e-12);
    for value in vector {
        *value /= norm;
    }
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() {
        return 0.0;
    }
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

/// Shared semantic provider handle stored by [`crate::ServerState`].
pub type SharedEmbeddingProvider = Arc<dyn EmbeddingProvider>;

#[cfg(test)]
mod tests {
    use super::*;
    use aidememo_domain::{
        ActorId, CanonicalResource, FactId, ProjectId, ProjectScope, ResourceId, ResourceKind,
        ResourceRef, Revision, SessionId, TenantId,
    };
    use std::collections::HashMap;

    struct FakeProvider {
        name: String,
        vectors: HashMap<String, Vec<f32>>,
    }

    impl FakeProvider {
        fn new(name: &str, pairs: &[(&str, [f32; 3])]) -> Self {
            Self {
                name: name.to_owned(),
                vectors: pairs
                    .iter()
                    .map(|(text, vector)| ((*text).to_owned(), vector.to_vec()))
                    .collect(),
            }
        }
    }

    impl EmbeddingProvider for FakeProvider {
        fn name(&self) -> String {
            self.name.clone()
        }

        fn dimension(&self) -> usize {
            3
        }

        fn embed_query(&self, text: &str) -> Result<Vec<f32>, DomainError> {
            self.vectors.get(text).cloned().ok_or_else(|| {
                DomainError::InvalidCommand(format!("missing fake query embedding for {text}"))
            })
        }

        fn embed_document_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, DomainError> {
            texts
                .iter()
                .map(|text| {
                    self.vectors.get(text).cloned().ok_or_else(|| {
                        DomainError::InvalidCommand(format!(
                            "missing fake document embedding for {text}"
                        ))
                    })
                })
                .collect()
        }
    }

    fn snapshot(seq: u64) -> Result<ProjectSnapshot, DomainError> {
        let scope = ProjectScope::new(
            TenantId::try_from("tenant")?,
            ProjectId::try_from("project")?,
        );
        let facts = [
            ("fact-db", "postgres connection pool exhausted"),
            ("fact-cache", "redis cache fallback"),
        ];
        let resources = facts
            .iter()
            .map(|(id, content)| {
                let record = FactRecord::new(
                    FactId::try_from(*id)?,
                    SessionId::try_from("session")?,
                    None,
                    ActorId::try_from("agent")?,
                    (*content).to_owned(),
                )?;
                Ok(CanonicalResource {
                    scope: scope.clone(),
                    resource: ResourceRef {
                        kind: ResourceKind::try_from(FACT_KIND)?,
                        id: ResourceId::try_from(*id)?,
                    },
                    revision: Revision::new(1)?,
                    state: ResourceState::Present {
                        body: serde_json::to_vec(&record).map_err(|error| {
                            DomainError::InvalidCommand(format!("encode test fact: {error}"))
                        })?,
                    },
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        ProjectSnapshot::new(
            scope,
            ProjectEpoch::try_from("epoch")?,
            ProjectSequence::new(seq),
            resources,
        )
    }

    #[test]
    fn semantic_projection_finds_non_lexical_match() -> Result<(), DomainError> {
        let provider = FakeProvider::new(
            "fake-v1",
            &[
                ("postgres connection pool exhausted", [1.0, 0.0, 0.0]),
                ("redis cache fallback", [0.0, 1.0, 0.0]),
                ("database saturation", [0.99, 0.01, 0.0]),
            ],
        );
        let snapshot = snapshot(2)?;
        let projection = SemanticProjection::rebuild(&snapshot, &provider)?;
        let hits = projection.search(&provider, "database saturation", None, 1)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].fact_id.as_str(), "fact-db");
        assert_eq!(projection.index_seq(), ProjectSequence::new(2));
        Ok(())
    }

    #[test]
    fn sequence_or_model_switch_invalidates_projection() -> Result<(), DomainError> {
        let provider = FakeProvider::new(
            "fake-v1",
            &[
                ("postgres connection pool exhausted", [1.0, 0.0, 0.0]),
                ("redis cache fallback", [0.0, 1.0, 0.0]),
            ],
        );
        let projection = SemanticProjection::rebuild(&snapshot(2)?, &provider)?;
        assert!(projection.matches(&snapshot(2)?, &provider));
        assert!(!projection.matches(&snapshot(3)?, &provider));
        let switched = FakeProvider::new(
            "fake-v2",
            &[
                ("postgres connection pool exhausted", [1.0, 0.0, 0.0]),
                ("redis cache fallback", [0.0, 1.0, 0.0]),
            ],
        );
        assert!(!projection.matches(&snapshot(2)?, &switched));
        Ok(())
    }
}
