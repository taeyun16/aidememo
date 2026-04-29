//! `wg adapt` — train, inspect, and evaluate the search adapter.
//!
//! TODO(phase4): training writes `~/.wg/adapter.json`, but the CLI search
//! path (`wg search` / `wg query`) does not consult it. End-to-end loop is
//! incomplete until `wg-core/src/search.rs` loads the adapter and applies
//! the per-fact bias term during ranking.

use bpaf::*;
use std::path::Path;

use crate::cmd::Command;
use wg_core::{AdaptEvalReport, AdaptResult, AdaptStatus, Config, WgError, WikiGraph};

#[derive(Debug, Clone)]
pub enum AdaptSub {
    Train,
    Status,
    Eval,
}

pub fn adapt_command() -> impl Parser<Command> {
    let train = pure(AdaptSub::Train)
        .to_options()
        .command("train")
        .help("Train the domain adapter from recorded feedback");

    let status = pure(AdaptSub::Status)
        .to_options()
        .command("status")
        .help("Show domain adapter status");

    let eval = pure(AdaptSub::Eval)
        .to_options()
        .command("eval")
        .help("Evaluate the domain adapter on recorded feedback");

    construct!([train, status, eval])
        .map(Command::Adapt)
        .to_options()
        .command("adapt")
        .help("Search adapter commands")
}

pub fn run_adapt(store_path: &Path, config: Config, sub: AdaptSub) -> Result<String, WgError> {
    let wiki = WikiGraph::open(store_path, config)?;

    match sub {
        AdaptSub::Train => format_train(wiki.adapt_train()?),
        AdaptSub::Status => format_status(wiki.adapt_status()?),
        AdaptSub::Eval => format_eval(wiki.adapt_eval()?),
    }
}

fn format_train(result: AdaptResult) -> Result<String, WgError> {
    Ok(format!(
        "Domain adapter trained\n  feedback_used: {}\n  helpful_count: {}\n  generation: {}",
        result.feedback_used, result.helpful_count, result.generation
    ))
}

fn format_status(status: AdaptStatus) -> Result<String, WgError> {
    Ok(format!(
        "Domain adapter status\n  has_adapter: {}\n  feedback_count: {}\n  generation: {}\n  ready: {}",
        status.has_adapter, status.feedback_count, status.generation, status.ready
    ))
}

fn format_eval(report: AdaptEvalReport) -> Result<String, WgError> {
    Ok(format!(
        "Domain adapter evaluation\n  total_feedback: {}\n  helpful_count: {}\n  skipped_count: {}\n  precision_at_10: {:.3}\n  recall_boost: {:.3}",
        report.total_feedback,
        report.helpful_count,
        report.skipped_count,
        report.precision_at_10,
        report.recall_boost
    ))
}
