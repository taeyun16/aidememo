//! `wg skill check` — validate Claude Code skill files (SKILL.md format).
//!
//! Inspired by gbrain's `skillify check`. Reads a SKILL.md file (or every
//! `*.md` under a directory) and reports frontmatter / content issues.
//!
//! Validation rules:
//! - Frontmatter (YAML between `---` markers) must exist and parse.
//! - Required fields: `name`, `description`.
//! - Recommended fields: `when_to_use`, `allowed-tools` (warns if missing).
//! - `allowed-tools` items: each must be either `Bash(...)` / `Bash(...:...)`
//!   or a known MCP tool name (`wg_search`, `wg_query`, …). Unknown names
//!   warn so users catch typos.
//! - `name` must be lowercase, alphanumeric + `-`/`_`.
//! - Body (after frontmatter) must be ≥ 50 chars.

use bpaf::*;
use std::path::{Path, PathBuf};

use crate::cmd::Command;
use crate::cmd::mcp_tools::list_tools;
use wg_core::WgError;

#[derive(Debug, Clone)]
pub enum SkillSub {
    Check { path: PathBuf },
}

pub fn skill_command() -> impl Parser<Command> {
    let path = positional::<PathBuf>("PATH").help("SKILL.md file or directory of skills");

    let check = construct!(SkillSub::Check { path })
        .to_options()
        .command("check")
        .help("Validate a SKILL.md file or directory of skills (use global --json for JSON)");

    construct!([check])
        .map(Command::Skill)
        .to_options()
        .command("skill")
        .help("Skill management (Claude Code SKILL.md files)")
}

// ---------------------------------------------------------------------------
// Validation report types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Issue {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Skill,
    Doc,
}

#[derive(Debug, serde::Serialize)]
pub struct FileReport {
    pub path: String,
    pub kind: FileKind,
    pub ok: bool,
    pub issues: Vec<Issue>,
}

#[derive(Debug, serde::Serialize)]
pub struct CheckSummary {
    pub total_files: usize,
    pub skills_checked: usize,
    pub docs_skipped: usize,
    pub passing: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub files: Vec<FileReport>,
}

// ---------------------------------------------------------------------------
// Mini YAML parser (just enough for our 4 frontmatter fields)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct SkillFm {
    /// `kind: skill` (default) or `kind: doc` — docs are skipped from
    /// validation. Other values are treated like `doc` (non-skill, skipped).
    kind: Option<String>,
    name: Option<String>,
    description: Option<String>,
    when_to_use: Option<Vec<String>>,
    allowed_tools: Option<Vec<String>>,
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.trim_start_matches('\n');
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let after = &rest[end + 4..];
    Some((fm, after.trim_start_matches('\n')))
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches(|c| c == '"' || c == '\'').to_string()
}

fn parse_inline_array(s: &str) -> Vec<String> {
    let s = s.trim().trim_start_matches('[').trim_end_matches(']');
    s.split(',')
        .map(unquote)
        .filter(|x| !x.is_empty())
        .collect()
}

fn parse_skill_frontmatter(fm: &str) -> SkillFm {
    let mut out = SkillFm::default();
    let mut current_list_key: Option<String> = None;
    let mut current_list: Vec<String> = Vec::new();

    let commit_list = |fm: &mut SkillFm, key: &str, items: Vec<String>| match key {
        "when_to_use" => fm.when_to_use = Some(items),
        "allowed-tools" | "allowed_tools" => fm.allowed_tools = Some(items),
        _ => {}
    };

    for raw_line in fm.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }

        // Indented list item under previous key.
        if line.starts_with("  - ") || line.starts_with("- ") {
            let item = line.trim_start_matches(' ').trim_start_matches('-').trim();
            current_list.push(unquote(item));
            continue;
        }

        // Top-level `key: ...` line. Commit any in-flight list first.
        if let Some(idx) = line.find(':') {
            if let Some(prev_key) = current_list_key.take() {
                commit_list(&mut out, &prev_key, std::mem::take(&mut current_list));
            }
            let key = line[..idx].trim().to_string();
            let value = line[idx + 1..].trim();

            if value.is_empty() {
                // Block-style list follows on next lines.
                current_list_key = Some(key);
                current_list.clear();
                continue;
            }
            if value.starts_with('[') {
                let items = parse_inline_array(value);
                commit_list(&mut out, &key, items);
                continue;
            }

            // Scalar value.
            let v = unquote(value);
            match key.as_str() {
                "kind" => out.kind = Some(v),
                "name" => out.name = Some(v),
                "description" => out.description = Some(v),
                "allowed-tools" | "allowed_tools" => {
                    // Tools can also be a single line like
                    // `Bash(wg:*), Bash(./target/debug/wg:*)`.
                    let items: Vec<String> = v
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    out.allowed_tools = Some(items);
                }
                _ => {}
            }
        }
    }
    if let Some(prev_key) = current_list_key {
        commit_list(&mut out, &prev_key, current_list);
    }
    out
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn known_mcp_tool_names() -> Vec<String> {
    list_tools().into_iter().map(|t| t.name).collect()
}

