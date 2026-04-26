//! Native, zero-copy Model2Vec inference.
//!
//! `model2vec-rs` (the upstream crate) reads `model.safetensors` into a
//! `Vec<u8>`, then re-encodes the F32 little-endian payload into a fresh
//! `Vec<f32>` via `chunks_exact(4).map(...).collect()`. For the default
//! 128M model that means **two ~500 MB heap allocations live at once**
//! during load — peak heap settles around 1.5 GB after the bytes vec
//! drops, and the `Vec<f32>` stays resident for the lifetime of the
//! model.
//!
//! This crate eliminates both copies:
//!
//! - **mmap** the weights file. No heap allocation for the matrix —
//!   the OS pages it in lazily, and the kernel can evict cold pages
//!   under memory pressure.
//! - **`bytemuck::cast_slice`** the mmap'd region to `&[f32]`. SafeTensors
//!   payloads are little-endian f32; on every platform Rust supports
//!   that's the same wire format as `f32`, so the cast is a pointer
//!   reinterpret with zero allocation.
//!
//! Net effect on the 128M default model: heap drops from ~1.5 GB to
//! ~100 MB (tokenizer only). Latency is unaffected — the inner pool
//! loop reads the same f32s either way, just from a different page
//! provenance.
//!
//! ## API surface
//!
//! [`StaticModel`] is intentionally a near drop-in for
//! `model2vec_rs::model::StaticModel`: same `from_pretrained`,
//! `encode_single`, `encode` shape so wg-core can swap one for the
//! other with a single import change.

#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use memmap2::Mmap;
use safetensors::SafeTensors;
use safetensors::tensor::Dtype;
use serde_json::Value;
use thiserror::Error;
use tokenizers::Tokenizer;

mod weights;
use weights::Weights;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to read {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("failed to mmap {0}: {1}")]
    Mmap(PathBuf, std::io::Error),
    #[error("safetensors parse: {0}")]
    SafeTensors(String),
    #[error("expected `embeddings` tensor in {0}")]
    MissingTensor(PathBuf),
    #[error("embeddings tensor is not 2-D in {0}")]
    NotMatrix(PathBuf),
    #[error("unsupported tensor dtype {0:?}")]
    UnsupportedDtype(Dtype),
    #[error("tokenizer load: {0}")]
    Tokenizer(String),
    #[error("tokenize: {0}")]
    Tokenize(String),
    #[error("config.json parse: {0}")]
    Config(String),
    #[cfg(feature = "hub")]
    #[error("HF hub: {0}")]
    Hub(String),
    #[error("model directory missing required file: {0}")]
    Missing(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Loaded Model2Vec model with mmap-backed weights.
pub struct StaticModel {
    tokenizer: Tokenizer,
    weights: Weights,
    /// Per-token scaling factor when the model ships a `weights` tensor
    /// (vocabulary-quantized variants). `None` → all 1.0.
    token_weights: Option<Vec<f32>>,
    /// Optional row remap (for vocab-quantized variants).
    token_mapping: Option<Vec<u32>>,
    normalize: bool,
    median_token_length: usize,
    unk_token_id: Option<u32>,
    /// Holds the mmap so it outlives `weights`. We hand `Weights` a
    /// `&[f32]` borrowed from this.
    _mmap: Arc<Mmap>,
    rows: usize,
    cols: usize,
}

impl std::fmt::Debug for StaticModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticModel")
            .field("rows", &self.rows)
            .field("cols", &self.cols)
            .field("normalize", &self.normalize)
            .field("has_token_weights", &self.token_weights.is_some())
            .field("has_token_mapping", &self.token_mapping.is_some())
            .finish()
    }
}

impl StaticModel {
    /// Load from a local model directory. The directory must contain
    /// `tokenizer.json`, `model.safetensors`, and `config.json`.
    ///
    /// The weights tensor is mmap'd, not copied. As a result, this
    /// function returns very quickly even for large models — pages are
    /// faulted in on first access during `encode`.
    pub fn from_pretrained<P: AsRef<Path>>(path: P, normalize: Option<bool>) -> Result<Self> {
        let dir = path.as_ref();
        let tok_path = dir.join("tokenizer.json");
        let mdl_path = dir.join("model.safetensors");
        let cfg_path = dir.join("config.json");
        for p in [&tok_path, &mdl_path, &cfg_path] {
            if !p.exists() {
                return Err(Error::Missing(p.clone()));
            }
        }
        Self::load_from_paths(&tok_path, &mdl_path, &cfg_path, normalize)
    }

