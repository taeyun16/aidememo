//! Configuration management for WikiGraph.
//!
//! Reads ~/.wg/config.toml and provides typed access to settings.

use crate::error::{Result, WgError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// WikiGraph configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub store: StoreConfig,
    pub model: ModelConfig,
    pub search: SearchConfig,
    pub lint: LintConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            store: StoreConfig::default(),
            model: ModelConfig::default(),
            search: SearchConfig::default(),
            lint: LintConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StoreConfig {
    /// Path to the redb file (relative to wiki root or absolute).
    pub path: String,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: "./_meta/wiki.redb".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelConfig {
    /// Model name (e.g., "minishlab/potion-multilingual-128M").
    pub name: String,
    /// Directory where downloaded model artifacts are stored.
    #[serde(default = "default_model_download_dir")]
    pub download_dir: String,
    /// Model cache directory.
    pub cache_dir: String,
    /// Auto-download model on first use.
    pub auto_download: bool,
}

fn default_model_download_dir() -> String {
    "~/.wg/models/downloads".to_string()
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: "minishlab/potion-multilingual-128M".to_string(),
            download_dir: default_model_download_dir(),
            cache_dir: "~/.wg/models".to_string(),
            auto_download: true,
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
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_limit: 10,
            min_trust: 0.0,
            bm25_weight: 1.0,
            semantic_weight: 1.0,
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

impl Config {
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
            _ => Err(WgError::ConfigKeyNotFound(key.to_string())),
        }
    }
}

impl StoreConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "path" => Some(self.path.clone()),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "path" => {
                self.path = value.to_string();
                Ok(())
            }
            _ => Err(WgError::ConfigKeyNotFound(format!("store.{}", key))),
        }
    }
}

impl ModelConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "name" => Some(self.name.clone()),
            "download_dir" => Some(self.download_dir.clone()),
            "cache_dir" => Some(self.cache_dir.clone()),
            "auto_download" => Some(self.auto_download.to_string()),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
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
}
