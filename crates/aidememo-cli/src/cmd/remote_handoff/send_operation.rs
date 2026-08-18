//! Durable identity reservation for remote handoff sends.
//!
//! A transport can fail after the server committed a handoff but before the
//! caller observed the receipt. Keep one immutable operation per
//! sender/receiver session route so an exact retry reuses the same
//! handoff/context IDs even after the server acknowledgement was recorded. A
//! tiny directory lock serializes competing CLI processes without adding
//! another storage dependency to the public CLI.
//!
//! The operation record is immutable. Successful acknowledgement creates a
//! separate marker. A changed assignment may replace a completed operation,
//! but an exact replay always recovers the original IDs.

use aidememo_core::AideMemoError;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime},
};

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const STALE_LOCK_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub(crate) struct SendOperationMeta<'a> {
    pub(crate) profile_name: &'a str,
    pub(crate) project_id: &'a str,
    pub(crate) actor_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReservedSendOperation {
    pub(crate) operation_id: String,
    pub(crate) handoff_id: String,
    pub(crate) context_id: String,
    pub(crate) reused_pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSendOperation {
    profile_name: String,
    project_id: String,
    actor_id: String,
    payload_hash: String,
    operation_id: String,
    handoff_id: String,
    context_id: String,
    created_at_ms: u64,
}

pub(crate) struct RemoteSendOperationStore {
    directory: PathBuf,
}

impl RemoteSendOperationStore {
    pub(crate) fn open_default() -> Result<Self, AideMemoError> {
        let home = std::env::var("HOME").map_err(|_| {
            AideMemoError::InvalidInput(
                "HOME env var not set — can't resolve the remote outbox directory".to_owned(),
            )
        })?;
        Self::open(PathBuf::from(home).join(".aidememo/remote-outbox/send"))
    }

    fn open(directory: PathBuf) -> Result<Self, AideMemoError> {
        fs::create_dir_all(&directory).map_err(|error| {
            AideMemoError::Internal(format!(
                "create remote outbox directory {}: {error}",
                directory.display()
            ))
        })?;
        Ok(Self { directory })
    }

    pub(crate) fn reserve_send(
        &mut self,
        intent_key: &str,
        payload_hash: &str,
        meta: SendOperationMeta<'_>,
    ) -> Result<ReservedSendOperation, AideMemoError> {
        validate_key(intent_key)?;
        let _lock = self.acquire_lock(intent_key)?;
        let intent_directory = self.directory.join(intent_key);

        if intent_directory.exists() {
            let operation_path = intent_directory.join("operation.json");
            if !operation_path.exists() {
                // Reservation never became durable, so no caller could have
                // safely started the remote mutation using its IDs.
                fs::remove_dir_all(&intent_directory).map_err(|error| {
                    io_error("remove incomplete remote send reservation", error)
                })?;
            } else {
                let existing = read_operation(&operation_path)?;
                ensure_identity(&existing, &meta)?;
                let committed = intent_directory.join("committed").exists();
                if existing.payload_hash == payload_hash {
                    return Ok(ReservedSendOperation {
                        operation_id: existing.operation_id,
                        handoff_id: existing.handoff_id,
                        context_id: existing.context_id,
                        reused_pending: true,
                    });
                }
                if !committed {
                    return Err(AideMemoError::InvalidInput(
                        "a pending remote send for this session route has different evidence; retry the original request or inspect the sender outbox before sending a changed assignment"
                            .to_owned(),
                    ));
                }

                let archive = self
                    .directory
                    .join(format!("{intent_key}.done.{}", existing.operation_id));
                if archive.exists() {
                    fs::remove_dir_all(&archive)
                        .map_err(|error| io_error("remove prior remote send archive", error))?;
                }
                fs::rename(&intent_directory, &archive).map_err(|error| {
                    io_error("archive completed remote send reservation", error)
                })?;
            }
        }

        fs::create_dir(&intent_directory)
            .map_err(|error| io_error("create remote send reservation", error))?;
        let operation = StoredSendOperation {
            profile_name: meta.profile_name.to_owned(),
            project_id: meta.project_id.to_owned(),
            actor_id: meta.actor_id.to_owned(),
            payload_hash: payload_hash.to_owned(),
            operation_id: generated_id("operation"),
            handoff_id: generated_id("handoff"),
            context_id: generated_id("context"),
            created_at_ms: aidememo_core::time::current_epoch_ms(),
        };
        write_operation(&intent_directory.join("operation.json"), &operation)?;

        Ok(ReservedSendOperation {
            operation_id: operation.operation_id,
            handoff_id: operation.handoff_id,
            context_id: operation.context_id,
            reused_pending: false,
        })
    }

    pub(crate) fn mark_committed(
        &mut self,
        intent_key: &str,
        operation_id: &str,
    ) -> Result<(), AideMemoError> {
        validate_key(intent_key)?;
        let _lock = self.acquire_lock(intent_key)?;
        let intent_directory = self.directory.join(intent_key);
        let operation = read_operation(&intent_directory.join("operation.json"))?;
        if operation.operation_id != operation_id {
            return Err(AideMemoError::Internal(
                "remote send reservation changed before commit acknowledgement".to_owned(),
            ));
        }
        let marker = intent_directory.join("committed");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
        {
            Ok(mut file) => {
                file.write_all(operation_id.as_bytes())
                    .map_err(|error| io_error("write remote send commit marker", error))?;
                file.sync_all()
                    .map_err(|error| io_error("sync remote send commit marker", error))?;
                Ok(())
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let existing = fs::read_to_string(&marker)
                    .map_err(|read_error| io_error("read remote send commit marker", read_error))?;
                if existing == operation_id {
                    Ok(())
                } else {
                    Err(AideMemoError::Internal(
                        "remote send commit marker belongs to a different operation".to_owned(),
                    ))
                }
            }
            Err(error) => Err(io_error("create remote send commit marker", error)),
        }
    }

    fn acquire_lock(&self, intent_key: &str) -> Result<IntentLock, AideMemoError> {
        let path = self.directory.join(format!("{intent_key}.lock"));
        let started = Instant::now();
        loop {
            match fs::create_dir(&path) {
                Ok(()) => return Ok(IntentLock { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        match fs::remove_dir(&path) {
                            Ok(()) => continue,
                            Err(remove_error) if remove_error.kind() == ErrorKind::NotFound => {
                                continue;
                            }
                            Err(_) => {}
                        }
                    }
                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err(AideMemoError::Internal(format!(
                            "timed out waiting for remote send reservation lock {}",
                            path.display()
                        )));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(io_error("create remote send reservation lock", error));
                }
            }
        }
    }
}

struct IntentLock {
    path: PathBuf,
}

impl Drop for IntentLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

fn read_operation(path: &Path) -> Result<StoredSendOperation, AideMemoError> {
    let bytes = fs::read(path).map_err(|error| io_error("read remote send reservation", error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        AideMemoError::Internal(format!(
            "decode remote send reservation {}: {error}",
            path.display()
        ))
    })
}

fn write_operation(path: &Path, operation: &StoredSendOperation) -> Result<(), AideMemoError> {
    let bytes = serde_json::to_vec_pretty(operation).map_err(|error| AideMemoError::Serialize {
        context: "remote send reservation".to_owned(),
        source: error,
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_error("create remote send reservation record", error))?;
    file.write_all(&bytes)
        .map_err(|error| io_error("write remote send reservation record", error))?;
    file.sync_all()
        .map_err(|error| io_error("sync remote send reservation record", error))?;
    Ok(())
}

fn ensure_identity(
    operation: &StoredSendOperation,
    meta: &SendOperationMeta<'_>,
) -> Result<(), AideMemoError> {
    if operation.profile_name == meta.profile_name
        && operation.project_id == meta.project_id
        && operation.actor_id == meta.actor_id
    {
        Ok(())
    } else {
        Err(AideMemoError::InvalidInput(
            "remote send operation identity collision detected".to_owned(),
        ))
    }
}

fn validate_key(intent_key: &str) -> Result<(), AideMemoError> {
    if intent_key.is_empty()
        || intent_key.len() > 128
        || !intent_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AideMemoError::Internal(
            "remote send intent key is not filesystem-safe".to_owned(),
        ));
    }
    Ok(())
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= STALE_LOCK_AFTER)
}

