use aidememo_domain::{
    ActorId, AuthenticatedActor, AuthorizedCommand, ChangeCursor, ChangeOperation, ClaimId,
    CommandEnvelope, CommandFingerprint, CommandId, CommandStore, DomainError, HandoffId,
    HandoffMailbox, HandoffQuery, HandoffRecord, HandoffStatus, HandoffStore, MembershipRole,
    MembershipStatus, MutationCommand, OperationName, ProjectAuthorization, ProjectEpoch,
    ProjectId, ProjectMembership, ProjectScope, ProjectSequence, ResourceId, ResourceKind,
    ResourceRef, ResourceState, Revision, SessionId, SessionRecord, SourceId, TenantId,
};
use aidememo_store_postgres::PostgresCommandStore;
use std::sync::{Arc, Barrier};

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn concurrent_identical_command_replays_one_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let scope = scope("tenant_concurrent_replay", "project_concurrent_replay")?;
    let epoch = ProjectEpoch::try_from("epoch_concurrent_replay")?;
    let resource = fact_resource("fact_concurrent_replay")?;
    let command = fact_mutation(
        &scope,
        "writer_concurrent_replay",
        "command_concurrent_replay",
        None,
        'a',
        resource.clone(),
        br#"{"value":"same"}"#,
    )?;
    initialize(&url, &scope, &epoch)?;

    let barrier = Arc::new(Barrier::new(2));
    let first = spawn_execute(url.clone(), barrier.clone(), command.clone());
    let second = spawn_execute(url.clone(), barrier, command);
    let first = first
        .join()
        .map_err(|_| "first replay contender panicked")??;
    let second = second
        .join()
        .map_err(|_| "second replay contender panicked")??;

    assert_eq!(first, second);
    assert_eq!(first.project_seq.get(), 1);
    assert_eq!(first.revision.get(), 1);

    let verify = PostgresCommandStore::connect_no_tls(&url)?;
    let changes = verify.changes(
        &scope,
        &ChangeCursor {
            project_epoch: epoch,
            after_seq: ProjectSequence::ZERO,
        },
        10,
    )?;
    assert_eq!(changes.entries.len(), 1);
    assert_eq!(changes.entries[0].seq, first.project_seq);
    assert_eq!(verify.receipt(&scope, &first.command_id)?, Some(first));
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn concurrent_command_id_conflict_commits_once() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let scope = scope("tenant_command_conflict", "project_command_conflict")?;
    let epoch = ProjectEpoch::try_from("epoch_command_conflict")?;
    let resource = fact_resource("fact_command_conflict")?;
    let first_command = fact_mutation(
        &scope,
        "writer_command_conflict",
        "command_conflict_shared",
        None,
        'a',
        resource.clone(),
        br#"{"winner":"a"}"#,
    )?;
    let second_command = fact_mutation(
        &scope,
        "writer_command_conflict",
        "command_conflict_shared",
        None,
        'b',
        resource,
        br#"{"winner":"b"}"#,
    )?;
    initialize(&url, &scope, &epoch)?;

    let barrier = Arc::new(Barrier::new(2));
    let first = spawn_execute(url.clone(), barrier.clone(), first_command);
    let second = spawn_execute(url.clone(), barrier, second_command);
    let first = first
        .join()
        .map_err(|_| "first conflict contender panicked")?;
    let second = second
        .join()
        .map_err(|_| "second conflict contender panicked")?;

    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(DomainError::CommandConflict)))
            .count(),
        1
    );

    let verify = PostgresCommandStore::connect_no_tls(&url)?;
    let changes = verify.changes(
        &scope,
        &ChangeCursor {
            project_epoch: epoch,
            after_seq: ProjectSequence::ZERO,
        },
        10,
    )?;
    assert_eq!(changes.entries.len(), 1);
    assert_eq!(changes.entries[0].seq.get(), 1);
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn concurrent_stale_cas_loser_is_domain_error() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let scope = scope("tenant_stale_cas", "project_stale_cas")?;
    let epoch = ProjectEpoch::try_from("epoch_stale_cas")?;
    let resource = fact_resource("fact_stale_cas")?;
    let mut setup = PostgresCommandStore::connect_no_tls(&url)?;
    setup.initialize_project(&scope, &epoch)?;
    let created = setup.execute(&fact_mutation(
        &scope,
        "writer_stale_cas",
        "command_stale_create",
        None,
        'a',
        resource.clone(),
        br#"{"revision":1}"#,
    )?)?;

    let first_command = fact_mutation(
        &scope,
        "writer_stale_cas",
        "command_stale_first",
        Some(created.revision),
        'b',
        resource.clone(),
        br#"{"revision":2,"writer":"first"}"#,
    )?;
    let second_command = fact_mutation(
        &scope,
        "writer_stale_cas",
        "command_stale_second",
        Some(created.revision),
        'c',
        resource,
        br#"{"revision":2,"writer":"second"}"#,
    )?;

    let barrier = Arc::new(Barrier::new(2));
    let first = spawn_execute(url.clone(), barrier.clone(), first_command);
    let second = spawn_execute(url.clone(), barrier, second_command);
    let first = first.join().map_err(|_| "first stale contender panicked")?;
    let second = second
        .join()
        .map_err(|_| "second stale contender panicked")?;
    let outcomes = [first, second];

    let committed = outcomes
        .iter()
        .find_map(|result| result.as_ref().ok())
        .ok_or("neither stale contender committed")?;
    assert_eq!(committed.project_seq.get(), 2);
    assert_eq!(committed.revision.get(), 2);
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| {
                matches!(
                    result,
                    Err(DomainError::StaleRevision { expected, current })
                        if expected.get() == 1 && current.get() == 2
                )
            })
            .count(),
        1
    );

    let verify = PostgresCommandStore::connect_no_tls(&url)?;
    let changes = verify.changes(
        &scope,
        &ChangeCursor {
            project_epoch: epoch,
            after_seq: ProjectSequence::ZERO,
        },
        10,
    )?;
    assert_eq!(changes.entries.len(), 2);
    assert_eq!(changes.entries[0].seq.get(), 1);
    assert_eq!(changes.entries[1].seq.get(), 2);
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn concurrent_same_ids_are_isolated_by_tenant_scope() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let first_scope = scope("tenant_isolation_first", "project_shared_id")?;
    let second_scope = scope("tenant_isolation_second", "project_shared_id")?;
    let first_epoch = ProjectEpoch::try_from("epoch_isolation_first")?;
    let second_epoch = ProjectEpoch::try_from("epoch_isolation_second")?;
    let resource = fact_resource("fact_shared_id")?;
    initialize(&url, &first_scope, &first_epoch)?;
    initialize(&url, &second_scope, &second_epoch)?;

    let first_command = fact_mutation(
        &first_scope,
        "writer_shared_id",
        "command_shared_id",
        None,
        'a',
        resource.clone(),
        br#"{"tenant":"first"}"#,
    )?;
    let second_command = fact_mutation(
        &second_scope,
        "writer_shared_id",
        "command_shared_id",
        None,
        'b',
        resource.clone(),
        br#"{"tenant":"second"}"#,
    )?;
    let barrier = Arc::new(Barrier::new(2));
    let first = spawn_execute(url.clone(), barrier.clone(), first_command);
    let second = spawn_execute(url.clone(), barrier, second_command);
    let first = first
        .join()
        .map_err(|_| "first tenant contender panicked")??;
    let second = second
        .join()
        .map_err(|_| "second tenant contender panicked")??;
    assert_eq!(first.project_seq.get(), 1);
    assert_eq!(second.project_seq.get(), 1);

    let verify = PostgresCommandStore::connect_no_tls(&url)?;
    let first_resource = verify
        .resource(&first_scope, &resource)?
        .ok_or("first tenant resource missing")?;
    let second_resource = verify
        .resource(&second_scope, &resource)?
        .ok_or("second tenant resource missing")?;
    assert!(matches!(
        first_resource.state,
        ResourceState::Present { body } if body == br#"{"tenant":"first"}"#
    ));
    assert!(matches!(
        second_resource.state,
        ResourceState::Present { body } if body == br#"{"tenant":"second"}"#
    ));
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn concurrent_handoff_claim_keeps_index_on_winner() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let scope = scope("tenant_handoff_race", "project_handoff_race")?;
    let epoch = ProjectEpoch::try_from("epoch_handoff_race")?;
    let sender = ActorId::try_from("sender_handoff_race")?;
    let receiver = ActorId::try_from("receiver_handoff_race")?;
    let source = SourceId::try_from("source_handoff_race")?;
    let session = SessionRecord::new(
        SessionId::try_from("session_handoff_race")?,
        Some(source.clone()),
        "Concurrent PostgreSQL handoff".to_owned(),
        sender.clone(),
    )?;
    let pending = HandoffRecord::new(
        HandoffId::try_from("handoff_race")?,
        &session,
        sender.clone(),
        receiver.clone(),
        Some("claim concurrently".to_owned()),
        None,
    )?;
    let resource = ResourceRef {
        kind: ResourceKind::try_from("handoff")?,
        id: ResourceId::try_from(pending.handoff_id.as_str())?,
    };
    let mut setup = PostgresCommandStore::connect_no_tls(&url)?;
    setup.initialize_project(&scope, &epoch)?;
    let created = setup.execute(&handoff_mutation(
        &scope,
        &sender,
        "handoff_race_create",
        None,
        'a',
        resource.clone(),
        &pending,
    )?)?;

    let first_claim = ClaimId::try_from("claim_race_first")?;
    let second_claim = ClaimId::try_from("claim_race_second")?;
    let mut first_state = pending.clone();
    first_state.accept(&receiver, first_claim.clone())?;
    let mut second_state = pending;
    second_state.accept(&receiver, second_claim.clone())?;
    let first_command = handoff_mutation(
        &scope,
        &receiver,
        "handoff_race_first",
        Some(created.revision),
        'b',
        resource.clone(),
        &first_state,
    )?;
    let second_command = handoff_mutation(
        &scope,
        &receiver,
        "handoff_race_second",
        Some(created.revision),
        'c',
        resource.clone(),
        &second_state,
    )?;

    let barrier = Arc::new(Barrier::new(2));
    let first = spawn_execute(url.clone(), barrier.clone(), first_command);
    let second = spawn_execute(url.clone(), barrier, second_command);
    let first = first.join().map_err(|_| "first claim contender panicked")?;
    let second = second
        .join()
        .map_err(|_| "second claim contender panicked")?;
    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(DomainError::StaleRevision { .. })))
            .count(),
        1
    );

    let verify = PostgresCommandStore::connect_no_tls(&url)?;
    let canonical = verify
        .resource(&scope, &resource)?
        .ok_or("canonical handoff missing after claim race")?;
    let ResourceState::Present { body } = canonical.state else {
        return Err("handoff unexpectedly deleted after claim race".into());
    };
    let winner: HandoffRecord = serde_json::from_slice(&body)?;
    assert_eq!(winner.status, HandoffStatus::Accepted);
    assert!(
        winner.claim_id.as_ref() == Some(&first_claim)
            || winner.claim_id.as_ref() == Some(&second_claim)
    );
    let inbox = verify.handoffs(
        &scope,
        &receiver,
        &HandoffQuery::new(HandoffMailbox::Inbox, Some(source), true, None, 10)?,
    )?;
    assert_eq!(inbox.assignments.len(), 1);
    assert_eq!(inbox.assignments[0].record, winner);
    assert_eq!(inbox.assignments[0].revision.get(), 2);
    assert_eq!(inbox.assignments[0].project_seq.get(), 2);
    Ok(())
}

