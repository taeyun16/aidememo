//! Lightweight cross-agent assignment ledger.
//!
//! This is deliberately not a message queue: records point at an existing
//! tracked session and carry routing/acknowledgement metadata only. There are
//! no topics, offsets, consumer groups, delivery retries, or payload copies.

use aidememo_core::types::{EntityInput, EntitySort, EntityType, EntityUpdate, ListOpts};
use aidememo_core::{AideMemo, AideMemoError, Result};
use serde::{Deserialize, Serialize};

const HANDOFF_ENTITY_TYPE: &str = "handoff";
const HANDOFF_ENTITY_TAG: &str = "aidememo:handoff";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HandoffStatus {
    Pending,
    Accepted,
    Completed,
}

impl HandoffStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffAssignment {
    pub handoff_id: String,
    pub session_id: String,
    pub source_id: Option<String>,
    pub from_actor: String,
    pub to_actor: String,
    pub from_agent: Option<String>,
    pub from_profile: Option<String>,
    pub to_agent: Option<String>,
    pub to_profile: Option<String>,
    pub focus: Option<String>,
    pub done_when: Option<String>,
    pub status: HandoffStatus,
    pub created_at: u64,
    pub accepted_at: Option<u64>,
    pub completed_at: Option<u64>,
    #[serde(default)]
    pub result_fact_id: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub returned_at: Option<u64>,
    #[serde(default)]
    pub last_heartbeat_at: Option<u64>,
    #[serde(default)]
    pub heartbeat_count: u64,
    #[serde(default)]
    pub upstream_system: Option<String>,
    #[serde(default)]
    pub upstream_task_id: Option<String>,
    #[serde(default)]
    pub upstream_board_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewHandoffAssignment {
    pub session_id: String,
    pub source_id: Option<String>,
    pub from_actor: String,
    pub to_actor: String,
    pub from_agent: Option<String>,
    pub from_profile: Option<String>,
    pub to_agent: Option<String>,
    pub to_profile: Option<String>,
    pub focus: Option<String>,
    pub done_when: Option<String>,
    pub upstream_system: Option<String>,
    pub upstream_task_id: Option<String>,
    pub upstream_board_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HandoffBoard {
    pub generated_at: u64,
    pub stale_after_ms: u64,
    pub lifecycle_owner: &'static str,
    pub lanes: HandoffBoardCounts,
    pub assignments: Vec<HandoffBoardItem>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HandoffBoardCounts {
    pub ready: usize,
    pub in_progress: usize,
    pub attention: usize,
    pub returned: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct HandoffBoardItem {
    pub assignment: HandoffAssignment,
    pub lane: &'static str,
    pub stale: bool,
    pub age_ms: u64,
    pub last_activity_at: u64,
    pub next_action: &'static str,
    pub lifecycle_owner: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub enum HandoffTransition {
    Accept,
    Complete,
}

pub fn actor_id(explicit: Option<&str>) -> Option<String> {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("AIDEMEMO_ACTOR_ID")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

pub fn dispatch(wiki: &AideMemo, input: NewHandoffAssignment) -> Result<HandoffAssignment> {
    validate_actor("from_actor", &input.from_actor)?;
    validate_actor("to_actor", &input.to_actor)?;
    let session = wiki.entity_get(input.session_id.trim())?;
    if session.entity_type.to_string() != "session" {
        return Err(AideMemoError::InvalidInput(format!(
            "{} is a {} entity, not a session",
            session.name, session.entity_type
        )));
    }

    let handoff_id = format!("handoff-{}", aidememo_core::ulid::Ulid::new());
    let record = HandoffAssignment {
        handoff_id: handoff_id.clone(),
        session_id: session.name,
        source_id: input.source_id,
        from_actor: input.from_actor,
        to_actor: input.to_actor,
        from_agent: input.from_agent,
        from_profile: input.from_profile,
        to_agent: input.to_agent,
        to_profile: input.to_profile,
        focus: input.focus,
        done_when: input.done_when,
        status: HandoffStatus::Pending,
        created_at: now_ms(),
        accepted_at: None,
        completed_at: None,
        result_fact_id: None,
        outcome: None,
        returned_at: None,
        last_heartbeat_at: None,
        heartbeat_count: 0,
        upstream_system: input.upstream_system,
        upstream_task_id: input.upstream_task_id,
        upstream_board_id: input.upstream_board_id,
    };
    let source_page = serialize_record(&record)?;
    wiki.entity_add(EntityInput {
        name: handoff_id,
        entity_type: Some(EntityType::parse(HANDOFF_ENTITY_TYPE)),
        tags: Some(record_tags(&record)),
        source_page: Some(source_page),
        ..Default::default()
    })?;
    Ok(record)
}

pub fn inbox(
    wiki: &AideMemo,
    actor: &str,
    source_id: Option<&str>,
    include_completed: bool,
    limit: usize,
) -> Result<Vec<HandoffAssignment>> {
    validate_actor("actor_id", actor)?;
    let summaries = wiki.entity_list(ListOpts {
        entity_type: Some(EntityType::parse(HANDOFF_ENTITY_TYPE)),
        sort_by: EntitySort::UpdatedAt,
        limit: None,
        ..Default::default()
    })?;
    let mut records = Vec::new();
    for summary in summaries {
        if !summary.tags.iter().any(|tag| tag == HANDOFF_ENTITY_TAG) {
            continue;
        }
        let entity = wiki.entity_get_by_id(summary.id)?;
        let record = parse_record(&entity.name, entity.source_page.as_deref())?;
        if record.to_actor != actor {
            continue;
        }
        if source_id.is_some_and(|scope| record.source_id.as_deref() != Some(scope)) {
            continue;
        }
        if !include_completed && record.status == HandoffStatus::Completed {
            continue;
        }
        records.push(record);
    }
    records.sort_by_key(|record| std::cmp::Reverse(record.created_at));
    records.truncate(limit);
    Ok(records)
}

pub fn outbox(
    wiki: &AideMemo,
    actor: &str,
    source_id: Option<&str>,
    include_completed: bool,
    limit: usize,
) -> Result<Vec<HandoffAssignment>> {
    assignments_for_actor(wiki, actor, source_id, include_completed, limit, true)
}

fn assignments_for_actor(
    wiki: &AideMemo,
    actor: &str,
    source_id: Option<&str>,
    include_completed: bool,
    limit: usize,
    sent: bool,
) -> Result<Vec<HandoffAssignment>> {
    validate_actor("actor_id", actor)?;
    let summaries = wiki.entity_list(ListOpts {
        entity_type: Some(EntityType::parse(HANDOFF_ENTITY_TYPE)),
        sort_by: EntitySort::UpdatedAt,
        limit: None,
        ..Default::default()
    })?;
    let mut records = Vec::new();
    for summary in summaries {
        if !summary.tags.iter().any(|tag| tag == HANDOFF_ENTITY_TAG) {
            continue;
        }
        let entity = wiki.entity_get_by_id(summary.id)?;
        let record = parse_record(&entity.name, entity.source_page.as_deref())?;
        let routed_actor = if sent {
            &record.from_actor
        } else {
            &record.to_actor
        };
        if routed_actor != actor {
            continue;
        }
        if source_id.is_some_and(|scope| record.source_id.as_deref() != Some(scope)) {
            continue;
        }
        if !include_completed && record.status == HandoffStatus::Completed {
            continue;
        }
        records.push(record);
    }
    records.sort_by_key(|record| std::cmp::Reverse(record.created_at));
    records.truncate(limit);
    Ok(records)
}

pub fn status(wiki: &AideMemo, handoff_id: &str, actor: &str) -> Result<HandoffAssignment> {
    validate_actor("actor_id", actor)?;
    let record = get(wiki, handoff_id)?;
    if record.from_actor != actor && record.to_actor != actor {
        return Err(AideMemoError::InvalidInput(format!(
            "handoff {} is not routed through actor {}",
            record.handoff_id, actor
        )));
    }
    Ok(record)
}

pub fn return_result(
    wiki: &AideMemo,
    handoff_id: &str,
    actor: &str,
    result_fact_id: &str,
    outcome: &str,
) -> Result<HandoffAssignment> {
    validate_actor("actor_id", actor)?;
    let outcome = outcome.trim().to_ascii_lowercase();
    if !matches!(outcome.as_str(), "succeeded" | "failed") {
        return Err(AideMemoError::InvalidInput(
            "outcome must be succeeded or failed".into(),
        ));
    }
    let fact_id = result_fact_id
        .trim()
        .parse::<aidememo_core::ulid::Ulid>()
        .map(aidememo_core::FactId)
        .map_err(|_| AideMemoError::InvalidInput("result_fact_id must be a valid ULID".into()))?;
    wiki.fact_get(&fact_id)?;

    let mut record = get(wiki, handoff_id)?;
    if record.to_actor != actor {
        return Err(AideMemoError::InvalidInput(format!(
            "handoff {} is assigned to {}, not {}",
            record.handoff_id, record.to_actor, actor
        )));
    }
    if record.status == HandoffStatus::Pending {
        return Err(AideMemoError::InvalidInput(format!(
            "accept handoff {} before returning a result",
            record.handoff_id
        )));
    }
    if record.status == HandoffStatus::Completed {
        if record.result_fact_id.as_deref() == Some(result_fact_id.trim())
            && record.outcome.as_deref() == Some(outcome.as_str())
        {
            return Ok(record);
        }
        return Err(AideMemoError::InvalidInput(format!(
            "handoff {} is already completed with a different result",
            record.handoff_id
        )));
    }

    let now = now_ms();
    record.result_fact_id = Some(result_fact_id.trim().to_string());
    record.outcome = Some(outcome.clone());
    record.returned_at = Some(now);
    if outcome == "succeeded" {
        record.status = HandoffStatus::Completed;
        record.completed_at = Some(now);
    }
    persist_record(wiki, &record)?;
    Ok(record)
}

pub fn get(wiki: &AideMemo, handoff_id: &str) -> Result<HandoffAssignment> {
    let entity = wiki.entity_get(handoff_id.trim())?;
    if entity.entity_type.to_string() != HANDOFF_ENTITY_TYPE {
        return Err(AideMemoError::InvalidInput(format!(
            "{} is a {} entity, not a handoff",
            entity.name, entity.entity_type
        )));
    }
    parse_record(&entity.name, entity.source_page.as_deref())
}

pub fn heartbeat(wiki: &AideMemo, handoff_id: &str, actor: &str) -> Result<HandoffAssignment> {
    validate_actor("actor_id", actor)?;
    let mut record = get(wiki, handoff_id)?;
    if record.to_actor != actor {
        return Err(AideMemoError::InvalidInput(format!(
            "handoff {} is assigned to {}, not {}",
            record.handoff_id, record.to_actor, actor
        )));
    }
    if record.status != HandoffStatus::Accepted {
        return Err(AideMemoError::InvalidInput(format!(
            "handoff {} must be accepted before sending a heartbeat",
            record.handoff_id
        )));
    }
    record.last_heartbeat_at = Some(now_ms());
    record.heartbeat_count = record.heartbeat_count.saturating_add(1);
    persist_record(wiki, &record)?;
    Ok(record)
}

pub fn board(
    wiki: &AideMemo,
    actor: Option<&str>,
    source_id: Option<&str>,
    stale_after_ms: u64,
    include_completed: bool,
    limit: usize,
) -> Result<HandoffBoard> {
    if stale_after_ms == 0 {
        return Err(AideMemoError::InvalidInput(
            "stale_after must be greater than zero".into(),
        ));
    }
    if let Some(actor) = actor {
        validate_actor("actor_id", actor)?;
    }
    let generated_at = now_ms();
    let summaries = wiki.entity_list(ListOpts {
        entity_type: Some(EntityType::parse(HANDOFF_ENTITY_TYPE)),
        sort_by: EntitySort::UpdatedAt,
        limit: None,
        ..Default::default()
    })?;
    let mut assignments = Vec::new();
    for summary in summaries {
        if !summary.tags.iter().any(|tag| tag == HANDOFF_ENTITY_TAG) {
            continue;
        }
        let entity = wiki.entity_get_by_id(summary.id)?;
        let record = parse_record(&entity.name, entity.source_page.as_deref())?;
        if actor.is_some_and(|value| record.from_actor != value && record.to_actor != value) {
            continue;
        }
        if source_id.is_some_and(|scope| record.source_id.as_deref() != Some(scope)) {
            continue;
        }
        if !include_completed && record.status == HandoffStatus::Completed {
            continue;
        }
        assignments.push(board_item(record, generated_at, stale_after_ms));
    }
    assignments.sort_by_key(|item| std::cmp::Reverse(item.last_activity_at));
    assignments.truncate(limit);
    let mut lanes = HandoffBoardCounts::default();
    for item in &assignments {
        match item.lane {
            "ready" => lanes.ready += 1,
            "in_progress" => lanes.in_progress += 1,
            "attention" => lanes.attention += 1,
            "returned" => lanes.returned += 1,
            _ => {}
        }
    }
    Ok(HandoffBoard {
        generated_at,
        stale_after_ms,
        lifecycle_owner: "upstream_orchestrator",
        lanes,
        assignments,
    })
}

fn board_item(
    record: HandoffAssignment,
    generated_at: u64,
    stale_after_ms: u64,
) -> HandoffBoardItem {
    let last_activity_at = record
        .returned_at
        .or(record.last_heartbeat_at)
        .or(record.accepted_at)
        .unwrap_or(record.created_at);
    let age_ms = generated_at.saturating_sub(last_activity_at);
    let stale = record.status != HandoffStatus::Completed && age_ms >= stale_after_ms;
    let (lane, next_action) = match record.status {
        HandoffStatus::Pending if stale => ("attention", "inspect_or_run"),
        HandoffStatus::Pending => ("ready", "accept_or_run"),
        HandoffStatus::Accepted if record.outcome.as_deref() == Some("failed") => {
            ("attention", "orchestrator_retry_or_close")
        }
        HandoffStatus::Accepted if stale => ("attention", "check_worker_liveness"),
        HandoffStatus::Accepted => ("in_progress", "wait_for_result"),
        HandoffStatus::Completed => ("returned", "validate_result"),
    };
    let lifecycle_owner = if record.upstream_system.as_deref() == Some("hermes_kanban") {
        "hermes_kanban"
    } else {
        "caller"
    };
    HandoffBoardItem {
        assignment: record,
        lane,
        stale,
        age_ms,
        last_activity_at,
        next_action,
        lifecycle_owner,
    }
}

pub fn transition(
    wiki: &AideMemo,
    handoff_id: &str,
    actor: &str,
    transition: HandoffTransition,
) -> Result<HandoffAssignment> {
    validate_actor("actor_id", actor)?;
    let mut record = get(wiki, handoff_id)?;
    if record.to_actor != actor {
        return Err(AideMemoError::InvalidInput(format!(
            "handoff {} is assigned to {}, not {}",
            record.handoff_id, record.to_actor, actor
        )));
    }
    let now = now_ms();
    match (transition, record.status) {
        (HandoffTransition::Accept, HandoffStatus::Pending) => {
            record.status = HandoffStatus::Accepted;
            record.accepted_at = Some(now);
        }
        (HandoffTransition::Accept, HandoffStatus::Accepted) => return Ok(record),
        (HandoffTransition::Accept, HandoffStatus::Completed) => {
            return Err(AideMemoError::InvalidInput(format!(
                "handoff {} is already completed",
                record.handoff_id
            )));
        }
        (HandoffTransition::Complete, HandoffStatus::Accepted) => {
            record.status = HandoffStatus::Completed;
            record.completed_at = Some(now);
        }
        (HandoffTransition::Complete, HandoffStatus::Completed) => return Ok(record),
        (HandoffTransition::Complete, HandoffStatus::Pending) => {
            return Err(AideMemoError::InvalidInput(format!(
                "accept handoff {} before completing it",
                record.handoff_id
            )));
        }
    }

    persist_record(wiki, &record)?;
    Ok(record)
}

fn persist_record(wiki: &AideMemo, record: &HandoffAssignment) -> Result<()> {
    wiki.entity_update(
        &record.handoff_id,
        EntityUpdate {
            tags: Some(record_tags(record)),
            source_page: Some(serialize_record(record)?),
            ..Default::default()
        },
    )?;
    Ok(())
}

fn record_tags(record: &HandoffAssignment) -> Vec<String> {
    let mut tags = vec![
        HANDOFF_ENTITY_TAG.to_string(),
        format!("status:{}", record.status.as_str()),
        format!("to_actor:{}", record.to_actor),
        format!("from_actor:{}", record.from_actor),
    ];
    if let Some(source_id) = record.source_id.as_deref() {
        tags.push(format!("source_id:{source_id}"));
    }
    if let Some(outcome) = record.outcome.as_deref() {
        tags.push(format!("outcome:{outcome}"));
    }
    tags
}

fn serialize_record(record: &HandoffAssignment) -> Result<String> {
    serde_json::to_string(record).map_err(|source| AideMemoError::Serialize {
        context: format!("handoff {}", record.handoff_id),
        source,
    })
}

fn parse_record(name: &str, source_page: Option<&str>) -> Result<HandoffAssignment> {
    let raw = source_page.ok_or_else(|| {
        AideMemoError::InvalidInput(format!("handoff {name} has no assignment metadata"))
    })?;
    serde_json::from_str(raw).map_err(|source| AideMemoError::Deserialize {
        context: format!("handoff {name}"),
        source,
    })
}

fn validate_actor(label: &str, actor: &str) -> Result<()> {
    let actor = actor.trim();
    if actor.is_empty() {
        return Err(AideMemoError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if actor.chars().any(char::is_control) {
        return Err(AideMemoError::InvalidInput(format!(
            "{label} must not contain control characters"
        )));
    }
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidememo_core::Config;

    fn wiki() -> (AideMemo, tempfile::TempDir) {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("handoff.sqlite");
        let wiki = AideMemo::open(&path, Config::default()).expect("open");
        wiki.entity_add(EntityInput {
            name: "session-test".to_string(),
            entity_type: Some(EntityType::parse("session")),
            source_page: Some("test workflow".to_string()),
            ..Default::default()
        })
        .expect("session");
        (wiki, temp)
    }

    #[test]
    fn assignment_is_a_session_pointer_not_a_payload_queue() {
        let (wiki, _temp) = wiki();
        let sent = dispatch(
            &wiki,
            NewHandoffAssignment {
                session_id: "session-test".to_string(),
                source_id: Some("project-a".to_string()),
                from_actor: "codex-one".to_string(),
                to_actor: "codex-two".to_string(),
                from_agent: Some("codex".to_string()),
                from_profile: Some("coding".to_string()),
                to_agent: Some("codex".to_string()),
                to_profile: Some("reviewer".to_string()),
                focus: Some("review patch".to_string()),
                done_when: Some("tests pass".to_string()),
                upstream_system: None,
                upstream_task_id: None,
                upstream_board_id: None,
            },
        )
        .expect("dispatch");
        assert_eq!(sent.status, HandoffStatus::Pending);
        assert_eq!(
            inbox(&wiki, "codex-two", Some("project-a"), false, 10)
                .expect("inbox")
                .len(),
            1
        );
        assert!(
            inbox(&wiki, "claude-main", Some("project-a"), false, 10)
                .expect("other inbox")
                .is_empty()
        );

        let accepted = transition(
            &wiki,
            &sent.handoff_id,
            "codex-two",
            HandoffTransition::Accept,
        )
        .expect("accept");
        assert_eq!(accepted.status, HandoffStatus::Accepted);
        let result_fact = wiki
            .fact_add(aidememo_core::FactInput {
                content: "Focused tests pass".to_string(),
                fact_type: Some(aidememo_core::FactType::Note),
                ..Default::default()
            })
            .expect("result fact");
        let completed = return_result(
            &wiki,
            &sent.handoff_id,
            "codex-two",
            &result_fact.0.to_string(),
            "succeeded",
        )
        .expect("return result");
        assert_eq!(completed.status, HandoffStatus::Completed);
        assert_eq!(completed.outcome.as_deref(), Some("succeeded"));
        assert_eq!(
            completed.result_fact_id.as_deref(),
            Some(result_fact.0.to_string().as_str())
        );
        assert_eq!(
            outbox(&wiki, "codex-one", Some("project-a"), true, 10)
                .expect("outbox")
                .len(),
            1
        );
        assert!(
            inbox(&wiki, "codex-two", Some("project-a"), false, 10)
                .expect("empty inbox")
                .is_empty()
        );
    }

    #[test]
    fn failed_result_stays_accepted_for_orchestrator_policy() {
        let (wiki, _temp) = wiki();
        let sent = dispatch(
            &wiki,
            NewHandoffAssignment {
                session_id: "session-test".to_string(),
                source_id: None,
                from_actor: "hermes-main".to_string(),
                to_actor: "codex-two".to_string(),
                from_agent: Some("hermes".to_string()),
                from_profile: None,
                to_agent: Some("codex".to_string()),
                to_profile: None,
                focus: Some("run tests".to_string()),
                done_when: Some("tests pass".to_string()),
                upstream_system: Some("hermes_kanban".to_string()),
                upstream_task_id: Some("task-42".to_string()),
                upstream_board_id: Some("board-a".to_string()),
            },
        )
        .expect("dispatch");
        transition(
            &wiki,
            &sent.handoff_id,
            "codex-two",
            HandoffTransition::Accept,
        )
        .expect("accept");
        let error_fact = wiki
            .fact_add(aidememo_core::FactInput {
                content: "Focused test failed".to_string(),
                fact_type: Some(aidememo_core::FactType::Error),
                ..Default::default()
            })
            .expect("error fact");
        let returned = return_result(
            &wiki,
            &sent.handoff_id,
            "codex-two",
            &error_fact.0.to_string(),
            "failed",
        )
        .expect("failed return");
        assert_eq!(returned.status, HandoffStatus::Accepted);
        assert_eq!(returned.outcome.as_deref(), Some("failed"));
        assert!(returned.completed_at.is_none());
    }

    #[test]
    fn heartbeat_and_board_add_liveness_without_owning_task_lifecycle() {
        let (wiki, _temp) = wiki();
        let sent = dispatch(
            &wiki,
            NewHandoffAssignment {
                session_id: "session-test".to_string(),
                source_id: None,
                from_actor: "hermes-main".to_string(),
                to_actor: "codex-two".to_string(),
                from_agent: Some("hermes".to_string()),
                from_profile: None,
                to_agent: Some("codex".to_string()),
                to_profile: None,
                focus: None,
                done_when: None,
                upstream_system: Some("hermes_kanban".to_string()),
                upstream_task_id: Some("task-42".to_string()),
                upstream_board_id: None,
            },
        )
        .expect("dispatch");
        assert!(heartbeat(&wiki, &sent.handoff_id, "codex-two").is_err());
        transition(
            &wiki,
            &sent.handoff_id,
            "codex-two",
            HandoffTransition::Accept,
        )
        .expect("accept");
        let pulsed = heartbeat(&wiki, &sent.handoff_id, "codex-two").expect("heartbeat");
        assert_eq!(pulsed.heartbeat_count, 1);
        assert!(pulsed.last_heartbeat_at.is_some());

        let report = board(&wiki, Some("codex-two"), None, 3_600_000, true, 20).expect("board");
        assert_eq!(report.lifecycle_owner, "upstream_orchestrator");
        assert_eq!(report.lanes.in_progress, 1);
        assert_eq!(report.assignments[0].lifecycle_owner, "hermes_kanban");
        assert_eq!(report.assignments[0].next_action, "wait_for_result");
    }
}
