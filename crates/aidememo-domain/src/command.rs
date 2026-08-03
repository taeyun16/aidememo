//! Mutation envelopes, authorization guards, receipts, and audit records.

use crate::{
    ActorId, CommandId, DomainError, ProjectAuthorization, ProjectId, ProjectScope,
    ProjectSequence, ResourceId, Revision, TenantId,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

const MAX_NAME_BYTES: usize = 96;

fn validate_name(kind: &'static str, value: &str) -> Result<(), DomainError> {
    if value.is_empty() || value.len() > MAX_NAME_BYTES {
        return Err(DomainError::InvalidIdentifier {
            kind,
            reason: format!("must contain 1 to {MAX_NAME_BYTES} bytes"),
        });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
    }) {
        return Err(DomainError::InvalidIdentifier {
            kind,
            reason: "must contain only lowercase ASCII letters, digits, '.', '-', or '_'"
                .to_owned(),
        });
    }
    Ok(())
}

macro_rules! command_name {
    ($name:ident, $kind:literal) => {
        #[doc = concat!("Validated ", $kind, ".")]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Return the ", $kind, " as text.")]
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = DomainError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                validate_name($kind, &value)?;
                Ok(Self(value))
            }
        }

        impl TryFrom<&str> for $name {
            type Error = DomainError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::try_from(value.to_owned())
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::try_from(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::try_from(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

command_name!(OperationName, "operation name");
command_name!(ResourceKind, "resource kind");

/// SHA-256 fingerprint of canonical project, precondition, operation, and payload.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommandFingerprint(String);

impl CommandFingerprint {
    /// Return the lowercase hexadecimal digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CommandFingerprint {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(DomainError::InvalidIdentifier {
                kind: "command fingerprint",
                reason: "must be 64 lowercase hexadecimal characters".to_owned(),
            });
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for CommandFingerprint {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl fmt::Display for CommandFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for CommandFingerprint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CommandFingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

/// Canonical resource identity within one project.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ResourceRef {
    /// Resource family such as `fact`, `entity`, `handoff`, or `artifact`.
    pub kind: ResourceKind,
    /// Opaque resource identity.
    pub id: ResourceId,
}

/// Untrusted client command body.
///
/// Tenant and actor identity are deliberately absent. The gateway must pair
/// this envelope with [`ProjectAuthorization`] before service execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandEnvelope<T> {
    /// Client-generated idempotency key.
    pub command_id: CommandId,
    /// Explicit project selection, checked against authenticated membership.
    pub project_id: ProjectId,
    /// Optimistic concurrency precondition for an existing resource.
    pub expected_revision: Option<Revision>,
    /// Stable service operation name.
    pub operation: OperationName,
    /// Operation-specific body.
    pub payload: T,
}

/// Command paired with server-owned authorization context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedCommand<T> {
    authorization: ProjectAuthorization,
    envelope: CommandEnvelope<T>,
}

impl<T> AuthorizedCommand<T> {
    /// Bind a client envelope to verified project authorization.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ProjectScopeMismatch`] when the untrusted
    /// envelope selects a different project.
    pub fn authorize(
        authorization: ProjectAuthorization,
        envelope: CommandEnvelope<T>,
    ) -> Result<Self, DomainError> {
        if authorization.project_id() != &envelope.project_id {
            return Err(DomainError::ProjectScopeMismatch {
                requested: envelope.project_id,
                authorized: authorization.project_id().clone(),
            });
        }
        Ok(Self {
            authorization,
            envelope,
        })
    }

    /// Verified authorization context.
    #[must_use]
    pub const fn authorization(&self) -> &ProjectAuthorization {
        &self.authorization
    }

    /// Original client envelope.
    #[must_use]
    pub const fn envelope(&self) -> &CommandEnvelope<T> {
        &self.envelope
    }

    /// Consume this value into its checked components.
    #[must_use]
    pub fn into_parts(self) -> (ProjectAuthorization, CommandEnvelope<T>) {
        (self.authorization, self.envelope)
    }
}