fn initialize(url: &str, scope: &ProjectScope, epoch: &ProjectEpoch) -> Result<(), DomainError> {
    let store = PostgresCommandStore::connect_no_tls(url)?;
    store.initialize_project(scope, epoch)
}

fn spawn_execute(
    url: String,
    barrier: Arc<Barrier>,
    command: MutationCommand,
) -> std::thread::JoinHandle<Result<aidememo_domain::CommandReceipt, DomainError>> {
    std::thread::spawn(move || {
        let mut store = PostgresCommandStore::connect_no_tls(&url)?;
        barrier.wait();
        store.execute(&command)
    })
}

fn scope(tenant: &str, project: &str) -> Result<ProjectScope, DomainError> {
    Ok(ProjectScope::new(
        TenantId::try_from(tenant)?,
        ProjectId::try_from(project)?,
    ))
}

fn fact_resource(id: &str) -> Result<ResourceRef, DomainError> {
    Ok(ResourceRef {
        kind: ResourceKind::try_from("fact")?,
        id: ResourceId::try_from(id)?,
    })
}

#[allow(clippy::too_many_arguments)]
fn fact_mutation(
    scope: &ProjectScope,
    actor: &str,
    command_id: &str,
    expected_revision: Option<Revision>,
    fingerprint_byte: char,
    resource: ResourceRef,
    body: &[u8],
) -> Result<MutationCommand, DomainError> {
    mutation(
        scope,
        &ActorId::try_from(actor)?,
        command_id,
        expected_revision,
        fingerprint_byte,
        resource,
        body.to_vec(),
        "fact.put",
    )
}

