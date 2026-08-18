from pathlib import Path

path = Path('crates/aidememo-cli/src/cmd/remote_handoff.rs')
text = path.read_text()

anchor = '''        HandoffSub::Return {
            actor_id,
            outcome,
            result_fact_id,
            handoff_id,
        } => {'''
heartbeat = '''        HandoffSub::Heartbeat {
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
            if required_str(record, "status")? != "accepted" {
                return Err(AideMemoError::InvalidInput(
                    "remote handoff must be accepted before heartbeat".to_owned(),
                ));
            }
            if optional_str(record, "outcome")?.is_some() {
                return Err(AideMemoError::InvalidInput(
                    "remote handoff with result evidence cannot be heartbeated".to_owned(),
                ));
            }
            let claim_id = required_str(record, "claim_id")?;
            let revision = required_u64(&status, "revision")?;
            let revision_key = revision.to_string();
            let command_id = stable_operation_id(
                "command_heartbeat",
                &[
                    &client.profile.project_id,
                    authenticated_actor,
                    &handoff_id,
                    claim_id,
                    &revision_key,
                ],
            );
            let receipt = client.post(
                &format!("/handoffs/{handoff_id}/heartbeat"),
                json!({
                    "command_id": command_id,
                    "expected_revision": revision,
                    "payload": {"claim_id": claim_id},
                }),
            )?;
            json!({
                "remote_profile": client.profile.name,
                "actor_id": authenticated_actor,
                "handoff_id": handoff_id,
                "command_id": command_id,
                "claim_id": claim_id,
                "expected_revision": revision,
                "receipt": receipt,
            })
        }
'''
if text.count(anchor) != 1:
    raise SystemExit('remote return anchor changed')
text = text.replace(anchor, heartbeat + anchor, 1)
old = '''        HandoffSub::Heartbeat { .. }
        | HandoffSub::Board { .. }
        | HandoffSub::Complete { .. }
        | HandoffSub::Run { .. } => {
            return Err(AideMemoError::InvalidInput(
                "remote profiles currently support handoff send, inbox, outbox, show/status, accept, and return"
                    .to_owned(),
            ));
        }'''
new = '''        HandoffSub::Board { .. } | HandoffSub::Complete { .. } | HandoffSub::Run { .. } => {
            return Err(AideMemoError::InvalidInput(
                "remote profiles currently support handoff send, inbox, outbox, show/status, accept, heartbeat, and return"
                    .to_owned(),
            ));
        }'''
if text.count(old) != 1:
    raise SystemExit('remote unsupported block changed')
path.write_text(text.replace(old, new, 1))
