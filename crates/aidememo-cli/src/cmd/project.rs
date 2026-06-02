//! `aidememo project` — manage named multi-store projects.
//!
//! ```text
//!   aidememo project list                — show registered projects + the default
//!   aidememo project show [<name>]       — details (defaults to the default project)
//!   aidememo project create <name> --path <PATH>
//!                                  — register a new project
//!   aidememo project use <name>          — set the default project
//!   aidememo project remove <name>       — unregister (does NOT delete files)
//! ```
//!
//! Projects live in `~/.aidememo/config.toml` under `[projects.<name>]`.

use aidememo_core::{AideMemoError, Config, ProjectConfig};
use bpaf::*;
use std::path::PathBuf;

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub enum ProjectSub {
    List,
    Show { name: Option<String> },
    Create { path: PathBuf, name: String },
    Use { name: String },
    Remove { name: String },
}

pub fn project_command() -> impl Parser<Command> {
    let list = pure(ProjectSub::List)
        .to_options()
        .command("list")
        .help("List registered projects + the default");

    let name = positional::<String>("NAME").optional();
    let show = construct!(ProjectSub::Show { name })
        .to_options()
        .command("show")
        .help("Show project details (default: current default project)");

    let path = long("path")
        .help("Path to the wiki.redb store file")
        .argument::<PathBuf>("PATH");
    let name = positional::<String>("NAME");
    let create = construct!(ProjectSub::Create { path, name })
        .to_options()
        .command("create")
        .help("Register a new project");

    let name = positional::<String>("NAME");
    let use_cmd = construct!(ProjectSub::Use { name })
        .to_options()
        .command("use")
        .help("Set the default project");

    let name = positional::<String>("NAME");
    let remove = construct!(ProjectSub::Remove { name })
        .to_options()
        .command("remove")
        .help("Unregister a project (does not delete files)");

    construct!([list, show, create, use_cmd, remove])
        .map(Command::Project)
        .to_options()
        .command("project")
        .help("Multi-project management")
}

pub fn run_project(mut config: Config, sub: ProjectSub) -> Result<String, AideMemoError> {
    match sub {
        ProjectSub::List => {
            if config.projects.is_empty() {
                return Ok(format!(
                    "No projects registered.\n\nDefault store: {}\nRegister with `aidememo project create <name> --path <PATH>`.",
                    config.default_store_path().display()
                ));
            }
            let mut out = String::from("Projects:\n");
            for (name, p) in &config.projects {
                let marker = if Some(name) == config.default_project.as_ref() {
                    " *"
                } else {
                    "  "
                };
                out.push_str(&format!("{marker}{name:<20} {}\n", p.path));
            }
            if let Some(d) = &config.default_project {
                out.push_str(&format!("\n* default: {d}\n"));
            } else {
                out.push_str(&format!(
                    "\nNo default set; falling back to store.path = {}\n",
                    config.store.path
                ));
            }
            Ok(out)
        }
        ProjectSub::Show { name } => {
            let target = match name {
                Some(n) => n,
                None => config.default_project.clone().ok_or_else(|| {
                    AideMemoError::InvalidInput("no default project set".to_string())
                })?,
            };
            let p = config.projects.get(&target).ok_or_else(|| {
                AideMemoError::InvalidInput(format!("project '{target}' not registered"))
            })?;
            Ok(format!(
                "Project: {target}\n  path: {}\n  resolved: {}",
                p.path,
                config.project_path(&target).unwrap_or_default().display()
            ))
        }
        ProjectSub::Create { name, path } => {
            if config.projects.contains_key(&name) {
                return Err(AideMemoError::InvalidInput(format!(
                    "project '{name}' already exists; use `aidememo project remove {name}` first"
                )));
            }
            config.projects.insert(
                name.clone(),
                ProjectConfig {
                    path: path.display().to_string(),
                },
            );
            // First project becomes the default automatically.
            if config.default_project.is_none() {
                config.default_project = Some(name.clone());
            }
            config.save()?;
            Ok(format!("Created project '{name}' → {}", path.display()))
        }
        ProjectSub::Use { name } => {
            if !config.projects.contains_key(&name) {
                return Err(AideMemoError::InvalidInput(format!(
                    "project '{name}' not registered; use `aidememo project create` first"
                )));
            }
            config.default_project = Some(name.clone());
            config.save()?;
            Ok(format!("Default project is now '{name}'"))
        }
        ProjectSub::Remove { name } => {
            if config.projects.remove(&name).is_none() {
                return Err(AideMemoError::InvalidInput(format!(
                    "project '{name}' not registered"
                )));
            }
            if config.default_project.as_deref() == Some(&name) {
                config.default_project = None;
            }
            config.save()?;
            Ok(format!("Removed project '{name}' (files left in place)"))
        }
    }
}
