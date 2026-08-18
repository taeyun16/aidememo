//! Authenticated CLI client for the typed remote handoff server surface.
//!
//! The caller selects a stored profile with `--remote-profile` or
//! `AIDEMEMO_REMOTE_PROFILE`; actor identity always comes from that profile's
//! server-side bearer binding.

mod send_operation;

use self::send_operation::{
    QueuedSendOperation, RemoteSendOperationStore, ReservedSendOperation, SendOperationMeta,
    SendOperationState, SendReplayPlan,
};
use crate::cmd::{HandoffSub, artifacts, auth};
use aidememo_core::{AideMemo, AideMemoError, Config};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn run_remote_handoff(
    store_path: &Path,
    config: Config,
    profile_name: &str,
    sub: HandoffSub,
    json_output: bool,
) -> Result<String, AideMemoError> {
    let wiki = if matches!(
        &sub,
        HandoffSub::Send { .. } | HandoffSub::Accept { .. } | HandoffSub::Return { .. }
    ) {
        Some(AideMemo::open(store_path, config)?)
    } else {
        None
    };
    let value = execute_remote_handoff(wiki.as_ref(), profile_name, sub)?;
    render(value, json_output)
}

pub(crate) fn execute_remote_handoff(
    wiki: Option<&AideMemo>,
    profile_name: &str,
    sub: HandoffSub,
) -> Result<Value, AideMemoError> {
    let client = RemoteHandoffClient::new(auth::load_remote_profile(profile_name)?)?;
    if matches!(&sub, HandoffSub::Send { .. }) {
        return execute_remote_send(wiki, &client, sub);
    }
    let identity = client.identity()?;
    let value = match sub {
        HandoffSub::Send { .. } => unreachable!("send is handled before remote identity lookup"),
        HandoffSub::Inbox {
            actor_id,
            source_id,
            include_completed,
            limit,
        } => {
            reject_actor_override(actor_id.as_deref())?;
            client.mailbox(
                "inbox",
                source_id.or_else(default_source_id).as_deref(),
                include_completed,
                limit.unwrap_or(20),
            )?
        }
        HandoffSub::Outbox {
            actor_id,
            source_id,
            include_completed,
            pending_only,
            limit,
        } => {
            reject_actor_override(actor_id.as_deref())?;
            client.mailbox(
                "outbox",
                source_id.or_else(default_source_id).as_deref(),
                include_completed || !pending_only,
                limit.unwrap_or(20),
            )?
        }
        HandoffSub::Show { handoff_id } => client.handoff(&handoff_id)?,
        HandoffSub::Status {
            actor_id,
            handoff_id,
        } => {
            reject_actor_override(actor_id.as_deref())?;
            client.handoff(&handoff_id)?
        }
        HandoffSub::Accept {
            actor_id,
            handoff_id,
        } => {
            reject_actor_override(actor_id.as_deref())?;
            let status = client.handoff(&handoff_id)?;
            let record = status.get("record").ok_or_else(|| {
                AideMemoError::Internal("remote handoff response omitted record".to_owned())
            })?;
            let authenticated_actor = required_str(&identity, "actor_id")?;
            if required_str(record, "to_actor")? != authenticated_actor {
                return Err(AideMemoError::InvalidInput(format!(
                    "remote handoff is not addressed to authenticated actor {authenticated_actor}"
                )));
            }
            let revision = required_u64(&status, "revision")?;
            let status_name = required_str(record, "status")?;
            let attempt_count = required_u64(record, "attempt_count")?;
            let previous_outcome = optional_str(record, "outcome")?;
            let next_attempt = match (status_name, previous_outcome) {
                ("pending", None) | ("accepted", Some("failed")) => {
                    attempt_count.checked_add(1).ok_or_else(|| {
                        AideMemoError::InvalidInput(
                            "remote handoff attempt counter overflow".to_owned(),
                        )
                    })?
                }
                ("accepted", None) | ("completed", Some("succeeded")) => attempt_count,
                _ => {
                    return Err(AideMemoError::InvalidInput(format!(
                        "remote handoff has inconsistent retry state: status={status_name}, outcome={}",
                        previous_outcome.unwrap_or("none")
                    )));
                }
            };
            let claim_id = stable_claim_id(
                &client.profile.project_id,
                authenticated_actor,
                &handoff_id,
                next_attempt,
            );
            let recovered = matches!(
                (status_name, previous_outcome),
                ("accepted", None) | ("completed", Some("succeeded"))
            );
            if recovered && optional_str(record, "claim_id")? != Some(claim_id.as_str()) {
                return Err(AideMemoError::InvalidInput(
                    "remote handoff was accepted with a legacy or different claim; automatic retry recovery is unsafe"
                        .to_owned(),
                ));
            }
            let command_id = stable_operation_id(
                "command_accept",
                &[
                    &client.profile.project_id,
                    authenticated_actor,
                    &handoff_id,
                    &claim_id,
                ],
            );
            let session_id = required_str(record, "session_id")?;
            let source_id = optional_str(record, "source_id")?;
            let session = client.resource("session", session_id)?.ok_or_else(|| {
                AideMemoError::Internal(format!(
                    "remote handoff session {session_id} is missing from canonical storage"
                ))
            })?;
            ensure_fields_match(
                &session,
                &json!({"session_id": session_id, "source_id": source_id}),
                &["session_id", "source_id"],
            )?;
            let context = optional_str(record, "context_id")?
                .map(|context_id| {
                    let context =
                        client
                            .resource("handoff_context", context_id)?
                            .ok_or_else(|| {
                                AideMemoError::Internal(format!(
                                    "remote handoff context {context_id} is missing"
                                ))
                            })?;
                    ensure_fields_match(
                        &context,
                        &json!({
                            "context_id": context_id,
                            "handoff_id": handoff_id,
                            "session_id": session_id,
                            "source_id": source_id,
                            "from_actor": required_str(record, "from_actor")?,
                            "to_actor": authenticated_actor,
                        }),
                        &[
                            "context_id",
                            "handoff_id",
                            "session_id",
                            "source_id",
                            "from_actor",
                            "to_actor",
                        ],
                    )?;
                    Ok(context)
                })
                .transpose()?;
            let wiki = required_wiki(wiki, "remote handoff accept")?;
            let local_context_fact_id =
                materialize_remote_context(wiki, record, &session, context.as_ref())?;
            let receipt = if recovered {
                Value::Null
            } else {
                client.post(
                    &format!("/handoffs/{handoff_id}/accept"),
                    json!({
                        "command_id": command_id,
                        "expected_revision": revision,
                        "payload": {"claim_id": claim_id},
                    }),
                )?
            };
            json!({
                "artifact": "remote_handoff_accept",
                "remote_profile": client.profile.name,
                "actor_id": identity["actor_id"],
                "handoff_id": handoff_id,
                "command_id": command_id,
                "claim_id": claim_id,
                "recovered": recovered,
                "session_id": session_id,
                "source_id": source_id,
                "context_id": optional_str(record, "context_id")?,
                "local_context_fact_id": local_context_fact_id.to_string(),
                "resume": {
                    "command": artifacts::session_resume_command(session_id, source_id),
                    "env": {
                        "AIDEMEMO_SESSION_ID": session_id,
                        "AIDEMEMO_SOURCE_ID": source_id,
                        "AIDEMEMO_ACTOR_ID": identity["actor_id"],
                    }
                },
                "receipt": receipt,
            })
        }
        HandoffSub::Return {
            actor_id,
            outcome,
            result_fact_id,
            handoff_id,
        } => {
            reject_actor_override(actor_id.as_deref())?;
            let outcome = outcome.trim().to_ascii_lowercase();
            if !matches!(outcome.as_str(), "succeeded" | "failed") {
                return Err(AideMemoError::InvalidInput(
                    "outcome must be succeeded or failed".to_owned(),
                ));
            }
            let status = client.handoff(&handoff_id)?;
            let record = status.get("record").ok_or_else(|| {
                AideMemoError::Internal("remote handoff response omitted record".to_owned())
            })?;
            let revision = required_u64(&status, "revision")?;
            let session_id = required_str(record, "session_id")?;
            let claim_id = required_str(record, "claim_id")?;
            let source_id = optional_str(record, "source_id")?;
            let actor_id = required_str(&identity, "actor_id")?;
            let existing_result_fact_id = optional_str(record, "result_fact_id")?;
            let existing_outcome = optional_str(record, "outcome")?;
            let recovered = existing_result_fact_id == Some(&result_fact_id)
                && existing_outcome == Some(&outcome);
            if !recovered && (existing_result_fact_id.is_some() || existing_outcome.is_some()) {
                return Err(AideMemoError::InvalidInput(
                    "remote handoff already contains different result evidence; automatic retry recovery is unsafe"
                        .to_owned(),
                ));
            }
            let command_id = stable_operation_id(
                "command_return",
                &[
                    &client.profile.project_id,
                    actor_id,
                    &handoff_id,
                    claim_id,
                    &result_fact_id,
                    &outcome,
                ],
            );

            let wiki = required_wiki(wiki, "remote handoff return")?;
            let fact_id = result_fact_id
                .trim()
                .parse::<aidememo_core::ulid::Ulid>()
                .map(aidememo_core::FactId)
                .map_err(|_| {
                    AideMemoError::InvalidInput(
                        "result_fact_id must be a valid local fact ULID".to_owned(),
                    )
                })?;
            let fact = wiki.fact_get_scoped(&fact_id, source_id)?;
            let session = wiki.entity_get_scoped(session_id, source_id)?;
            if !fact.entity_ids.contains(&session.id) {
                return Err(AideMemoError::InvalidInput(format!(
                    "result fact {fact_id} is not attached to remote handoff session {session_id}"
                )));
            }
            if fact.actor_id.as_deref() != Some(actor_id) {
                return Err(AideMemoError::InvalidInput(format!(
                    "result fact {fact_id} was not written by authenticated remote actor {actor_id}"
                )));
            }
            client.ensure_fact(
                &result_fact_id,
                session_id,
                source_id,
                actor_id,
                &fact.content,
            )?;
            let receipt = if recovered {
                Value::Null
            } else {
                client.post(
                    &format!("/handoffs/{handoff_id}/return"),
                    json!({
                        "command_id": command_id,
                        "expected_revision": revision,
                        "payload": {
                            "claim_id": claim_id,
                            "result_fact_id": result_fact_id,
                            "outcome": outcome,
                        }
                    }),
                )?
            };
            json!({
                "remote_profile": client.profile.name,
                "handoff_id": handoff_id,
                "command_id": command_id,
                "result_fact_id": result_fact_id,
                "outcome": outcome,
                "recovered": recovered,
                "receipt": receipt,
            })
        }
        HandoffSub::Heartbeat { .. }
        | HandoffSub::Board { .. }
        | HandoffSub::Complete { .. }
        | HandoffSub::Run { .. } => {
            return Err(AideMemoError::InvalidInput(
                "remote profiles currently support handoff send, inbox, outbox, show/status, accept, and return"
                    .to_owned(),
            ));
        }
    };
    Ok(value)
}

