//! Typed session, fact, and handoff records for the remote SSOT boundary.

use crate::{ActorId, ClaimId, DomainError, FactId, HandoffId, SessionId, SourceId};
use serde::{Deserialize, Serialize};

const MAX_TOPIC_BYTES: usize = 512;
const MAX_FACT_BYTES: usize = 65_536;
const MAX_HANDOFF_TEXT_BYTES: usize = 4_096;

/// Minimal canonical session record used for remote continuity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Stable session identity.
    pub session_id: SessionId,
    /// Optional application namespace inside the project.
    pub source_id: Option<SourceId>,
    /// Human-readable work topic.
    pub topic: String,
    /// Authenticated actor that created the session.
    pub created_by: ActorId,
}

impl SessionRecord {
    /// Build a validated session record.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidCommand`] for an invalid topic.
    pub fn new(
        session_id: SessionId,
        source_id: Option<SourceId>,
        topic: String,
        created_by: ActorId,
    ) -> Result<Self, DomainError> {
        validate_text("session topic", &topic, MAX_TOPIC_BYTES)?;
        Ok(Self {
            session_id,
            source_id,
            topic,
            created_by,
        })
    }
}

/// Minimal canonical fact used as handoff result evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FactRecord {
    /// Stable fact identity.
    pub fact_id: FactId,
    /// Session continuity attachment.
    pub session_id: SessionId,
    /// Application namespace, which must match a returned handoff.
    pub source_id: Option<SourceId>,
    /// Authenticated writer provenance.
    pub actor_id: ActorId,
    /// Result or memory content.
    pub content: String,
}

impl FactRecord {
    /// Build a validated fact record with server-owned actor provenance.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidCommand`] for invalid content.
    pub fn new(
        fact_id: FactId,
        session_id: SessionId,
        source_id: Option<SourceId>,
        actor_id: ActorId,
        content: String,
    ) -> Result<Self, DomainError> {
        validate_text("fact content", &content, MAX_FACT_BYTES)?;
        Ok(Self {
            fact_id,
            session_id,
            source_id,
            actor_id,
            content,
        })
    }
}

/// Canonical handoff lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    /// Routed but not yet claimed.
    Pending,
    /// Exclusively claimed by the receiving actor.
    Accepted,
    /// Returned successfully with validated fact evidence.
    Completed,
}

/// Receiver-reported handoff outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffOutcome {
    /// Result evidence satisfies the assignment.
    Succeeded,
    /// Attempt returned evidence but remains eligible for a new claim.
    Failed,
}

/// Typed handoff pointer and state machine.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HandoffRecord {
    /// Stable handoff identity.
    pub handoff_id: HandoffId,
    /// Session whose continuity transfers.
    pub session_id: SessionId,
    /// Application namespace inherited from the session.
    pub source_id: Option<SourceId>,
    /// Authenticated sender.
    pub from_actor: ActorId,
    /// Active project member expected to receive the work.
    pub to_actor: ActorId,
    /// Optional bounded assignment focus.
    pub focus: Option<String>,
    /// Optional bounded completion condition.
    pub done_when: Option<String>,
    /// Current lifecycle state.
    pub status: HandoffStatus,
    /// Active exclusive worker claim.
    pub claim_id: Option<ClaimId>,
    /// Number of accepted claims, including retries after failure.
    pub attempt_count: u64,
    /// Validated result fact, if returned.
    pub result_fact_id: Option<FactId>,
    /// Receiver outcome, if returned.
    pub outcome: Option<HandoffOutcome>,
}

impl HandoffRecord {
    /// Create a pending handoff routed between distinct actors.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidCommand`] for self-routing or invalid
    /// focus/completion text.
    pub fn new(
        handoff_id: HandoffId,
        session: &SessionRecord,
        from_actor: ActorId,
        to_actor: ActorId,
        focus: Option<String>,
        done_when: Option<String>,
    ) -> Result<Self, DomainError> {
        if from_actor == to_actor {
            return Err(DomainError::InvalidCommand(
                "handoff sender and receiver must differ".to_owned(),
            ));
        }
        validate_optional_text("handoff focus", focus.as_deref(), MAX_HANDOFF_TEXT_BYTES)?;
        validate_optional_text(
            "handoff done_when",
            done_when.as_deref(),
            MAX_HANDOFF_TEXT_BYTES,
        )?;
        Ok(Self {
            handoff_id,
            session_id: session.session_id.clone(),
            source_id: session.source_id.clone(),
            from_actor,
            to_actor,
            focus,
            done_when,
            status: HandoffStatus::Pending,
            claim_id: None,
            attempt_count: 0,
            result_fact_id: None,
            outcome: None,
        })
    }