    /// Load from the HuggingFace Hub. Uses `hf-hub` to resolve files to
    /// the local cache (downloading if absent), then mmap's them. Same
    /// memory profile as a local load once the cache is warm.
    #[cfg(feature = "hub")]
    pub fn from_hub(repo: &str, normalize: Option<bool>) -> Result<Self> {
        let api = hf_hub::api::sync::Api::new().map_err(|e| Error::Hub(e.to_string()))?;
        let model = api.model(repo.to_string());
        let tok = model
            .get("tokenizer.json")
            .map_err(|e| Error::Hub(e.to_string()))?;
        let mdl = model
            .get("model.safetensors")
            .map_err(|e| Error::Hub(e.to_string()))?;
        let cfg = model
            .get("config.json")
            .map_err(|e| Error::Hub(e.to_string()))?;
        Self::load_from_paths(&tok, &mdl, &cfg, normalize)
    }

    fn load_from_paths(
        tok_path: &Path,
        mdl_path: &Path,
        cfg_path: &Path,
        normalize: Option<bool>,
    ) -> Result<Self> {
        // Tokenizer (real cost — vocab dictionary lives on heap, ~tens of MB).
        let tokenizer =
            Tokenizer::from_file(tok_path).map_err(|e| Error::Tokenizer(e.to_string()))?;

        // Read normalize default from config.json. The whole file is
        // tiny so cost is negligible.
        let cfg_bytes = std::fs::read(cfg_path).map_err(|e| Error::Io(cfg_path.into(), e))?;
        let cfg: Value =
            serde_json::from_slice(&cfg_bytes).map_err(|e| Error::Config(e.to_string()))?;
        let cfg_norm = cfg
            .get("normalize")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let normalize = normalize.unwrap_or(cfg_norm);

        // Median token length — used by upstream's char-truncation
        // heuristic. Cheap, computed once.
        let mut lens: Vec<usize> = tokenizer.get_vocab(false).keys().map(|t| t.len()).collect();
        lens.sort_unstable();
        let median_token_length = lens.get(lens.len() / 2).copied().unwrap_or(1);

        // Resolve UNK so we can drop those tokens from the pool.
        // Upstream model2vec-rs serialized the tokenizer back to JSON
        // here just to read `model.unk_token` — that costs ~19 MB of
        // peak heap on a 500k-vocab multilingual tokenizer (dhat).
        // Tokenizers expose `tokenizer.token_to_id` directly, so we
        // probe a short list of conventional UNK strings instead.
        // Exact match wins; absence is fine (we just don't filter UNK,
        // matching the original "no UNK declared" branch).
        let unk_token_id = ["[UNK]", "<unk>", "<UNK>", "[unk]"]
            .iter()
            .find_map(|tok| tokenizer.token_to_id(tok));

        // mmap the weights file. The Mmap holds an Arc<File> internally.
        let file = std::fs::File::open(mdl_path).map_err(|e| Error::Io(mdl_path.into(), e))?;
        // SAFETY: mmap is unsafe in general because the file backing
        // could change under us. For a HuggingFace model artifact we
        // accept that risk: it's the same risk model2vec-rs takes by
        // reading the file once and trusting it. We never write
        // through the mapping, and we hold the mmap for the lifetime
        // of the StaticModel.
        let mmap = unsafe { Mmap::map(&file) }.map_err(|e| Error::Mmap(mdl_path.into(), e))?;
        let mmap = Arc::new(mmap);

        // SafeTensors view — header-only parse over the mmap'd bytes.
        let safet =
            SafeTensors::deserialize(&mmap[..]).map_err(|e| Error::SafeTensors(e.to_string()))?;
        let tensor = safet
            .tensor("embeddings")
            .or_else(|_| safet.tensor("0"))
            .map_err(|_| Error::MissingTensor(mdl_path.into()))?;
        let shape: &[usize] = tensor.shape();
        let (rows, cols) = match shape {
            [r, c] => (*r, *c),
            _ => return Err(Error::NotMatrix(mdl_path.into())),
        };

        // Build a Weights view that borrows from the mmap. For F32 LE
        // we can keep it strictly zero-copy; for F16/I8 we have to
        // expand to f32 (one allocation, but ~half / quarter the size).
        let weights = Weights::from_tensor(tensor.dtype(), tensor.data(), &mmap, rows, cols)?;

        // Optional vocab-quantization tensors. These are small (one f32
        // per vocab token), so the upstream's `Vec<f32>` collect is fine
        // here — keeping it simple.
        let token_weights = match safet.tensor("weights") {
            Ok(t) => Some(decode_weights_tensor(t.dtype(), t.data())?),
            Err(_) => None,
        };
        let token_mapping = match safet.tensor("mapping") {
            Ok(t) => {
                let raw = t.data();
                let v: Vec<u32> = raw
                    .chunks_exact(4)
                    .map(|b| {
                        // i32 in source, but only positive indices are
                        // meaningful here.
                        i32::from_le_bytes([b[0], b[1], b[2], b[3]]).max(0) as u32
                    })
                    .collect();
                Some(v)
            }
            Err(_) => None,
        };

        Ok(Self {
            tokenizer,
            weights,
            token_weights,
            token_mapping,
            normalize,
            median_token_length,
            unk_token_id,
            _mmap: mmap,
            rows,
            cols,
        })
    }

