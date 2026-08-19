use aidememo_domain::{
    ActorId, AuthenticatedActor, AuthorizedCommand, ChangeOperation, ClaimId, CommandEnvelope,
    CommandFingerprint, CommandId, CommandStore, FactId, FactRecord, HandoffId, HandoffMailbox,
    HandoffOutcome, HandoffQuery, HandoffRecord, HandoffStore, MembershipRole, MembershipStatus,
    MutationCommand, OperationName, ProjectAuthorization, ProjectEpoch, ProjectId,
    ProjectMembership, ProjectScope, ResourceId, ResourceKind, ResourceRef, Revision, SessionId,
    SessionRecord, SourceId, TenantId, conformance,
};
use aidememo_store_postgres::PostgresCommandStore;

/// Run against a disposable, empty PostgreSQL database.
///
/// The test is ignored by default because normal CI does not provision a
/// database service. Set `AIDEMEMO_POSTGRES_URL` and run with `--ignored` in
/// the dedicated PostgreSQL conformance job.
#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn portable_command_store_conformance() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let epoch = ProjectEpoch::try_from("epoch_fixture")?;
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_fixture")?,
        ProjectId::try_from("project_fixture")?,
    );
    let mut store = PostgresCommandStore::connect_no_tls(&url)?;
    store.initialize_project(&scope, &epoch)?;
    let report = conformance::run(&mut store, epoch)?;
    assert_eq!(report.final_sequence.get(), 3);
    assert_eq!(report.tombstone_revision.get(), 3);
    assert_eq!(report.checks.len(), 15);
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn concurrent_schema_initialization_is_serialized() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let first_url = url.clone();
    let second_url = url;
    let first = std::thread::spawn(move || PostgresCommandStore::connect_no_tls(&first_url));
    let second = std::thread::spawn(move || PostgresCommandStore::connect_no_tls(&second_url));
    let first = first
        .join()
        .map_err(|_| "first PostgreSQL init thread panicked")??;
    let second = second
        .join()
        .map_err(|_| "second PostgreSQL init thread panicked")??;
    assert_eq!(first.schema_version()?, 1);
    assert_eq!(second.schema_version()?, 1);
    Ok(())
}

