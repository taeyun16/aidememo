use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, MembershipRole, MembershipStatus, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectRecord, ProjectScope, RecordStatus, Revision, ServerIdentityStore,
    TenantId, TenantRecord,
};
use aidememo_store_postgres::PostgresCommandStore;
use postgres::{Client, NoTls};

const MIGRATION_DATABASE: &str = "aidememo_identity_migration_v1_v2";

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn v1_schema_migrates_to_identity_v2_without_losing_project_head()
-> Result<(), Box<dyn std::error::Error>> {
    let base_url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let prefix = base_url
        .rsplit_once('/')
        .map(|(prefix, _)| prefix)
        .ok_or("PostgreSQL URL does not contain a database path")?;
    let migration_url = format!("{prefix}/{MIGRATION_DATABASE}");

    let mut admin = Client::connect(&base_url, NoTls)?;
    admin.simple_query(&format!(
        "DROP DATABASE IF EXISTS {MIGRATION_DATABASE} WITH (FORCE)"
    ))?;
    admin.simple_query(&format!("CREATE DATABASE {MIGRATION_DATABASE}"))?;

    let mut legacy = Client::connect(&migration_url, NoTls)?;
    legacy.batch_execute(include_str!("fixtures/schema_v1.sql"))?;
    legacy.execute(
        "INSERT INTO ssot_projects (tenant_id, project_id, project_epoch, next_seq)
         VALUES ($1, $2, $3, $4)",
        &[&"tenant_migration", &"project_migration", &"epoch_migration", &7_i64],
    )?;
    drop(legacy);

    let mut store = PostgresCommandStore::connect_no_tls(&migration_url)?;
    assert_eq!(store.schema_version()?, 2);
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_migration")?,
        ProjectId::try_from("project_migration")?,
    );
    assert_eq!(
        store.project_epoch(&scope)?,
        Some(ProjectEpoch::try_from("epoch_migration")?)
    );

    let tenant = TenantRecord {
        tenant_id: scope.tenant_id.clone(),
        display_name: "Migrated tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };
    let project = ProjectRecord {
        tenant_id: scope.tenant_id.clone(),
        project_id: scope.project_id.clone(),
        project_epoch: ProjectEpoch::try_from("epoch_migration")?,
        display_name: "Migrated project".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };
    store.bootstrap_project(&tenant, &project)?;

    let actor = ActorRecord {
        tenant_id: scope.tenant_id.clone(),
        actor_id: ActorId::try_from("actor_migration")?,
        display_name: "Migrated actor".to_owned(),
        kind: ActorKind::Agent,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };
    let membership = ProjectMembership {
        tenant_id: scope.tenant_id.clone(),
        project_id: scope.project_id.clone(),
        actor_id: actor.actor_id.clone(),
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    let token = [0x6d_u8; 32];
    store.provision_actor(&actor, &membership, &token, 1_000)?;
    let authenticated = store
        .authenticate_token(&token)?
        .ok_or("migrated actor token did not authenticate")?;
    assert_eq!(authenticated.actor_id(), &actor.actor_id);
    assert_eq!(store.membership(&authenticated, &scope.project_id)?, Some(membership));

    let mut verify = Client::connect(&migration_url, NoTls)?;
    let next_seq: i64 = verify
        .query_one(
            "SELECT next_seq FROM ssot_projects WHERE tenant_id = $1 AND project_id = $2",
            &[&scope.tenant_id.as_str(), &scope.project_id.as_str()],
        )?
        .get(0);
    assert_eq!(next_seq, 7);
    drop(verify);
    drop(store);

    admin.simple_query(&format!(
        "DROP DATABASE {MIGRATION_DATABASE} WITH (FORCE)"
    ))?;
    Ok(())
}
