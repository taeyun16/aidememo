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
    // Display includes the fuzzy suggestions when any were collected so
    // the agent doesn't see a bare "entity not found" — the description
    // for `wg_entity_get` advertises this hint and callers depend on
    // pattern-matching on the message to recover from typos.
    #[error("entity not found: '{name}'{}", format_entity_suggestions(suggestions))]
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

/// Render the suggestion list as ` (did you mean: a, b, c?)` or "" when
/// none. Capped at three entries so the message stays readable on a
/// terminal and an LLM doesn't have to scan a long alias dump.
fn format_entity_suggestions(suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        return String::new();
    }
    let preview: Vec<&str> = suggestions.iter().take(3).map(|s| s.as_str()).collect();
    format!(" (did you mean: {}?)", preview.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_not_found_display_includes_suggestions() {
        let err = WgError::entity_not_found(
            "Postgrs".into(),
            vec!["Postgres".into(), "Postgresql".into()],
        );
        let msg = err.to_string();
        assert!(msg.contains("Postgrs"), "name present: {msg}");
        assert!(msg.contains("did you mean"), "hint present: {msg}");
        assert!(msg.contains("Postgres"), "first suggestion present: {msg}");
    }

    #[test]
    fn entity_not_found_display_omits_hint_when_no_suggestions() {
        let err = WgError::entity_not_found("Unknown".into(), vec![]);
        let msg = err.to_string();
        assert_eq!(msg, "entity not found: 'Unknown'");
    }

    #[test]
    fn entity_not_found_display_caps_suggestions_at_three() {
        let many = vec!["a", "b", "c", "d", "e"]
            .into_iter()
            .map(String::from)
            .collect();
        let err = WgError::entity_not_found("x".into(), many);
        let msg = err.to_string();
        // Should mention a/b/c but not d/e — keeps the line readable.
        // Substring "d" hits the literal "did" in "did you mean", so
        // assert against the comma-separated tail explicitly.
        assert!(msg.contains("a, b, c?"), "first three present: {msg}");
        assert!(!msg.contains(", d"), "trailing entries omitted: {msg}");
        assert!(!msg.contains("e?"), "last entry omitted: {msg}");
    }
}