#[test]
#[ignore = "requires disposable PostgreSQL via AIDEMEMO_POSTGRES_URL"]
fn handoff_mailbox_projection_tracks_canonical_lifecycle() -> Result<(), Box<dyn std::error::Error>>
{
    let url = std::env::var("AIDEMEMO_POSTGRES_URL")?;
    let scope = ProjectScope::new(
        TenantId::try_from("tenant_handoff_fixture")?,
        ProjectId::try_from("project_handoff_fixture")?,
    );
    let epoch = ProjectEpoch::try_from("epoch_handoff_fixture")?;
    let sender = ActorId::try_from("sender_fixture")?;
    let receiver = ActorId::try_from("receiver_fixture")?;
    let source = SourceId::try_from("source_fixture")?;
    let session = SessionRecord::new(
        SessionId::try_from("session_fixture")?,
        Some(source.clone()),
        "PostgreSQL handoff projection".to_owned(),
        sender.clone(),
    )?;
    let mut handoff = HandoffRecord::new(
        HandoffId::try_from("handoff_fixture")?,
        &session,
        sender.clone(),
        receiver.clone(),
        Some("verify PostgreSQL mailbox projection".to_owned()),
        None,
    )?;
    let resource = ResourceRef {
        kind: ResourceKind::try_from("handoff")?,
        id: ResourceId::try_from(handoff.handoff_id.as_str())?,
    };

    let mut store = PostgresCommandStore::connect_no_tls(&url)?;
    store.initialize_project(&scope, &epoch)?;

    let created = store.execute(&handoff_mutation(
        &scope,
        &sender,
        "handoff_create",
        None,
        'a',
        resource.clone(),
        Some(&handoff),
        ChangeOperation::Upsert,
    )?)?;
    assert_eq!(created.project_seq.get(), 1);
    assert_eq!(created.revision.get(), 1);

    let inbox = store.handoffs(
        &scope,
        &receiver,
        &HandoffQuery::new(HandoffMailbox::Inbox, Some(source.clone()), false, None, 10)?,
    )?;
    let outbox = store.handoffs(
        &scope,
        &sender,
        &HandoffQuery::new(
            HandoffMailbox::Outbox,
            Some(source.clone()),
            false,
            None,
            10,
        )?,
    )?;
    assert_eq!(inbox.assignments.len(), 1);
    assert_eq!(outbox.assignments.len(), 1);
    assert_eq!(inbox.assignments[0].record, handoff);
    assert_eq!(inbox.assignments[0].revision, created.revision);
    assert_eq!(outbox.assignments[0].project_seq, created.project_seq);

    let wrong_source = store.handoffs(
        &scope,
        &receiver,
        &HandoffQuery::new(
            HandoffMailbox::Inbox,
            Some(SourceId::try_from("source_other")?),
            true,
            None,
            10,
        )?,
    )?;
    assert!(wrong_source.assignments.is_empty());

    let claim = ClaimId::try_from("claim_fixture")?;
    handoff.accept(&receiver, claim.clone())?;
    let accepted = store.execute(&handoff_mutation(
        &scope,
        &receiver,
        "handoff_accept",
        Some(created.revision),
        'b',
        resource.clone(),
        Some(&handoff),
        ChangeOperation::Upsert,
    )?)?;
    assert_eq!(accepted.project_seq.get(), 2);
    assert_eq!(accepted.revision.get(), 2);

    let result_fact = FactRecord::new(
        FactId::try_from("handoff_result_fixture")?,
        session.session_id.clone(),
        session.source_id.clone(),
        receiver.clone(),
        "PostgreSQL handoff result evidence".to_owned(),
    )?;
    handoff.return_result(&receiver, &claim, &result_fact, HandoffOutcome::Succeeded)?;
    let completed = store.execute(&handoff_mutation(
        &scope,
        &receiver,
        "handoff_complete",
        Some(accepted.revision),
        'c',
        resource.clone(),
        Some(&handoff),
        ChangeOperation::Upsert,
    )?)?;
    assert_eq!(completed.project_seq.get(), 3);
    assert_eq!(completed.revision.get(), 3);

    let active_only = store.handoffs(
        &scope,
        &receiver,
        &HandoffQuery::new(HandoffMailbox::Inbox, Some(source.clone()), false, None, 10)?,
    )?;
    assert!(active_only.assignments.is_empty());
    let including_completed = store.handoffs(
        &scope,
        &receiver,
        &HandoffQuery::new(HandoffMailbox::Inbox, Some(source), true, None, 10)?,
    )?;
    assert_eq!(including_completed.assignments.len(), 1);
    assert_eq!(including_completed.assignments[0].record, handoff);
    assert_eq!(
        including_completed.assignments[0].revision,
        completed.revision
    );
    assert_eq!(
        including_completed.assignments[0].project_seq,
        completed.project_seq
    );

    let deleted = store.execute(&handoff_mutation(
        &scope,
        &sender,
        "handoff_delete",
        Some(completed.revision),
        'd',
        resource,
        None,
        ChangeOperation::Delete,
    )?)?;
    assert_eq!(deleted.project_seq.get(), 4);
    assert_eq!(deleted.revision.get(), 4);
    let after_delete = store.handoffs(
        &scope,
        &receiver,
        &HandoffQuery::new(HandoffMailbox::Inbox, None, true, None, 10)?,
    )?;
    assert!(after_delete.assignments.is_empty());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handoff_mutation(
    scope: &ProjectScope,
    actor_id: &ActorId,
    command_id: &str,
    expected_revision: Option<Revision>,
    fingerprint_byte: char,
    resource: ResourceRef,
    handoff: Option<&HandoffRecord>,
    change: ChangeOperation,
) -> Result<MutationCommand, Box<dyn std::error::Error>> {
    let authenticated = AuthenticatedActor::new(scope.tenant_id.clone(), actor_id.clone());
    let authorization = ProjectAuthorization::authorize(
        &authenticated,
        &ProjectMembership {
            tenant_id: scope.tenant_id.clone(),
            project_id: scope.project_id.clone(),
            actor_id: actor_id.clone(),
            role: MembershipRole::Writer,
            status: MembershipStatus::Active,
        },
    )?;
    let envelope = CommandEnvelope {
        command_id: CommandId::try_from(command_id)?,
        project_id: scope.project_id.clone(),
        expected_revision,
        operation: OperationName::try_from(match change {
            ChangeOperation::Upsert => "handoff.put",
            ChangeOperation::Delete => "handoff.delete",
        })?,
        payload: (),
    };
    Ok(MutationCommand {
        command: AuthorizedCommand::authorize(authorization, envelope)?,
        fingerprint: CommandFingerprint::try_from(fingerprint_byte.to_string().repeat(64))?,
        resource,
        change,
        resource_body: handoff.map(serde_json::to_vec).transpose()?,
    })
}