fn execute_remote_send(
    wiki: Option<&AideMemo>,
    client: &RemoteHandoffClient,
    sub: HandoffSub,
) -> Result<Value, AideMemoError> {
    let HandoffSub::Send {
        from_actor,
        source_id,
        focus,
        done_when,
        kanban_task,
        kanban_board,
        installation,
        session,
    } = sub
    else {
        return Err(AideMemoError::Internal(
            "remote send helper received a non-send command".to_owned(),
        ));
    };
    reject_actor_override(from_actor.as_deref())?;
    if kanban_task.is_some() || kanban_board.is_some() {
        return Err(AideMemoError::InvalidInput(
            "remote handoff send does not yet persist Hermes Kanban metadata".to_owned(),
        ));
    }
    validate_id("receiver actor", &installation)?;
    let source_id = source_id.or_else(default_source_id);
    let wiki = required_wiki(wiki, "remote handoff send")?;
    let artifact = artifacts::agent_handoff(
        wiki,
        session.as_deref(),
        80,
        false,
        artifacts::AgentHandoffRoute {
            to_actor: Some(&installation),
            focus: focus.as_deref(),
            done_when: done_when.as_deref(),
            source_id: source_id.as_deref(),
            ..Default::default()
        },
    )?;
    let topic = artifact
        .topic
        .clone()
        .unwrap_or_else(|| artifact.session_id.clone());
    if artifact.body.len() > 65_536 {
        return Err(AideMemoError::InvalidInput(format!(
            "remote handoff packet is {} bytes; canonical handoff contexts are limited to 65536 bytes",
            artifact.body.len()
        )));
    }

    let profile_generation = client.profile.added_at.to_string();
    let intent_key = stable_operation_id(
        "send_intent_v2",
        &[
            &client.profile.name,
            &client.profile.url,
            &client.profile.project_id,
            &profile_generation,
            &installation,
            &artifact.session_id,
            source_id.as_deref().unwrap_or(""),
        ],
    );
    let payload_hash = stable_operation_id(
        "send_payload_v2",
        &[
            &client.profile.name,
            &client.profile.url,
            &client.profile.project_id,
            &profile_generation,
            &installation,
            &artifact.session_id,
            source_id.as_deref().unwrap_or(""),
            focus.as_deref().unwrap_or(""),
            done_when.as_deref().unwrap_or(""),
            &artifact.body,
        ],
    );
    let mut operation_store = RemoteSendOperationStore::open_default()?;
    let reservation =
        operation_store.reserve_send(&intent_key, &payload_hash, send_operation_meta(client))?;
    let plan = SendReplayPlan {
        session_id: artifact.session_id.clone(),
        source_id: source_id.clone(),
        topic,
        to_actor: installation.clone(),
        focus: focus.clone(),
        done_when: done_when.clone(),
        content: artifact.body.clone(),
    };
    operation_store.store_replay_plan(&intent_key, &reservation.operation_id, &plan)?;

    let response = |actor_id: Option<&str>,
                    state: SendOperationState,
                    dispatched: bool,
                    queued: bool,
                    receipt: Value,
                    last_error: Option<&str>| {
        json!({
            "artifact": "agent_handoff",
            "remote_profile": &client.profile.name,
            "actor_id": actor_id,
            "operation_id": &reservation.operation_id,
            "recovered": reservation.reused_pending,
            "handoff_id": &reservation.handoff_id,
            "status": state.as_str(),
            "dispatched": dispatched,
            "queued": queued,
            "last_error": last_error,
            "session_id": &artifact.session_id,
            "topic": &artifact.topic,
            "source_id": &source_id,
            "to_actor": &installation,
            "focus": &focus,
            "done_when": &done_when,
            "context_id": &reservation.context_id,
            "fact_count": artifact.fact_count,
            "bytes": artifact.body.len(),
            "resume": {
                "command": artifacts::session_resume_command(
                    &artifact.session_id,
                    source_id.as_deref(),
                ),
                "env": {
                    "AIDEMEMO_SESSION_ID": &artifact.session_id,
                    "AIDEMEMO_SOURCE_ID": &source_id,
                }
            },
            "content": &artifact.body,
            "receipt": receipt,
        })
    };

    if reservation.already_committed {
        return Ok(response(
            reservation.actor_id.as_deref(),
            SendOperationState::Committed,
            true,
            false,
            Value::Null,
            None,
        ));
    }

    match publish_reserved_send(
        client,
        &mut operation_store,
        &intent_key,
        &reservation,
        &plan,
    ) {
        Ok((actor_id, receipt)) => Ok(response(
            Some(&actor_id),
            SendOperationState::Committed,
            true,
            false,
            receipt,
            None,
        )),
        Err(error) => {
            let state = send_failure_state(&error);
            let message = error.to_string();
            operation_store.record_failure(
                &intent_key,
                &reservation.operation_id,
                state,
                &message,
            )?;
            let actor_id = operation_store
                .pending(Some(&client.profile.name))?
                .into_iter()
                .find(|entry| entry.operation_id == reservation.operation_id)
                .and_then(|entry| entry.actor_id);
            Ok(response(
                actor_id.as_deref(),
                state,
                false,
                true,
                Value::Null,
                Some(&message),
            ))
        }
    }
}

