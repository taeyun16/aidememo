//! Configuration management for WikiGraph.
//!
//! Reads ~/.wg/config.toml and provides typed access to settings.

use crate::error::{Result, WgError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// WikiGraph configuration.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    pub store: StoreConfig,
    pub model: ModelConfig,
    pub search: SearchConfig,
    pub lint: LintConfig,
    /// Optional cross-encoder reranker for `hybrid_search` results.
    /// Disabled by default; set `rerank.provider = "tei"` to enable.
    #[serde(default)]
    pub rerank: RerankConfig,
    /// Named projects (multi-store support). Empty by default.
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectConfig>,
    /// Name of the project to use when neither --store nor --project is given.
    /// If unset (or the project is missing), falls back to `store.path`.
    #[serde(default)]
    pub default_project: Option<String>,
}

/// One registered project (name → store path).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProjectConfig {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StoreConfig {
    /// Path to the redb file (relative to wiki root or absolute).
    pub path: String,
    /// redb commit durability. Two options:
    ///
    /// - `"immediate"` (default, recommended) — every commit fsyncs to
    ///   disk before returning. Survives both process crash and power
    ///   loss. Floors single-fact `fact_add` at the OS fsync cost
    ///   (~3-5 ms on macOS APFS, ~0.1-1 ms on Linux ext4).
    /// - `"eventual"` — commits are queued; the kernel's page cache
    ///   eventually flushes. Survives process crash (page cache
    ///   outlives the process), but power loss within ~30s of a write
    ///   can lose recent commits. About 10× faster than `immediate`.
    ///   Opt in only when the workload (e.g. high-frequency LLM fact
    ///   capture where re-running is cheap) tolerates that exposure.
    ///
    /// `Durability::None` is intentionally not exposed — redb's docs
    /// note that exclusive use causes "rapid growth of the database
    /// file" because pages aren't freed until a higher-durability
    /// commit, which is too easy to misuse.
    #[serde(default = "default_durability")]
    pub durability: String,
}