fn validate_allowed_tool(spec: &str, mcp_tools: &[String]) -> Option<Issue> {
    let s = spec.trim();
    if s.is_empty() {
        return Some(Issue {
            severity: Severity::Warning,
            code: "empty-tool".into(),
            message: "empty entry in allowed-tools".into(),
        });
    }
    // Bash(...) form: anything inside parens is OK for MVP.
    if s.starts_with("Bash(") && s.ends_with(')') {
        return None;
    }
    // MCP tool name (e.g. `wg_search`)
    if mcp_tools.iter().any(|t| t == s) {
        return None;
    }
    // Common non-Bash forms used by Claude Code (Read, Write, Edit, …) — accept.
    let known_builtins = [
        "Read",
        "Write",
        "Edit",
        "Glob",
        "Grep",
        "WebFetch",
        "WebSearch",
        "TaskCreate",
        "TaskUpdate",
    ];
    if known_builtins.contains(&s) {
        return None;
    }
    Some(Issue {
        severity: Severity::Warning,
        code: "unknown-tool".into(),
        message: format!(
            "allowed-tool '{}' is neither Bash(…), a wg MCP tool, nor a known Claude Code built-in",
            s
        ),
    })
}

fn check_one(path: &Path) -> std::io::Result<FileReport> {
    let content = std::fs::read_to_string(path)?;
    let mut issues: Vec<Issue> = Vec::new();

    let (fm_text, body) = match split_frontmatter(&content) {
        Some(parts) => parts,
        None => {
            issues.push(Issue {
                severity: Severity::Error,
                code: "missing-frontmatter".into(),
                message: "no `---`-delimited YAML frontmatter at top of file".into(),
            });
            return Ok(FileReport {
                path: path.display().to_string(),
                kind: FileKind::Skill,
                ok: false,
                issues,
            });
        }
    };

    let fm = parse_skill_frontmatter(fm_text);

    // `kind: doc` (or any kind != "skill") marks the file as documentation —
    // we don't validate it as a skill. Use `kind: skill` (or omit `kind`) to
    // opt in to skill validation.
    if matches!(fm.kind.as_deref(), Some(k) if k != "skill") {
        return Ok(FileReport {
            path: path.display().to_string(),
            kind: FileKind::Doc,
            ok: true,
            issues: Vec::new(),
        });
    }

    // Required fields
    match &fm.name {
        None => issues.push(Issue {
            severity: Severity::Error,
            code: "missing-name".into(),
            message: "frontmatter is missing required `name` field".into(),
        }),
        Some(name) => {
            if name.is_empty() {
                issues.push(Issue {
                    severity: Severity::Error,
                    code: "empty-name".into(),
                    message: "`name` is empty".into(),
                });
            } else if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || name.chars().next().is_some_and(|c| !c.is_ascii_lowercase())
            {
                issues.push(Issue {
                    severity: Severity::Warning,
                    code: "name-format".into(),
                    message: format!(
                        "`name` should be lowercase ASCII (a-z, 0-9, -, _) starting with a letter; got `{}`",
                        name
                    ),
                });
            }
        }
    }

    match &fm.description {
        None => issues.push(Issue {
            severity: Severity::Error,
            code: "missing-description".into(),
            message: "frontmatter is missing required `description` field".into(),
        }),
        Some(d) if d.len() < 30 => issues.push(Issue {
            severity: Severity::Warning,
            code: "short-description".into(),
            message: format!(
                "`description` is only {} chars; longer descriptions improve auto-trigger accuracy",
                d.len()
            ),
        }),
        Some(_) => {}
    }

    // Recommended fields
    if fm.when_to_use.is_none() {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "missing-when-to-use".into(),
            message: "consider adding `when_to_use:` so the agent knows when to invoke this skill"
                .into(),
        });
    }

    // Tool validation
    if let Some(tools) = &fm.allowed_tools {
        let mcp_tools = known_mcp_tool_names();
        for spec in tools {
            if let Some(issue) = validate_allowed_tool(spec, &mcp_tools) {
                issues.push(issue);
            }
        }
    }

    // Body length
    let body_trimmed = body.trim();
    if body_trimmed.len() < 50 {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "thin-body".into(),
            message: format!(
                "skill body is {} chars; agents need real instructions, not just frontmatter",
                body_trimmed.len()
            ),
        });
    }

    let has_error = issues.iter().any(|i| i.severity == Severity::Error);
    Ok(FileReport {
        path: path.display().to_string(),
        kind: FileKind::Skill,
        ok: !has_error,
        issues,
    })
}

