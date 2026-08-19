//! Backend-neutral server identity and provisioning conformance fixture.

use crate::{
    ActorId, ActorKind, ActorRecord, DomainError, MembershipRole, MembershipStatus, ProjectEpoch,
    ProjectId, ProjectMembership, ProjectRecord, ProjectScope, RecordStatus, Revision,
    ServerIdentityStore, TenantId, TenantRecord,
};

/// Successful identity/provisioning checks completed by an adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityConformanceReport {
    /// Stable checks completed by the adapter.
    pub checks: Vec<&'static str>,
}

/// Run the portable server identity/provisioning contract.
///
/// The adapter must not already contain `tenant_identity_fixture`.
///
/// # Errors
///
/// Returns the adapter's stable domain error, or
/// [`DomainError::ConformanceViolation`] when an observed result breaks the contract.
pub fn run<S: ServerIdentityStore>(
    store: &mut S,
) -> Result<IdentityConformanceReport, DomainError> {
    let tenant = tenant()?;
    let project = project(&tenant)?;
    let actor = actor(&tenant)?;
    let membership = membership(&project, &actor);
    let token = [0x61_u8; 32];

    store.bootstrap_project(&tenant, &project)?;
    store.bootstrap_project(&tenant, &project)?;
    check(
        store.project_epoch(&scope(&project))? == Some(project.project_epoch.clone()),
        "project_bootstrap_idempotent",
        "project bootstrap retry must preserve the original epoch",
    )?;

    let mut conflicting_project = project.clone();
    conflicting_project.project_epoch = ProjectEpoch::try_from("epoch_identity_conflict")?;
    check(
        store
            .bootstrap_project(&tenant, &conflicting_project)
            .is_err(),
        "project_epoch_conflict",
        "reusing a project scope with another epoch must fail closed",
    )?;

    store.provision_actor(&actor, &membership, &token, 1_000)?;
    store.provision_actor(&actor, &membership, &token, 9_999)?;
    let authenticated = store
        .authenticate_token(&token)?
        .ok_or_else(|| violation("token_authentication", "active token binding disappeared"))?;
    check(
        authenticated.tenant_id() == &tenant.tenant_id
            && authenticated.actor_id() == &actor.actor_id,
        "token_authentication",
        "bearer digest must resolve to the exact provisioned tenant and actor",
    )?;
    check(
        store.membership(&authenticated, &project.project_id)? == Some(membership.clone()),
        "active_membership",
        "authenticated identity must load its active project membership",
    )?;
    check(
        store.project_membership(&scope(&project), &actor.actor_id)? == Some(membership.clone()),
        "exact_membership_lookup",
        "exact tenant-project actor lookup must return the provisioned membership",
    )?;

    let mut conflicting_actor = actor.clone();
    conflicting_actor.kind = ActorKind::Service;
    check(
        store
            .provision_actor(&conflicting_actor, &membership, &[0x62_u8; 32], 1_000)
            .is_err(),
        "actor_identity_conflict",
        "same actor identity with another immutable kind must fail closed",
    )?;

    let other_actor = ActorRecord {
        tenant_id: tenant.tenant_id.clone(),
        actor_id: ActorId::try_from("actor_identity_fixture_other")?,
        display_name: "Other identity fixture actor".to_owned(),
        kind: ActorKind::Agent,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };
    let other_membership = ProjectMembership {
        tenant_id: tenant.tenant_id.clone(),
        project_id: project.project_id.clone(),
        actor_id: other_actor.actor_id.clone(),
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    check(
        store
            .provision_actor(&other_actor, &other_membership, &token, 1_000)
            .is_err(),
        "token_binding_conflict",
        "one bearer digest must not bind to another tenant/actor identity",
    )?;
    let authenticated_after_conflict = store
        .authenticate_token(&token)?
        .ok_or_else(|| violation("token_binding_rollback", "original token binding disappeared"))?;
    check(
        authenticated_after_conflict.actor_id() == &actor.actor_id,
        "token_binding_rollback",
        "failed conflicting provisioning must preserve the original binding",
    )?;

    check(
        matches!(
            store.authenticate_token(&[0_u8; 31]),
            Err(DomainError::InvalidCommand(_))
        ),
        "invalid_digest_length",
        "non-SHA-256 bearer digests must be rejected before lookup",
    )?;
    check(
        store
            .project_membership(
                &ProjectScope::new(
                    tenant.tenant_id.clone(),
                    ProjectId::try_from("project_identity_missing")?,
                ),
                &actor.actor_id,
            )?
            .is_none(),
        "membership_scope_isolation",
        "membership lookup must not escape its exact tenant-project scope",
    )?;

    Ok(IdentityConformanceReport {
        checks: vec![
            "project_bootstrap_idempotent",
            "project_epoch_conflict",
            "token_authentication",
            "active_membership",
            "exact_membership_lookup",
            "actor_identity_conflict",
            "token_binding_conflict",
            "token_binding_rollback",
            "invalid_digest_length",
            "membership_scope_isolation",
        ],
    })
}

fn tenant() -> Result<TenantRecord, DomainError> {
    Ok(TenantRecord {
        tenant_id: TenantId::try_from("tenant_identity_fixture")?,
        display_name: "Identity fixture tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    })
}

fn project(tenant: &TenantRecord) -> Result<ProjectRecord, DomainError> {
    Ok(ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_identity_fixture")?,
        project_epoch: ProjectEpoch::try_from("epoch_identity_fixture")?,
        display_name: "Identity fixture project".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    })
}

fn actor(tenant: &TenantRecord) -> Result<ActorRecord, DomainError> {
    Ok(ActorRecord {
        tenant_id: tenant.tenant_id.clone(),
        actor_id: ActorId::try_from("actor_identity_fixture")?,
        display_name: "Identity fixture actor".to_owned(),
        kind: ActorKind::Agent,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    })
}

fn membership(project: &ProjectRecord, actor: &ActorRecord) -> ProjectMembership {
    ProjectMembership {
        tenant_id: project.tenant_id.clone(),
        project_id: project.project_id.clone(),
        actor_id: actor.actor_id.clone(),
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    }
}

fn scope(project: &ProjectRecord) -> ProjectScope {
    ProjectScope::new(project.tenant_id.clone(), project.project_id.clone())
}

fn check(condition: bool, name: &'static str, detail: &str) -> Result<(), DomainError> {
    if condition {
        Ok(())
    } else {
        Err(violation(name, detail))
    }
}

fn violation(check: &'static str, detail: &str) -> DomainError {
    DomainError::ConformanceViolation {
        check,
        detail: detail.to_owned(),
    }
}
