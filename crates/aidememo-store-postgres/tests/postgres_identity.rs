use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectId,
    ProjectMembership, ProjectRecord, ProjectScope, RecordStatus, Revision, ServerIdentityStore,
    TenantId, TenantRecord,
};
use aidememo_store_postgres::PostgresCommandStore;
use postgres::{Client, NoTls};

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn bootstrap_provision_and_authentication_are_idempotent() -> Result<(), Box<dyn std::error::Error>>
{
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let (tenant, project, actor, membership) = fixture("identity_idempotent")?;
    let token = [0x11_u8; 32];
    let mut store = PostgresCommandStore::connect_no_tls(&url)?;

    store.bootstrap_project(&tenant, &project)?;
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(&actor, &membership, &token, 1_000)?;
    store.provision_actor(&actor, &membership, &token, 9_999)?;

    let authenticated = store
        .authenticate_token(&token)?
        .ok_or("active token did not authenticate")?;
    assert_eq!(authenticated.tenant_id(), &tenant.tenant_id);
    assert_eq!(authenticated.actor_id(), &actor.actor_id);
    assert_eq!(
        store.membership(&authenticated, &project.project_id)?,
        Some(membership.clone())
    );
    assert_eq!(
        store.project_membership(
            &ProjectScope::new(tenant.tenant_id.clone(), project.project_id.clone()),
            &actor.actor_id,
        )?,
        Some(membership)
    );
    assert_eq!(
        store.project_epoch(&project_scope(&project))?,
        Some(project.project_epoch)
    );
    assert_eq!(store.schema_version()?, 2);
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn conflicting_token_binding_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let (tenant, project, actor, membership) = fixture("identity_token_conflict")?;
    let token = [0x22_u8; 32];
    let mut store = PostgresCommandStore::connect_no_tls(&url)?;
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(&actor, &membership, &token, 1_000)?;

    let other_actor = ActorRecord {
        tenant_id: tenant.tenant_id.clone(),
        actor_id: ActorId::try_from("actor_identity_token_conflict_other")?,
        display_name: "Other actor".to_owned(),
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
    assert!(
        store
            .provision_actor(&other_actor, &other_membership, &token, 1_000)
            .is_err()
    );

    let authenticated = store
        .authenticate_token(&token)?
        .ok_or("original token binding disappeared")?;
    assert_eq!(authenticated.actor_id(), &actor.actor_id);
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn suspended_identity_and_membership_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let (tenant, project, actor, membership) = fixture("identity_suspend")?;
    let token = [0x33_u8; 32];
    let mut store = PostgresCommandStore::connect_no_tls(&url)?;
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(&actor, &membership, &token, 1_000)?;

    let mut admin = Client::connect(&url, NoTls)?;
    admin.execute(
        "UPDATE ssot_memberships SET status = 'suspended'
         WHERE tenant_id = $1 AND project_id = $2 AND actor_id = $3",
        &[
            &tenant.tenant_id.as_str(),
            &project.project_id.as_str(),
            &actor.actor_id.as_str(),
        ],
    )?;
    let authenticated = store
        .authenticate_token(&token)?
        .ok_or("actor should still authenticate while only membership is suspended")?;
    assert_eq!(store.membership(&authenticated, &project.project_id)?, None);

    admin.execute(
        "UPDATE ssot_memberships SET status = 'active', role = 'reader'
         WHERE tenant_id = $1 AND project_id = $2 AND actor_id = $3",
        &[
            &tenant.tenant_id.as_str(),
            &project.project_id.as_str(),
            &actor.actor_id.as_str(),
        ],
    )?;
    let reader = store
        .membership(&authenticated, &project.project_id)?
        .ok_or("reactivated membership missing")?;
    assert_eq!(reader.role, MembershipRole::Reader);

    admin.execute(
        "UPDATE ssot_actors SET status = 'suspended'
         WHERE tenant_id = $1 AND actor_id = $2",
        &[&tenant.tenant_id.as_str(), &actor.actor_id.as_str()],
    )?;
    assert_eq!(store.authenticate_token(&token)?, None);
    assert_eq!(
        store.project_membership(&project_scope(&project), &actor.actor_id)?,
        None
    );
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn token_and_membership_are_tenant_scoped() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let (first_tenant, first_project, first_actor, first_membership) = fixture("identity_scope_a")?;
    let (second_tenant, second_project, second_actor, second_membership) =
        fixture("identity_scope_b")?;
    let first_token = [0x44_u8; 32];
    let second_token = [0x55_u8; 32];
    let mut store = PostgresCommandStore::connect_no_tls(&url)?;
    store.bootstrap_project(&first_tenant, &first_project)?;
    store.bootstrap_project(&second_tenant, &second_project)?;
    store.provision_actor(&first_actor, &first_membership, &first_token, 1_000)?;
    store.provision_actor(&second_actor, &second_membership, &second_token, 1_000)?;

    let first_auth = store
        .authenticate_token(&first_token)?
        .ok_or("first token missing")?;
    let second_auth = store
        .authenticate_token(&second_token)?
        .ok_or("second token missing")?;
    assert_eq!(first_auth.tenant_id(), &first_tenant.tenant_id);
    assert_eq!(second_auth.tenant_id(), &second_tenant.tenant_id);
    assert_eq!(
        store.membership(&first_auth, &second_project.project_id)?,
        None
    );
    assert_eq!(
        store.membership(&second_auth, &first_project.project_id)?,
        None
    );
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn invalid_bearer_digest_length_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let store = PostgresCommandStore::connect_no_tls(&url)?;
    assert!(store.authenticate_token(&[0_u8; 31]).is_err());
    Ok(())
}

fn fixture(
    suffix: &str,
) -> Result<(TenantRecord, ProjectRecord, ActorRecord, ProjectMembership), Box<dyn std::error::Error>>
{
    let tenant_id = TenantId::try_from(format!("tenant_{suffix}"))?;
    let project_id = ProjectId::try_from(format!("project_{suffix}"))?;
    let actor_id = ActorId::try_from(format!("actor_{suffix}"))?;
    let tenant = TenantRecord {
        tenant_id: tenant_id.clone(),
        display_name: format!("Tenant {suffix}"),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };
    let project = ProjectRecord {
        tenant_id: tenant_id.clone(),
        project_id: project_id.clone(),
        project_epoch: aidememo_domain::ProjectEpoch::try_from(format!("epoch_{suffix}"))?,
        display_name: format!("Project {suffix}"),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };
    let actor = ActorRecord {
        tenant_id: tenant_id.clone(),
        actor_id: actor_id.clone(),
        display_name: format!("Actor {suffix}"),
        kind: ActorKind::Agent,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };
    let membership = ProjectMembership {
        tenant_id,
        project_id,
        actor_id,
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    Ok((tenant, project, actor, membership))
}

fn project_scope(project: &ProjectRecord) -> ProjectScope {
    ProjectScope::new(project.tenant_id.clone(), project.project_id.clone())
}
