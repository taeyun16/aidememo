//! Portable canonical command ledger boundary.

use crate::{
    CanonicalResource, ChangeBatch, ChangeCursor, CommandId, CommandReceipt, DomainError,
    MutationCommand, ProjectScope, ResourceRef,
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
