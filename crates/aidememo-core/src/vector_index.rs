//! HNSW-backed semantic candidate index.
//!
//! Replaces the BM25-prefilter top-50 path with O(log N) graph
//! traversal over fact embeddings. The motivation came from
//! MIRACL/ko (real Korean Wikipedia retrieval): the BM25 prefilter
//! was silently dropping ~2.5pp of correct candidates because the
//! whitespace tokenizer can't decompose Korean morphology, so
//! semantic re-ranking had no chance to recover them. Brute-force
//! cosine over the whole corpus matched the recall ceiling but
//! paid 2× latency. HNSW closes both gaps. See
//! `docs/MEASUREMENTS.md` for the prototype data.
//!
//! Design notes:
//!
//! - **Sidecar persistence.** The index lives next to the redb
//!   store as `wiki.hnsw.bin`. Header carries the model name and
//!   dimension; load aborts (and triggers rebuild) if the active
//!   model has changed since the index was built. Same pattern as
//!   the i8 quantized weights sidecar.
//! - **F32 only for now.** A future i8 path would shave 4× memory
//!   but needs a custom `Point::distance` over simsimd::i8::dot;
//!   keeping it scalar-f32 first to bound the diff.
//! - **Built lazily.** Constructed on first `hybrid_search` if the
//!   sidecar is missing, or rebuilt if `AideMemo::vector_index_rebuild`
//!   is called explicitly. Fact additions don't auto-update the
//!   index — they mark it dirty, and the next rebuild picks up
//!   the change. (Matches BM25's behavior; incremental insert is
//!   a follow-up.)

#![cfg(feature = "semantic")]

use crate::error::{AideMemoError, Result};
use crate::types::FactId;
use instant_distance::{Builder, HnswMap, Point as IDPoint, Search};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Fact embedding wrapped for instant-distance. The vector is L2-
/// normalized at insert time so cosine distance reduces to
/// `1 - dot(a, b)` — the simsimd-friendly path matches the rest
/// of the semantic stack.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactPoint {
    pub vec: Vec<f32>,
}

impl IDPoint for FactPoint {
    fn distance(&self, other: &Self) -> f32 {
        use simsimd::SpatialSimilarity;
        match f32::dot(&self.vec, &other.vec) {
            Some(d) => (1.0 - d) as f32,
            None => 0.0,
        }
    }
}

/// On-disk header: makes the sidecar self-describing so loading can
/// verify the model + dimension before deserializing the (much
/// larger) graph payload.
#[derive(Serialize, Deserialize)]
struct SidecarHeader {
    /// Format version. Bump on incompatible layout change.
    version: u32,
    /// `provider.name()` at build time (e.g. `"model2vec(potion-base-4M)"`).
    model: String,
    /// Output vector dimension. Must match the active provider on load.
    dim: usize,
    /// Number of facts indexed.
    count: usize,
}

const SIDECAR_VERSION: u32 = 1;

/// Wraps an HnswMap keyed by `FactId` with the metadata needed to
/// validate the sidecar against the live provider.
pub struct HnswIndex {
    pub model: String,
    pub dim: usize,
    map: HnswMap<FactPoint, FactId>,
}

impl HnswIndex {
    /// Build a fresh index from `(fact_id, embedding)` pairs.
    /// Vectors are normalized in place so the search distance is
    /// `1 - cosine_similarity`.
    pub fn build(model: &str, dim: usize, mut entries: Vec<(FactId, Vec<f32>)>) -> Self {
        for (_, v) in entries.iter_mut() {
            l2_normalize(v);
        }
        let (ids, points): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .map(|(id, v)| (id, FactPoint { vec: v }))
            .unzip();
        let map = Builder::default()
            .ef_construction(200)
            .seed(42)
            .build(points, ids);
        Self {
            model: model.to_string(),
            dim,
            map,
        }
    }

    /// Top-K candidate FactIds for a query vector. Caller is
    /// expected to L2-normalize the query first; the search short-
    /// circuits to zero distances on a non-unit input.
    pub fn search(&self, query: &[f32], k: usize) -> Vec<FactId> {
        let mut scratch = Search::default();
        let q = FactPoint {
            vec: query.to_vec(),
        };
        self.map
            .search(&q, &mut scratch)
            .take(k)
            .map(|item| *item.value)
            .collect()
    }

    /// Number of indexed facts.
    pub fn len(&self) -> usize {
        // instant-distance's HnswMap doesn't expose len() directly;
        // iter() yields one item per indexed point.
        self.map.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.map.iter().next().is_none()
    }

    /// Serialize to `path`. Atomic write: stage to `path.tmp`, then
    /// rename. A killed mid-write process leaves the previous
    /// sidecar (if any) intact and triggers a rebuild rather than
    /// loading torn data.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let header = SidecarHeader {
            version: SIDECAR_VERSION,
            model: self.model.clone(),
            dim: self.dim,
            count: self.len(),
        };
        let payload = bincode::serialize(&(&header, &self.map))
            .map_err(|e| AideMemoError::Internal(format!("hnsw serialize failed: {e}")))?;

        let tmp = path.with_extension("hnsw.bin.tmp");
        std::fs::write(&tmp, &payload)
            .map_err(|e| AideMemoError::Internal(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| AideMemoError::Internal(format!("rename {}: {e}", path.display())))?;
        Ok(())
    }

    /// Load from `path`. Returns `Ok(None)` if the sidecar is missing,
    /// `Err` only on actual I/O / format failures (so callers can
    /// treat absence as "build me one" without filtering by io::ErrorKind).
    pub fn load_from(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(path)
            .map_err(|e| AideMemoError::FileRead(path.to_path_buf(), e.to_string()))?;
        let (header, map): (SidecarHeader, HnswMap<FactPoint, FactId>) =
            bincode::deserialize(&bytes).map_err(|e| {
                AideMemoError::Internal(format!("hnsw deserialize {}: {e}", path.display()))
            })?;
        if header.version != SIDECAR_VERSION {
            return Err(AideMemoError::Internal(format!(
                "hnsw sidecar version {} != {} — rebuild required",
                header.version, SIDECAR_VERSION
            )));
        }
        Ok(Some(Self {
            model: header.model,
            dim: header.dim,
            map,
        }))
    }

    /// Did the index get built against the same model + dimension
    /// as the live provider? Returns false if either differs — the
    /// caller should rebuild.
    pub fn matches_provider(&self, model: &str, dim: usize) -> bool {
        self.model == model && self.dim == dim
    }

    /// Materialize the `(FactId → Vec<f32>)` mapping from the index.
    /// Used by `vector_index_rebuild` to skip re-embedding facts
    /// whose content didn't change. The vectors come back already
    /// L2-normalized — same shape we'd hand back to the next
    /// `Builder::build` call.
    pub fn extract_vectors(&self) -> std::collections::HashMap<FactId, Vec<f32>> {
        self.map
            .iter()
            .map(|(pid, point)| {
                let id = self.map.values[pid.into_inner() as usize];
                (id, point.vec.clone())
            })
            .collect()
    }
}

/// L2-normalize a vector in place. Matches the prototype + the
/// behaviour of the Model2Vec provider itself, so cached + freshly-
/// embedded vectors compose without double-normalization.
pub(crate) fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    for x in v.iter_mut() {
        *x /= norm;
    }
}