fn send_operation_meta(client: &RemoteHandoffClient) -> SendOperationMeta<'_> {
    SendOperationMeta {
        profile_name: &client.profile.name,
        profile_url: &client.profile.url,
        profile_added_at: client.profile.added_at,
        project_id: &client.profile.project_id,
    }
}

fn publish_reserved_send(
    client: &RemoteHandoffClient,
    store: &mut RemoteSendOperationStore,
    intent_key: &str,
    reservation: &ReservedSendOperation,
    plan: &SendReplayPlan,
) -> Result<(String, Value), AideMemoError> {
    let identity = client.identity()?;
    let actor_id = required_str(&identity, "actor_id")?.to_owned();
    store.bind_actor(
        intent_key,
        &reservation.operation_id,
        send_operation_meta(client),
        &actor_id,
    )?;
    store.mark_queued(intent_key, &reservation.operation_id)?;
    let receipt = submit_remote_send(
        client,
        &actor_id,
        &reservation.handoff_id,
        &reservation.context_id,
        plan,
    )?;
    store.mark_committed(intent_key, &reservation.operation_id)?;
    Ok((actor_id, receipt))
}

fn publish_queued_send(
    client: &RemoteHandoffClient,
    store: &mut RemoteSendOperationStore,
    entry: &QueuedSendOperation,
    actor_id: &str,
) -> Result<Value, AideMemoError> {
    store.bind_actor(
        &entry.intent_key,
        &entry.operation_id,
        send_operation_meta(client),
        actor_id,
    )?;
    store.mark_queued(&entry.intent_key, &entry.operation_id)?;
    let receipt = submit_remote_send(
        client,
        actor_id,
        &entry.handoff_id,
        &entry.context_id,
        &entry.plan,
    )?;
    store.mark_committed(&entry.intent_key, &entry.operation_id)?;
    Ok(receipt)
}

