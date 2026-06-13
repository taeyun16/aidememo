//! `aidememo daemon` — manage a long-lived `aidememo mcp-serve` background process.
//!
//! Why this exists: a fresh `aidememo search` CLI spawn pays ~70 ms (BM25)
//! to ~1 s (hybrid + cold model load) every invocation. A daemon
//! amortises both the store open and the embedding model load so the
//! same query takes ~5 ms (BM25) or ~43 ms (HNSW) — see
//! `bench/beads-vs-aidememo/scenario_5_daemon_warm.py`.
//!
//! Subcommands
//! -----------
//!   aidememo daemon start [--port N] [--store PATH]
//!       Spawn `aidememo --backend <resolved> mcp-serve` in the background;
//!       record port + PID + store path + backend in ~/.aidememo/daemon.json.
//!       Idempotent: if a healthy daemon is already registered, return its info.
//!
//!   aidememo daemon stop
//!       Read the registry, send SIGTERM to the recorded PID, delete
//!       the registry file.
//!
//!   aidememo daemon status
//!       Show what (if anything) the registry points at and whether
//!       the daemon answers /health.
//!
//! Discovery
//! ---------
//! Read CLI commands (`aidememo search`, `aidememo query`, …) consult the
//! registry first. If a healthy daemon is registered AND its store
//! and backend match the resolved CLI context, the CLI dispatches over HTTP
//! instead of opening the store in-process. Set `AIDEMEMO_NO_DAEMON=1` to opt
//! out for one invocation. See `daemon::registered_endpoint()`.

use aidememo_core::AideMemoError;
use bpaf::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Stdio};
use std::time::{Duration, Instant};

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub enum DaemonSub {
    Start {
        port: Option<u16>,
        store: Option<PathBuf>,
    },
    Stop,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonRegistry {
    pub port: u16,
    pub pid: u32,
    pub store: PathBuf,
    #[serde(default)]
    pub backend: Option<String>,
    pub started_at: u64, // epoch ms
}

pub fn daemon_command() -> impl Parser<Command> {
    let start_port = long("port")
        .help("Port for the background mcp-serve (default: auto-pick)")
        .argument::<u16>("PORT")
        .optional();
    let start_store = long("store")
        .help("Store path the daemon should serve (defaults to resolved CLI store)")
        .argument::<PathBuf>("PATH")
        .optional();
    let start = construct!(DaemonSub::Start {
        port(start_port),
        store(start_store),
    })
    .to_options()
    .command("start")
    .help("Spawn `aidememo mcp-serve` in the background");

    let stop = pure(DaemonSub::Stop)
        .to_options()
        .command("stop")
        .help("Stop the background daemon");

    let status = pure(DaemonSub::Status)
        .to_options()
        .command("status")
        .help("Show registered daemon info + health");

    construct!([start, stop, status])
        .map(Command::Daemon)
        .to_options()
        .command("daemon")
        .help("Manage a long-lived background mcp-serve")
}

// === Registry I/O ===

pub fn registry_path() -> Result<PathBuf, AideMemoError> {
    let home =
        dirs::home_dir().ok_or_else(|| AideMemoError::Internal("no home directory".into()))?;
    Ok(home.join(".aidememo").join("daemon.json"))
}

fn load_registry() -> Option<DaemonRegistry> {
    let path = registry_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_registry(reg: &DaemonRegistry) -> Result<(), AideMemoError> {
    let path = registry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AideMemoError::Internal(format!("create {}: {e}", parent.display())))?;
    }
    let text = serde_json::to_string_pretty(reg)
        .map_err(|e| AideMemoError::Internal(format!("serialize registry: {e}")))?;
    std::fs::write(&path, text)
        .map_err(|e| AideMemoError::Internal(format!("write registry: {e}")))?;
    Ok(())
}

fn delete_registry() -> Result<(), AideMemoError> {
    let path = registry_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| AideMemoError::Internal(format!("remove registry: {e}")))?;
    }
    Ok(())
}

// === Health probe ===

