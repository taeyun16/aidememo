//! Pluggable embedding-provider abstraction.
//!
//! Inspired by the most-requested issue across mem0 / gbrain / Graphiti:
//! "make the embedding provider configurable". wg's default stays offline
//! (Model2Vec), but users can opt into any OpenAI-compatible HTTP endpoint
//! — Ollama, OpenAI, OpenRouter, vLLM, LocalAI, llama.cpp's server — by
//! flipping `model.provider` in `~/.wg/config.toml`.
//!
//! Identity contract:
//! - `model2vec` (default) keeps the "single binary, fully offline, zero
//!   network" experience. No HTTP at all.
//! - `openai` issues blocking HTTP POSTs to the configured endpoint. Each
//!   query/ingest pass is one batch request, so latency is dominated by the
//!   provider, not by us.
//! - Switching providers requires a re-embed of existing facts (different
//!   models produce incompatible vectors). For now we just compute on the
//!   fly during search, so swap is risk-free.

use crate::config::Config;
use crate::error::{Result, WgError};

/// A plug-and-play source of text embeddings.
///
/// Implementations are sync because the existing search path is sync; an
/// async variant can be added later without breaking callers.
pub trait EmbeddingProvider: Send + Sync {
    /// Short, human-readable name (e.g. `model2vec`, `openai-compat(ollama)`).
    fn name(&self) -> String;

    /// Vector dimension. Used to validate config consistency and for
    /// future on-disk vector indices.
    fn dimension(&self) -> usize;

    /// Embed a single text.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed a batch in one call when the provider supports it. Default
    /// impl is a serial loop; override in providers that expose a real
    /// batch endpoint (Model2Vec, OpenAI).
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }
}

/// The set of providers wg knows how to construct from `Config`. Useful
/// for `wg model providers` and for help-text auto-listing.
pub fn known_providers() -> &'static [&'static str] {
    &["model2vec", "openai", "tei"]
}

/// Build a provider from `config.model`. Errors if the provider name is
/// unknown or required fields are missing.
#[cfg(feature = "semantic")]
pub fn load_provider(config: &Config) -> Result<Box<dyn EmbeddingProvider>> {
    let provider = config.model.provider.trim();
    let name = if provider.is_empty() {
        "model2vec"
    } else {
        provider
    };
    match name {
        "model2vec" => Ok(Box::new(model2vec::Model2VecProvider::load(config)?)),
        "openai" | "openai-compat" | "openai-compatible" | "ollama" => Ok(Box::new(
            openai::OpenAICompatibleProvider::from_config(config)?,
        )),
        "tei" | "text-embeddings-inference" => Ok(Box::new(tei::TeiProvider::from_config(config)?)),
        other => Err(WgError::InvalidInput(format!(
            "unknown embedding provider '{other}' — expected one of {:?}",
            known_providers()
        ))),
    }
}

// ---------------------------------------------------------------------------
// Model2Vec (default, offline)
// ---------------------------------------------------------------------------

#[cfg(feature = "semantic")]
mod model2vec {
    use super::{Config, EmbeddingProvider, Result, WgError};
    use model2vec_native::StaticModel;
    use std::path::{Path, PathBuf};

    pub struct Model2VecProvider {
        inner: StaticModel,
        name: String,
        dim: usize,
    }

    impl Model2VecProvider {
        pub fn load(config: &Config) -> Result<Self> {
            let cache_dir = expand_tilde(&config.model.cache_dir);
            let configured_name = Path::new(&config.model.name);
            let local_candidate = if configured_name.exists() {
                configured_name.to_path_buf()
            } else {
                cache_dir.join(&config.model.name)
            };

            // model.quantize=true: load with int8 weights (~4× smaller
            // heap, ~0.5% cosine loss vs f32). When false, the f32
            // matrix stays mmap'd zero-copy. The dispatch happens
            // here so wg-core code below never has to know which
            // storage form is in play.
            let q = config.model.quantize;
            let inner = if local_candidate.exists() {
                // Local directory layout (tokenizer.json/model.safetensors/config.json).
                let r = if q {
                    StaticModel::from_pretrained_quantized(&local_candidate, None)
                } else {
                    StaticModel::from_pretrained(&local_candidate, None)
                };
                r.map_err(|source| WgError::ModelLoadFailed {
                    path: local_candidate.clone(),
                    source: Box::new(std::io::Error::other(source.to_string())),
                })?
            } else if config.model.auto_download {
                // HF Hub fallback. mmap kicks in on the cached files.
                let r = if q {
                    StaticModel::from_hub_quantized(&config.model.name, None)
                } else {
                    StaticModel::from_hub(&config.model.name, None)
                };
                r.map_err(|source| WgError::ModelLoadFailed {
                    path: PathBuf::from(&config.model.name),
                    source: Box::new(std::io::Error::other(source.to_string())),
                })?
            } else {
                return Err(WgError::ModelNotFound {
                    name: config.model.name.clone(),
                    cache_dir,
                });
            };

            let dim = inner.dimension();
            Ok(Self {
                inner,
                name: config.model.name.clone(),
                dim,
            })
        }
    }

