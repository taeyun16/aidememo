//! Stable domain errors shared by transports and storage adapters.

use crate::{ProjectEpoch, ProjectId, Revision};
use serde::Serialize;
use thiserror::Error;

/// Stable machine-readable domain error code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// An opaque identifier did not satisfy the portable identifier grammar.
    InvalidIdentifier,
    /// Authenticated identity and membership identity disagree.
    IdentityMismatch,
    /// The actor does not have an active writable project membership.
    ProjectUnauthorized,
    /// The command selected a project other than the authorized project.
    ProjectScopeMismatch,
    /// A command ID was reused with a different request fingerprint.
    CommandConflict,
    /// Optimistic concurrency precondition did not match the current revision.
    StaleRevision,
    /// The requested resource does not exist.
    ResourceNotFound,
    /// A cursor belongs to a replaced or restored project history.
    CursorEpochMismatch,
    /// A change batch violates sequence, scope, or cursor invariants.
    InvalidChangeBatch,
    /// A logical artifact path is not canonical.
    InvalidArtifactPath,
    /// Artifact metadata is incomplete or internally inconsistent.
    InvalidArtifactReference,
    /// A backend failed a portable conformance check.
    ConformanceViolation,
}

/// Portable error returned by domain validation and conforming adapters.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DomainError {
    /// Invalid opaque identity.
    #[error("invalid {kind}: {reason}")]
    InvalidIdentifier {
        /// Stable identifier family name.
        kind: &'static str,
        /// Human-readable validation failure.
        reason: String,
    },
    /// Authenticated identity differs from the membership record.
    #[error("authenticated identity does not match project membership")]
    IdentityMismatch,
    /// The actor cannot mutate the requested project.
    #[error("actor is not authorized to mutate project {project_id}")]
    ProjectUnauthorized {
        /// Requested project.
        project_id: ProjectId,
    },
    /// Envelope project differs from the authorized project.
    #[error("command project {requested} does not match authorized project {authorized}")]
    ProjectScopeMismatch {
        /// Project selected by the untrusted envelope.
        requested: ProjectId,
        /// Project fixed by authenticated membership.
        authorized: ProjectId,
    },
    /// Existing receipt fingerprint differs for the same command ID.
    #[error("command ID was already used with a different request")]
    CommandConflict,
    /// Expected revision differs from the canonical record.
    #[error("stale revision: expected {expected}, current {current}")]
    StaleRevision {
        /// Client precondition.
        expected: Revision,
        /// Current canonical revision.
        current: Revision,
    },
    /// The canonical resource was not found.
    #[error("resource not found")]
    ResourceNotFound,
    /// Cursor epoch differs from the canonical project epoch.
    #[error("cursor epoch {cursor} does not match current project epoch {current}")]
    CursorEpochMismatch {
        /// Epoch carried by the replica cursor.
        cursor: ProjectEpoch,
        /// Current canonical epoch.
        current: ProjectEpoch,
    },
    /// Change-feed batch is invalid.
    #[error("invalid change batch: {0}")]
    InvalidChangeBatch(String),
    /// Logical artifact path is invalid.
    #[error("invalid artifact path: {0}")]
    InvalidArtifactPath(String),
    /// Artifact metadata is invalid.
    #[error("invalid artifact reference: {0}")]
    InvalidArtifactReference(String),
    /// Backend-neutral conformance assertion failed.
    #[error("conformance check '{check}' failed: {detail}")]
    ConformanceViolation {
        /// Stable fixture check name.
        check: &'static str,
        /// Observed mismatch.
        detail: String,
    },
}

impl DomainError {
    /// Return the stable code exposed by HTTP, MCP, and language bindings.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidIdentifier { .. } => ErrorCode::InvalidIdentifier,
            Self::IdentityMismatch => ErrorCode::IdentityMismatch,
            Self::ProjectUnauthorized { .. } => ErrorCode::ProjectUnauthorized,
            Self::ProjectScopeMismatch { .. } => ErrorCode::ProjectScopeMismatch,
            Self::CommandConflict => ErrorCode::CommandConflict,
            Self::StaleRevision { .. } => ErrorCode::StaleRevision,
            Self::ResourceNotFound => ErrorCode::ResourceNotFound,
            Self::CursorEpochMismatch { .. } => ErrorCode::CursorEpochMismatch,
            Self::InvalidChangeBatch(_) => ErrorCode::InvalidChangeBatch,
            Self::InvalidArtifactPath(_) => ErrorCode::InvalidArtifactPath,
            Self::InvalidArtifactReference(_) => ErrorCode::InvalidArtifactReference,
            Self::ConformanceViolation { .. } => ErrorCode::ConformanceViolation,
        }
    }
}