    /// Accept or retry an assignment with an exclusive claim.
    ///
    /// Repeating the same accepted claim is idempotent. A failed attempt may be
    /// retried only with a different claim.
    ///
    /// # Errors
    ///
    /// Returns a handoff actor or state conflict when the transition is not
    /// allowed.
    pub fn accept(&mut self, actor: &ActorId, claim_id: ClaimId) -> Result<(), DomainError> {
        self.require_receiver(actor)?;
        match self.status {
            HandoffStatus::Pending => self.begin_claim(claim_id),
            HandoffStatus::Accepted if self.claim_id.as_ref() == Some(&claim_id) => Ok(()),
            HandoffStatus::Accepted if self.outcome == Some(HandoffOutcome::Failed) => {
                self.begin_claim(claim_id)
            }
            HandoffStatus::Accepted => Err(DomainError::HandoffConflict(
                "handoff is already claimed by another worker".to_owned(),
            )),
            HandoffStatus::Completed => Err(DomainError::HandoffConflict(
                "completed handoff cannot be accepted".to_owned(),
            )),
        }
    }

    /// Return a result fact under the active receiving claim.
    ///
    /// The fact must match the exact session, source, and receiving actor.
    /// Successful return completes the handoff; failed return leaves it
    /// accepted so a new claim can retry it.
    ///
    /// # Errors
    ///
    /// Returns a handoff conflict for route, claim, state, or evidence mismatch.
    pub fn return_result(
        &mut self,
        actor: &ActorId,
        claim_id: &ClaimId,
        fact: &FactRecord,
        outcome: HandoffOutcome,
    ) -> Result<(), DomainError> {
        self.require_receiver(actor)?;
        self.validate_result_fact(fact)?;
        if self.status == HandoffStatus::Completed {
            if self.claim_id.as_ref() == Some(claim_id)
                && self.result_fact_id.as_ref() == Some(&fact.fact_id)
                && self.outcome == Some(outcome)
            {
                return Ok(());
            }
            return Err(DomainError::HandoffConflict(
                "handoff is already completed with different evidence".to_owned(),
            ));
        }
        if self.status != HandoffStatus::Accepted {
            return Err(DomainError::HandoffConflict(
                "handoff must be accepted before returning a result".to_owned(),
            ));
        }
        if self.claim_id.as_ref() != Some(claim_id) {
            return Err(DomainError::HandoffConflict(
                "result claim does not match the active handoff claim".to_owned(),
            ));
        }
        self.result_fact_id = Some(fact.fact_id.clone());
        self.outcome = Some(outcome);
        if outcome == HandoffOutcome::Succeeded {
            self.status = HandoffStatus::Completed;
        }
        Ok(())
    }

    /// Whether an actor is the sender or receiver of this handoff.
    #[must_use]
    pub fn is_visible_to(&self, actor: &ActorId) -> bool {
        &self.from_actor == actor || &self.to_actor == actor
    }

    fn require_receiver(&self, actor: &ActorId) -> Result<(), DomainError> {
        if &self.to_actor == actor {
            Ok(())
        } else {
            Err(DomainError::HandoffActorMismatch)
        }
    }

    fn begin_claim(&mut self, claim_id: ClaimId) -> Result<(), DomainError> {
        self.attempt_count = self.attempt_count.checked_add(1).ok_or_else(|| {
            DomainError::HandoffConflict("handoff attempt counter overflow".to_owned())
        })?;
        self.status = HandoffStatus::Accepted;
        self.claim_id = Some(claim_id);
        self.result_fact_id = None;
        self.outcome = None;
        Ok(())
    }

    fn validate_result_fact(&self, fact: &FactRecord) -> Result<(), DomainError> {
        if fact.session_id != self.session_id {
            return Err(DomainError::HandoffConflict(
                "result fact belongs to a different session".to_owned(),
            ));
        }
        if fact.source_id != self.source_id {
            return Err(DomainError::HandoffConflict(
                "result fact belongs to a different source".to_owned(),
            ));
        }
        if fact.actor_id != self.to_actor {
            return Err(DomainError::HandoffConflict(
                "result fact was not written by the receiving actor".to_owned(),
            ));
        }
        Ok(())
    }
}