fn collect_files(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() && p.extension().is_some_and(|e| e == "md") {
            out.push(p);
        } else if p.is_dir() {
            out.extend(collect_files(&p)?);
        }
    }
    out.sort();
    Ok(out)
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

pub fn run_skill(sub: SkillSub, json: bool) -> Result<String, WgError> {
    match sub {
        SkillSub::Check { path } => {
            let files = collect_files(&path)
                .map_err(|e| WgError::FileRead(path.clone(), format!("read skill path: {}", e)))?;
            if files.is_empty() {
                return Err(WgError::InvalidInput(format!(
                    "no .md files found at {}",
                    path.display()
                )));
            }
            let mut reports: Vec<FileReport> = Vec::with_capacity(files.len());
            for f in &files {
                let report =
                    check_one(f).map_err(|e| WgError::FileRead(f.clone(), e.to_string()))?;
                reports.push(report);
            }
            let skills_checked = reports
                .iter()
                .filter(|r| matches!(r.kind, FileKind::Skill))
                .count();
            let docs_skipped = reports.len() - skills_checked;
            let summary = CheckSummary {
                total_files: reports.len(),
                skills_checked,
                docs_skipped,
                passing: reports
                    .iter()
                    .filter(|r| matches!(r.kind, FileKind::Skill) && r.ok)
                    .count(),
                error_count: reports
                    .iter()
                    .flat_map(|r| &r.issues)
                    .filter(|i| matches!(i.severity, Severity::Error))
                    .count(),
                warning_count: reports
                    .iter()
                    .flat_map(|r| &r.issues)
                    .filter(|i| matches!(i.severity, Severity::Warning))
                    .count(),
                files: reports,
            };
            if json {
                serde_json::to_string_pretty(&summary).map_err(|e| WgError::Serialize {
                    context: "skill check".to_string(),
                    source: e,
                })
            } else {
                Ok(format_human(&summary))
            }
        }
    }
}