    /// Output dimensionality. Cheap accessor; same as `cols`.
    pub fn dimension(&self) -> usize {
        self.cols
    }

    /// Number of rows in the embedding matrix (vocab size).
    pub fn vocab_rows(&self) -> usize {
        self.rows
    }

    /// Encode a single text into a vector. Convenience wrapper around
    /// `encode_with_args`.
    pub fn encode_single(&self, text: &str) -> Vec<f32> {
        self.encode_with_args(&[text.to_string()], Some(512), 1024)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// Encode a batch with default `max_length=512`, `batch_size=1024`.
    pub fn encode(&self, texts: &[String]) -> Vec<Vec<f32>> {
        self.encode_with_args(texts, Some(512), 1024)
    }

    /// Encode with explicit truncation and batching parameters. Mirrors
    /// the upstream API.
    pub fn encode_with_args(
        &self,
        texts: &[String],
        max_length: Option<usize>,
        batch_size: usize,
    ) -> Vec<Vec<f32>> {
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(batch_size.max(1)) {
            let truncated: Vec<&str> = chunk
                .iter()
                .map(|t| match max_length {
                    Some(m) => truncate_str(t, m, self.median_token_length),
                    None => t.as_str(),
                })
                .collect();

            let encodings = match self
                .tokenizer
                .encode_batch_fast::<String>(truncated.into_iter().map(Into::into).collect(), false)
            {
                Ok(e) => e,
                Err(_) => {
                    // On tokenizer failure, return zero-vectors for this
                    // batch. Caller can detect via score == 0 in cosine.
                    for _ in chunk {
                        out.push(vec![0.0; self.cols]);
                    }
                    continue;
                }
            };

            for enc in encodings {
                let mut ids: Vec<u32> = enc.get_ids().to_vec();
                if let Some(unk) = self.unk_token_id {
                    ids.retain(|&i| i != unk);
                }
                if let Some(m) = max_length {
                    ids.truncate(m);
                }
                out.push(self.pool(&ids));
            }
        }
        out
    }

    /// Mean-pool a single token-id list into a fresh `Vec<f32>`.
    ///
    /// Walks `ids`, looks up each row in the embedding matrix (zero-copy
    /// slice from mmap), accumulates into a stack-allocated sum buffer,
    /// then divides + normalizes. Allocation footprint per call is one
    /// `Vec<f32>` of length `cols` — same as upstream.
    fn pool(&self, ids: &[u32]) -> Vec<f32> {
        let dim = self.cols;
        let mut sum = vec![0.0_f32; dim];
        let mut cnt: usize = 0;

        for &id in ids {
            // Optional vocab remap.
            let row_idx = match &self.token_mapping {
                Some(m) => *m.get(id as usize).unwrap_or(&id) as usize,
                None => id as usize,
            };
            if row_idx >= self.rows {
                continue;
            }

            let scale = match &self.token_weights {
                Some(w) => *w.get(id as usize).unwrap_or(&1.0),
                None => 1.0,
            };

            let row = self.weights.row(row_idx);
            // SIMD-friendly loop; rustc autovectorizes this on modern
            // targets. Could be hand-tuned with simsimd later if a
            // bench shows it matters.
            for (s, &v) in sum.iter_mut().zip(row.iter()) {
                *s += v * scale;
            }
            cnt += 1;
        }

        let denom = cnt.max(1) as f32;
        for x in sum.iter_mut() {
            *x /= denom;
        }
        if self.normalize {
            let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
            for x in sum.iter_mut() {
                *x /= norm;
            }
        }
        sum
    }
}

/// Char-level pre-truncation to roughly `max_tokens * median_len` chars.
/// Cheap conservative cap so the tokenizer doesn't see runaway-long
/// inputs. Mirrors upstream behavior exactly.
fn truncate_str(s: &str, max_tokens: usize, median_len: usize) -> &str {
    let max_chars = max_tokens.saturating_mul(median_len);
    match s.char_indices().nth(max_chars) {
        Some((b, _)) => &s[..b],
        None => s,
    }
}

fn decode_weights_tensor(dtype: Dtype, raw: &[u8]) -> Result<Vec<f32>> {
    Ok(match dtype {
        Dtype::F32 => raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
        Dtype::F16 => raw
            .chunks_exact(2)
            .map(|b| half::f16::from_le_bytes([b[0], b[1]]).to_f32())
            .collect(),
        Dtype::F64 => raw
            .chunks_exact(8)
            .map(|b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32)
            .collect(),
        other => return Err(Error::UnsupportedDtype(other)),
    })
}
