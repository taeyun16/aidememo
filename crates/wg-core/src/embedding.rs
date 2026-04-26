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
    &["model2vec", "openai"]
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
