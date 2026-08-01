//! Portable canonical command ledger boundary.

use crate::{
    ActorId, CanonicalResource, ChangeBatch, ChangeCursor, CommandId, CommandReceipt, DomainError,
    HandoffPage, HandoffQuery, MaterializedChangeBatch, MutationCommand, ProjectScope,
    ProjectSnapshot, ResourceRef,
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