fn validate_text(label: &str, value: &str, max_bytes: usize) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.len() > max_bytes {
        return Err(DomainError::InvalidCommand(format!(
            "{label} must contain 1 to {max_bytes} bytes"
        )));
    }
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(DomainError::InvalidCommand(format!(
            "{label} must not contain control characters"
        )));
    }
    Ok(())
}

fn validate_optional_text(
    label: &str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<(), DomainError> {
    if let Some(value) = value {
        validate_text(label, value, max_bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Result<SessionRecord, DomainError> {
        SessionRecord::new(
            SessionId::try_from("session_test")?,
            Some(SourceId::try_from("project:aidememo")?),
            "Remote handoff".to_owned(),
            ActorId::try_from("codex-p1")?,
        )
    }

    fn handoff() -> Result<HandoffRecord, DomainError> {
        HandoffRecord::new(
            HandoffId::try_from("handoff_test")?,
            &session()?,
            ActorId::try_from("codex-p1")?,
            ActorId::try_from("codex-p2")?,
            Some("Review server contract".to_owned()),
            None,
        )
    }

    fn result_fact(actor: &str) -> Result<FactRecord, DomainError> {
        FactRecord::new(
            FactId::try_from("fact_result")?,
            SessionId::try_from("session_test")?,
            Some(SourceId::try_from("project:aidememo")?),
            ActorId::try_from(actor)?,
            "Completed review".to_owned(),
        )
    }

    #[test]
    fn claim_is_exclusive_and_exact_retry_is_idempotent() -> Result<(), DomainError> {
        let mut handoff = handoff()?;
        let receiver = ActorId::try_from("codex-p2")?;
        let claim = ClaimId::try_from("claim_one")?;
        handoff.accept(&receiver, claim.clone())?;
        handoff.accept(&receiver, claim)?;
        assert_eq!(handoff.attempt_count, 1);
        assert!(matches!(
            handoff.accept(&receiver, ClaimId::try_from("claim_two")?),
            Err(DomainError::HandoffConflict(_))
        ));
        Ok(())
    }

    #[test]
    fn result_fact_must_match_session_source_and_receiver() -> Result<(), DomainError> {
        let mut handoff = handoff()?;
        let receiver = ActorId::try_from("codex-p2")?;
        let claim = ClaimId::try_from("claim_one")?;
        handoff.accept(&receiver, claim.clone())?;
        assert!(matches!(
            handoff.return_result(
                &receiver,
                &claim,
                &result_fact("codex-p1")?,
                HandoffOutcome::Succeeded
            ),
            Err(DomainError::HandoffConflict(_))
        ));
        let mut wrong_source = result_fact("codex-p2")?;
        wrong_source.source_id = Some(SourceId::try_from("project:other")?);
        assert!(matches!(
            handoff.return_result(&receiver, &claim, &wrong_source, HandoffOutcome::Succeeded),
            Err(DomainError::HandoffConflict(_))
        ));
        Ok(())
    }

    #[test]
    fn failed_result_allows_new_claim_and_successful_completion() -> Result<(), DomainError> {
        let mut handoff = handoff()?;
        let receiver = ActorId::try_from("codex-p2")?;
        let first_claim = ClaimId::try_from("claim_one")?;
        let second_claim = ClaimId::try_from("claim_two")?;
        let fact = result_fact("codex-p2")?;
        handoff.accept(&receiver, first_claim.clone())?;
        handoff.return_result(&receiver, &first_claim, &fact, HandoffOutcome::Failed)?;
        assert_eq!(handoff.status, HandoffStatus::Accepted);
        handoff.accept(&receiver, second_claim.clone())?;
        handoff.return_result(&receiver, &second_claim, &fact, HandoffOutcome::Succeeded)?;
        assert_eq!(handoff.status, HandoffStatus::Completed);
        handoff.return_result(&receiver, &second_claim, &fact, HandoffOutcome::Succeeded)?;
        Ok(())
    }

    #[test]
    fn only_receiver_can_transition() -> Result<(), DomainError> {
        let mut handoff = handoff()?;
        assert_eq!(
            handoff.accept(
                &ActorId::try_from("codex-p1")?,
                ClaimId::try_from("claim_one")?
            ),
            Err(DomainError::HandoffActorMismatch)
        );
        Ok(())
    }
}