fn default_durability() -> String {
    "immediate".to_string()
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: "./_meta/wiki.redb".to_string(),
            durability: default_durability(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelConfig {
    /// Embedding provider: "model2vec" (default, offline) or "openai"
    /// (HTTP — works for OpenAI / Ollama / OpenRouter / vLLM / LocalAI).
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Model name. For model2vec this is the HuggingFace handle; for
    /// openai-compat it's the model id sent in the request body
    /// (e.g. "text-embedding-3-small", "nomic-embed-text").
    pub name: String,
    /// Directory where downloaded model artifacts are stored.
    #[serde(default = "default_model_download_dir")]
    pub download_dir: String,
    /// Model cache directory.
    pub cache_dir: String,
    /// Auto-download model on first use (model2vec only).
    pub auto_download: bool,
    /// HTTP endpoint for openai-compat providers, e.g.
    /// `http://localhost:11434/v1/embeddings` or
    /// `https://api.openai.com/v1/embeddings`.
    #[serde(default)]
    pub endpoint: String,
    /// Env var name to read the API key from (e.g. `OPENAI_API_KEY`).
    /// Empty means no auth header sent — fine for Ollama / LocalAI.
    #[serde(default)]
    pub api_key_env: String,
    /// Vector dimension. Required for openai-compat (different models
    /// have different dims); for model2vec it's auto-detected.
    #[serde(default)]
    pub dimension: usize,
    /// model2vec only: quantize the embedding matrix to int8 at load
    /// time. Cuts heap from ~489 MB to ~124 MB on the 128M model with
    /// <0.5% cosine similarity loss. mmap zero-copy is given up when
    /// this is on.
    #[serde(default)]
    pub quantize: bool,
}

fn default_provider() -> String {
    "model2vec".to_string()
}

fn default_model_download_dir() -> String {
    "~/.wg/models/downloads".to_string()
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            name: "minishlab/potion-multilingual-128M".to_string(),
            download_dir: default_model_download_dir(),
            cache_dir: "~/.wg/models".to_string(),
            auto_download: true,
            endpoint: String::new(),
            api_key_env: String::new(),
            dimension: 0,
            quantize: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchConfig {
    /// Default number of results to return.
    pub default_limit: usize,
    /// Minimum trust (source_confidence) threshold.
    pub min_trust: f32,
    /// BM25 weight in RRF fusion.
    pub bm25_weight: f32,
    /// Semantic weight in RRF fusion.
    pub semantic_weight: f32,
    /// Tier 7-B: how many BM25 candidates to feed into the semantic
    /// re-ranker. Cap on per-query embedding inference cost. 0 disables
    /// the prefilter (fall back to scoring every fact).
    #[serde(default = "default_semantic_prefilter")]
    pub semantic_prefilter: usize,
    /// Tier 7-D: enable graph-aware prefilter — facts attached to
    /// entities in the BM25 result set, plus their N-hop neighbors,
    /// are added to the semantic re-ranker's candidate pool. Closes
    /// the gap where BM25 misses semantic-only matches but graph
    /// neighborhood would have surfaced them.
    #[serde(default = "default_true")]
    pub graph_prefilter: bool,
    /// Tier 7-D: how many hops to expand from BM25 result entities
    /// (1 = direct neighbors only).
    #[serde(default = "default_graph_depth")]
    pub graph_depth: u32,
    /// Tier 7-D: cap on extra facts pulled in from graph expansion.
    /// Bounds the worst-case candidate pool size.
    #[serde(default = "default_graph_fact_cap")]
    pub graph_fact_cap: usize,
    /// Tier 8: which semantic candidate path to use.
    ///   - `"bm25"`  (default) — top-`semantic_prefilter` BM25 hits
    ///                 + graph expansion → semantic re-rank.
    ///                 Cheap; ties on accuracy when fact text shares
    ///                 keywords with the query.
    ///   - `"hnsw"` — HNSW ANN over fact embeddings.
    ///                 Closes the recall gap on languages where BM25
    ///                 tokenization is weak (Korean, Japanese, etc.).
    ///                 Requires `wg vector-rebuild` once after the
    ///                 store is populated; rebuild is automatic if
    ///                 the sidecar's model name no longer matches.
    #[serde(default = "default_semantic_index")]
    pub semantic_index: String,
    /// Multiplicative weighting of `source_confidence` and
    /// `relevance_score` into the final hybrid-search ranking. When
    /// `true` (default), a fact's RRF-fused score is multiplied by
    /// `source_confidence × max(relevance_score, 0.1)` so a low-trust
    /// fact ranks below an equally-relevant high-trust one. Setting
    /// `false` reverts to the binary `min_trust` filter only — useful
    /// for debugging or when every fact in the wiki is hand-curated.
    #[serde(default = "default_true")]
    pub weight_by_confidence: bool,
    /// Time-decay constant in milliseconds. A fact's score is
    /// multiplied by `exp(-age_ms / time_decay_tau_ms)` where
    /// `age_ms = now - (observed_at OR created_at)`. The default is
    /// 90 days — a fact stays at >50% weight for ~62 days, drops to
    /// ~37% at 90 days, and ~5% by 9 months. Set to `0` to disable
    /// (every fact gets weight 1.0 regardless of age).
    #[serde(default = "default_time_decay_tau")]
    pub time_decay_tau_ms: u64,
}

fn default_semantic_prefilter() -> usize {
    50
}

fn default_true() -> bool {
    true
}

fn default_graph_depth() -> u32 {
    1
}

fn default_graph_fact_cap() -> usize {
    50
}

fn default_time_decay_tau() -> u64 {
    // 90 days in milliseconds. Picked so a 60-day-old decision still
    // ranks at >50% weight (e^(-2/3) ≈ 0.51) and a fact older than a
    // year drops below 5% — enough to push stale claims behind fresh
    // ones without erasing them outright. Operators can dial this up
    // for archival wikis or down for fast-moving project notes.
    90 * 24 * 60 * 60 * 1000
}

fn default_semantic_index() -> String {
    // HNSW gives the bigger recall on multilingual / paraphrase
    // workloads (+12% R@10 on Korean MIRACL) and ties on English
    // synthetic data. The build cost at ingest time is sub-4s
    // even at 5500 facts (see .notes/bench-hnsw-integrated.md and
    // the hnsw_timing benchmark). Operators on tiny wikis or
    // latency-bound English-only deployments can flip back to
    // "bm25" with `wg config set search.semantic_index bm25`.
    "hnsw".to_string()
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_limit: 10,
            min_trust: 0.0,
            bm25_weight: 1.0,
            semantic_weight: 1.0,
            semantic_prefilter: default_semantic_prefilter(),
            graph_prefilter: default_true(),
            graph_depth: default_graph_depth(),
            graph_fact_cap: default_graph_fact_cap(),
            semantic_index: default_semantic_index(),
            weight_by_confidence: default_true(),
            time_decay_tau_ms: default_time_decay_tau(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LintConfig {
    /// Minimum inbound links for an entity to not be considered orphan.
    pub orphan_threshold: u32,
    /// Number of days before an entity/fact is considered stale.
    pub stale_days: u32,
    /// Trigram similarity threshold for duplicate detection.
    pub duplicate_similarity: f32,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            orphan_threshold: 0,
            stale_days: 90,
            duplicate_similarity: 0.9,
        }
    }
}

/// Optional cross-encoder reranker that runs after RRF fusion in
/// `hybrid_search`. Default state is "disabled" — `provider = ""`.
/// Set `rerank.provider = "tei"` and `rerank.endpoint = ...` to
/// enable.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RerankConfig {
    /// Reranker provider name. Empty string disables reranking.
    /// Currently only `"tei"` (HuggingFace text-embeddings-inference,
    /// `/rerank` endpoint) is supported.
    #[serde(default)]
    pub provider: String,
    /// Base URL of the reranker server. The `/rerank` path is added
    /// internally; both `http://host:8081` and the explicit
    /// `http://host:8081/rerank` form are accepted.
    #[serde(default)]
    pub endpoint: String,
    /// Reranker model id (e.g. `BAAI/bge-reranker-base`). Free-form;
    /// shown in the doctor / health output and not validated.
    #[serde(default)]
    pub model: String,
    /// Env var name that holds an `Authorization: Bearer ...` token
    /// for the reranker endpoint. Empty means no auth header.
    #[serde(default)]
    pub api_key_env: String,
    /// How many of the top RRF candidates to send to the reranker.
    /// Cross-encoders cost roughly 10 ms per pair on Metal-accelerated
    /// TEI, ~50 ms per pair under Docker amd64 emulation, so the
    /// per-search rerank tax is `top_k × per-pair-cost` — see
    /// `.notes/bench-tei-overhead.md` for the measured curve.
    /// Default 8 is a compromise: it adds ~80 ms p50 on a native
    /// Apple Silicon host while still polishing the head of the
    /// list. Bump to 16/32 for recall-heavy work where the latency
    /// budget allows. Note: TEI's `max_client_batch_size = 32` is
    /// the upstream cap unless the operator redeploys with a higher
    /// flag.
    #[serde(default = "default_rerank_top_k")]
    pub top_k: usize,
}

fn default_rerank_top_k() -> usize {
    8
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            endpoint: String::new(),
            model: String::new(),
            api_key_env: String::new(),
            top_k: default_rerank_top_k(),
        }
    }
}

