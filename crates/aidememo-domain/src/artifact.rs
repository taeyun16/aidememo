//! Logical artifact namespace and immutable body references.

use crate::{ArtifactId, DomainError, ProjectScope, Revision};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

const MAX_PATH_BYTES: usize = 1_024;

/// Canonical project-relative artifact path.
///
/// This is a logical namespace key, not an operating-system path. It rejects
/// traversal and platform separators so all adapters resolve the same record.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactPath(String);

impl ArtifactPath {
    /// Return the canonical absolute logical path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ArtifactPath {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() < 2 || value.len() > MAX_PATH_BYTES || !value.starts_with('/') {
            return Err(DomainError::InvalidArtifactPath(format!(
                "must start with '/' and contain 2 to {MAX_PATH_BYTES} bytes"
            )));
        }
        if value.ends_with('/') || value.contains('\\') || value.chars().any(char::is_control) {
            return Err(DomainError::InvalidArtifactPath(
                "must not end with '/', contain backslashes, or contain control characters"
                    .to_owned(),
            ));
        }
        if value
            .split('/')
            .skip(1)
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        {
            return Err(DomainError::InvalidArtifactPath(
                "segments must be non-empty and may not be '.' or '..'".to_owned(),
            ));
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for ArtifactPath {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl fmt::Display for ArtifactPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for ArtifactPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ArtifactPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

/// Lowercase SHA-256 digest for server-observed artifact bytes.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ContentDigest(String);

impl ContentDigest {
    /// Return the lowercase hexadecimal SHA-256 digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContentDigest {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(DomainError::InvalidArtifactReference(
                "content digest must be 64 lowercase hexadecimal characters".to_owned(),
            ));
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for ContentDigest {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl Serialize for ContentDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ContentDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

/// Immutable artifact body location.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum ArtifactBodyRef {
    /// Bounded bytes stored in the same canonical metadata transaction.
    Inline {
        /// Body size observed by the server.
        size_bytes: u64,
        /// Digest of the exact bytes.
        digest: ContentDigest,
    },
    /// Immutable generation in filesystem, R2, or S3-compatible storage.
    Object {
        /// Adapter-owned opaque object key.
        object_key: String,
        /// Immutable generation/version chosen before publication.
        generation: String,
        /// Body size observed by the server.
        size_bytes: u64,
        /// Object-store entity tag observed after upload.
        etag: String,
        /// Optional provider version identifier.
        version: Option<String>,
        /// Optional end-to-end digest.
        digest: Option<ContentDigest>,
    },
}

impl ArtifactBodyRef {
    /// Validate provider-neutral reference fields.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidArtifactReference`] when required object
    /// metadata is empty, unbounded, or contains control characters.
    pub fn validate(&self) -> Result<(), DomainError> {
        if let Self::Object {
            object_key,
            generation,
            etag,
            version,
            ..
        } = self
        {
            for (name, value) in [
                ("object_key", object_key.as_str()),
                ("generation", generation.as_str()),
                ("etag", etag.as_str()),
            ] {
                validate_metadata_text(name, value)?;
            }
            if let Some(value) = version {
                validate_metadata_text("version", value)?;
            }
        }
        Ok(())
    }
}

fn validate_metadata_text(name: &str, value: &str) -> Result<(), DomainError> {
    if value.is_empty() || value.len() > MAX_PATH_BYTES || value.chars().any(char::is_control) {
        return Err(DomainError::InvalidArtifactReference(format!(
            "{name} must be non-empty, bounded, and contain no control characters"
        )));
    }
    Ok(())
}

/// Revisioned artifact metadata; body bytes remain outside this record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactReference {
    /// Artifact resource identity.
    pub artifact_id: ArtifactId,
    /// Authorized tenant-project scope.
    pub scope: ProjectScope,
    /// Canonical logical namespace path.
    pub path: ArtifactPath,
    /// Metadata revision used for compare-and-swap publication.
    pub revision: Revision,
    /// Opaque token that changes whenever the path reservation changes.
    pub mutation_token: String,
    /// Inline or immutable object body reference.
    pub body: ArtifactBodyRef,
}

/// Exclusive, expiring permission to upload one immutable artifact generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactReservation {
    /// Stable artifact resource identity for this logical path.
    pub artifact_id: ArtifactId,
    /// Authorized tenant-project scope.
    pub scope: ProjectScope,
    /// Canonical logical namespace path.
    pub path: ArtifactPath,
    /// Revision that publication will create.
    pub revision: Revision,
    /// Opaque compare-and-swap token for this reservation.
    pub mutation_token: String,
    /// Adapter-selected immutable object generation.
    pub generation: String,
    /// UTC Unix timestamp in milliseconds after which publication is rejected.
    pub expires_at_ms: i64,
}

impl ArtifactReservation {
    /// Validate portable reservation fields.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidArtifactReference`] when the token,
    /// generation, or expiry is invalid.
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_metadata_text("mutation_token", &self.mutation_token)?;
        validate_metadata_text("generation", &self.generation)?;
        if self.expires_at_ms <= 0 {
            return Err(DomainError::InvalidArtifactReference(
                "reservation expiry must be a positive Unix timestamp".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Metadata observed from an immutable body after upload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactObservation {
    /// Adapter-owned opaque object key.
    pub object_key: String,
    /// Immutable generation selected by the reservation.
    pub generation: String,
    /// Exact body size observed after upload.
    pub size_bytes: u64,
    /// Provider or adapter entity tag observed after upload.
    pub etag: String,
    /// Optional provider version identifier.
    pub version: Option<String>,
    /// Optional end-to-end content digest.
    pub digest: Option<ContentDigest>,
}

impl ArtifactObservation {
    /// Validate and convert observed metadata into a portable object reference.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidArtifactReference`] for incomplete or
    /// invalid observed metadata.
    pub fn body_ref(&self) -> Result<ArtifactBodyRef, DomainError> {
        let body = ArtifactBodyRef::Object {
            object_key: self.object_key.clone(),
            generation: self.generation.clone(),
            size_bytes: self.size_bytes,
            etag: self.etag.clone(),
            version: self.version.clone(),
            digest: self.digest.clone(),
        };
        body.validate()?;
        Ok(body)
    }
}

impl ArtifactReference {
    /// Validate the mutation token and body reference before persistence.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidArtifactReference`] when the token or
    /// immutable body metadata is not portable across adapters.
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_metadata_text("mutation_token", &self.mutation_token)?;
        self.body.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_rejects_traversal_and_platform_aliases() {
        assert!(ArtifactPath::try_from("/sessions/s1/canvas.md").is_ok());
        assert!(ArtifactPath::try_from("/sessions/../secret").is_err());
        assert!(ArtifactPath::try_from("/sessions//canvas.md").is_err());
        assert!(ArtifactPath::try_from("/sessions\\canvas.md").is_err());
    }

    #[test]
    fn object_reference_requires_observed_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let reference = ArtifactReference {
            artifact_id: ArtifactId::try_from("artifact_01")?,
            scope: ProjectScope::new(
                crate::TenantId::try_from("tenant_a")?,
                crate::ProjectId::try_from("project_a")?,
            ),
            path: ArtifactPath::try_from("/sessions/s1/result.json")?,
            revision: Revision::new(1)?,
            mutation_token: "token_01".to_owned(),
            body: ArtifactBodyRef::Object {
                object_key: "".to_owned(),
                generation: "generation_01".to_owned(),
                size_bytes: 10,
                etag: "etag_01".to_owned(),
                version: None,
                digest: None,
            },
        };
        assert!(reference.validate().is_err());
        Ok(())
    }
}