fn probe_health(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    ureq::get(&url)
        .timeout(Duration::from_millis(500))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

/// Return a daemon URL the CLI should dispatch through, or None if
/// no daemon is healthy / no registry exists / the daemon's store
/// and backend don't match the requested context. `AIDEMEMO_NO_DAEMON=1`
/// short-circuits to None.
pub fn registered_endpoint(want_store: &Path, want_backend: &str) -> Option<String> {
    if std::env::var("AIDEMEMO_NO_DAEMON").is_ok() {
        return None;
    }
    daemon_for_hint(want_store, want_backend).map(|reg| format!("http://127.0.0.1:{}", reg.port))
}

/// Discovery info used by error-hint code on the failure path.
/// Ignores `AIDEMEMO_NO_DAEMON` — when the user opts out of dispatch they
/// still need the diagnostic ("daemon is the one holding the lock").
/// Returns the registry only if the recorded store/backend match AND
/// /health responds. Use `registry_state` if you also want to
/// distinguish "no registry" from "stale registry".
pub fn daemon_for_hint(want_store: &Path, want_backend: &str) -> Option<DaemonRegistry> {
    let reg = load_registry()?;
    if !registry_matches_request(&reg, want_store, want_backend) {
        return None;
    }
    if !probe_health(reg.port) {
        return None;
    }
    Some(reg)
}

/// Three-way classification for error-hint code:
///   - `Healthy(reg)`  : registry exists, store/backend match, /health OK
///   - `StaleRegistry` : registry exists but isn't usable for this request
///   - `None`          : no registry on disk
pub enum RegistryState {
    Healthy(DaemonRegistry),
    StaleRegistry,
    None,
}

pub fn registry_state(want_store: &Path, want_backend: &str) -> RegistryState {
    let Some(reg) = load_registry() else {
        return RegistryState::None;
    };
    if registry_matches_request(&reg, want_store, want_backend) && probe_health(reg.port) {
        RegistryState::Healthy(reg)
    } else {
        RegistryState::StaleRegistry
    }
}

fn registry_matches_request(reg: &DaemonRegistry, want_store: &Path, want_backend: &str) -> bool {
    reg.store == want_store && registry_backend_matches(reg.backend.as_deref(), want_backend)
}

fn registry_backend_matches(recorded_backend: Option<&str>, want_backend: &str) -> bool {
    let Some(recorded_backend) = recorded_backend else {
        return false;
    };
    canonical_backend(recorded_backend) == canonical_backend(want_backend)
}

fn canonical_backend(backend: &str) -> String {
    match backend.trim().to_ascii_lowercase().as_str() {
        "" | "sqlite" | "libsqlite" => "sqlite".to_string(),
        other => other.to_string(),
    }
}

// === Subcommand handlers ===

fn pick_free_port() -> Option<u16> {
    use std::net::TcpListener;
    TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn run_daemon(
    sub: DaemonSub,
    store_path: PathBuf,
    store_backend: String,
) -> Result<String, AideMemoError> {
    match sub {
        DaemonSub::Start { port, store } => {
            start_daemon(port, store.unwrap_or(store_path), store_backend)
        }
        DaemonSub::Stop => stop_daemon(),
        DaemonSub::Status => status_daemon(store_path, store_backend),
    }
}

fn start_daemon(
    port: Option<u16>,
    store: PathBuf,
    store_backend: String,
) -> Result<String, AideMemoError> {
    let daemon_backend = canonical_backend(&store_backend);
    // Fast path — already running and healthy.
    if let Some(reg) = load_registry() {
        if registry_matches_request(&reg, &store, &daemon_backend) && probe_health(reg.port) {
            return Ok(format!(
                "aidememo daemon already running (pid={}, port={}, store={}, backend={})",
                reg.pid,
                reg.port,
                reg.store.display(),
                reg.backend.as_deref().unwrap_or("unknown"),
            ));
        }
        // Stale registry. Best-effort cleanup.
        let _ = delete_registry();
    }

    let port = port
        .or_else(pick_free_port)
        .ok_or_else(|| AideMemoError::Internal("no free port available".into()))?;

    // Spawn `aidememo --backend BACKEND mcp-serve --port PORT <store>`
    // detached. We use the current binary so a `cargo run` build path works just as well
    // as an installed `aidememo`.
    let exe = std::env::current_exe()
        .map_err(|e| AideMemoError::Internal(format!("current_exe: {e}")))?;
    let log_path = registry_path()?.with_file_name("daemon.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AideMemoError::Internal(format!("create {}: {e}", parent.display())))?;
    }
    let log = std::fs::File::create(&log_path)
        .map_err(|e| AideMemoError::Internal(format!("create {}: {e}", log_path.display())))?;
    let log_err = log
        .try_clone()
        .map_err(|e| AideMemoError::Internal(format!("dup log fd: {e}")))?;

    let child = ProcCommand::new(&exe)
        .arg("--backend")
        .arg(&store_backend)
        .arg("mcp-serve")
        .arg("--port")
        .arg(port.to_string())
        .arg(&store)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .map_err(|e| AideMemoError::Internal(format!("spawn mcp-serve: {e}")))?;

    let pid = child.id();

    // Don't reap the child — we want it to survive this CLI exit.
    // (No std::mem::forget needed; dropping `child` doesn't kill it
    // on Unix because std::process::Child::drop is a no-op there.)
    let _ = child;

    // Wait up to 5s for /health to come up.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if probe_health(port) {
            let reg = DaemonRegistry {
                port,
                pid,
                store: store.clone(),
                backend: Some(daemon_backend.clone()),
                started_at: now_ms(),
            };
            save_registry(&reg)?;
            return Ok(format!(
                "aidememo daemon started (pid={pid}, port={port}, store={}, backend={}, log={})",
                store.display(),
                daemon_backend,
                log_path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(AideMemoError::Internal(format!(
        "daemon spawned (pid={pid}) but /health didn't respond within 5s — see {}",
        log_path.display()
    )))
}

fn stop_daemon() -> Result<String, AideMemoError> {
    let reg = load_registry()
        .ok_or_else(|| AideMemoError::Internal("no daemon registry — nothing to stop".into()))?;

    // SIGTERM via the kill(2) syscall. We avoid `std::process::Child`
    // because we don't own the original handle (the spawning CLI exited).
    #[cfg(unix)]
    {
        let pid = reg.pid as i32;
        // SAFETY: libc::kill is a syscall thin-wrapper. We're sending
        // SIGTERM to a PID we wrote into the registry ourselves.
        let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
        if rc != 0 {
            // ESRCH (no such process) is fine — daemon already gone.
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(AideMemoError::Internal(format!(
                    "kill({pid}, SIGTERM): {err}"
                )));
            }
        }
    }
    #[cfg(not(unix))]
    {
        return Err(AideMemoError::Internal(
            "aidememo daemon stop is only implemented on Unix (PR welcome)".into(),
        ));
    }

    delete_registry()?;
    Ok(format!("aidememo daemon stopped (pid was {})", reg.pid))
}

fn status_daemon(cli_store: PathBuf, cli_backend: String) -> Result<String, AideMemoError> {
    match load_registry() {
        None => Ok("aidememo daemon: not running (no registry at ~/.aidememo/daemon.json)".into()),
        Some(reg) => {
            let healthy = probe_health(reg.port);
            let match_str = if registry_matches_request(&reg, &cli_store, &cli_backend) {
                "matches CLI store/backend"
            } else if reg.store == cli_store {
                "same store but DIFFERENT/unknown backend (CLI will not auto-discover)"
            } else {
                "DIFFERENT from CLI store/backend (CLI will not auto-discover)"
            };
            Ok(format!(
                "aidememo daemon: pid={}, port={}, store={}, backend={}\n\
                 health: {}\n\
                 store match: {}",
                reg.pid,
                reg.port,
                reg.store.display(),
                reg.backend.as_deref().unwrap_or("unknown"),
                if healthy {
                    "OK (responding)"
                } else {
                    "STALE (no /health)"
                },
                match_str,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(store: &str, backend: Option<&str>) -> DaemonRegistry {
        DaemonRegistry {
            port: 3000,
            pid: 123,
            store: PathBuf::from(store),
            backend: backend.map(str::to_string),
            started_at: 0,
        }
    }

    #[test]
    fn registry_match_requires_store_and_backend() {
        let reg = registry("/tmp/wiki.sqlite", Some("sqlite"));

        assert!(registry_matches_request(
            &reg,
            Path::new("/tmp/wiki.sqlite"),
            "libsqlite"
        ));
        assert!(!registry_matches_request(
            &reg,
            Path::new("/tmp/wiki.sqlite"),
            "redb"
        ));
        assert!(!registry_matches_request(
            &reg,
            Path::new("/tmp/other.sqlite"),
            "sqlite"
        ));
    }

    #[test]
    fn legacy_registry_without_backend_does_not_auto_match() {
        let reg = registry("/tmp/wiki.sqlite", None);

        assert!(!registry_matches_request(
            &reg,
            Path::new("/tmp/wiki.sqlite"),
            "sqlite"
        ));
    }
}
