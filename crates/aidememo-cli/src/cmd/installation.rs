//! Local coding-agent installation profiles for external handoff workers.

use aidememo_core::{AideMemoError, Config, InstallationConfig};
use bpaf::*;
use std::path::Path;

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub enum InstallationSub {
    Add {
        agent: String,
        binary: Option<String>,
        config_home: Option<String>,
        workspace: Option<String>,
        source_id: Option<String>,
        model: Option<String>,
        env_policy: String,
        pass_env: Vec<String>,
        alias: String,
    },
    List,
    Show {
        alias: String,
    },
    Remove {
        alias: String,
    },
}

fn installation_subcommands(
    agent_flag: &'static str,
    config_home_flag: &'static str,
) -> impl Parser<InstallationSub> {
    let agent = long(agent_flag)
        .help("Worker adapter: codex or claude")
        .argument::<String>("AGENT");
    let binary = long("binary")
        .help("Optional coding-agent executable override")
        .argument::<String>("PATH")
        .optional();
    let config_home = long(config_home_flag)
        .help("Agent state/config root; maps to CODEX_HOME for Codex")
        .argument::<String>("PATH")
        .optional();
    let workspace = long("workspace")
        .help("Default workspace for external handoff execution")
        .argument::<String>("PATH")
        .optional();
    let source_id = long("source-id")
        .help("Default AideMemo source namespace")
        .argument::<String>("SOURCE_ID")
        .optional();
    let model = long("model")
        .help("Optional coding-agent model override")
        .argument::<String>("MODEL")
        .optional();
    let env_policy = long("env-policy")
        .help("Child environment policy: core (default) or all")
        .argument::<String>("POLICY")
        .fallback("core".to_string());
    let pass_env = long("pass-env")
        .help("Environment-variable name to inherit without storing its value; repeatable")
        .argument::<String>("NAME")
        .many();
    let alias = positional::<String>("ALIAS");
    let add = construct!(InstallationSub::Add {
        agent,
        binary,
        config_home,
        workspace,
        source_id,
        model,
        env_policy,
        pass_env,
        alias,
    })
    .to_options()
    .command("add")
    .help("Add or replace a local coding-agent installation profile");

    let list = pure(InstallationSub::List)
        .to_options()
        .command("list")
        .help("List local coding-agent installation profiles");

    let alias = positional::<String>("ALIAS");
    let show = construct!(InstallationSub::Show { alias })
        .to_options()
        .command("show")
        .help("Show one local coding-agent installation profile");

    let alias = positional::<String>("ALIAS");
    let remove = construct!(InstallationSub::Remove { alias })
        .to_options()
        .command("remove")
        .help("Remove one local coding-agent installation profile");

    construct!([add, list, show, remove])
}

pub fn installation_command() -> impl Parser<Command> {
    installation_subcommands("agent", "config-home")
        .map(Command::Installation)
        .to_options()
        .command("installation")
        .help("Manage local Codex/Claude account runtime profiles (no stored credentials)")
}

/// Friendly alias for the infrastructure-oriented `installation` command.
///
/// `agent add codex-two --type codex --home ...` keeps the common path in the
/// vocabulary users already have while preserving the existing command for
/// scripts and advanced documentation.
pub fn agent_command() -> impl Parser<Command> {
    installation_subcommands("type", "home")
        .map(Command::Installation)
        .to_options()
        .command("agent")
        .help("Connect local Codex/Claude accounts for cross-agent handoff")
}