fn submit_remote_send(
    client: &RemoteHandoffClient,
    actor_id: &str,
    handoff_id: &str,
    context_id: &str,
    plan: &SendReplayPlan,
) -> Result<Value, AideMemoError> {
    client.ensure_session(&plan.session_id, plan.source_id.as_deref(), &plan.topic)?;
    client.ensure_handoff_context(
        context_id,
        handoff_id,
        &plan.session_id,
        plan.source_id.as_deref(),
        actor_id,
        &plan.to_actor,
        &plan.content,
    )?;
    client.post(
        "/handoffs",
        json!({
            "command_id": stable_operation_id(
                "command_send",
                &[&client.profile.project_id, handoff_id],
            ),
            "payload": {
                "handoff_id": handoff_id,
                "session_id": &plan.session_id,
                "to_actor": &plan.to_actor,
                "focus": &plan.focus,
                "done_when": &plan.done_when,
                "context_id": context_id,
            }
        }),
    )
}

fn send_failure_state(error: &AideMemoError) -> SendOperationState {
    let message = error.to_string();
    if message.contains("remote server returned HTTP 409")
        || message.contains("different evidence")
        || message.contains("profile identity changed")
        || message.contains("different authenticated actor")
        || message.contains("already bound to a different")
    {
        SendOperationState::Conflict
    } else {
        SendOperationState::Failed
    }
}

pub(crate) fn identity_for_profile(profile_name: &str) -> Result<Value, AideMemoError> {
    RemoteHandoffClient::new(auth::load_remote_profile(profile_name)?)?.identity()
}

