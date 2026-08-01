//! Canonical tenant, project, and actor records.

use crate::{ActorId, ProjectEpoch, ProjectId, Revision, TenantId};
use serde::{Deserialize, Serialize};

/// Lifecycle status shared by top-level server records.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    /// Record accepts normal reads and writes.
    Active,
    /// Record is retained but does not grant access or accept mutations.
    Suspended,
    /// Project is read-only and retained for export or audit.
    Archived,
}

/// Authenticated actor category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// Interactive person.
    Human,
    /// Named coding or reasoning agent profile.
    Agent,
    /// Non-interactive integration or worker identity.
    Service,
}

/// Canonical tenant record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TenantRecord {
    /// Stable tenant identity.
    pub tenant_id: TenantId,
    /// Human-readable label; never used for authorization.
    pub display_name: String,
    /// Tenant lifecycle status.
    pub status: RecordStatus,
    /// Optimistic concurrency revision.
    pub revision: Revision,
    /// UTC Unix creation timestamp in milliseconds.
    pub created_at_ms: i64,
    /// UTC Unix update timestamp in milliseconds.
    pub updated_at_ms: i64,
}

/// Canonical project and change-history generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectRecord {
    /// Owning tenant.
    pub tenant_id: TenantId,
    /// Stable project identity.
    pub project_id: ProjectId,
    /// Human-readable label; never used as a scope key.
    pub display_name: String,
    /// Current change-feed generation.
    pub project_epoch: ProjectEpoch,
    /// Project lifecycle status.
    pub status: RecordStatus,
    /// Optimistic concurrency revision of project metadata.
    pub revision: Revision,
    /// UTC Unix creation timestamp in milliseconds.
    pub created_at_ms: i64,
    /// UTC Unix update timestamp in milliseconds.
    pub updated_at_ms: i64,
}

/// Canonical actor profile within a tenant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActorRecord {
    /// Owning tenant.
    pub tenant_id: TenantId,
    /// Stable authenticated principal identity.
    pub actor_id: ActorId,
    /// Human-readable label; aliases are routing metadata, not credentials.
    pub display_name: String,
    /// Human, agent, or service identity.
    pub kind: ActorKind,
    /// Actor lifecycle status.
    pub status: RecordStatus,
    /// Optimistic concurrency revision.
    pub revision: Revision,
    /// UTC Unix creation timestamp in milliseconds.
    pub created_at_ms: i64,
    /// UTC Unix update timestamp in milliseconds.
    pub updated_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_schema_round_trips_epoch_and_scope() -> Result<(), Box<dyn std::error::Error>> {
        let project = ProjectRecord {
            tenant_id: TenantId::try_from("tenant_a")?,
            project_id: ProjectId::try_from("project_a")?,
            display_name: "AideMemo".to_owned(),
            project_epoch: ProjectEpoch::try_from("epoch_01")?,
            status: RecordStatus::Active,
            revision: Revision::new(1)?,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_000,
        };
        let encoded = serde_json::to_string(&project)?;
        let decoded: ProjectRecord = serde_json::from_str(&encoded)?;
        assert_eq!(decoded, project);
        Ok(())
    }
}
