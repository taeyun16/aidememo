//! `wg daemon` — manage a long-lived `wg mcp-serve` background process.
//!
//! Why this exists: a fresh `wg search` CLI spawn pays ~70 ms (BM25)
//! to ~1 s (hybrid + cold model load) every invocation. A daemon
//! amortises both the redb open and the embedding model load so the
//! same query takes ~5 ms (BM25) or ~43 ms (HNSW) — see
//! `bench/beads-vs-wg/scenario_5_daemon_warm.py`.
//!
//! Subcommands
//! -----------
//!   wg daemon start [--port N] [--store PATH]
//!       Spawn `wg mcp-serve` in the background; record port + PID +
//!       store path in ~/.wg/daemon.json. Idempotent: if a healthy
//!       daemon is already registered, return its info.
//!
//!   wg daemon stop
//!       Read the registry, send SIGTERM to the recorded PID, delete
//!       the registry file.
//!
//!   wg daemon status
//!       Show what (if anything) the registry points at and whether
//!       the daemon answers /health.
//!
//! Discovery
//! ---------
//! Read CLI commands (`wg search`, `wg query`, …) consult the
//! registry first. If a healthy daemon is registered AND its store
//! matches the resolved store path, the CLI dispatches over HTTP
//! instead of opening redb in-process. Set `WG_NO_DAEMON=1` to opt
//! out for one invocation. See `daemon::registered_endpoint()`.

use bpaf::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command as ProcCommand, Stdio};
use std::time::{Duration, Instant};
use wg_core::{Config, WgError};

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
    .help("Spawn `wg mcp-serve` in the background");

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

pub fn registry_path() -> Result<PathBuf, WgError> {
    let home = dirs::home_dir().ok_or_else(|| WgError::Internal("no home directory".into()))?;
    Ok(home.join(".wg").join("daemon.json"))
}

fn load_registry() -> Option<DaemonRegistry> {
    let path = registry_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_registry(reg: &DaemonRegistry) -> Result<(), WgError> {
    let path = registry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| WgError::Internal(format!("create {}: {e}", parent.display())))?;
    }
    let text = serde_json::to_string_pretty(reg)
        .map_err(|e| WgError::Internal(format!("serialize registry: {e}")))?;
    std::fs::write(&path, text).map_err(|e| WgError::Internal(format!("write registry: {e}")))?;
    Ok(())
}

fn delete_registry() -> Result<(), WgError> {
    let path = registry_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| WgError::Internal(format!("remove registry: {e}")))?;
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
/// doesn't match the requested store. `WG_NO_DAEMON=1` short-circuits
/// to None.
pub fn registered_endpoint(want_store: &std::path::Path) -> Option<String> {
    if std::env::var("WG_NO_DAEMON").is_ok() {
        return None;
    }
    daemon_for_hint(want_store).map(|reg| format!("http://127.0.0.1:{}", reg.port))
}

/// Discovery info used by error-hint code on the failure path.
/// Ignores `WG_NO_DAEMON` — when the user opts out of dispatch they
/// still need the diagnostic ("daemon is the one holding the lock").
/// Returns the registry only if the recorded store matches AND
/// /health responds. Use `registry_state` if you also want to
/// distinguish "no registry" from "stale registry".
pub fn daemon_for_hint(want_store: &std::path::Path) -> Option<DaemonRegistry> {
    let reg = load_registry()?;
    if reg.store != want_store {
        return None;
    }
    if !probe_health(reg.port) {
        return None;
    }
    Some(reg)
}

/// Three-way classification for error-hint code:
///   - `Healthy(reg)`  : registry exists, store matches, /health OK
///   - `StaleRegistry` : registry exists but isn't responding
///   - `None`          : no registry on disk
pub enum RegistryState {
    Healthy(DaemonRegistry),
    StaleRegistry,
    None,
}

pub fn registry_state(want_store: &std::path::Path) -> RegistryState {
    let Some(reg) = load_registry() else {
        return RegistryState::None;
    };
    if reg.store == want_store && probe_health(reg.port) {
        RegistryState::Healthy(reg)
    } else {
        RegistryState::StaleRegistry
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

pub fn run_daemon(sub: DaemonSub, store_path: PathBuf) -> Result<String, WgError> {
    match sub {
        DaemonSub::Start { port, store } => start_daemon(port, store.unwrap_or(store_path)),
        DaemonSub::Stop => stop_daemon(),
        DaemonSub::Status => status_daemon(),
    }
}

fn start_daemon(port: Option<u16>, store: PathBuf) -> Result<String, WgError> {
    // Fast path — already running and healthy.
    if let Some(reg) = load_registry() {
        if reg.store == store && probe_health(reg.port) {
            return Ok(format!(
                "wg daemon already running (pid={}, port={}, store={})",
                reg.pid,
                reg.port,
                reg.store.display()
            ));
        }
        // Stale registry. Best-effort cleanup.
        let _ = delete_registry();
    }

    let port = port
        .or_else(pick_free_port)
        .ok_or_else(|| WgError::Internal("no free port available".into()))?;

    // Spawn `wg mcp-serve --port PORT <store>` detached. We use the
    // current binary so a `cargo run` build path works just as well
    // as an installed `wg`.
    let exe =
        std::env::current_exe().map_err(|e| WgError::Internal(format!("current_exe: {e}")))?;
    let log_path = registry_path()?.with_file_name("daemon.log");
    let log = std::fs::File::create(&log_path)
        .map_err(|e| WgError::Internal(format!("create {}: {e}", log_path.display())))?;
    let log_err = log
        .try_clone()
        .map_err(|e| WgError::Internal(format!("dup log fd: {e}")))?;

    let child = ProcCommand::new(&exe)
        .arg("mcp-serve")
        .arg("--port")
        .arg(port.to_string())
        .arg(&store)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .map_err(|e| WgError::Internal(format!("spawn mcp-serve: {e}")))?;

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
                started_at: now_ms(),
            };
            save_registry(&reg)?;
            return Ok(format!(
                "wg daemon started (pid={pid}, port={port}, store={}, log={})",
                store.display(),
                log_path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(WgError::Internal(format!(
        "daemon spawned (pid={pid}) but /health didn't respond within 5s — see {}",
        log_path.display()
    )))
}

fn stop_daemon() -> Result<String, WgError> {
    let reg = load_registry()
        .ok_or_else(|| WgError::Internal("no daemon registry — nothing to stop".into()))?;

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
                return Err(WgError::Internal(format!("kill({pid}, SIGTERM): {err}")));
            }
        }
    }
    #[cfg(not(unix))]
    {
        return Err(WgError::Internal(
            "wg daemon stop is only implemented on Unix (PR welcome)".into(),
        ));
    }

    delete_registry()?;
    Ok(format!("wg daemon stopped (pid was {})", reg.pid))
}

fn status_daemon() -> Result<String, WgError> {
    match load_registry() {
        None => Ok("wg daemon: not running (no registry at ~/.wg/daemon.json)".into()),
        Some(reg) => {
            let healthy = probe_health(reg.port);
            let config = Config::load().unwrap_or_default();
            let cli_store = PathBuf::from(&config.store.path);
            let match_str = if reg.store == cli_store {
                "matches CLI default store"
            } else {
                "DIFFERENT from CLI default store (CLI may not auto-discover)"
            };
            Ok(format!(
                "wg daemon: pid={}, port={}, store={}\n\
                 health: {}\n\
                 store match: {}",
                reg.pid,
                reg.port,
                reg.store.display(),
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