pub(crate) fn actor_id_for_profile(profile_name: &str) -> Result<String, AideMemoError> {
    let identity = identity_for_profile(profile_name)?;
    required_str(&identity, "actor_id").map(str::to_owned)
}
pub(crate) fn remote_outbox_entries(profile_name: Option<&str>) -> Result<Value, AideMemoError> {
    let store = RemoteSendOperationStore::open_default()?;
    let queued = store.pending(profile_name)?;
    let entries = queued
        .iter()
        .map(|entry| {
            queued_entry_json(entry, entry.state, entry.last_error.as_deref(), Value::Null)
        })
        .collect::<Vec<_>>();
    Ok(json!({"count": entries.len(), "entries": entries}))
}

pub(crate) fn publish_remote_outbox(profile_name: &str) -> Result<Value, AideMemoError> {
    let client = RemoteHandoffClient::new(auth::load_remote_profile(profile_name)?)?;
    let mut store = RemoteSendOperationStore::open_default()?;
    let queued = store.pending(Some(profile_name))?;
    if queued.is_empty() {
        return Ok(json!({
            "remote_profile": profile_name,
            "attempted": 0,
            "published": 0,
            "failed": 0,
            "conflicts": 0,
            "entries": [],
        }));
    }

    let identity = client.identity();
    let mut attempted = 0_usize;
    let mut published = 0_usize;
    let mut failed = 0_usize;
    let mut conflicts = 0_usize;
    let mut results = Vec::with_capacity(queued.len());

    match identity {
        Ok(identity) => {
            let actor_id = required_str(&identity, "actor_id")?.to_owned();
            for entry in queued {
                if entry.state == SendOperationState::Conflict {
                    conflicts += 1;
                    results.push(queued_entry_json(
                        &entry,
                        SendOperationState::Conflict,
                        entry.last_error.as_deref(),
                        Value::Null,
                    ));
                    continue;
                }
                attempted += 1;
                match publish_queued_send(&client, &mut store, &entry, &actor_id) {
                    Ok(receipt) => {
                        published += 1;
                        results.push(queued_entry_json(
                            &entry,
                            SendOperationState::Committed,
                            None,
                            receipt,
                        ));
                    }
                    Err(error) => {
                        let state = send_failure_state(&error);
                        let message = error.to_string();
                        store.record_failure(
                            &entry.intent_key,
                            &entry.operation_id,
                            state,
                            &message,
                        )?;
                        if state == SendOperationState::Conflict {
                            conflicts += 1;
                        } else {
                            failed += 1;
                        }
                        results.push(queued_entry_json(
                            &entry,
                            state,
                            Some(&message),
                            Value::Null,
                        ));
                    }
                }
            }
        }
        Err(error) => {
            let state = send_failure_state(&error);
            let message = error.to_string();
            for entry in queued {
                if entry.state == SendOperationState::Conflict {
                    conflicts += 1;
                    results.push(queued_entry_json(
                        &entry,
                        SendOperationState::Conflict,
                        entry.last_error.as_deref(),
                        Value::Null,
                    ));
                    continue;
                }
                attempted += 1;
                store.record_failure(&entry.intent_key, &entry.operation_id, state, &message)?;
                if state == SendOperationState::Conflict {
                    conflicts += 1;
                } else {
                    failed += 1;
                }
                results.push(queued_entry_json(
                    &entry,
                    state,
                    Some(&message),
                    Value::Null,
                ));
            }
        }
    }

    Ok(json!({
        "remote_profile": profile_name,
        "attempted": attempted,
        "published": published,
        "failed": failed,
        "conflicts": conflicts,
        "entries": results,
    }))
}

fn queued_entry_json(
    entry: &QueuedSendOperation,
    state: SendOperationState,
    last_error: Option<&str>,
    receipt: Value,
) -> Value {
    json!({
        "profile_name": &entry.profile_name,
        "profile_url": &entry.profile_url,
        "profile_added_at": entry.profile_added_at,
        "project_id": &entry.project_id,
        "actor_id": &entry.actor_id,
        "operation_id": &entry.operation_id,
        "handoff_id": &entry.handoff_id,
        "context_id": &entry.context_id,
        "session_id": &entry.plan.session_id,
        "source_id": &entry.plan.source_id,
        "to_actor": &entry.plan.to_actor,
        "focus": &entry.plan.focus,
        "done_when": &entry.plan.done_when,
        "bytes": entry.plan.content.len(),
        "created_at_ms": entry.created_at_ms,
        "state": state.as_str(),
        "last_error": last_error,
        "receipt": receipt,
    })
}

fn required_wiki<'a>(
    wiki: Option<&'a AideMemo>,
    operation: &str,
) -> Result<&'a AideMemo, AideMemoError> {
    wiki.ok_or_else(|| {
        AideMemoError::Internal(format!("{operation} requires the embedded session store"))
    })
}

