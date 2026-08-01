use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, AuthenticatedActor, ChangeCursor, ChangeOperation,
    CommandEnvelope, CommandId, CommandStore, MembershipRole, MembershipStatus, OperationName,
    ProjectEpoch, ProjectId, ProjectMembership, ProjectRecord, ProjectScope, ProjectSequence,
    RecordStatus, ResourceId, ResourceKind, ResourceRef, Revision, TenantId, TenantRecord,
    conformance,
};
use aidememo_service::CommandService;
use aidememo_store_local::SqliteCommandStore;
use serde_json::json;
use std::sync::{Arc, Barrier};

#[test]
fn sqlite_adapter_passes_portable_conformance_suite() -> Result<(), Box<dyn std::error::Error>> {
    let epoch = ProjectEpoch::try_from("epoch_fixture")?;
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_fixture")?,
        ProjectId::try_from("project_fixture")?,
    );
    let mut store = SqliteCommandStore::open_in_memory()?;
    store.initialize_project(&scope, &epoch)?;

    let report = conformance::run(&mut store, epoch)?;
    assert_eq!(report.final_sequence, ProjectSequence::new(3));
    assert_eq!(store.audit_count(&scope)?, 3);
    Ok(())
}

#[test]
fn receipt_and_tombstone_survive_process_reopen() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("ssot.sqlite");
    let epoch = ProjectEpoch::try_from("epoch_restart")?;
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_restart")?,
        ProjectId::try_from("project_restart")?,
    );
    let authenticated =
        AuthenticatedActor::new(scope.tenant_id.clone(), ActorId::try_from("codex-p1")?);
    let membership = ProjectMembership {
        tenant_id: scope.tenant_id.clone(),
        project_id: scope.project_id.clone(),
        actor_id: ActorId::try_from("codex-p1")?,
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    let envelope = CommandEnvelope {
        command_id: CommandId::try_from("command_restart")?,
        project_id: scope.project_id.clone(),
        expected_revision: None,
        operation: OperationName::try_from("fact.delete")?,
        payload: json!({"fact_id": "fact_restart"}),
    };
    let resource = ResourceRef {
        kind: ResourceKind::try_from("fact")?,
        id: ResourceId::try_from("fact_restart")?,
    };

    let first = {
        let mut store = SqliteCommandStore::open(&path)?;
        store.initialize_project(&scope, &epoch)?;
        let mut service = CommandService::new(store);
        service.execute(
            &authenticated,
            &membership,
            envelope.clone(),
            resource.clone(),
            ChangeOperation::Delete,
        )?
    };

    let mut service = CommandService::new(SqliteCommandStore::open(&path)?);
    let replay = service.execute(
        &authenticated,
        &membership,
        envelope,
        resource,
        ChangeOperation::Delete,
    )?;
    assert_eq!(replay, first);

    let batch = service.changes(
        &authenticated,
        &membership,
        &ChangeCursor {
            project_epoch: epoch,
            after_seq: ProjectSequence::ZERO,
        },
        10,
    )?;
    assert_eq!(batch.entries.len(), 1);
    assert_eq!(batch.entries[0].operation, ChangeOperation::Delete);
    assert_eq!(service.store().audit_count(&scope)?, 1);
    Ok(())
}

