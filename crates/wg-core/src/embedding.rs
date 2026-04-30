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
    if cfg!(feature = "fastembed") {
        &["model2vec", "openai", "tei", "fastembed"]
    } else {
        &["model2vec", "openai", "tei"]
    }
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
        #[cfg(feature = "fastembed")]
        "fastembed" | "bge" | "onnx" => Ok(Box::new(
            fastembed_provider::FastembedProvider::from_config(config)?,
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

// ---------------------------------------------------------------------------
// fastembed (ONNX-Runtime-backed BGE / E5 / Nomic models)
// ---------------------------------------------------------------------------

/// `fastembed` provider — wraps the [`fastembed`] crate, which itself
/// runs `pykeio/ort` (ONNX Runtime) on CPU. Brings parity with the
/// embedding choices that English-tuned competitors (OMEGA's
/// `bge-small-en-v1.5`, Mastra's stack) use, while keeping wg's
/// default Model2Vec path untouched. Opt in by:
///
///   1. building wg with `--features fastembed`
///   2. setting `model.provider = "fastembed"` in `~/.wg/config.toml`
///   3. setting `model.name = "bge-small-en-v1.5"` (or any other
///      [`EmbeddingModel`] enum the fastembed crate ships)
///
/// First call downloads the ONNX weights to the user's HF cache
/// (`~/.cache/huggingface/`). Subsequent calls are local.
#[cfg(feature = "fastembed")]
mod fastembed_provider {
    use super::{Config, EmbeddingProvider, Result, WgError};
    use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
    use parking_lot::Mutex;

    pub struct FastembedProvider {
        // fastembed's TextEmbedding holds the ONNX session, which the
        // crate marks `&mut self`-required for `embed`. Wrap in a Mutex
        // so the provider trait can stay `&self`.
        inner: Mutex<TextEmbedding>,
        model_name: String,
        dim: usize,
    }

    impl FastembedProvider {
        pub fn from_config(config: &Config) -> Result<Self> {
            let model_id = if config.model.name.is_empty() {
                "bge-small-en-v1.5".to_string()
            } else {
                config.model.name.clone()
            };
            let (model_enum, dim) = parse_model(&model_id)?;
            let mut opts = InitOptions::new(model_enum);
            // Honour the user's configured cache_dir — keeps every wg
            // download under the same root rather than scattering
            // model weights across HF + Model2Vec caches.
            if !config.model.cache_dir.is_empty() {
                let cache = expand_tilde(&config.model.cache_dir);
                if !cache.as_os_str().is_empty() {
                    opts = opts.with_cache_dir(cache);
                }
            }
            let inner = TextEmbedding::try_new(opts).map_err(|e| WgError::ModelLoadFailed {
                path: std::path::PathBuf::from(&model_id),
                source: Box::new(std::io::Error::other(e.to_string())),
            })?;

            Ok(Self {
                inner: Mutex::new(inner),
                model_name: model_id,
                dim,
            })
        }
    }

    impl EmbeddingProvider for FastembedProvider {
        fn name(&self) -> String {
            format!("fastembed({})", self.model_name)
        }
        fn dimension(&self) -> usize {
            self.dim
        }
        fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let mut guard = self.inner.lock();
            let mut out = guard
                .embed(vec![text.to_string()], None)
                .map_err(|e| WgError::Internal(format!("fastembed embed: {e}")))?;
            out.pop()
                .ok_or_else(|| WgError::Internal("fastembed returned empty batch".into()))
        }
        fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let mut guard = self.inner.lock();
            guard
                .embed(texts, None)
                .map_err(|e| WgError::Internal(format!("fastembed embed_batch: {e}")))
        }
    }

    /// Map a config string to the fastembed `EmbeddingModel` enum
    /// AND its known output dimension (BGE / E5 / MiniLM models all
    /// have published dimension counts). Hardcoding the dim avoids a
    /// runtime probe inference per provider construction.
    /// Accepts both lowercase / hyphenated forms ("bge-small-en-v1.5")
    /// and the verbatim PascalCase enum names ("BGESmallENV15") so
    /// existing config.toml setups don't have to learn the crate's
    /// naming convention. Unknown names error rather than silently
    /// falling back — saves a 90 MB download if the user typo'd.
    fn parse_model(s: &str) -> Result<(EmbeddingModel, usize)> {
        let canon = s.to_lowercase().replace(['_', ' '], "-");
        Ok(match canon.as_str() {
            "bge-small-en-v1.5" | "bgesmallenv15" => (EmbeddingModel::BGESmallENV15, 384),
            "bge-base-en-v1.5" | "bgebaseenv15" => (EmbeddingModel::BGEBaseENV15, 768),
            "bge-large-en-v1.5" | "bgelargeenv15" => (EmbeddingModel::BGELargeENV15, 1024),
            "bge-small-zh-v1.5" | "bgesmallzhv15" => (EmbeddingModel::BGESmallZHV15, 512),
            "bge-large-zh-v1.5" | "bgelargezhv15" => (EmbeddingModel::BGELargeZHV15, 1024),
            "bge-m3" | "bgem3" => (EmbeddingModel::BGEM3, 1024),
            "all-mini-lm-l6-v2" | "all-minilm-l6-v2" | "allminilml6v2" => {
                (EmbeddingModel::AllMiniLML6V2, 384)
            }
            "all-mini-lm-l12-v2" | "all-minilm-l12-v2" | "allminilml12v2" => {
                (EmbeddingModel::AllMiniLML12V2, 384)
            }
            "all-mpnet-base-v2" | "allmpnetbasev2" => (EmbeddingModel::AllMpnetBaseV2, 768),
            "multilingual-e5-small" | "multilinguale5small" => {
                (EmbeddingModel::MultilingualE5Small, 384)
            }
            "multilingual-e5-base" | "multilinguale5base" => {
                (EmbeddingModel::MultilingualE5Base, 768)
            }
            "multilingual-e5-large" | "multilinguale5large" => {
                (EmbeddingModel::MultilingualE5Large, 1024)
            }
            "nomic-embed-text-v1.5" | "nomicembedtextv15" => {
                (EmbeddingModel::NomicEmbedTextV15, 768)
            }
            "jina-embeddings-v2-base-en" | "jinaembeddingsv2baseen" => {
                (EmbeddingModel::JinaEmbeddingsV2BaseEN, 768)
            }
            other => {
                return Err(WgError::InvalidInput(format!(
                    "unknown fastembed model '{other}' — accepted: bge-small-en-v1.5 \
                     (default), bge-base-en-v1.5, bge-large-en-v1.5, bge-m3, \
                     all-mini-lm-l6-v2, all-mini-lm-l12-v2, all-mpnet-base-v2, \
                     multilingual-e5-small/base/large, nomic-embed-text-v1.5, \
                     jina-embeddings-v2-base-en. See \
                     https://github.com/Anush008/fastembed-rs#supported-models"
                )));
            }
        })
    }

    fn expand_tilde(s: &str) -> std::path::PathBuf {
        if let Some(rest) = s.strip_prefix("~/")
            && let Some(home) = std::env::var_os("HOME")
        {
            return std::path::PathBuf::from(home).join(rest);
        }
        std::path::PathBuf::from(s)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::config::Config;

        // The smoke tests download model weights from HuggingFace on
        // first run (~90 MB for bge-small-en-v1.5). Marked #[ignore]
        // so CI stays offline.
        #[test]
        #[ignore = "downloads ONNX weights from HuggingFace — local only"]
        fn fastembed_provider_loads_default_bge() {
            let mut config = Config::default();
            config.model.provider = "fastembed".into();
            config.model.name = "bge-small-en-v1.5".into();
            let p = FastembedProvider::from_config(&config).unwrap();
            assert_eq!(p.dimension(), 384);
            let v = p.embed("hello world").unwrap();
            assert_eq!(v.len(), 384);
            assert!(
                v.iter().any(|x| *x != 0.0),
                "embedding should not be all-zeros"
            );
        }

        #[test]
        fn fastembed_parser_accepts_canonical_names() {
            let cases: &[(&str, usize)] = &[
                ("bge-small-en-v1.5", 384),
                ("BGESmallENV15", 384),
                ("bge-base-en-v1.5", 768),
                ("multilingual-e5-large", 1024),
                ("nomic-embed-text-v1.5", 768),
            ];
            for (name, dim) in cases {
                let (_, d) = parse_model(name).expect(name);
                assert_eq!(d, *dim, "dim mismatch for {name}");
            }
        }

        #[test]
        fn fastembed_parser_rejects_unknown_with_helpful_message() {
            let err = parse_model("bge-tiny-en-v1.5").unwrap_err().to_string();
            assert!(err.contains("unknown fastembed model"));
            // Must list at least one accepted name so the typo is
            // recoverable from the error alone.
            assert!(err.contains("bge-small-en-v1.5"));
        }
    }
}