fn materialize_remote_context(
    wiki: &AideMemo,
    handoff: &Value,
    session: &Value,
    context: Option<&Value>,
) -> Result<aidememo_core::FactId, AideMemoError> {
    let handoff_id = required_str(handoff, "handoff_id")?;
    let session_id = required_str(session, "session_id")?;
    let topic = required_str(session, "topic")?;
    let source_id = optional_str(session, "source_id")?;
    let from_actor = required_str(handoff, "from_actor")?;

    let session_entity = match wiki.entity_get(session_id) {
        Ok(entity) => {
            if entity.entity_type.to_string() != "session" {
                return Err(AideMemoError::InvalidInput(format!(
                    "local entity {session_id} is not a session"
                )));
            }
            if entity
                .source_page
                .as_deref()
                .is_some_and(|local| local != topic)
            {
                return Err(AideMemoError::InvalidInput(format!(
                    "local session {session_id} has a different topic than canonical SSOT"
                )));
            }
            entity
        }
        Err(AideMemoError::EntityNotFound { .. }) => {
            let id = wiki.entity_add(aidememo_core::EntityInput {
                name: session_id.to_owned(),
                entity_type: Some(aidememo_core::EntityType::parse("session")),
                source_page: Some(topic.to_owned()),
                ..Default::default()
            })?;
            wiki.entity_get_by_id(id)?
        }
        Err(error) => return Err(error),
    };

    let content = if let Some(context) = context {
        required_str(context, "content")?.to_owned()
    } else {
        let mut content = format!("Remote handoff {handoff_id} from {from_actor}: {topic}");
        if let Some(focus) = optional_str(handoff, "focus")? {
            content.push_str("\n\nFocus: ");
            content.push_str(focus);
        }
        if let Some(done_when) = optional_str(handoff, "done_when")? {
            content.push_str("\n\nDone when: ");
            content.push_str(done_when);
        }
        content
    };
    let fact_id = wiki.fact_add(aidememo_core::FactInput {
        content,
        fact_type: Some(aidememo_core::FactType::Note),
        entity_ids: Some(vec![session_entity.id]),
        tags: Some(vec![
            "remote-handoff-context".to_owned(),
            format!("handoff:{handoff_id}"),
        ]),
        source: Some(format!("remote-handoff:{handoff_id}")),
        source_id: source_id.map(str::to_owned),
        actor_id: Some(from_actor.to_owned()),
        source_confidence: Some(1.0),
        observed_at: None,
    })?;
    wiki.entity_get_scoped(session_id, source_id)?;
    Ok(fact_id)
}

struct RemoteHandoffClient {
    profile: auth::RemoteAuthProfile,
    agent: ureq::Agent,
}

impl RemoteHandoffClient {
    fn new(profile: auth::RemoteAuthProfile) -> Result<Self, AideMemoError> {
        validate_id("project_id", &profile.project_id)?;
        Ok(Self {
            profile,
            agent: ureq::AgentBuilder::new().build(),
        })
    }

    fn identity(&self) -> Result<Value, AideMemoError> {
        self.get("/identity")
    }

    fn handoff(&self, handoff_id: &str) -> Result<Value, AideMemoError> {
        validate_id("handoff_id", handoff_id)?;
        self.get(&format!("/handoffs/{handoff_id}"))
    }

    fn mailbox(
        &self,
        mailbox: &str,
        source_id: Option<&str>,
        include_completed: bool,
        limit: usize,
    ) -> Result<Value, AideMemoError> {
        if !(1..=100).contains(&limit) {
            return Err(AideMemoError::InvalidInput(
                "remote handoff limit must be between 1 and 100".to_owned(),
            ));
        }
        let endpoint = self.endpoint("/handoffs");
        let mut request = self
            .agent
            .get(&endpoint)
            .set("Authorization", &format!("Bearer {}", self.profile.token))
            .query("box", mailbox)
            .query(
                "include_completed",
                if include_completed { "true" } else { "false" },
            )
            .query("limit", &limit.to_string());
        if let Some(source_id) = source_id {
            validate_id("source_id", source_id)?;
            request = request.query("source_id", source_id);
        }
        decode(request.call())
    }

    fn ensure_session(
        &self,
        session_id: &str,
        source_id: Option<&str>,
        topic: &str,
    ) -> Result<(), AideMemoError> {
        validate_id("session_id", session_id)?;
        if let Some(existing) = self.resource("session", session_id)? {
            let expected = json!({
                "session_id": session_id,
                "source_id": source_id,
                "topic": topic,
            });
            ensure_fields_match(&existing, &expected, &["session_id", "source_id", "topic"])?;
            return Ok(());
        }
        self.post(
            "/sessions",
            json!({
                "command_id": stable_operation_id(
                    "command_session",
                    &[&self.profile.project_id, session_id],
                ),
                "payload": {
                    "session_id": session_id,
                    "source_id": source_id,
                    "topic": topic,
                }
            }),
        )?;
        Ok(())
    }

