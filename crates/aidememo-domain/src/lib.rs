//! Portable domain contracts for AideMemo server and SSOT adapters.
//!
//! This crate intentionally contains no database, filesystem, network, model,
//! or runtime dependencies. Embedded SQLite, PostgreSQL, and optional Durable
//! Object adapters should share these identities, command guards, receipts,
//! change-feed records, and conformance fixtures.

mod artifact;
mod change;
mod command;
pub mod conformance;
mod error;
mod handoff;
mod identity;
mod record;
mod storage;

pub use artifact::{ArtifactBodyRef, ArtifactPath, ArtifactReference, ContentDigest};
pub use change::{ChangeBatch, ChangeCursor, ChangeEntry, ChangeOperation};
pub use command::{
    AuditEntry, AuthorizedCommand, CanonicalResource, CommandEnvelope, CommandFingerprint,
    CommandReceipt, MutationCommand, OperationName, ResourceKind, ResourceRef, ResourceState,
};
pub use error::{DomainError, ErrorCode};
pub use handoff::{
    FactRecord, HandoffListEntry, HandoffMailbox, HandoffOutcome, HandoffPage, HandoffQuery,
    HandoffRecord, HandoffStatus, SessionRecord,
};
pub use identity::{
    ActorId, ArtifactId, AuthenticatedActor, ClaimId, CommandId, FactId, HandoffId, MembershipRole,
    MembershipStatus, ProjectAccess, ProjectAuthorization, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectScope, ProjectSequence, ResourceId, Revision, SessionId, SourceId,
    TenantId,
};
pub use record::{ActorKind, ActorRecord, ProjectRecord, RecordStatus, TenantRecord};
pub use storage::{CommandStore, HandoffStore};