    impl EmbeddingProvider for Model2VecProvider {
        fn name(&self) -> String {
            format!("model2vec({})", self.name)
        }
        fn dimension(&self) -> usize {
            self.dim
        }
        fn embed(&self, text: &str) -> Result<Vec<f32>> {
            Ok(self.inner.encode_single(text))
        }
        fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(self.inner.encode(texts))
        }
    }

    fn expand_tilde(s: &str) -> PathBuf {
        if let Some(rest) = s.strip_prefix("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home).join(rest);
            }
        }
        PathBuf::from(s)
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible HTTP (Ollama / OpenAI / OpenRouter / vLLM / LocalAI / …)
// ---------------------------------------------------------------------------

#[cfg(feature = "semantic")]
mod openai {
    use super::{Config, EmbeddingProvider, Result, WgError};
    use serde::Deserialize;

    pub struct OpenAICompatibleProvider {
        endpoint: String,
        model: String,
        api_key: Option<String>,
        dimension: usize,
        nice_name: String,
    }

    impl OpenAICompatibleProvider {
        pub fn from_config(config: &Config) -> Result<Self> {
            let endpoint = config.model.endpoint.trim();
            if endpoint.is_empty() {
                return Err(WgError::InvalidInput(
                    "model.endpoint is required for the openai provider (e.g. http://localhost:11434/v1/embeddings)".into(),
                ));
            }
            let api_key = if config.model.api_key_env.trim().is_empty() {
                None
            } else {
                std::env::var(&config.model.api_key_env).ok()
            };
            // Try to honor user-set dimension; auto-detect on first call if 0.
            let dim = config.model.dimension;
            let nice_name = if endpoint.contains("11434") {
                "ollama".to_string()
            } else if endpoint.contains("openai.com") {
                "openai".to_string()
            } else if endpoint.contains("openrouter") {
                "openrouter".to_string()
            } else {
                "openai-compat".to_string()
            };
            Ok(Self {
                endpoint: endpoint.to_string(),
                model: config.model.name.clone(),
                api_key,
                dimension: dim,
                nice_name,
            })
        }
    }

    #[derive(Debug, Deserialize)]
    struct EmbeddingsResponse {
        data: Vec<EmbeddingItem>,
    }
    #[derive(Debug, Deserialize)]
    struct EmbeddingItem {
        embedding: Vec<f32>,
    }

    impl EmbeddingProvider for OpenAICompatibleProvider {
        fn name(&self) -> String {
            format!("{}({})", self.nice_name, self.model)
        }
        fn dimension(&self) -> usize {
            self.dimension
        }
        fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let mut v = self.embed_batch(&[text.to_string()])?;
            Ok(v.pop().unwrap_or_default())
        }
        fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let body = serde_json::json!({
                "model": self.model,
                "input": texts,
            });
            let mut req = ureq::post(&self.endpoint).set("Content-Type", "application/json");
            if let Some(key) = &self.api_key {
                req = req.set("Authorization", &format!("Bearer {key}"));
            }
            let resp = req.send_json(body).map_err(|e| {
                WgError::Internal(format!(
                    "embedding request to {} failed: {e}",
                    self.endpoint
                ))
            })?;
            let parsed: EmbeddingsResponse = resp
                .into_json()
                .map_err(|e| WgError::Internal(format!("embedding response parse: {e}")))?;
            Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
        }
    }
}

// ---------------------------------------------------------------------------
// HuggingFace text-embeddings-inference (native)
// ---------------------------------------------------------------------------
//
// TEI is also reachable through the `openai` provider via its
// `/v1/embeddings` compat endpoint, but the native `/embed` endpoint
// is faster (no OpenAI envelope to parse) and `/info` lets us
// auto-discover the model id, dim, and max input length without the
// user having to edit `model.dimension` by hand.

#[cfg(feature = "semantic")]
mod tei {
    use super::{Config, EmbeddingProvider, Result, WgError};
    use serde::Deserialize;

    pub struct TeiProvider {
        embed_endpoint: String,
        api_key: Option<String>,
        dimension: usize,
        nice_name: String,
        max_input_length: Option<usize>,
        /// Hard cap on `texts.len()` per HTTP request. TEI rejects
        /// batches larger than `max_client_batch_size` (default 32)
        /// with HTTP 413; bigger requests hit the gateway's body
        /// limit anyway. We chunk transparently in `embed_batch`.
        max_client_batch_size: usize,
    }

