//! Authenticated identity, membership, revision, and sequence types.

use crate::DomainError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, num::NonZeroU64, str::FromStr};

const MAX_ID_BYTES: usize = 128;

fn validate_id(kind: &'static str, value: &str) -> Result<(), DomainError> {
    if value.is_empty() {
        return Err(DomainError::InvalidIdentifier {
            kind,
            reason: "must not be empty".to_owned(),
        });
    }
    if value.len() > MAX_ID_BYTES {
        return Err(DomainError::InvalidIdentifier {
            kind,
            reason: format!("must be at most {MAX_ID_BYTES} bytes"),
        });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@')
    }) {
        return Err(DomainError::InvalidIdentifier {
            kind,
            reason: "must contain only ASCII letters, digits, '-', '_', '.', ':', or '@'"
                .to_owned(),
        });
    }
    Ok(())
}

macro_rules! string_id {
    ($name:ident, $kind:literal) => {
        #[doc = concat!("Validated opaque ", $kind, ".")]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Return the ", $kind, " as a string slice.")]
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = DomainError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                validate_id($kind, &value)?;
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

string_id!(TenantId, "tenant_id");
string_id!(ProjectId, "project_id");
string_id!(ActorId, "actor_id");
string_id!(CommandId, "command_id");
string_id!(ProjectEpoch, "project_epoch");
string_id!(ResourceId, "resource_id");
string_id!(ArtifactId, "artifact_id");

/// Positive optimistic-concurrency revision of one canonical resource.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(NonZeroU64);

impl Revision {
    /// Create a positive revision.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidIdentifier`] when `value` is zero.
    pub fn new(value: u64) -> Result<Self, DomainError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or_else(|| DomainError::InvalidIdentifier {
                kind: "revision",
                reason: "must be greater than zero".to_owned(),
            })
    }

    /// Return the integer revision.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Return the next revision, or an error on overflow.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidIdentifier`] when the revision cannot be
    /// incremented without overflow.
    pub fn next(self) -> Result<Self, DomainError> {
        self.get()
            .checked_add(1)
            .ok_or_else(|| DomainError::InvalidIdentifier {
                kind: "revision",
                reason: "overflow".to_owned(),
            })
            .and_then(Self::new)
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

/// Monotonic per-project sequence. Zero is the initial cursor position.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ProjectSequence(u64);

impl ProjectSequence {
    /// Initial position before the first committed project mutation.
    pub const ZERO: Self = Self(0);

    /// Create a sequence position.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the integer sequence position.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ProjectSequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Role attached to a tenant-project membership.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    /// Tenant owner.
    Owner,
    /// Project and membership administrator.
    Admin,
    /// Canonical record writer.
    Writer,
    /// Read-only member.
    Reader,
}

impl MembershipRole {
    /// Whether this role may submit mutating commands.
    #[must_use]
    pub const fn can_mutate(self) -> bool {
        !matches!(self, Self::Reader)
    }
}

/// Lifecycle status of a membership.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipStatus {
    /// Membership can be used for authorization.
    Active,
    /// Membership is retained for audit but grants no access.
    Suspended,
}

/// Persisted tenant-project membership record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectMembership {
    /// Owning tenant.
    pub tenant_id: TenantId,
    /// Authorized project.
    pub project_id: ProjectId,
    /// Authenticated principal.
    pub actor_id: ActorId,
    /// Project role.
    pub role: MembershipRole,
    /// Membership lifecycle status.
    pub status: MembershipStatus,
}

/// Identity derived from verified authentication, never from a command body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedActor {
    tenant_id: TenantId,
    actor_id: ActorId,
}

impl AuthenticatedActor {
    /// Create verified gateway identity.
    #[must_use]
    pub const fn new(tenant_id: TenantId, actor_id: ActorId) -> Self {
        Self {
            tenant_id,
            actor_id,
        }
    }

    /// Authenticated tenant.
    #[must_use]
    pub const fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    /// Authenticated actor.
    #[must_use]
    pub const fn actor_id(&self) -> &ActorId {
        &self.actor_id
    }
}

/// Server-owned authorization context for one project command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectAuthorization {
    tenant_id: TenantId,
    project_id: ProjectId,
    actor_id: ActorId,
    role: MembershipRole,
}

impl ProjectAuthorization {
    /// Bind verified identity to an active writable membership.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::IdentityMismatch`] when tenant or actor differs,
    /// or [`DomainError::ProjectUnauthorized`] when membership is suspended or
    /// read-only.
    pub fn authorize(
        authenticated: &AuthenticatedActor,
        membership: &ProjectMembership,
    ) -> Result<Self, DomainError> {
        if authenticated.tenant_id() != &membership.tenant_id
            || authenticated.actor_id() != &membership.actor_id
        {
            return Err(DomainError::IdentityMismatch);
        }
        if membership.status != MembershipStatus::Active || !membership.role.can_mutate() {
            return Err(DomainError::ProjectUnauthorized {
                project_id: membership.project_id.clone(),
            });
        }
        Ok(Self {
            tenant_id: membership.tenant_id.clone(),
            project_id: membership.project_id.clone(),
            actor_id: membership.actor_id.clone(),
            role: membership.role,
        })
    }

    /// Authorized tenant.
    #[must_use]
    pub const fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    /// Authorized project.
    #[must_use]
    pub const fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    /// Authorized actor.
    #[must_use]
    pub const fn actor_id(&self) -> &ActorId {
        &self.actor_id
    }

    /// Effective membership role.
    #[must_use]
    pub const fn role(&self) -> MembershipRole {
        self.role
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_validate_during_json_deserialization() -> Result<(), Box<dyn std::error::Error>> {
        let valid: ProjectId = serde_json::from_str("\"project_01K.test\"")?;
        assert_eq!(valid.as_str(), "project_01K.test");
        assert!(serde_json::from_str::<ProjectId>("\"bad project\"").is_err());
        Ok(())
    }

    #[test]
    fn authorization_rejects_identity_override_and_read_only_membership()
    -> Result<(), Box<dyn std::error::Error>> {
        let authenticated = AuthenticatedActor::new(
            TenantId::try_from("tenant_a")?,
            ActorId::try_from("codex-p1")?,
        );
        let mut membership = ProjectMembership {
            tenant_id: TenantId::try_from("tenant_b")?,
            project_id: ProjectId::try_from("project_a")?,
            actor_id: ActorId::try_from("codex-p1")?,
            role: MembershipRole::Writer,
            status: MembershipStatus::Active,
        };
        assert_eq!(
            ProjectAuthorization::authorize(&authenticated, &membership),
            Err(DomainError::IdentityMismatch)
        );

        membership.tenant_id = TenantId::try_from("tenant_a")?;
        membership.role = MembershipRole::Reader;
        assert_eq!(
            ProjectAuthorization::authorize(&authenticated, &membership),
            Err(DomainError::ProjectUnauthorized {
                project_id: ProjectId::try_from("project_a")?
            })
        );
        Ok(())
    }
}