/// Storage-neutral mutation used by the backend conformance fixture.
///
/// The service owns the operation payload. Adapters persist its canonical
/// fingerprint, resource mutation, receipt, change entry, and audit entry in
/// one transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationCommand {
    /// Authorized metadata-only command.
    pub command: AuthorizedCommand<()>,
    /// Canonical hash of the real project, precondition, operation, and payload.
    pub fingerprint: CommandFingerprint,
    /// Mutated canonical resource.
    pub resource: ResourceRef,
    /// Upsert or durable deletion tombstone.
    pub change: crate::ChangeOperation,
    /// Canonical JSON resource body for upserts; absent for deletions.
    pub resource_body: Option<Vec<u8>>,
}

/// Canonical resource state returned to replicas after a change notification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanonicalResource {
    /// Tenant-project scope.
    pub scope: ProjectScope,
    /// Resource coordinate.
    pub resource: ResourceRef,
    /// Current resource or tombstone revision.
    pub revision: Revision,
    /// Upsert body or durable deletion marker.
    pub state: ResourceState,
}

/// Materialized canonical resource state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ResourceState {
    /// Canonical JSON bytes for the current record.
    Present {
        /// Recursively key-sorted JSON representation.
        body: Vec<u8>,
    },
    /// Durable deletion tombstone.
    Deleted,
}

/// Stored result returned for both the first commit and an identical retry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandReceipt {
    /// Idempotency key.
    pub command_id: CommandId,
    /// Hash used to reject command-ID reuse with a different body.
    pub fingerprint: CommandFingerprint,
    /// Server-derived tenant.
    pub tenant_id: TenantId,
    /// Authorized project.
    pub project_id: ProjectId,
    /// Server-derived actor.
    pub actor_id: ActorId,
    /// Project change sequence committed by the transaction.
    pub project_seq: ProjectSequence,
    /// Mutated resource.
    pub resource: ResourceRef,
    /// New resource revision, including deletion revisions.
    pub revision: Revision,
    /// UTC Unix timestamp in milliseconds.
    pub committed_at_ms: i64,
}

/// Immutable audit record committed with a mutation and its receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Server-derived tenant.
    pub tenant_id: TenantId,
    /// Authorized project.
    pub project_id: ProjectId,
    /// Project change sequence.
    pub project_seq: ProjectSequence,
    /// Originating idempotency key.
    pub command_id: CommandId,
    /// Stable service operation.
    pub operation: OperationName,
    /// Mutated resource.
    pub resource: ResourceRef,
    /// Server-derived actor provenance.
    pub actor_id: ActorId,
    /// UTC Unix timestamp in milliseconds.
    pub committed_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthenticatedActor, MembershipRole, MembershipStatus, ProjectMembership, TenantId,
    };

    fn authorization() -> Result<ProjectAuthorization, DomainError> {
        let authenticated = AuthenticatedActor::new(
            TenantId::try_from("tenant_a")?,
            ActorId::try_from("codex-p1")?,
        );
        ProjectAuthorization::authorize(
            &authenticated,
            &ProjectMembership {
                tenant_id: TenantId::try_from("tenant_a")?,
                project_id: ProjectId::try_from("project_a")?,
                actor_id: ActorId::try_from("codex-p1")?,
                role: MembershipRole::Writer,
                status: MembershipStatus::Active,
            },
        )
    }

    #[test]
    fn envelope_cannot_override_authorized_project() -> Result<(), Box<dyn std::error::Error>> {
        let envelope = CommandEnvelope {
            command_id: CommandId::try_from("cmd_01")?,
            project_id: ProjectId::try_from("project_b")?,
            expected_revision: None,
            operation: OperationName::try_from("fact.add")?,
            payload: (),
        };
        assert_eq!(
            AuthorizedCommand::authorize(authorization()?, envelope),
            Err(DomainError::ProjectScopeMismatch {
                requested: ProjectId::try_from("project_b")?,
                authorized: ProjectId::try_from("project_a")?,
            })
        );
        Ok(())
    }

    #[test]
    fn fingerprint_requires_canonical_lower_hex() {
        assert!(CommandFingerprint::try_from("a".repeat(64)).is_ok());
        assert!(CommandFingerprint::try_from("A".repeat(64)).is_err());
        assert!(CommandFingerprint::try_from("a".repeat(63)).is_err());
    }
}