fn generated_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::new())
}

fn io_error(operation: &str, error: std::io::Error) -> AideMemoError {
    AideMemoError::Internal(format!("{operation}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> SendOperationMeta<'static> {
        SendOperationMeta {
            profile_name: "codex-p1",
            project_id: "project",
            actor_id: "codex-p1",
        }
    }

    #[test]
    fn pending_send_is_reused_after_reopen() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let path = directory.path().join("send");
        let first = {
            let mut store = RemoteSendOperationStore::open(path.clone())?;
            store.reserve_send("intent", "payload", meta())?
        };
        let second =
            RemoteSendOperationStore::open(path)?.reserve_send("intent", "payload", meta())?;
        assert!(second.reused_pending);
        assert_eq!(second.operation_id, first.operation_id);
        assert_eq!(second.handoff_id, first.handoff_id);
        assert_eq!(second.context_id, first.context_id);
        Ok(())
    }

    #[test]
    fn changed_pending_payload_fails_closed() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("send"))?;
        store.reserve_send("intent", "payload-a", meta())?;
        let error = store
            .reserve_send("intent", "payload-b", meta())
            .expect_err("changed pending evidence must fail");
        assert!(error.to_string().contains("different evidence"));
        Ok(())
    }

    #[test]
    fn committed_exact_send_is_recovered() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("send"))?;
        let first = store.reserve_send("intent", "payload", meta())?;
        store.mark_committed("intent", &first.operation_id)?;
        let second = store.reserve_send("intent", "payload", meta())?;
        assert!(second.reused_pending);
        assert_eq!(second.operation_id, first.operation_id);
        assert_eq!(second.handoff_id, first.handoff_id);
        assert_eq!(second.context_id, first.context_id);
        Ok(())
    }

    #[test]
    fn changed_committed_send_starts_a_new_assignment() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("send"))?;
        let first = store.reserve_send("intent", "payload-a", meta())?;
        store.mark_committed("intent", &first.operation_id)?;
        let second = store.reserve_send("intent", "payload-b", meta())?;
        assert!(!second.reused_pending);
        assert_ne!(second.operation_id, first.operation_id);
        assert_ne!(second.handoff_id, first.handoff_id);
        assert_ne!(second.context_id, first.context_id);
        Ok(())
    }

    #[test]
    fn commit_marker_is_idempotent_for_same_operation() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("send"))?;
        let operation = store.reserve_send("intent", "payload", meta())?;
        store.mark_committed("intent", &operation.operation_id)?;
        store.mark_committed("intent", &operation.operation_id)?;
        Ok(())
    }
}