pub fn run_installation(
    mut config: Config,
    sub: InstallationSub,
    json: bool,
) -> Result<String, AideMemoError> {
    match sub {
        InstallationSub::Add {
            agent,
            binary,
            config_home,
            workspace,
            source_id,
            model,
            env_policy,
            pass_env,
            alias,
        } => {
            let alias = validate_alias(&alias)?;
            let agent = normalise_agent(&agent)?;
            let config_home = validate_optional_dir("--config-home", config_home)?;
            let workspace = validate_optional_dir("--workspace", workspace)?;
            let pass_env = validate_env_names(pass_env)?;
            let env_policy = normalise_env_policy(&env_policy)?;
            let profile = InstallationConfig {
                agent,
                binary: clean_optional(binary),
                config_home,
                workspace,
                source_id: clean_optional(source_id),
                model: clean_optional(model),
                env_policy,
                pass_env,
            };
            let replaced = config
                .installations
                .insert(alias.clone(), profile.clone())
                .is_some();
            config.save()?;
            if json {
                serde_json::to_string_pretty(&serde_json::json!({
                    "alias": alias,
                    "replaced": replaced,
                    "installation": profile,
                }))
                .map_err(|source| AideMemoError::Serialize {
                    context: "installation add".to_string(),
                    source,
                })
            } else {
                Ok(format!(
                    "{} agent profile {} ({})",
                    if replaced { "Updated" } else { "Added" },
                    alias,
                    profile.agent
                ))
            }
        }
        InstallationSub::List => {
            if json {
                serde_json::to_string_pretty(&config.installations).map_err(|source| {
                    AideMemoError::Serialize {
                        context: "installation list".to_string(),
                        source,
                    }
                })
            } else if config.installations.is_empty() {
                Ok("(no connected agent profiles)".to_string())
            } else {
                let mut out = String::new();
                for (alias, profile) in &config.installations {
                    out.push_str(&format!(
                        "{} agent={} source={} workspace={}\n",
                        alias,
                        profile.agent,
                        profile.source_id.as_deref().unwrap_or("-"),
                        profile.workspace.as_deref().unwrap_or("-")
                    ));
                }
                Ok(out.trim_end().to_string())
            }
        }
        InstallationSub::Show { alias } => {
            let alias = validate_alias(&alias)?;
            let profile = config.installations.get(&alias).ok_or_else(|| {
                AideMemoError::InvalidInput(format!("agent profile {alias:?} not found"))
            })?;
            if json {
                serde_json::to_string_pretty(&serde_json::json!({
                    "alias": alias,
                    "installation": profile,
                }))
                .map_err(|source| AideMemoError::Serialize {
                    context: "installation show".to_string(),
                    source,
                })
            } else {
                toml::to_string_pretty(profile)
                    .map(|body| format!("alias = {alias:?}\n{body}"))
                    .map_err(|source| {
                        AideMemoError::Internal(format!("serialize installation: {source}"))
                    })
            }
        }
        InstallationSub::Remove { alias } => {
            let alias = validate_alias(&alias)?;
            if config.installations.remove(&alias).is_none() {
                return Err(AideMemoError::InvalidInput(format!(
                    "agent profile {alias:?} not found"
                )));
            }
            config.save()?;
            if json {
                Ok(serde_json::json!({"alias": alias, "removed": true}).to_string())
            } else {
                Ok(format!("Removed agent profile {alias}"))
            }
        }
    }
}

fn validate_alias(value: &str) -> Result<String, AideMemoError> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(AideMemoError::InvalidInput(
            "installation alias must use letters, digits, '.', '-', or '_'".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn normalise_agent(value: &str) -> Result<String, AideMemoError> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "codex" | "claude" => Ok(value),
        _ => Err(AideMemoError::InvalidInput(
            "--agent must be codex or claude".to_string(),
        )),
    }
}

fn normalise_env_policy(value: &str) -> Result<String, AideMemoError> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "core" | "all" => Ok(value),
        _ => Err(AideMemoError::InvalidInput(
            "--env-policy must be core or all".to_string(),
        )),
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_optional_dir(
    label: &str,
    value: Option<String>,
) -> Result<Option<String>, AideMemoError> {
    let Some(value) = clean_optional(value) else {
        return Ok(None);
    };
    let path = Path::new(&value);
    if !path.is_dir() {
        return Err(AideMemoError::InvalidInput(format!(
            "{label} must name an existing directory: {value}"
        )));
    }
    Ok(Some(value))
}

fn validate_env_names(values: Vec<String>) -> Result<Vec<String>, AideMemoError> {
    let mut out = Vec::new();
    for value in values {
        let value = value.trim();
        let valid = !value.is_empty()
            && value.chars().enumerate().all(|(index, ch)| {
                ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
            });
        if !valid {
            return Err(AideMemoError::InvalidInput(format!(
                "--pass-env expects an environment variable name, got {value:?}"
            )));
        }
        if !out.iter().any(|existing| existing == value) {
            out.push(value.to_string());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_aliases_and_env_names() {
        assert_eq!(validate_alias("codex-two").unwrap(), "codex-two");
        assert!(validate_alias("codex two").is_err());
        assert_eq!(
            validate_env_names(vec!["OPENAI_API_KEY".to_string()]).unwrap(),
            vec!["OPENAI_API_KEY"]
        );
        assert!(validate_env_names(vec!["1BAD".to_string()]).is_err());
    }
}
