//! Portable canonical command ledger boundary.

use crate::{
    ActorId, ActorRecord, AuthenticatedActor, CanonicalResource, ChangeBatch, ChangeCursor,
    CommandId, CommandReceipt, DomainError, HandoffPage, HandoffQuery, MaterializedChangeBatch,
    MutationCommand, ProjectEpoch, ProjectId, ProjectMembership, ProjectRecord, ProjectScope,
    ProjectSnapshot, ResourceRef, TenantRecord,
};

/// Atomic receipt, resource-revision, audit, and change-feed persistence.
///
/// Implementations may be synchronous wrappers over SQLite, PostgreSQL, or a
/// project-scoped Durable Object. Network transport is intentionally outside
/// this trait.
pub trait CommandStore {
    /// Atomically apply a command or replay its stored receipt.
    ///
    /// # Errors
    ///
    /// Returns a stable [`DomainError`] when scope, idempotency, CAS, or durable
    /// persistence rejects the mutation.
    fn execute(&mut self, command: &MutationCommand) -> Result<CommandReceipt, DomainError>;

    /// Look up a committed receipt before re-evaluating mutable domain state.
    ///
    /// # Errors
    ///
    /// Returns a stable [`DomainError`] when the lookup cannot complete.
    fn receipt(
        &self,
        scope: &ProjectScope,
        command_id: &CommandId,
    ) -> Result<Option<CommandReceipt>, DomainError>;

    /// Pull ordered mutations after a replica cursor.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::CursorEpochMismatch`] for invalidated history or
    /// another stable [`DomainError`] when the read cannot complete.
    fn changes(
        &self,
        scope: &ProjectScope,
        cursor: &ChangeCursor,
        limit: usize,
    ) -> Result<ChangeBatch, DomainError>;

    /// Pull ordered mutations with exact resource state at each revision.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::SnapshotRequired`] when legacy history cannot be
    /// materialized exactly, or another stable error when the read fails.
    fn materialized_changes(
        &self,
        scope: &ProjectScope,
        cursor: &ChangeCursor,
        limit: usize,
    ) -> Result<MaterializedChangeBatch, DomainError>;

    /// Read a complete current-state snapshot and its project head atomically.
    ///
    /// # Errors
    ///
    /// Returns a stable [`DomainError`] when the snapshot cannot be produced.
    fn snapshot(&self, scope: &ProjectScope) -> Result<ProjectSnapshot, DomainError>;

    /// Fetch current canonical state or deletion tombstone by resource ID.
    ///
    /// # Errors
    ///
    /// Returns a stable [`DomainError`] when the lookup cannot complete.
    fn resource(
        &self,
        scope: &ProjectScope,
        resource: &ResourceRef,
    ) -> Result<Option<CanonicalResource>, DomainError>;
}

/// Server identity, membership, and bootstrap persistence shared by canonical adapters.
///
/// Bearer plaintext is deliberately outside this contract. Callers validate and
/// hash tokens before provisioning or authentication and pass only SHA-256 bytes.
pub trait ServerIdentityStore {
    /// Return the adapter schema version exposed by server health endpoints.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when schema metadata cannot be read.
    fn schema_version(&self) -> Result<u32, DomainError>;

    /// Return the current epoch for one existing tenant-project scope.
    ///
    /// # Errors
    ///
    /// Returns a stable storage error when the project lookup cannot complete.
    fn project_epoch(&self, scope: &ProjectScope) -> Result<Option<ProjectEpoch>, DomainError>;

    /// Atomically bootstrap an idempotent tenant/project pair.
    ///
    /// # Errors
    ///
    /// Conflicting immutable identity, epoch, status, or revision state fails closed.
    fn bootstrap_project(
        &mut self,
        tenant: &TenantRecord,
        project: &ProjectRecord,
    ) -> Result<(), DomainError>;

    /// Provision an actor, membership, and active bearer digest atomically.
    ///
    /// # Errors
    ///
    /// Conflicting actor, membership, or token binding state fails closed.
    fn provision_actor(
        &mut self,
        actor: &ActorRecord,
        membership: &ProjectMembership,
        token_sha256: &[u8],
        created_at_ms: i64,
    ) -> Result<(), DomainError>;

    /// Resolve an active bearer digest to authenticated identity.
    ///
    /// # Errors
    ///
    /// Returns a stable validation/storage error for invalid digest length or lookup failure.
    fn authenticate_token(
        &self,
        token_sha256: &[u8],
    ) -> Result<Option<AuthenticatedActor>, DomainError>;

    /// Load one active membership inside an exact tenant-project scope.
    ///
    /// # Errors
    ///
    /// Returns a stable storage/decode error when membership lookup cannot complete.
    fn project_membership(
        &self,
        scope: &ProjectScope,
        actor_id: &ActorId,
    ) -> Result<Option<ProjectMembership>, DomainError>;

    /// Load active project membership for an authenticated identity.
    ///
    /// # Errors
    ///
    /// Returns a stable storage/decode error when membership lookup cannot complete.
    fn membership(
        &self,
        authenticated: &AuthenticatedActor,
        project_id: &ProjectId,
    ) -> Result<Option<ProjectMembership>, DomainError> {
        self.project_membership(
            &ProjectScope::new(authenticated.tenant_id().clone(), project_id.clone()),
            authenticated.actor_id(),
        )
    }
}

/// Indexed typed-handoff query boundary for canonical adapters.
///
/// The authenticated actor is a separate argument so an untrusted query cannot
/// select another actor's inbox or outbox.
pub trait HandoffStore: CommandStore {
    /// Read one newest-first mailbox page from a transactional handoff index.
    ///
    /// # Errors
    ///
    /// Returns a stable [`DomainError`] when the query cannot complete.
    fn handoffs(
        &self,
        scope: &ProjectScope,
        actor_id: &ActorId,
        query: &HandoffQuery,
    ) -> Result<HandoffPage, DomainError>;
}
