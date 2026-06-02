//! `aidememo feedback` — record feedback for a search result.

use bpaf::*;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cmd::Command;
use aidememo_core::{AideMemo, AideMemoError, Config, SearchFeedback};

#[derive(Debug, Clone)]
pub struct FeedbackSub {
    pub helpful: bool,
    pub not_helpful: bool,
    pub session_id: String,
    pub fact_id: String,
}

pub fn feedback_command() -> impl Parser<Command> {
    let helpful = long("helpful").help("Mark feedback as helpful").switch();
    let not_helpful = long("not-helpful")
        .help("Mark feedback as not helpful")
        .switch();
    let session_id = positional::<String>("SESSION_ID").help("Search session ID");
    let fact_id = positional::<String>("FACT_ID").help("Fact ID from the search result");

    construct!(FeedbackSub {
        helpful,
        not_helpful,
        session_id,
        fact_id,
    })
    .map(Command::Feedback)
    .to_options()
    .command("feedback")
    .help("Record search result feedback")
}

pub fn run_feedback(
    store_path: &Path,
    config: Config,
    sub: FeedbackSub,
) -> Result<String, AideMemoError> {
    if sub.helpful == sub.not_helpful {
        return Err(AideMemoError::InvalidInput(
            "Specify exactly one of --helpful or --not-helpful".to_string(),
        ));
    }

    let fact_id = aidememo_core::FactId(
        aidememo_core::ulid::Ulid::from_string(&sub.fact_id).map_err(|_| {
            AideMemoError::InvalidInput(format!("Invalid fact ID: {}", sub.fact_id))
        })?,
    );

    let feedback = SearchFeedback {
        session_id: sub.session_id.clone(),
        fact_id,
        helpful: sub.helpful,
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| AideMemoError::Internal(format!("system clock error: {}", e)))?
            .as_millis() as u64,
    };

    let wiki = AideMemo::open(store_path, config)?;
    wiki.search_feedback_add(&feedback)?;

    Ok(format!(
        "Recorded {} feedback for session {} fact {}",
        if sub.helpful {
            "helpful"
        } else {
            "not helpful"
        },
        sub.session_id,
        sub.fact_id
    ))
}