impl RerankConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "provider" => Some(self.provider.clone()),
            "endpoint" => Some(self.endpoint.clone()),
            "model" => Some(self.model.clone()),
            "api_key_env" => Some(self.api_key_env.clone()),
            "top_k" => Some(self.top_k.to_string()),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "provider" => {
                let v = value.trim().to_string();
                // Validate against the known set so a typo doesn't
                // silently disable rerank at search time.
                if !matches!(v.as_str(), "" | "tei" | "text-embeddings-inference") {
                    return Err(WgError::InvalidInput(format!(
                        "rerank.provider must be one of [\"\", \"tei\"], got '{value}'"
                    )));
                }
                self.provider = v;
                Ok(())
            }
            "endpoint" => {
                self.endpoint = value.trim().to_string();
                Ok(())
            }
            "model" => {
                self.model = value.trim().to_string();
                Ok(())
            }
            "api_key_env" => {
                self.api_key_env = value.trim().to_string();
                Ok(())
            }
            "top_k" => {
                self.top_k = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|e| WgError::InvalidInput(format!("rerank.top_k: {e}")))?;
                Ok(())
            }
            _ => Err(WgError::ConfigKeyNotFound(format!("rerank.{}", key))),
        }
    }
}

impl Config {
    /// Resolve the store path for a named project.
    ///
    /// Returns `None` if the project doesn't exist; expands `~` to the home
    /// directory if present.
    pub fn project_path(&self, name: &str) -> Option<PathBuf> {
        self.projects.get(name).map(|p| expand_home(&p.path))
    }