#[allow(clippy::too_many_arguments)]
fn handoff_mutation(
    scope: &ProjectScope,
    actor: &ActorId,
    command_id: &str,
    expected_revision: Option<Revision>,
    fingerprint_byte: char,
    resource: ResourceRef,
    handoff: &HandoffRecord,
) -> Result<MutationCommand, Box<dyn std::error::Error>> {
    Ok(mutation(
        scope,
        actor,
        command_id,
        expected_revision,
        fingerprint_byte,
        resource,
        serde_json::to_vec(handoff)?,
        "handoff.put",
    )?)
}

#[allow(clippy::too_many_arguments)]
fn mutation(
    scope: &ProjectScope,
    actor: &ActorId,
    command_id: &str,
    expected_revision: Option<Revision>,
    fingerprint_byte: char,
    resource: ResourceRef,
    body: Vec<u8>,
    operation: &str,
) -> Result<MutationCommand, DomainError> {
    let authenticated = AuthenticatedActor::new(scope.tenant_id.clone(), actor.clone());
    let authorization = ProjectAuthorization::authorize(
        &authenticated,
        &ProjectMembership {
            tenant_id: scope.tenant_id.clone(),
            project_id: scope.project_id.clone(),
            actor_id: actor.clone(),
            role: MembershipRole::Writer,
            status: MembershipStatus::Active,
        },
    )?;
    let envelope = CommandEnvelope {
        command_id: CommandId::try_from(command_id)?,
        project_id: scope.project_id.clone(),
        expected_revision,
        operation: OperationName::try_from(operation)?,
        payload: (),
    };
    Ok(MutationCommand {
        command: AuthorizedCommand::authorize(authorization, envelope)?,
        fingerprint: CommandFingerprint::try_from(fingerprint_byte.to_string().repeat(64))?,
        resource,
        change: ChangeOperation::Upsert,
        resource_body: Some(body),
    })
}