    impl TeiProvider {
        pub fn from_config(config: &Config) -> Result<Self> {
            let endpoint = config.model.endpoint.trim();
            if endpoint.is_empty() {
                return Err(WgError::InvalidInput(
                    "model.endpoint is required for the tei provider \
                     (e.g. http://localhost:8080 — point at the TEI base URL, \
                     not /v1/embeddings)"
                        .into(),
                ));
            }
            let base = endpoint.trim_end_matches('/').to_string();
            // If the user accidentally pasted /v1/embeddings or /embed, strip
            // it — TEI's native endpoint set is at the root.
            let base = base
                .trim_end_matches("/v1/embeddings")
                .trim_end_matches("/embed")
                .trim_end_matches('/')
                .to_string();
            let embed_endpoint = format!("{base}/embed");
            let info_endpoint = format!("{base}/info");
            let api_key = if config.model.api_key_env.trim().is_empty() {
                None
            } else {
                std::env::var(&config.model.api_key_env).ok()
            };

            // Auto-discover model id + dimension via /info. Falls back
            // to whatever the user configured in `model.dimension` if
            // /info isn't reachable — the operator may know the
            // dimension and want to skip the round-trip on every
            // process start.
            let info = fetch_info(&info_endpoint, api_key.as_deref()).ok();
            let model_id = info
                .as_ref()
                .map(|i| i.model_id.clone())
                .unwrap_or_else(|| config.model.name.clone());
            let max_input_length = info.as_ref().and_then(|i| i.max_input_length);
            let discovered_dim = info.as_ref().and_then(|i| i.dimension);
            // TEI publishes `max_client_batch_size` in /info (default
            // 32). We round it to a sane chunk size; 32 is also the
            // TEI documented hard cap for `/rerank`.
            let max_client_batch_size = info
                .as_ref()
                .and_then(|i| i.max_client_batch_size)
                .unwrap_or(32)
                .max(1);

            let dimension = if let Some(d) = discovered_dim {
                d
            } else if config.model.dimension > 0 {
                config.model.dimension
            } else {
                // Probe /embed once with a single token to learn the dim.
                probe_dimension(&embed_endpoint, api_key.as_deref())?
            };

            let nice_name = format!("tei({})", model_id);

            Ok(Self {
                embed_endpoint,
                api_key,
                dimension,
                nice_name,
                max_input_length,
                max_client_batch_size,
            })
        }
    }

    /// `GET /info`. Some TEI builds don't include `dimension` in the
    /// response; we extract whatever's there and fall back to a probe
    /// on the embed endpoint when needed.
    #[derive(Debug, Deserialize)]
    struct InfoResponse {
        #[serde(default)]
        model_id: String,
        #[serde(default)]
        max_input_length: Option<usize>,
        #[serde(default)]
        dimension: Option<usize>,
        #[serde(default)]
        max_client_batch_size: Option<usize>,
    }

    fn fetch_info(url: &str, api_key: Option<&str>) -> Result<InfoResponse> {
        let mut req = ureq::get(url);
        if let Some(key) = api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }
        let resp = req
            .call()
            .map_err(|e| WgError::Internal(format!("tei /info request failed at {url}: {e}")))?;
        resp.into_json()
            .map_err(|e| WgError::Internal(format!("tei /info parse failed: {e}")))
    }

    fn probe_dimension(embed_endpoint: &str, api_key: Option<&str>) -> Result<usize> {
        let body = serde_json::json!({"inputs": "."});
        let mut req = ureq::post(embed_endpoint).set("Content-Type", "application/json");
        if let Some(key) = api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }
        let resp = req.send_json(body).map_err(|e| {
            WgError::Internal(format!(
                "tei dimension probe failed at {embed_endpoint}: {e}"
            ))
        })?;
        let vectors: Vec<Vec<f32>> = resp
            .into_json()
            .map_err(|e| WgError::Internal(format!("tei dimension probe parse: {e}")))?;
        vectors
            .into_iter()
            .next()
            .map(|v| v.len())
            .ok_or_else(|| WgError::Internal("tei /embed returned an empty array".into()))
    }

    impl EmbeddingProvider for TeiProvider {
        fn name(&self) -> String {
            self.nice_name.clone()
        }
        fn dimension(&self) -> usize {
            self.dimension
        }
        fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let mut v = self.embed_batch(&[text.to_string()])?;
            Ok(v.pop().unwrap_or_default())
        }
        fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            // TEI rejects requests with `texts.len() > max_client_batch_size`
            // (default 32) — anything larger gets HTTP 413. wg's call sites
            // routinely hand in 5500+ facts at once during HNSW rebuild,
            // so we transparently chunk into batches of `max_client_batch_size`
            // and stitch the results back together.
            let chunk_size = self.max_client_batch_size;
            let mut out = Vec::with_capacity(texts.len());
            for chunk in texts.chunks(chunk_size) {
                let body = if let Some(max_len) = self.max_input_length {
                    serde_json::json!({
                        "inputs": chunk,
                        "truncate": true,
                        "truncate_direction": "Right",
                        "max_input_length": max_len
                    })
                } else {
                    serde_json::json!({"inputs": chunk, "truncate": true})
                };
                let mut req =
                    ureq::post(&self.embed_endpoint).set("Content-Type", "application/json");
                if let Some(key) = &self.api_key {
                    req = req.set("Authorization", &format!("Bearer {key}"));
                }
                let resp = req.send_json(body).map_err(|e| {
                    WgError::Internal(format!(
                        "tei embed request to {} failed: {e}",
                        self.embed_endpoint
                    ))
                })?;
                let mut parsed: Vec<Vec<f32>> = resp
                    .into_json()
                    .map_err(|e| WgError::Internal(format!("tei /embed response parse: {e}")))?;
                out.append(&mut parsed);
            }
            Ok(out)
        }
    }
}
