//! `wg completions <shell>` — emit shell completion scripts.
//!
//! bpaf ships its own runtime completion handler under the
//! `autocomplete` feature, which we enable in `Cargo.toml`. The
//! handler is wired up automatically: invoking `wg --bpaf-complete-
//! rev=<N> <args>` returns completion candidates for the shell,
//! and the small shell-specific shim emitted below teaches each
//! shell how to call into it.
//!
//! We hand-emit the shim (rather than shelling out to
//! `wg --bpaf-complete-style-bash`) because it's three tiny
//! templates — no need for a self-process spawn just to print a
//! string. Templates are kept in lockstep with bpaf's bundled
//! versions; if the upstream `--bpaf-complete-rev=N` protocol
//! version changes, our `N` here has to follow.

use bpaf::*;

use crate::cmd::Command;
use wg_core::WgError;

#[derive(Debug, Clone)]
pub struct CompletionsSub {
    pub shell: String,
}

pub fn completions_command() -> impl Parser<Command> {
    let shell = positional::<String>("SHELL").help("Target shell: bash, zsh, fish, or elvish");

    construct!(CompletionsSub { shell })
        .map(Command::Completions)
        .to_options()
        .command("completions")
        .help(
            "Emit a shell completion script. Pipe into your shell's \
             completion directory or eval it directly:\n  \
             eval \"$(wg completions bash)\"\n  \
             wg completions zsh > ~/.zsh/completions/_wg",
        )
}

pub fn run_completions(sub: CompletionsSub) -> Result<String, WgError> {
    match sub.shell.to_lowercase().as_str() {
        "bash" => Ok(BASH_TEMPLATE.replace("{name}", "wg")),
        "zsh" => Ok(ZSH_TEMPLATE.replace("{name}", "wg")),
        "fish" => Ok(FISH_TEMPLATE.replace("{name}", "wg")),
        "elvish" => Ok(ELVISH_TEMPLATE.replace("{name}", "wg")),
        other => Err(WgError::InvalidInput(format!(
            "unknown shell `{}` — supported: bash, zsh, fish, elvish",
            other
        ))),
    }
}

// Templates copied verbatim from bpaf 0.9.x's complete_run.rs (the
// bash/zsh/fish/elvish dump_*_completer fns). Keeping them in-tree
// lets us emit the script without a self-process spawn; the price
// is a one-line bump if bpaf changes the protocol revision.

const BASH_TEMPLATE: &str = r#"_bpaf_dynamic_completion()
{
    line="$1 --bpaf-complete-rev=8 ${COMP_WORDS[@]:1}"
    if [[ ${COMP_WORDS[-1]} == "" ]]; then
        line="${line} \"\""
    fi
    source <( eval ${line})
}
complete -o nosort -F _bpaf_dynamic_completion {name}
"#;

const ZSH_TEMPLATE: &str = r#"#compdef {name}
local line
line="${words[1]} --bpaf-complete-rev=7 ${words[@]:1}"
if [[ ${words[-1]} == "" ]]; then
    line="${line} \"\""
fi
source <(eval ${line})
"#;

const FISH_TEMPLATE: &str = r#"function _bpaf_dynamic_completion
    set -l current (commandline --tokenize --current-process)
    set -l tmpline --bpaf-complete-rev=9 $current[2..]
    if test (commandline --current-process) != (string trim (commandline --current-process))
        set tmpline $tmpline ""
    end
    eval $current[1] \"$tmpline\"
end

complete --no-files --command {name} --arguments '(_bpaf_dynamic_completion)'
"#;

const ELVISH_TEMPLATE: &str = r#"set edit:completion:arg-completer[{name}] = { |@args| var args = $args[1..];
     var @lines = ( {name} --bpaf-complete-rev=1 $@args );
     use str;
     for line $lines {
         var @arg = (str:split "\t" $line)
         try {
             edit:complex-candidate $arg[0] &display=( printf "%-19s %s" $arg[0] $arg[1] )
         } catch {
             edit:complex-candidate $line
         }
     }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_bash_template_with_binary_name() {
        let out = run_completions(CompletionsSub {
            shell: "bash".to_string(),
        })
        .unwrap();
        assert!(out.contains("_bpaf_dynamic_completion"));
        assert!(out.contains("complete -o nosort -F _bpaf_dynamic_completion wg"));
    }

    #[test]
    fn emits_zsh_template() {
        let out = run_completions(CompletionsSub {
            shell: "zsh".to_string(),
        })
        .unwrap();
        assert!(out.starts_with("#compdef wg"));
    }

    #[test]
    fn emits_fish_template() {
        let out = run_completions(CompletionsSub {
            shell: "fish".to_string(),
        })
        .unwrap();
        assert!(out.contains("complete --no-files --command wg"));
    }

    #[test]
    fn case_insensitive_shell_name() {
        let bash_lower = run_completions(CompletionsSub {
            shell: "bash".to_string(),
        })
        .unwrap();
        let bash_upper = run_completions(CompletionsSub {
            shell: "BASH".to_string(),
        })
        .unwrap();
        assert_eq!(bash_lower, bash_upper);
    }

    #[test]
    fn unknown_shell_errors() {
        let err = run_completions(CompletionsSub {
            shell: "nope".to_string(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("unknown shell"));
    }
}
