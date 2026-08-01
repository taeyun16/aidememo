//! Authenticated CLI client for the typed remote handoff server surface.
//!
//! The caller selects a stored profile with `--remote-profile` or
//! `AIDEMEMO_REMOTE_PROFILE`; actor identity always comes from that profile's
//! server-side bearer binding.

use crate::cmd::{HandoffSub, artifacts, auth};
use aidememo_core::{AideMemo, AideMemoError, Config};
use serde_json::{Value, json};
use std::path::Path;

pub fn run_remote_handoff(
    store_path: &Path,
    config: Config,
    profile_name: &str,
    sub: HandoffSub,
    json_output: bool,
) -> Result<String, AideMemoError> {
    let client = RemoteHandoffClient::new(auth::load_remote_profile(profile_name)?)?;
    let identity = client.identity()?;
    let value = match sub {
        HandoffSub::Send {
            from_actor,
            source_id,
            focus,
            done_when,
            kanban_task,
            kanban_board,
            installation,
            session,
        } => {
            reject_actor_override(from_actor.as_deref())?;
            if kanban_task.is_some() || kanban_board.is_some() {
                return Err(AideMemoError::InvalidInput(
                    "remote handoff send does not yet persist Hermes Kanban metadata".to_owned(),
                ));
            }
            validate_id("receiver actor", &installation)?;
            let source_id = source_id.or_else(default_source_id);
            let wiki = AideMemo::open(store_path, config)?;
            let artifact = artifacts::agent_handoff(
                &wiki,
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
                .unwrap_or_else(|| artifact.session_id.clone());
            client.ensure_session(&artifact.session_id, source_id.as_deref(), &topic)?;
            let handoff_id = generated_id("handoff");
            let receipt = client.post(
                "/handoffs",
                json!({
                    "command_id": generated_id("command_send"),
                    "payload": {
                        "handoff_id": handoff_id,
                        "session_id": artifact.session_id,
                        "to_actor": installation,
                        "focus": focus,
                        "done_when": done_when,
                    }
                }),
            )?;
            json!({
                "remote_profile": client.profile.name,
                "actor_id": identity["actor_id"],
                "handoff_id": handoff_id,
                "receipt": receipt,
            })
        }
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
            let revision = required_u64(&status, "revision")?;
            let claim_id = generated_id("claim");
            let receipt = client.post(
                &format!("/handoffs/{handoff_id}/accept"),
                json!({
                    "command_id": generated_id("command_accept"),
                    "expected_revision": revision,
                    "payload": {"claim_id": claim_id},
                }),
            )?;
            json!({
                "remote_profile": client.profile.name,
                "handoff_id": handoff_id,
                "claim_id": claim_id,
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

            let wiki = AideMemo::open(store_path, config)?;
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
            let receipt = client.post(
                &format!("/handoffs/{handoff_id}/return"),
                json!({
                    "command_id": generated_id("command_return"),
                    "expected_revision": revision,
                    "payload": {
                        "claim_id": claim_id,
                        "result_fact_id": result_fact_id,
                        "outcome": outcome,
                    }
                }),
            )?;
            json!({
                "remote_profile": client.profile.name,
                "handoff_id": handoff_id,
                "result_fact_id": result_fact_id,
                "outcome": outcome,
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
    render(value, json_output)
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
                "command_id": generated_id("command_session"),
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
                "command_id": generated_id("command_fact"),
                "payload": {
                    "fact_id": fact_id,
                    "session_id": session_id,
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
        let request = self
            .agent
            .post(&self.endpoint(path))
            .set("Authorization", &format!("Bearer {}", self.profile.token));
        decode(request.send_json(body))
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
            "remote canonical resource already exists with different session/fact evidence"
                .to_owned(),
        ))
    }
}

fn generated_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::new())
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