fn format_human(s: &CheckSummary) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "wg skill check — {} file(s)  ({} skill(s) passing, {} doc(s) skipped, {} error(s), {} warning(s))\n\n",
        s.total_files, s.passing, s.docs_skipped, s.error_count, s.warning_count
    ));
    for f in &s.files {
        let mark = match (&f.kind, f.ok) {
            (FileKind::Doc, _) => "·",
            (FileKind::Skill, true) => "✓",
            (FileKind::Skill, false) => "✗",
        };
        let tag = match f.kind {
            FileKind::Doc => " (doc, skipped)",
            FileKind::Skill => "",
        };
        out.push_str(&format!("{} {}{}\n", mark, f.path, tag));
        for i in &f.issues {
            let prefix = match i.severity {
                Severity::Error => "  [error]",
                Severity::Warning => "  [warn ]",
            };
            out.push_str(&format!("{} [{}] {}\n", prefix, i.code, i.message));
        }
        if !f.issues.is_empty() {
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn check_str(content: &str) -> Vec<Issue> {
        // Write to a temp file and check it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, content).unwrap();
        check_one(&path).unwrap().issues
    }

    fn codes(issues: &[Issue]) -> Vec<&str> {
        issues.iter().map(|i| i.code.as_str()).collect()
    }

    #[test]
    fn valid_skill_passes() {
        let body = "---\nname: wg\ndescription: A long enough description for the skill to make sense to anyone reading it.\nwhen_to_use:\n  - User asks something\nallowed-tools: Bash(wg:*)\n---\n\n# Body\n\nThis is a real skill with actual usage instructions and content for the agent.";
        let issues = check_str(body);
        assert_eq!(codes(&issues), Vec::<&str>::new(), "{:?}", issues);
    }

    #[test]
    fn missing_frontmatter_errors() {
        let issues = check_str("# No frontmatter\n\nbody");
        assert!(codes(&issues).contains(&"missing-frontmatter"));
    }

    #[test]
    fn missing_name_errors() {
        let issues = check_str(
            "---\ndescription: long enough description for the skill to make some sense.\n---\n\nbody body body body body body body body body",
        );
        assert!(codes(&issues).contains(&"missing-name"));
    }

    #[test]
    fn short_description_warns() {
        let issues = check_str(
            "---\nname: wg\ndescription: short\n---\n\nbody body body body body body body body body body",
        );
        assert!(codes(&issues).contains(&"short-description"));
    }

    #[test]
    fn name_format_warns_on_uppercase() {
        let issues = check_str(
            "---\nname: WG\ndescription: A long enough description that makes sense to readers.\n---\n\nbody body body body body body body body body body",
        );
        assert!(codes(&issues).contains(&"name-format"));
    }

    #[test]
    fn unknown_tool_warns() {
        let issues = check_str(
            "---\nname: wg\ndescription: A long enough description that makes sense to readers.\nallowed-tools: [some_random_tool, Bash(ls:*)]\n---\n\nbody body body body body body body body body body",
        );
        assert!(
            codes(&issues).contains(&"unknown-tool"),
            "expected unknown-tool warning, got {:?}",
            issues
        );
    }

    #[test]
    fn known_mcp_tool_passes() {
        let issues = check_str(
            "---\nname: wg\ndescription: A long enough description that makes sense to readers.\nallowed-tools: [wg_search, wg_query, Bash(wg:*)]\n---\n\nbody body body body body body body body body body",
        );
        assert!(
            codes(&issues).iter().all(|c| *c != "unknown-tool"),
            "{:?}",
            issues
        );
    }

    #[test]
    fn block_list_when_to_use_parses() {
        let issues = check_str(
            "---\nname: wg\ndescription: A long enough description that makes sense to readers.\nwhen_to_use:\n  - First trigger\n  - Second trigger\nallowed-tools: [Bash(wg:*)]\n---\n\nbody body body body body body body body body body",
        );
        // missing-when-to-use should NOT appear since it parsed.
        assert!(
            !codes(&issues).contains(&"missing-when-to-use"),
            "{:?}",
            issues
        );
    }

    #[test]
    fn kind_doc_skips_validation() {
        // No `name`/`description` — would normally error — but `kind: doc` means
        // skip and pass.
        let body = "---\nkind: doc\ntitle: Some setup guide\n---\n\nany body";
        let report = check_one_str(body);
        assert_eq!(report.kind, FileKind::Doc);
        assert!(report.ok);
        assert!(report.issues.is_empty(), "{:?}", report.issues);
    }

    #[test]
    fn kind_skill_explicit_still_validates() {
        let body = "---\nkind: skill\nname: wg\ndescription: A long enough description for the skill.\nallowed-tools: [Bash(wg:*)]\n---\n\nbody body body body body body body body body body";
        let report = check_one_str(body);
        assert_eq!(report.kind, FileKind::Skill);
        assert!(report.ok, "{:?}", report.issues);
    }

    fn check_one_str(content: &str) -> FileReport {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, content).unwrap();
        check_one(&path).unwrap()
    }

    #[test]
    fn thin_body_warns() {
        let issues = check_str(
            "---\nname: wg\ndescription: A long enough description that makes sense to readers.\nallowed-tools: [Bash(wg:*)]\n---\n\ntoo short",
        );
        assert!(codes(&issues).contains(&"thin-body"));
    }
}
