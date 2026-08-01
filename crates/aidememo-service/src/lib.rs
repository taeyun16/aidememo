//! Storage-neutral command orchestration for AideMemo server profiles.
//!
//! Gateways authenticate an actor and load membership. This service binds that
//! context to an untrusted command envelope, computes a deterministic request
//! fingerprint, and delegates one atomic mutation to a [`CommandStore`]. It
//! contains no transport, database, filesystem, or model assumptions.

use aidememo_domain::{
    AuthenticatedActor, AuthorizedCommand, ChangeBatch, ChangeCursor, ChangeOperation,
    CommandEnvelope, CommandFingerprint, CommandReceipt, CommandStore, DomainError,
    MutationCommand, ProjectAccess, ProjectMembership, ResourceRef,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Portable command/query orchestration over one canonical ledger adapter.
pub struct CommandService<S> {
    store: S,
}

impl<S> CommandService<S> {
    /// Wrap a canonical command ledger.
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    /// Borrow the adapter for health and adapter-specific administration.
    #[must_use]
    pub const fn store(&self) -> &S {
        &self.store
    }

    /// Mutably borrow the adapter for administration.
    #[must_use]
    pub const fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    /// Consume the service and return its adapter.
    #[must_use]
    pub fn into_store(self) -> S {
        self.store
    }
}

impl<S: CommandStore> CommandService<S> {
    /// Authorize, fingerprint, and atomically execute one mutation.
    ///
    /// Tenant and actor are accepted only through `authenticated`; the command
    /// body cannot supply or override them. Project selection is checked against
    /// the active membership before the adapter observes the command.
    ///
    /// # Errors
    ///
    /// Returns a stable [`DomainError`] for identity or project authorization,
    /// canonical payload encoding, idempotency, CAS, or storage failure.
    pub fn execute<T: Serialize>(
        &mut self,
        authenticated: &AuthenticatedActor,
        membership: &ProjectMembership,
        envelope: CommandEnvelope<T>,
        resource: ResourceRef,
        change: ChangeOperation,
    ) -> Result<CommandReceipt, DomainError> {
        let access = ProjectAccess::authorize(authenticated, membership)?;
        let authorization = access.require_write()?;
        if authorization.project_id() != &envelope.project_id {
            return Err(DomainError::ProjectScopeMismatch {
                requested: envelope.project_id,
                authorized: authorization.project_id().clone(),
            });
        }
        let fingerprint = command_fingerprint(&envelope)?;
        let metadata = CommandEnvelope {
            command_id: envelope.command_id,
            project_id: envelope.project_id,
            expected_revision: envelope.expected_revision,
            operation: envelope.operation,
            payload: (),
        };
        let command = MutationCommand {
            command: AuthorizedCommand::authorize(authorization, metadata)?,
            fingerprint,
            resource,
            change,
        };
        self.store.execute(&command)
    }

    /// Pull a change batch through authenticated active project membership.
    ///
    /// Read-only members may sync. Suspended members and identity mismatches are
    /// rejected before the adapter receives a scope.
    ///
    /// # Errors
    ///
    /// Returns a stable [`DomainError`] for authorization, invalidated cursor
    /// epoch, invalid batch, or storage failure.
    pub fn changes(
        &self,
        authenticated: &AuthenticatedActor,
        membership: &ProjectMembership,
        cursor: &ChangeCursor,
        limit: usize,
    ) -> Result<ChangeBatch, DomainError> {
        let access = ProjectAccess::authorize(authenticated, membership)?;
        self.store.changes(&access.scope(), cursor, limit)
    }
}

/// Compute the idempotency fingerprint for every untrusted command field except
/// `command_id`, which is the lookup key itself.
///
/// JSON object keys are recursively sorted before SHA-256 so equivalent map
/// insertion order does not create different requests.
///
/// # Errors
///
/// Returns [`DomainError::InvalidCommand`] when the payload cannot be converted
/// to or encoded from canonical JSON.
pub fn command_fingerprint<T: Serialize>(
    envelope: &CommandEnvelope<T>,
) -> Result<CommandFingerprint, DomainError> {
    #[derive(Serialize)]
    struct FingerprintMaterial<'a, T> {
        project_id: &'a aidememo_domain::ProjectId,
        expected_revision: Option<aidememo_domain::Revision>,
        operation: &'a aidememo_domain::OperationName,
        payload: &'a T,
    }

    let material = FingerprintMaterial {
        project_id: &envelope.project_id,
        expected_revision: envelope.expected_revision,
        operation: &envelope.operation,
        payload: &envelope.payload,
    };
    let value = serde_json::to_value(material)
        .map_err(|error| DomainError::InvalidCommand(error.to_string()))?;
    let canonical = canonicalize_json(value);
    let encoded = serde_json::to_vec(&canonical)
        .map_err(|error| DomainError::InvalidCommand(error.to_string()))?;
    let digest = Sha256::digest(encoded);
    let mut fingerprint = String::with_capacity(64);
    for byte in digest {
        write!(&mut fingerprint, "{byte:02x}")
            .map_err(|error| DomainError::InvalidCommand(error.to_string()))?;
    }
    CommandFingerprint::try_from(fingerprint)
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect(),
            )
        }
        primitive => primitive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidememo_domain::{CommandId, OperationName, ProjectId};
    use serde_json::json;

    fn envelope(payload: Value) -> Result<CommandEnvelope<Value>, DomainError> {
        Ok(CommandEnvelope {
            command_id: CommandId::try_from("command_01")?,
            project_id: ProjectId::try_from("project_01")?,
            expected_revision: None,
            operation: OperationName::try_from("fact.add")?,
            payload,
        })
    }

    #[test]
    fn fingerprint_is_independent_of_json_object_insertion_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let left = envelope(json!({"a": 1, "b": {"c": 2, "d": 3}}))?;
        let right = envelope(json!({"b": {"d": 3, "c": 2}, "a": 1}))?;
        assert_eq!(command_fingerprint(&left)?, command_fingerprint(&right)?);
        Ok(())
    }

    #[test]
    fn fingerprint_includes_revision_precondition() -> Result<(), Box<dyn std::error::Error>> {
        let left = envelope(json!({"content": "same"}))?;
        let mut right = left.clone();
        right.expected_revision = Some(aidememo_domain::Revision::new(1)?);
        assert_ne!(command_fingerprint(&left)?, command_fingerprint(&right)?);
        Ok(())
    }
}