    /// Resolve the store path that should be used when no `--store` / `--project`
    /// is given. Falls through `default_project` → `store.path`.
    pub fn default_store_path(&self) -> PathBuf {
        if let Some(name) = &self.default_project {
            if let Some(p) = self.project_path(name) {
                return p;
            }
        }
        expand_home(&self.store.path)
    }

    /// Load configuration from ~/.wg/config.toml.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    /// Load configuration from a specific path.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            // Return default config if file doesn't exist
            return Ok(Config::default());
        }

        let content = std::fs::read_to_string(path).map_err(|e| WgError::ConfigRead {
            path: path.to_path_buf(),
            source: e,
        })?;

        toml::from_str(&content).map_err(|e| WgError::ConfigParse {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Save configuration to ~/.wg/config.toml.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        self.save_to(&path)
    }

    /// Save configuration to a specific path.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WgError::ConfigRead {
                path: path.to_path_buf(),
                source: e,
            })?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| WgError::Internal(format!("failed to serialize config: {e}")))?;

        std::fs::write(path, content).map_err(|e| WgError::ConfigRead {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Get the configuration file path.
    fn config_path() -> Result<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| WgError::InvalidInput("cannot find home directory".to_string()))?;
        Ok(home.join(".wg").join("config.toml"))
    }

    /// Get a config value by key path (e.g., "model.name").
    pub fn get(&self, key: &str) -> Option<String> {
        let parts: Vec<&str> = key.split('.').collect();
        match parts.as_slice() {
            ["store", k] => self.store.get(k),
            ["model", k] => self.model.get(k),
            ["search", k] => self.search.get(k),
            ["lint", k] => self.lint.get(k),
            ["rerank", k] => self.rerank.get(k),
            _ => None,
        }
    }

    /// Set a config value by key path.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let parts: Vec<&str> = key.split('.').collect();
        match parts.as_slice() {
            ["store", k] => self.store.set(k, value),
            ["model", k] => self.model.set(k, value),
            ["search", k] => self.search.set(k, value),
            ["lint", k] => self.lint.set(k, value),
            ["rerank", k] => self.rerank.set(k, value),
            _ => Err(WgError::ConfigKeyNotFound(key.to_string())),
        }
    }
}

impl StoreConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "path" => Some(self.path.clone()),
            "durability" => Some(self.durability.clone()),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "path" => {
                self.path = value.to_string();
                Ok(())
            }
            "durability" => {
                let normalized = value.to_lowercase();
                match normalized.as_str() {
                    "immediate" | "eventual" => {
                        self.durability = normalized;
                        Ok(())
                    }
                    _ => Err(WgError::InvalidInput(format!(
                        "store.durability must be 'immediate' or 'eventual', got '{}'",
                        value
                    ))),
                }
            }
            _ => Err(WgError::ConfigKeyNotFound(format!("store.{}", key))),
        }
    }
}