#[test]
fn same_project_id_is_isolated_by_tenant_scope() -> Result<(), Box<dyn std::error::Error>> {
    let mut store = SqliteCommandStore::open_in_memory()?;
    let project_id = ProjectId::try_from("shared_slug")?;
    let scope_a = ProjectScope::new(TenantId::try_from("tenant_a")?, project_id.clone());
    let scope_b = ProjectScope::new(TenantId::try_from("tenant_b")?, project_id);
    let epoch_a = ProjectEpoch::try_from("epoch_a")?;
    let epoch_b = ProjectEpoch::try_from("epoch_b")?;
    store.initialize_project(&scope_a, &epoch_a)?;
    store.initialize_project(&scope_b, &epoch_b)?;

    let authenticated =
        AuthenticatedActor::new(scope_a.tenant_id.clone(), ActorId::try_from("actor_a")?);
    let membership = ProjectMembership {
        tenant_id: scope_a.tenant_id.clone(),
        project_id: scope_a.project_id.clone(),
        actor_id: authenticated.actor_id().clone(),
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    let mut service = CommandService::new(store);
    service.execute(
        &authenticated,
        &membership,
        CommandEnvelope {
            command_id: CommandId::try_from("command_tenant_a")?,
            project_id: scope_a.project_id.clone(),
            expected_revision: None,
            operation: OperationName::try_from("fact.add")?,
            payload: json!({"tenant": "a"}),
        },
        ResourceRef {
            kind: ResourceKind::try_from("fact")?,
            id: ResourceId::try_from("fact_tenant_a")?,
        },
        ChangeOperation::Upsert,
    )?;

    let changes_a = service.store().changes(
        &scope_a,
        &ChangeCursor {
            project_epoch: epoch_a,
            after_seq: ProjectSequence::ZERO,
        },
        10,
    )?;
    let changes_b = service.store().changes(
        &scope_b,
        &ChangeCursor {
            project_epoch: epoch_b,
            after_seq: ProjectSequence::ZERO,
        },
        10,
    )?;
    assert_eq!(changes_a.scope, scope_a);
    assert_eq!(changes_b.scope, scope_b);
    assert_eq!(changes_a.entries.len(), 1);
    assert!(changes_b.entries.is_empty());
    Ok(())
}

#[test]
fn concurrent_duplicate_submission_commits_one_mutation() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("concurrent.sqlite");
    let epoch = ProjectEpoch::try_from("epoch_concurrent")?;
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_concurrent")?,
        ProjectId::try_from("project_concurrent")?,
    );
    {
        let mut store = SqliteCommandStore::open(&path)?;
        store.initialize_project(&scope, &epoch)?;
    }

    let authenticated =
        AuthenticatedActor::new(scope.tenant_id.clone(), ActorId::try_from("codex-p1")?);
    let membership = ProjectMembership {
        tenant_id: scope.tenant_id.clone(),
        project_id: scope.project_id.clone(),
        actor_id: authenticated.actor_id().clone(),
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    let envelope = CommandEnvelope {
        command_id: CommandId::try_from("command_concurrent")?,
        project_id: scope.project_id.clone(),
        expected_revision: None,
        operation: OperationName::try_from("fact.add")?,
        payload: json!({"content": "one canonical mutation"}),
    };
    let resource = ResourceRef {
        kind: ResourceKind::try_from("fact")?,
        id: ResourceId::try_from("fact_concurrent")?,
    };
    let barrier = Arc::new(Barrier::new(3));

    let spawn_client = |barrier: Arc<Barrier>| {
        let path = path.clone();
        let authenticated = authenticated.clone();
        let membership = membership.clone();
        let envelope = envelope.clone();
        let resource = resource.clone();
        std::thread::spawn(move || {
            let mut service = CommandService::new(SqliteCommandStore::open(path)?);
            barrier.wait();
            service.execute(
                &authenticated,
                &membership,
                envelope,
                resource,
                ChangeOperation::Upsert,
            )
        })
    };
    let first = spawn_client(Arc::clone(&barrier));
    let second = spawn_client(Arc::clone(&barrier));
    barrier.wait();

    let first_receipt = first
        .join()
        .map_err(|_| std::io::Error::other("first command client panicked"))??;
    let second_receipt = second
        .join()
        .map_err(|_| std::io::Error::other("second command client panicked"))??;
    assert_eq!(first_receipt, second_receipt);

    let store = SqliteCommandStore::open(&path)?;
    let changes = store.changes(
        &scope,
        &ChangeCursor {
            project_epoch: epoch,
            after_seq: ProjectSequence::ZERO,
        },
        10,
    )?;
    assert_eq!(changes.entries.len(), 1);
    assert_eq!(store.audit_count(&scope)?, 1);
    Ok(())
}

#[test]
fn active_reader_can_sync_but_cannot_mutate() -> Result<(), Box<dyn std::error::Error>> {
    let epoch = ProjectEpoch::try_from("epoch_reader")?;
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_reader")?,
        ProjectId::try_from("project_reader")?,
    );
    let mut store = SqliteCommandStore::open_in_memory()?;
    store.initialize_project(&scope, &epoch)?;
    let mut service = CommandService::new(store);
    let reader = AuthenticatedActor::new(scope.tenant_id.clone(), ActorId::try_from("reader_01")?);
    let membership = ProjectMembership {
        tenant_id: scope.tenant_id.clone(),
        project_id: scope.project_id.clone(),
        actor_id: reader.actor_id().clone(),
        role: MembershipRole::Reader,
        status: MembershipStatus::Active,
    };
    let cursor = ChangeCursor {
        project_epoch: epoch,
        after_seq: ProjectSequence::ZERO,
    };
    assert!(
        service
            .changes(&reader, &membership, &cursor, 10)?
            .entries
            .is_empty()
    );

    let result = service.execute(
        &reader,
        &membership,
        CommandEnvelope {
            command_id: CommandId::try_from("command_reader")?,
            project_id: scope.project_id,
            expected_revision: None,
            operation: OperationName::try_from("fact.add")?,
            payload: json!({"content": "must be rejected"}),
        },
        ResourceRef {
            kind: ResourceKind::try_from("fact")?,
            id: ResourceId::try_from("fact_reader")?,
        },
        ChangeOperation::Upsert,
    );
    assert!(matches!(
        result,
        Err(aidememo_domain::DomainError::ProjectUnauthorized { .. })
    ));
    Ok(())
}