    fn ensure_fact(
        &self,
        fact_id: &str,
        session_id: &str,
        source_id: Option<&str>,
        actor_id: &str,
        content: &str,
    ) -> Result<(), AideMemoError> {
        validate_id("fact_id", fact_id)?;
        if let Some(existing) = self.resource("fact", fact_id)? {
            let expected = json!({
                "fact_id": fact_id,
                "session_id": session_id,
                "source_id": source_id,
                "actor_id": actor_id,
                "content": content,
            });
            ensure_fields_match(
                &existing,
                &expected,
                &["fact_id", "session_id", "source_id", "actor_id", "content"],
            )?;
            return Ok(());
        }
        self.post(
            "/facts",
            json!({
                "command_id": stable_operation_id(
                    "command_fact",
                    &[&self.profile.project_id, fact_id],
                ),
                "payload": {
                    "fact_id": fact_id,
                    "session_id": session_id,
                    "content": content,
                }
            }),
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_handoff_context(
        &self,
        context_id: &str,
        handoff_id: &str,
        session_id: &str,
        source_id: Option<&str>,
        from_actor: &str,
        to_actor: &str,
        content: &str,
    ) -> Result<(), AideMemoError> {
        validate_id("context_id", context_id)?;
        if let Some(existing) = self.resource("handoff_context", context_id)? {
            let expected = json!({
                "context_id": context_id,
                "handoff_id": handoff_id,
                "session_id": session_id,
                "source_id": source_id,
                "from_actor": from_actor,
                "to_actor": to_actor,
                "content": content,
            });
            ensure_fields_match(
                &existing,
                &expected,
                &[
                    "context_id",
                    "handoff_id",
                    "session_id",
                    "source_id",
                    "from_actor",
                    "to_actor",
                    "content",
                ],
            )?;
            return Ok(());
        }
        self.post(
            "/handoff-contexts",
            json!({
                "command_id": stable_operation_id(
                    "command_context",
                    &[&self.profile.project_id, context_id],
                ),
                "payload": {
                    "context_id": context_id,
                    "handoff_id": handoff_id,
                    "session_id": session_id,
                    "to_actor": to_actor,
                    "content": content,
                }
            }),
        )?;
        Ok(())
    }

    fn resource(&self, kind: &str, id: &str) -> Result<Option<Value>, AideMemoError> {
        let endpoint = format!(
            "{}/v1/projects/{}/resources/{kind}/{id}",
            self.profile.url, self.profile.project_id
        );
        let request = self
            .agent
            .get(&endpoint)
            .set("Authorization", &format!("Bearer {}", self.profile.token));
        match request.call() {
            Ok(response) => {
                let value = response.into_json::<Value>().map_err(|error| {
                    AideMemoError::Internal(format!("decode remote resource response: {error}"))
                })?;
                Ok(value
                    .get("state")
                    .and_then(|state| state.get("body"))
                    .cloned())
            }
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(error) => Err(remote_error(error)),
        }
    }

    fn get(&self, path: &str) -> Result<Value, AideMemoError> {
        let request = self
            .agent
            .get(&self.endpoint(path))
            .set("Authorization", &format!("Bearer {}", self.profile.token));
        decode(request.call())
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, AideMemoError> {
        let endpoint = self.endpoint(path);
        let request = self
            .agent
            .post(&endpoint)
            .set("Authorization", &format!("Bearer {}", self.profile.token));
        match request.send_json(body.clone()) {
            Ok(response) => decode(Ok(response)),
            Err(ureq::Error::Transport(_)) => {
                let retry = self
                    .agent
                    .post(&endpoint)
                    .set("Authorization", &format!("Bearer {}", self.profile.token));
                decode(retry.send_json(body))
            }
            Err(error) => Err(remote_error(error)),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/v1/projects/{}{}",
            self.profile.url, self.profile.project_id, path
        )
    }
}

fn decode(result: Result<ureq::Response, ureq::Error>) -> Result<Value, AideMemoError> {
    result
        .map_err(remote_error)?
        .into_json::<Value>()
        .map_err(|error| AideMemoError::Internal(format!("decode remote response: {error}")))
}

fn remote_error(error: ureq::Error) -> AideMemoError {
    match error {
        ureq::Error::Status(status, response) => {
            let detail = response
                .into_json::<Value>()
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "remote request failed".to_owned());
            AideMemoError::InvalidInput(format!("remote server returned HTTP {status}: {detail}"))
        }
        ureq::Error::Transport(error) => {
            AideMemoError::Internal(format!("remote server request failed: {error}"))
        }
    }
}

fn reject_actor_override(actor_id: Option<&str>) -> Result<(), AideMemoError> {
    if actor_id.is_some() {
        Err(AideMemoError::InvalidInput(
            "--actor-id/--from cannot override actor identity for a remote profile".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn validate_id(label: &str, value: &str) -> Result<(), AideMemoError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@')
        })
    {
        return Err(AideMemoError::InvalidInput(format!(
            "{label} is not a valid remote identifier"
        )));
    }
    Ok(())
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, AideMemoError> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| {
        AideMemoError::Internal(format!("remote response omitted string field {key}"))
    })
}

fn optional_str<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, AideMemoError> {
    match value.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value.as_str().map(Some).ok_or_else(|| {
            AideMemoError::Internal(format!("remote response field {key} is not a string"))
        }),
    }
}

fn required_u64(value: &Value, key: &str) -> Result<u64, AideMemoError> {
    value.get(key).and_then(Value::as_u64).ok_or_else(|| {
        AideMemoError::Internal(format!("remote response omitted integer field {key}"))
    })
}

fn ensure_fields_match(
    existing: &Value,
    expected: &Value,
    fields: &[&str],
) -> Result<(), AideMemoError> {
    if fields
        .iter()
        .all(|field| existing.get(*field) == expected.get(*field))
    {
        Ok(())
    } else {
        Err(AideMemoError::InvalidInput(
            "remote canonical resource already exists with different evidence".to_owned(),
        ))
    }
}

fn stable_claim_id(project_id: &str, actor_id: &str, handoff_id: &str, attempt: u64) -> String {
    stable_operation_id(
        "claim",
        &[project_id, actor_id, handoff_id, &attempt.to_string()],
    )
}

fn stable_operation_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update((prefix.len() as u64).to_be_bytes());
    hasher.update(prefix.as_bytes());
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut id = String::with_capacity(prefix.len() + 1 + digest.len() * 2);
    id.push_str(prefix);
    id.push('_');
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        id.push(char::from(HEX[usize::from(byte >> 4)]));
        id.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    id
}