impl ModelConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "provider" => Some(self.provider.clone()),
            "name" => Some(self.name.clone()),
            "download_dir" => Some(self.download_dir.clone()),
            "cache_dir" => Some(self.cache_dir.clone()),
            "auto_download" => Some(self.auto_download.to_string()),
            "endpoint" => Some(self.endpoint.clone()),
            "api_key_env" => Some(self.api_key_env.clone()),
            "dimension" => Some(self.dimension.to_string()),
            "quantize" => Some(self.quantize.to_string()),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "provider" => {
                self.provider = value.to_string();
                Ok(())
            }
            "name" => {
                self.name = value.to_string();
                Ok(())
            }
            "download_dir" => {
                self.download_dir = value.to_string();
                Ok(())
            }
            "cache_dir" => {
                self.cache_dir = value.to_string();
                Ok(())
            }
            "auto_download" => {
                self.auto_download = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid boolean: {}", value)))?;
                Ok(())
            }
            "endpoint" => {
                self.endpoint = value.to_string();
                Ok(())
            }
            "api_key_env" => {
                self.api_key_env = value.to_string();
                Ok(())
            }
            "dimension" => {
                self.dimension = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid integer: {}", value)))?;
                Ok(())
            }
            "quantize" => {
                self.quantize = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid boolean: {}", value)))?;
                Ok(())
            }
            _ => Err(WgError::ConfigKeyNotFound(format!("model.{}", key))),
        }
    }
}

impl SearchConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "default_limit" => Some(self.default_limit.to_string()),
            "min_trust" => Some(self.min_trust.to_string()),
            "bm25_weight" => Some(self.bm25_weight.to_string()),
            "semantic_weight" => Some(self.semantic_weight.to_string()),
            "semantic_prefilter" => Some(self.semantic_prefilter.to_string()),
            "graph_prefilter" => Some(self.graph_prefilter.to_string()),
            "graph_depth" => Some(self.graph_depth.to_string()),
            "graph_fact_cap" => Some(self.graph_fact_cap.to_string()),
            "semantic_index" => Some(self.semantic_index.clone()),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "default_limit" => {
                self.default_limit = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid integer: {}", value)))?;
                Ok(())
            }
            "min_trust" => {
                self.min_trust = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid float: {}", value)))?;
                Ok(())
            }
            "bm25_weight" => {
                self.bm25_weight = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid float: {}", value)))?;
                Ok(())
            }
            "semantic_weight" => {
                self.semantic_weight = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid float: {}", value)))?;
                Ok(())
            }
            "semantic_prefilter" => {
                self.semantic_prefilter = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid integer: {}", value)))?;
                Ok(())
            }
            "graph_prefilter" => {
                self.graph_prefilter = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid boolean: {}", value)))?;
                Ok(())
            }
            "graph_depth" => {
                self.graph_depth = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid integer: {}", value)))?;
                Ok(())
            }
            "graph_fact_cap" => {
                self.graph_fact_cap = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid integer: {}", value)))?;
                Ok(())
            }
            "semantic_index" => {
                let v = value.trim().to_ascii_lowercase();
                if v != "bm25" && v != "hnsw" {
                    return Err(WgError::InvalidInput(format!(
                        "search.semantic_index must be 'bm25' or 'hnsw', got '{}'",
                        value
                    )));
                }
                self.semantic_index = v;
                Ok(())
            }
            _ => Err(WgError::ConfigKeyNotFound(format!("search.{}", key))),
        }
    }
}

