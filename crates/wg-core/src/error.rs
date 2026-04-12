//! Structured error types for WikiGraph.
//!
//! All public APIs return `Result<T, WgError>`.
//! Errors include context (file paths, entity names, attempted operations).

use std::path::PathBuf;
use thiserror::Error;

/// WikiGraph error type.
///
/// All variants include contextual information for debugging.
#[derive(Error, Debug)]
pub enum WgError {
    // === Storage ===
    #[error("failed to open store at {path}: {source}")]
    StoreOpen {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("failed to read from table '{table}' key '{key}': {source}")]
    StoreRead {
        table: &'static str,
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("failed to write to table '{table}' key '{key}': {source}")]
    StoreWrite {
        table: &'static str,
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("failed to begin transaction: {source}")]
    TransactionBegin {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("transaction conflict (concurrent write): retry needed")]
    TransactionConflict,

    // === Serialization ===
    #[error("serialization failed for {context}: {source}")]
    Serialize {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("deserialization failed for {context}: {source}")]
    Deserialize {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    // === Entity ===
    #[error("entity not found: '{name}'")]
    EntityNotFound {
        name: String,
        suggestions: Vec<String>,
    },

    #[error("entity already exists: '{name}'")]
    EntityAlreadyExists { name: String },

    #[error("entity ID not found: '{0}'")]
    EntityIdNotFound(String),

    // === Fact ===
    #[error("fact not found: '{0}'")]
    FactNotFound(String),

    // === Relation ===
    #[error("relation not found: {source_name} --{rel_type}-> {target}")]
    RelationNotFound {
        source_name: String,
        rel_type: String,
        target: String,
    },

    #[error("cycle detected in path: {path:?}")]
    CycleDetected { path: Vec<String> },

    // === Graph ===
    #[error("path not found from '{from}' to '{to}'")]
    PathNotFound { from: String, to: String },

    // === Configuration ===
    #[error("failed to read config from {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config at {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("config key not found: '{0}'")]
    ConfigKeyNotFound(String),

    // === Ingest ===
    #[error("failed to parse frontmatter in {file}: {message}")]
    FrontmatterParse { file: PathBuf, message: String },

    #[error("failed to parse wikilink at {file}:{line}: {link}")]
    WikilinkParse {
        file: PathBuf,
        line: usize,
        link: String,
    },

    #[error("ingest failed: {0}")]
    IngestFailed(String),

    #[error("failed to read file {0}: {1}")]
    FileRead(PathBuf, String),

    // === Model (semantic feature) ===
    #[cfg(feature = "semantic")]
    #[error("model not found: '{name}' (cache dir: {cache_dir})")]
    ModelNotFound { name: String, cache_dir: PathBuf },

    #[cfg(feature = "semantic")]
    #[error("failed to download model '{name}': {source}")]
    ModelDownloadFailed {
        name: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[cfg(feature = "semantic")]
    #[error("failed to load model from {path}: {source}")]
    ModelLoadFailed {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[cfg(feature = "semantic")]
    #[error("model inference failed: {0}")]
    ModelInferenceFailed(String),

    // === Migration ===
    #[error("migration failed in phase '{phase}': {source}")]
    MigrationFailed {
        phase: String,
        #[source]
        source: Box<WgError>,
    },

    #[error("schema version mismatch: found {found}, expected {expected}")]
    SchemaVersionMismatch { found: u32, expected: u32 },

    #[error("unsupported schema version: {0}")]
    UnsupportedSchemaVersion(u32),

    // === S3 (feature-gated) ===
    #[cfg(feature = "remote")]
    #[error("remote IO failed for {url}: {source}")]
    RemoteIo {
        url: String,
        #[source]
        source: oneio::OneIoError,
    },

    // === Search ===
    #[error("search failed: {0}")]
    SearchFailed(String),

    #[error("index corrupted, rebuild required: {0}")]
    IndexCorrupted(String),

    // === Lint ===
    #[error("lint failed: {0}")]
    LintFailed(String),

    // === General ===
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl WgError {
    /// Create an EntityNotFound error with fuzzy suggestions.
    pub fn entity_not_found(name: String, suggestions: Vec<String>) -> Self {
        Self::EntityNotFound { name, suggestions }
    }

    /// Create an InvalidInput error.
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }

    /// Check if this is a "not found" error type.
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::EntityNotFound { .. }
                | Self::EntityIdNotFound(_)
                | Self::FactNotFound(_)
                | Self::RelationNotFound { .. }
                | Self::PathNotFound { .. }
        )
    }

    /// Get the entity name if this is an EntityNotFound error.
    pub fn entity_name(&self) -> Option<&str> {
        match self {
            Self::EntityNotFound { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// Result type alias for WikiGraph operations.
pub type Result<T> = std::result::Result<T, WgError>;