fn default_source_id() -> Option<String> {
    std::env::var("AIDEMEMO_SOURCE_ID")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn render(value: Value, json_output: bool) -> Result<String, AideMemoError> {
    if json_output {
        return serde_json::to_string_pretty(&value).map_err(|source| AideMemoError::Serialize {
            context: "remote handoff JSON".to_owned(),
            source,
        });
    }
    if value.get("artifact").and_then(Value::as_str) == Some("remote_handoff_accept") {
        let session_id = required_str(&value, "session_id")?;
        let source_id = optional_str(&value, "source_id")?;
        let actor_id = required_str(&value, "actor_id")?;
        let handoff_id = required_str(&value, "handoff_id")?;
        let local_context_fact_id = required_str(&value, "local_context_fact_id")?;
        return Ok(format!(
            "# aidememo remote handoff accepted: {handoff_id}\n# local context fact: {local_context_fact_id}\n{}\nexport AIDEMEMO_ACTOR_ID={actor_id}",
            artifacts::session_resume_exports(session_id, source_id),
        ));
    }
    if let Some(assignments) = value.get("assignments").and_then(Value::as_array) {
        if assignments.is_empty() {
            return Ok("(no remote handoff assignments)".to_owned());
        }
        let mut output = format!("{} remote handoff assignment(s):\n", assignments.len());
        for assignment in assignments {
            let record = assignment.get("record").unwrap_or(assignment);
            output.push_str(&format!(
                "  {} [{}] {} -> {} session={}\n",
                record
                    .get("handoff_id")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
                record.get("status").and_then(Value::as_str).unwrap_or("-"),
                record
                    .get("from_actor")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
                record
                    .get("to_actor")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
                record
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
            ));
        }
        return Ok(output.trim_end().to_owned());
    }
    serde_json::to_string_pretty(&value).map_err(|source| AideMemoError::Serialize {
        context: "remote handoff output".to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_handoff_renders_shell_evaluable_session_exports() -> Result<(), AideMemoError> {
        let output = render(
            json!({
                "artifact": "remote_handoff_accept",
                "handoff_id": "handoff_test",
                "session_id": "session_test",
                "source_id": "project:aidememo",
                "actor_id": "codex-p2",
                "local_context_fact_id": "01TEST",
            }),
            false,
        )?;
        assert!(output.starts_with("# aidememo remote handoff accepted: handoff_test"));
        assert!(output.contains("export AIDEMEMO_SESSION_ID='session_test'"));
        assert!(output.contains("export AIDEMEMO_SOURCE_ID='project:aidememo'"));
        assert!(output.ends_with("export AIDEMEMO_ACTOR_ID=codex-p2"));
        Ok(())
    }

    #[test]
    fn outbox_failure_classification_separates_conflicts() {
        let conflict = AideMemoError::InvalidInput(
            "remote server returned HTTP 409: command conflict".to_owned(),
        );
        assert_eq!(send_failure_state(&conflict), SendOperationState::Conflict);
        let offline =
            AideMemoError::Internal("remote server request failed: connection refused".to_owned());
        assert_eq!(send_failure_state(&offline), SendOperationState::Failed);
    }

    #[test]
    fn stable_operation_ids_are_repeatable_and_domain_separated() {
        let accept = stable_operation_id("command_accept", &["project", "actor", "handoff"]);
        assert_eq!(
            accept,
            stable_operation_id("command_accept", &["project", "actor", "handoff"])
        );
        assert_ne!(
            accept,
            stable_operation_id("command_return", &["project", "actor", "handoff"])
        );
        assert_ne!(
            stable_operation_id("command_accept", &["ab", "c"]),
            stable_operation_id("command_accept", &["a", "bc"])
        );
    }
}
