//! `wg edit fact <ID>` — incremental fact content edits.
//!
//! ```text
//! Operations:
//!   --append <text>             append `\n<text>` to fact.content
//!   --prepend <text>            prepend `<text>\n`
//!   --find <s> --replace <s>    in-place find/replace (errors if not found)
//!   --content <s>               full replace
//! ```
//!
//! Exactly one operation must be selected.

use bpaf::*;
use std::path::Path;
use wg_core::{Config, FactId, FactUpdate, WgError, WikiGraph};

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub enum EditSub {
    Fact {
        id: String,
        append: Option<String>,
        prepend: Option<String>,
        find: Option<String>,
        replace: Option<String>,
        content: Option<String>,
    },
}

pub fn edit_command() -> impl Parser<Command> {
    let append = long("append")
        .help("Append a line to the fact content")
        .argument::<String>("TEXT")
        .optional();
    let prepend = long("prepend")
        .help("Prepend a line to the fact content")
        .argument::<String>("TEXT")
        .optional();
    let find = long("find")
        .help("Substring to find (use with --replace)")
        .argument::<String>("FIND")
        .optional();
    let replace = long("replace")
        .help("Replacement text (use with --find)")
        .argument::<String>("REPL")
        .optional();
    let content = long("content")
        .help("Replace fact content entirely")
        .argument::<String>("TEXT")
        .optional();
    let id = positional::<String>("FACT_ID");

    let fact = construct!(EditSub::Fact {
        append,
        prepend,
        find,
        replace,
        content,
        id,
    })
    .to_options()
    .command("fact")
    .help("Edit a fact's content");

    construct!([fact])
        .map(Command::Edit)
        .to_options()
        .command("edit")
        .help("Incremental edits to facts")
}

pub fn run_edit(
    store_path: &Path,
    config: Config,
    sub: EditSub,
    global_json: bool,
) -> Result<String, WgError> {
    match sub {
        EditSub::Fact {
            id,
            append,
            prepend,
            find,
            replace,
            content,
        } => {
            let fact_id = FactId(
                wg_core::ulid::Ulid::from_string(&id)
                    .map_err(|_| WgError::InvalidInput(format!("Invalid fact ID: {id}")))?,
            );

            // Daemon discovery — wg_fact_edit MCP tool takes the same
            // append/prepend/find+replace/content shape, so the args
            // forward 1:1.
            if let Some(via) = crate::cmd::daemon::registered_endpoint(store_path) {
                tracing::debug!(via = %via, "auto-discovered daemon for fact edit");
                return run_fact_edit_via_daemon(
                    &via,
                    &id,
                    append.as_deref(),
                    prepend.as_deref(),
                    find.as_deref(),
                    replace.as_deref(),
                    content.as_deref(),
                );
            }

            let wiki = WikiGraph::open(store_path, config)?;
            let current = wiki.fact_get(&fact_id)?;
            let original = current.content.clone();

            let mut new_content = current.content;
            let mut ops_applied = 0;

            if let Some(extra) = append {
                let sep = if new_content.is_empty() || new_content.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                new_content.push_str(sep);
                new_content.push_str(&extra);
                ops_applied += 1;
            }
            if let Some(extra) = prepend {
                let sep = if extra.ends_with('\n') { "" } else { "\n" };
                new_content = format!("{extra}{sep}{new_content}");
                ops_applied += 1;
            }
            match (find, replace) {
                (Some(f), Some(r)) => {
                    if !new_content.contains(&f) {
                        return Err(WgError::InvalidInput(format!(
                            "--find substring not present in fact {id}: {f:?}"
                        )));
                    }
                    new_content = new_content.replace(&f, &r);
                    ops_applied += 1;
                }
                (Some(_), None) | (None, Some(_)) => {
                    return Err(WgError::InvalidInput(
                        "--find and --replace must be used together".into(),
                    ));
                }
                (None, None) => {}
            }
            if let Some(full) = content {
                new_content = full;
                ops_applied += 1;
            }

            if ops_applied == 0 {
                return Err(WgError::InvalidInput(
                    "no edit operation given (use --append / --prepend / --find+--replace / --content)"
                        .into(),
                ));
            }
            if ops_applied > 1 {
                return Err(WgError::InvalidInput(
                    "specify exactly one edit operation".into(),
                ));
            }

            wiki.fact_update(
                &fact_id,
                FactUpdate {
                    content: Some(new_content.clone()),
                    fact_type: None,
                    tags: None,
                    source: None,
                    source_id: None,
                    observed_at: None,
                    superseded_at: None,
                    superseded_by: None,
                    pinned: None,
                },
            )?;

            if global_json {
                let payload = serde_json::json!({
                    "fact_id": id,
                    "before": original,
                    "after": new_content,
                });
                return serde_json::to_string_pretty(&payload).map_err(|e| WgError::Serialize {
                    context: "edit".to_string(),
                    source: e,
                });
            }
            Ok(format!(
                "Updated fact {id}\n  before: {original}\n  after:  {new_content}"
            ))
        }
    }
}

/// `wg fact edit` daemon path. wg_fact_edit MCP tool takes the same
/// append/prepend/find+replace/content shape and returns a text
/// summary; we forward verbatim. The tool itself rejects multi-op
/// combinations and missing-substring find — same validation the
/// in-process path does.
#[allow(clippy::too_many_arguments)]
fn run_fact_edit_via_daemon(
    base_url: &str,
    id: &str,
    append: Option<&str>,
    prepend: Option<&str>,
    find: Option<&str>,
    replace: Option<&str>,
    content: Option<&str>,
) -> Result<String, WgError> {
    let url = format!("{}/mcp", base_url.trim_end_matches('/'));
    let mut args = serde_json::json!({"id": id});
    if let Some(s) = append {
        args["append"] = serde_json::json!(s);
    }
    if let Some(s) = prepend {
        args["prepend"] = serde_json::json!(s);
    }
    if let Some(s) = find {
        args["find"] = serde_json::json!(s);
    }
    if let Some(s) = replace {
        args["replace"] = serde_json::json!(s);
    }
    if let Some(s) = content {
        args["content"] = serde_json::json!(s);
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": {"name": "wg_fact_edit", "arguments": args}
    });
    crate::daemon_tool_call(&url, body, "wg_fact_edit")
}
