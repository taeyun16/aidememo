//! `aidememo auth` — bearer-token UX helpers for the Phase 2 sync surface.
//!
//! Three pieces:
//!   * `generate` — emit a fresh random hex token (default 32 bytes)
//!     so operators don't have to remember `openssl rand -hex 32`.
//!   * `login <URL> --token T | --token-file PATH` — persist the
//!     token in `~/.aidememo/auth.json` keyed by the upstream URL. Mode
//!     0600. Subsequent `aidememo sync pull <URL>` reads it transparently.
//!   * `logout <URL>` / `list` — manage stored entries.
//!
//! Storage lives at `~/.aidememo/auth.json`; the `aidememo-cli` `sync pull`
//! handler reads via `load_token_for(url)` defined here.

use aidememo_core::AideMemoError;
use bpaf::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub enum AuthSub {
    Generate {
        bytes: Option<usize>,
    },
    Login {
        url: String,
        token: Option<String>,
        token_file: Option<PathBuf>,
    },
    Logout {
        url: String,
    },
    List,
}

pub fn auth_command() -> impl Parser<Command> {
    let bytes = long("bytes")
        .help("Token byte length before hex-encoding (default 32 → 64 hex chars).")
        .argument::<usize>("N")
        .optional();
    let generate = construct!(AuthSub::Generate { bytes })
        .to_options()
        .command("generate")
        .help("Print a fresh high-entropy bearer token to stdout.");

    let url = positional::<String>("URL")
        .help("Upstream `aidememo mcp-serve` base URL (e.g. http://team-host:3000)");
    let token = long("token")
        .help("Bearer token literal. Mutually exclusive with --token-file.")
        .argument::<String>("TOKEN")
        .optional();
    let token_file = long("token-file")
        .help("Path to a file holding the token (single line, trimmed).")
        .argument::<PathBuf>("PATH")
        .optional();
    let login = construct!(AuthSub::Login {
        token,
        token_file,
        url,
    })
    .to_options()
    .command("login")
    .help("Persist a bearer token in ~/.aidememo/auth.json keyed by URL.");

    let url = positional::<String>("URL").help("URL whose stored token should be removed.");
    let logout = construct!(AuthSub::Logout { url })
        .to_options()
        .command("logout")
        .help("Remove a stored token from ~/.aidememo/auth.json.");

    let list = pure(AuthSub::List)
        .to_options()
        .command("list")
        .help("List the URLs that currently have a stored token (token values redacted).");

    construct!([generate, login, logout, list])
        .map(Command::Auth)
        .to_options()
        .command("auth")
        .help(
            "Bearer-token UX helpers. Subcommands: \
             `generate` (random token), \
             `login <URL> [--token T | --token-file P]` (store token), \
             `logout <URL>`, `list`.",
        )
}

pub fn run_auth(sub: AuthSub) -> Result<String, AideMemoError> {
    match sub {
        AuthSub::Generate { bytes } => {
            let n = bytes.unwrap_or(32);
            if n == 0 || n > 256 {
                return Err(AideMemoError::InvalidInput(format!(
                    "auth generate --bytes must be in 1..=256, got {n}"
                )));
            }
            generate_token_hex(n)
        }
        AuthSub::Login {
            url,
            token,
            token_file,
        } => {
            let resolved = resolve_token_input(token, token_file)?;
            let key = canonical_url(&url);
            let path = auth_file_path()?;
            let mut store = load_auth_file(&path);
            store.remotes.insert(
                key.clone(),
                StoredAuth {
                    token: resolved,
                    added_at: aidememo_core::time::current_epoch_ms(),
                },
            );
            save_auth_file(&path, &store)?;
            Ok(format!("stored token for {} (in {})", key, path.display()))
        }
        AuthSub::Logout { url } => {
            let key = canonical_url(&url);
            let path = auth_file_path()?;
            let mut store = load_auth_file(&path);
            let removed = store.remotes.remove(&key).is_some();
            if removed {
                save_auth_file(&path, &store)?;
                Ok(format!("removed stored token for {}", key))
            } else {
                Ok(format!("no stored token for {} — nothing to do", key))
            }
        }
        AuthSub::List => {
            let path = auth_file_path()?;
            let store = load_auth_file(&path);
            if store.remotes.is_empty() {
                return Ok(format!("no stored tokens (file: {})", path.display()));
            }
            let mut out = format!("stored tokens ({}):\n", path.display());
            for (url, entry) in &store.remotes {
                let prefix: String = entry.token.chars().take(6).collect();
                out.push_str(&format!(
                    "  {}  token={}…  added_at={}\n",
                    url, prefix, entry.added_at
                ));
            }
            Ok(out)
        }
    }
}

/// Read `~/.aidememo/auth.json` and return the token associated with `url`,
/// if any. Used by `handle_sync_pull` as the last step in the
/// resolution chain.
pub fn load_token_for(url: &str) -> Option<String> {
    let path = auth_file_path().ok()?;
    let store = load_auth_file(&path);
    store
        .remotes
        .get(&canonical_url(url))
        .map(|e| e.token.clone())
}

// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct AuthFile {
    /// Keyed by canonical URL (trailing slash stripped).
    remotes: BTreeMap<String, StoredAuth>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredAuth {
    token: String,
    added_at: u64,
}

fn auth_file_path() -> Result<PathBuf, AideMemoError> {
    let home = std::env::var("HOME").map_err(|_| {
        AideMemoError::InvalidInput(
            "HOME env var not set — can't resolve ~/.aidememo/auth.json".into(),
        )
    })?;
    let dir = PathBuf::from(home).join(".aidememo");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| AideMemoError::Internal(format!("create {}: {e}", dir.display())))?;
    }
    Ok(dir.join("auth.json"))
}

fn canonical_url(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

fn load_auth_file(path: &Path) -> AuthFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_auth_file(path: &Path, store: &AuthFile) -> Result<(), AideMemoError> {
    let bytes = serde_json::to_vec_pretty(store).map_err(|e| AideMemoError::Serialize {
        context: "auth.json".into(),
        source: e,
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)
        .map_err(|e| AideMemoError::Internal(format!("write {}: {e}", tmp.display())))?;
    // 0600 — owner read/write only. Best-effort; if the FS doesn't
    // honour it (FAT, network mount), the rename still proceeds and
    // we leave the stricter mode as advisory.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| AideMemoError::Internal(format!("rename auth.json: {e}")))?;
    Ok(())
}

fn resolve_token_input(
    token: Option<String>,
    token_file: Option<PathBuf>,
) -> Result<String, AideMemoError> {
    match (token, token_file) {
        (Some(_), Some(_)) => Err(AideMemoError::InvalidInput(
            "pass --token OR --token-file, not both".into(),
        )),
        (Some(t), None) => {
            let trimmed = t.trim().to_string();
            if trimmed.is_empty() {
                return Err(AideMemoError::InvalidInput("--token is empty".into()));
            }
            Ok(trimmed)
        }
        (None, Some(p)) => crate::cmd::mcp_serve::read_token_file(&p),
        (None, None) => Err(AideMemoError::InvalidInput(
            "aidememo auth login requires --token or --token-file".into(),
        )),
    }
}

fn generate_token_hex(byte_len: usize) -> Result<String, AideMemoError> {
    use std::io::Read;
    let mut buf = vec![0u8; byte_len];
    let mut f = std::fs::File::open("/dev/urandom")
        .map_err(|e| AideMemoError::Internal(format!("open /dev/urandom: {e}")))?;
    f.read_exact(&mut buf)
        .map_err(|e| AideMemoError::Internal(format!("read /dev/urandom: {e}")))?;
    let mut hex = String::with_capacity(byte_len * 2);
    for b in &buf {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", b);
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_url_strips_trailing_slash() {
        assert_eq!(canonical_url("http://x:3000"), "http://x:3000");
        assert_eq!(canonical_url("http://x:3000/"), "http://x:3000");
        assert_eq!(canonical_url("http://x:3000///"), "http://x:3000");
    }

    #[test]
    fn resolve_token_input_rejects_both() {
        let err = resolve_token_input(Some("a".into()), Some(PathBuf::from("/x"))).unwrap_err();
        assert!(err.to_string().contains("not both"));
    }

    #[test]
    fn resolve_token_input_rejects_neither() {
        let err = resolve_token_input(None, None).unwrap_err();
        assert!(err.to_string().contains("requires --token"));
    }

    #[test]
    fn resolve_token_input_rejects_empty_literal() {
        let err = resolve_token_input(Some("   \n".into()), None).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn resolve_token_input_trims_literal() {
        let t = resolve_token_input(Some("  abc \n".into()), None).unwrap();
        assert_eq!(t, "abc");
    }

    #[test]
    fn save_and_load_roundtrip_with_unique_home() {
        let dir = tempfile::tempdir().unwrap();
        // Set HOME inside this test only — auth_file_path() reads it.
        // SAFETY: env is process-global; we rely on cargo test's
        // single-threaded default for this module. Use --test-threads=1
        // if you ever add concurrent env-mutating tests here.
        // Reverting at the end keeps neighbouring tests sane.
        let prev = std::env::var("HOME").ok();
        // SAFETY: see comment above re: process-global env mutation.
        unsafe { std::env::set_var("HOME", dir.path()) };

        let path = auth_file_path().unwrap();
        let mut store = AuthFile::default();
        store.remotes.insert(
            "http://x:3000".into(),
            StoredAuth {
                token: "T1".into(),
                added_at: 100,
            },
        );
        save_auth_file(&path, &store).unwrap();

        assert_eq!(load_token_for("http://x:3000"), Some("T1".into()));
        assert_eq!(load_token_for("http://x:3000/"), Some("T1".into()));
        assert_eq!(load_token_for("http://other"), None);

        if let Some(p) = prev {
            // SAFETY: see comment above.
            unsafe { std::env::set_var("HOME", p) };
        } else {
            // SAFETY: see comment above.
            unsafe { std::env::remove_var("HOME") };
        }
    }
}