#[test]
fn bootstrap_persists_token_identity_and_membership() -> Result<(), Box<dyn std::error::Error>> {
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_bootstrap")?,
        display_name: "Bootstrap tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_bootstrap")?,
        display_name: "Bootstrap project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_bootstrap")?,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let actor = ActorRecord {
        tenant_id: tenant.tenant_id.clone(),
        actor_id: ActorId::try_from("codex-p1")?,
        display_name: "Codex primary".to_owned(),
        kind: ActorKind::Agent,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let membership = ProjectMembership {
        tenant_id: tenant.tenant_id.clone(),
        project_id: project.project_id.clone(),
        actor_id: actor.actor_id.clone(),
        role: MembershipRole::Writer,
        status: MembershipStatus::Active,
    };
    let digest = [7_u8; 32];
    let mut store = SqliteCommandStore::open_in_memory()?;
    store.bootstrap_project(&tenant, &project)?;
    store.provision_actor(&actor, &membership, &digest, timestamp)?;

    let mut retry_tenant = tenant.clone();
    retry_tenant.display_name = "Ignored retry tenant label".to_owned();
    retry_tenant.created_at_ms += 1;
    retry_tenant.updated_at_ms += 1;
    let mut retry_project = project.clone();
    retry_project.display_name = "Ignored retry project label".to_owned();
    retry_project.created_at_ms += 1;
    retry_project.updated_at_ms += 1;
    let mut retry_actor = actor.clone();
    retry_actor.display_name = "Ignored retry actor label".to_owned();
    retry_actor.created_at_ms += 1;
    retry_actor.updated_at_ms += 1;
    store.bootstrap_project(&retry_tenant, &retry_project)?;
    store.provision_actor(&retry_actor, &membership, &digest, timestamp + 1)?;

    let authenticated = store
        .authenticate_token(&digest)?
        .ok_or_else(|| std::io::Error::other("token did not authenticate"))?;
    assert_eq!(authenticated.tenant_id(), &tenant.tenant_id);
    assert_eq!(authenticated.actor_id(), &actor.actor_id);
    assert_eq!(
        store.membership(&authenticated, &project.project_id)?,
        Some(membership)
    );
    assert!(store.authenticate_token(&[8_u8; 32])?.is_none());
    assert_eq!(
        store.project_epoch(&ProjectScope::new(
            tenant.tenant_id.clone(),
            project.project_id.clone()
        ))?,
        Some(project.project_epoch)
    );
    assert_eq!(store.schema_version()?, 2);
    Ok(())
}

#[test]
fn v1_ledger_migrates_and_adopts_bootstrap_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("v1.sqlite");
    {
        let connection = rusqlite::Connection::open(&path)?;
        connection.execute_batch(
            "CREATE TABLE ssot_projects (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_epoch TEXT NOT NULL,
                next_seq INTEGER NOT NULL CHECK (next_seq >= 0),
                PRIMARY KEY (tenant_id, project_id)
             ) STRICT;
             CREATE TABLE ssot_resources (
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK (revision > 0),
                deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
                PRIMARY KEY (tenant_id, project_id, resource_kind, resource_id)
             ) STRICT;
             INSERT INTO ssot_projects VALUES ('tenant_v1', 'project_v1', 'epoch_v1', 0);
             PRAGMA user_version = 1;",
        )?;
    }
    let timestamp = 1_700_000_000_000;
    let tenant = TenantRecord {
        tenant_id: TenantId::try_from("tenant_v1")?,
        display_name: "Migrated tenant".to_owned(),
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let project = ProjectRecord {
        tenant_id: tenant.tenant_id.clone(),
        project_id: ProjectId::try_from("project_v1")?,
        display_name: "Migrated project".to_owned(),
        project_epoch: ProjectEpoch::try_from("epoch_v1")?,
        status: RecordStatus::Active,
        revision: Revision::new(1)?,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    let mut store = SqliteCommandStore::open(path)?;
    assert_eq!(store.schema_version()?, 2);
    store.bootstrap_project(&tenant, &project)?;
    Ok(())
}
