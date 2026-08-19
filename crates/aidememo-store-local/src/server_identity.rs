use crate::SqliteCommandStore;
use aidememo_domain::{
    ActorId, ActorRecord, AuthenticatedActor, DomainError, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, ProjectScope, ServerIdentityStore, TenantRecord,
};

impl ServerIdentityStore for SqliteCommandStore {
    fn schema_version(&self) -> Result<u32, DomainError> {
        SqliteCommandStore::schema_version(self)
    }

    fn project_epoch(&self, scope: &ProjectScope) -> Result<Option<ProjectEpoch>, DomainError> {
        SqliteCommandStore::project_epoch(self, scope)
    }

    fn bootstrap_project(
        &mut self,
        tenant: &TenantRecord,
        project: &ProjectRecord,
    ) -> Result<(), DomainError> {
        SqliteCommandStore::bootstrap_project(self, tenant, project)
    }

    fn provision_actor(
        &mut self,
        actor: &ActorRecord,
        membership: &ProjectMembership,
        token_sha256: &[u8],
        created_at_ms: i64,
    ) -> Result<(), DomainError> {
        SqliteCommandStore::provision_actor(self, actor, membership, token_sha256, created_at_ms)
    }

    fn authenticate_token(
        &self,
        token_sha256: &[u8],
    ) -> Result<Option<AuthenticatedActor>, DomainError> {
        SqliteCommandStore::authenticate_token(self, token_sha256)
    }

    fn project_membership(
        &self,
        scope: &ProjectScope,
        actor_id: &ActorId,
    ) -> Result<Option<ProjectMembership>, DomainError> {
        SqliteCommandStore::project_membership(self, scope, actor_id)
    }

    fn membership(
        &self,
        authenticated: &AuthenticatedActor,
        project_id: &ProjectId,
    ) -> Result<Option<ProjectMembership>, DomainError> {
        SqliteCommandStore::membership(self, authenticated, project_id)
    }
}