impl LintConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "orphan_threshold" => Some(self.orphan_threshold.to_string()),
            "stale_days" => Some(self.stale_days.to_string()),
            "duplicate_similarity" => Some(self.duplicate_similarity.to_string()),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "orphan_threshold" => {
                self.orphan_threshold = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid integer: {}", value)))?;
                Ok(())
            }
            "stale_days" => {
                self.stale_days = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid integer: {}", value)))?;
                Ok(())
            }
            "duplicate_similarity" => {
                self.duplicate_similarity = value
                    .parse()
                    .map_err(|_| WgError::InvalidInput(format!("invalid float: {}", value)))?;
                Ok(())
            }
            _ => Err(WgError::ConfigKeyNotFound(format!("lint.{}", key))),
        }
    }
}

/// Expand a leading `~` in a path string to the user's home directory.
fn expand_home(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Simple helper to get home directory.
mod dirs {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        std::env::var("HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.store.path, "./_meta/wiki.redb");
        assert_eq!(config.model.name, "minishlab/potion-multilingual-128M");
        assert_eq!(config.model.download_dir, "~/.wg/models/downloads");
        assert_eq!(config.search.default_limit, 10);
        assert_eq!(config.lint.stale_days, 90);
    }

    #[test]
    fn test_config_get_set() {
        let mut config = Config::default();
        assert_eq!(
            config.get("model.name"),
            Some("minishlab/potion-multilingual-128M".to_string())
        );

        config
            .set("model.name", "minishlab/potion-base-8M")
            .unwrap();
        assert_eq!(
            config.get("model.name"),
            Some("minishlab/potion-base-8M".to_string())
        );
    }

    #[test]
    fn store_durability_default_is_immediate() {
        let config = Config::default();
        assert_eq!(config.store.durability, "immediate");
        assert_eq!(
            config.get("store.durability"),
            Some("immediate".to_string())
        );
    }

    #[test]
    fn store_durability_accepts_eventual() {
        let mut config = Config::default();
        config.set("store.durability", "Eventual").unwrap();
        // Normalized to lowercase.
        assert_eq!(config.store.durability, "eventual");
    }

    #[test]
    fn store_durability_rejects_unknown_values() {
        let mut config = Config::default();
        let err = config
            .set("store.durability", "none")
            .expect_err("none should be rejected");
        assert!(format!("{err}").contains("immediate"));

        let err = config
            .set("store.durability", "fast")
            .expect_err("garbage should be rejected");
        assert!(format!("{err}").contains("immediate"));
    }

    #[test]
    fn rerank_default_is_disabled() {
        let cfg = Config::default();
        assert_eq!(cfg.rerank.provider, "");
        // Default top_k=8 was picked from the TEI-overhead bench
        // (.notes/bench-tei-overhead.md) so the rerank tax stays
        // around 80 ms p50 on native Metal.
        assert_eq!(cfg.rerank.top_k, 8);
        assert_eq!(cfg.get("rerank.provider"), Some(String::new()));
        assert_eq!(cfg.get("rerank.top_k"), Some("8".to_string()));
    }

    #[test]
    fn rerank_set_accepts_tei_and_clears_with_empty_string() {
        let mut cfg = Config::default();
        cfg.set("rerank.provider", "tei").unwrap();
        assert_eq!(cfg.rerank.provider, "tei");
        // Empty string disables — the check / search path treats it
        // as "no reranker configured."
        cfg.set("rerank.provider", "").unwrap();
        assert_eq!(cfg.rerank.provider, "");
    }

    #[test]
    fn rerank_set_rejects_unknown_provider() {
        let mut cfg = Config::default();
        let err = cfg
            .set("rerank.provider", "cohere")
            .expect_err("only tei is supported");
        assert!(format!("{err}").contains("tei"));
    }

    #[test]
    fn rerank_set_top_k_parses_integer() {
        let mut cfg = Config::default();
        cfg.set("rerank.top_k", "16").unwrap();
        assert_eq!(cfg.rerank.top_k, 16);
        let err = cfg
            .set("rerank.top_k", "lots")
            .expect_err("non-integer must error");
        assert!(format!("{err}").contains("rerank.top_k"));
    }
}
