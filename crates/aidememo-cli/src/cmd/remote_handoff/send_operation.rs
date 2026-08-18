//! Durable identity and replay data for remote handoff sends.
//!
//! A transport can fail after the server committed a handoff but before the
//! caller observed the receipt. Keep one immutable operation per local remote
//! profile/session route so an exact retry reuses the same handoff/context IDs.
//! Replay data, state, and the last publish error are stored without bearer
//! credentials so explicit recovery can happen after connectivity returns.

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
    pub(crate) profile_url: &'a str,
    pub(crate) profile_added_at: u64,
    pub(crate) project_id: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SendOperationState {
    Queued,
    Failed,
    Conflict,
    Committed,
}

impl SendOperationState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Failed => "failed",
            Self::Conflict => "conflict",
            Self::Committed => "committed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReservedSendOperation {
    pub(crate) operation_id: String,
    pub(crate) handoff_id: String,
    pub(crate) context_id: String,
    pub(crate) actor_id: Option<String>,
    pub(crate) reused_pending: bool,
    pub(crate) already_committed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SendReplayPlan {
    pub(crate) session_id: String,
    pub(crate) source_id: Option<String>,
    pub(crate) topic: String,
    pub(crate) to_actor: String,
    pub(crate) focus: Option<String>,
    pub(crate) done_when: Option<String>,
    pub(crate) content: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct QueuedSendOperation {
    pub(crate) intent_key: String,
    pub(crate) profile_name: String,
    pub(crate) profile_url: String,
    pub(crate) profile_added_at: u64,
    pub(crate) project_id: String,
    pub(crate) actor_id: Option<String>,
    pub(crate) operation_id: String,
    pub(crate) handoff_id: String,
    pub(crate) context_id: String,
    pub(crate) created_at_ms: u64,
    pub(crate) state: SendOperationState,
    pub(crate) last_error: Option<String>,
    pub(crate) plan: SendReplayPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSendOperation {
    profile_name: String,
    #[serde(default)]
    profile_url: String,
    #[serde(default)]
    profile_added_at: u64,
    project_id: String,
    #[serde(default)]
    actor_id: Option<String>,
    payload_hash: String,
    operation_id: String,
    handoff_id: String,
    context_id: String,
    created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSendStatus {
    state: SendOperationState,
    #[serde(default)]
    last_error: Option<String>,
    updated_at_ms: u64,
}

impl StoredSendStatus {
    fn queued() -> Self {
        Self {
            state: SendOperationState::Queued,
            last_error: None,
            updated_at_ms: aidememo_core::time::current_epoch_ms(),
        }
    }
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

    pub(crate) fn open(directory: PathBuf) -> Result<Self, AideMemoError> {
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
                fs::remove_dir_all(&intent_directory).map_err(|error| {
                    io_error("remove incomplete remote send reservation", error)
                })?;
            } else {
                let existing = read_json::<StoredSendOperation>(&operation_path)?;
                ensure_profile(&existing, &meta)?;
                let committed = intent_directory.join("committed").exists();
                if existing.payload_hash == payload_hash {
                    return Ok(ReservedSendOperation {
                        operation_id: existing.operation_id,
                        handoff_id: existing.handoff_id,
                        context_id: existing.context_id,
                        actor_id: existing.actor_id,
                        reused_pending: true,
                        already_committed: committed,
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
            profile_url: meta.profile_url.to_owned(),
            profile_added_at: meta.profile_added_at,
            project_id: meta.project_id.to_owned(),
            actor_id: None,
            payload_hash: payload_hash.to_owned(),
            operation_id: generated_id("operation"),
            handoff_id: generated_id("handoff"),
            context_id: generated_id("context"),
            created_at_ms: aidememo_core::time::current_epoch_ms(),
        };
        write_json(&intent_directory.join("operation.json"), &operation)?;
        write_json(
            &intent_directory.join("status.json"),
            &StoredSendStatus::queued(),
        )?;

        Ok(ReservedSendOperation {
            operation_id: operation.operation_id,
            handoff_id: operation.handoff_id,
            context_id: operation.context_id,
            actor_id: None,
            reused_pending: false,
            already_committed: false,
        })
    }

    pub(crate) fn store_replay_plan(
        &mut self,
        intent_key: &str,
        operation_id: &str,
        plan: &SendReplayPlan,
    ) -> Result<(), AideMemoError> {
        validate_key(intent_key)?;
        let _lock = self.acquire_lock(intent_key)?;
        let intent_directory = self.directory.join(intent_key);
        let operation = read_json::<StoredSendOperation>(&intent_directory.join("operation.json"))?;
        ensure_operation_id(&operation, operation_id, "replay data was stored")?;
        let path = intent_directory.join("replay.json");
        if path.exists() {
            let existing = read_json::<SendReplayPlan>(&path)?;
            if existing == *plan {
                return Ok(());
            }
            return Err(AideMemoError::InvalidInput(
                "remote send replay data changed for an existing operation".to_owned(),
            ));
        }
        write_json(&path, plan)
    }

    pub(crate) fn bind_actor(
        &mut self,
        intent_key: &str,
        operation_id: &str,
        meta: SendOperationMeta<'_>,
        actor_id: &str,
    ) -> Result<(), AideMemoError> {
        validate_key(intent_key)?;
        validate_actor_id(actor_id)?;
        let _lock = self.acquire_lock(intent_key)?;
        let operation_path = self.directory.join(intent_key).join("operation.json");
        let mut operation = read_json::<StoredSendOperation>(&operation_path)?;
        ensure_operation_id(&operation, operation_id, "actor identity was bound")?;
        ensure_profile(&operation, &meta)?;
        if operation
            .actor_id
            .as_deref()
            .is_some_and(|existing| existing != actor_id)
        {
            return Err(AideMemoError::InvalidInput(
                "remote send operation is already bound to a different authenticated actor"
                    .to_owned(),
            ));
        }
        let changed = operation.actor_id.as_deref() != Some(actor_id)
            || operation.profile_url.is_empty()
            || operation.profile_added_at == 0;
        operation.actor_id = Some(actor_id.to_owned());
        if operation.profile_url.is_empty() {
            operation.profile_url = meta.profile_url.to_owned();
        }
        if operation.profile_added_at == 0 {
            operation.profile_added_at = meta.profile_added_at;
        }
        if changed {
            replace_json(&operation_path, &operation)?;
        }
        Ok(())
    }

    pub(crate) fn mark_queued(
        &mut self,
        intent_key: &str,
        operation_id: &str,
    ) -> Result<(), AideMemoError> {
        self.update_status(intent_key, operation_id, SendOperationState::Queued, None)
    }

    pub(crate) fn record_failure(
        &mut self,
        intent_key: &str,
        operation_id: &str,
        state: SendOperationState,
        message: &str,
    ) -> Result<(), AideMemoError> {
        if !matches!(
            state,
            SendOperationState::Failed | SendOperationState::Conflict
        ) {
            return Err(AideMemoError::Internal(
                "remote send failure state must be failed or conflict".to_owned(),
            ));
        }
        self.update_status(intent_key, operation_id, state, Some(message.to_owned()))
    }

    pub(crate) fn pending(
        &self,
        profile_name: Option<&str>,
    ) -> Result<Vec<QueuedSendOperation>, AideMemoError> {
        let mut queued = Vec::new();
        for entry in fs::read_dir(&self.directory)
            .map_err(|error| io_error("list remote send outbox", error))?
        {
            let entry = entry.map_err(|error| io_error("read remote send outbox entry", error))?;
            let file_type = entry
                .file_type()
                .map_err(|error| io_error("inspect remote send outbox entry", error))?;
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".lock") || name.contains(".done.") {
                continue;
            }
            let path = entry.path();
            if path.join("committed").exists() {
                continue;
            }
            let operation_path = path.join("operation.json");
            let replay_path = path.join("replay.json");
            if !operation_path.exists() || !replay_path.exists() {
                continue;
            }
            let operation = read_json::<StoredSendOperation>(&operation_path)?;
            if profile_name.is_some_and(|name| operation.profile_name != name) {
                continue;
            }
            let plan = read_json::<SendReplayPlan>(&replay_path)?;
            let status_path = path.join("status.json");
            let status = if status_path.exists() {
                read_json::<StoredSendStatus>(&status_path)?
            } else {
                StoredSendStatus::queued()
            };
            if status.state == SendOperationState::Committed {
                continue;
            }
            queued.push(QueuedSendOperation {
                intent_key: name,
                profile_name: operation.profile_name,
                profile_url: operation.profile_url,
                profile_added_at: operation.profile_added_at,
                project_id: operation.project_id,
                actor_id: operation.actor_id,
                operation_id: operation.operation_id,
                handoff_id: operation.handoff_id,
                context_id: operation.context_id,
                created_at_ms: operation.created_at_ms,
                state: status.state,
                last_error: status.last_error,
                plan,
            });
        }
        queued.sort_by_key(|entry| entry.created_at_ms);
        Ok(queued)
    }

    pub(crate) fn mark_committed(
        &mut self,
        intent_key: &str,
        operation_id: &str,
    ) -> Result<(), AideMemoError> {
        validate_key(intent_key)?;
        let _lock = self.acquire_lock(intent_key)?;
        let intent_directory = self.directory.join(intent_key);
        let operation = read_json::<StoredSendOperation>(&intent_directory.join("operation.json"))?;
        ensure_operation_id(
            &operation,
            operation_id,
            "commit acknowledgement was recorded",
        )?;
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
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let existing = fs::read_to_string(&marker)
                    .map_err(|read_error| io_error("read remote send commit marker", read_error))?;
                if existing != operation_id {
                    return Err(AideMemoError::Internal(
                        "remote send commit marker belongs to a different operation".to_owned(),
                    ));
                }
            }
            Err(error) => return Err(io_error("create remote send commit marker", error)),
        }
        write_status(
            &intent_directory.join("status.json"),
            &StoredSendStatus {
                state: SendOperationState::Committed,
                last_error: None,
                updated_at_ms: aidememo_core::time::current_epoch_ms(),
            },
        )
    }

    fn update_status(
        &mut self,
        intent_key: &str,
        operation_id: &str,
        state: SendOperationState,
        last_error: Option<String>,
    ) -> Result<(), AideMemoError> {
        validate_key(intent_key)?;
        let _lock = self.acquire_lock(intent_key)?;
        let intent_directory = self.directory.join(intent_key);
        let operation = read_json::<StoredSendOperation>(&intent_directory.join("operation.json"))?;
        ensure_operation_id(&operation, operation_id, "outbox status was updated")?;
        write_status(
            &intent_directory.join("status.json"),
            &StoredSendStatus {
                state,
                last_error,
                updated_at_ms: aidememo_core::time::current_epoch_ms(),
            },
        )
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

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AideMemoError> {
    let bytes =
        fs::read(path).map_err(|error| io_error("read remote send outbox record", error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        AideMemoError::Internal(format!(
            "decode remote send outbox record {}: {error}",
            path.display()
        ))
    })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AideMemoError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| AideMemoError::Serialize {
        context: "remote send outbox record".to_owned(),
        source: error,
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_error("create remote send outbox record", error))?;
    file.write_all(&bytes)
        .map_err(|error| io_error("write remote send outbox record", error))?;
    file.sync_all()
        .map_err(|error| io_error("sync remote send outbox record", error))?;
    Ok(())
}

fn replace_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AideMemoError> {
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("outbox"),
        ulid::Ulid::new()
    ));
    write_json(&temporary, value)?;
    fs::rename(&temporary, path)
        .map_err(|error| io_error("replace remote send outbox record", error))
}

fn write_status(path: &Path, status: &StoredSendStatus) -> Result<(), AideMemoError> {
    if path.exists() {
        replace_json(path, status)
    } else {
        write_json(path, status)
    }
}

fn ensure_profile(
    operation: &StoredSendOperation,
    meta: &SendOperationMeta<'_>,
) -> Result<(), AideMemoError> {
    let profile_matches = operation.profile_name == meta.profile_name
        && operation.project_id == meta.project_id
        && (operation.profile_url.is_empty() || operation.profile_url == meta.profile_url)
        && (operation.profile_added_at == 0 || operation.profile_added_at == meta.profile_added_at);
    if profile_matches {
        Ok(())
    } else {
        Err(AideMemoError::InvalidInput(
            "remote send operation profile identity changed; inspect the outbox before publishing"
                .to_owned(),
        ))
    }
}

fn ensure_operation_id(
    operation: &StoredSendOperation,
    operation_id: &str,
    action: &str,
) -> Result<(), AideMemoError> {
    if operation.operation_id == operation_id {
        Ok(())
    } else {
        Err(AideMemoError::Internal(format!(
            "remote send reservation changed before {action}"
        )))
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

fn validate_actor_id(actor_id: &str) -> Result<(), AideMemoError> {
    if actor_id.is_empty()
        || actor_id.len() > 128
        || !actor_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@')
        })
    {
        return Err(AideMemoError::InvalidInput(
            "authenticated actor is not a valid remote identifier".to_owned(),
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
            profile_url: "http://server.test",
            profile_added_at: 42,
            project_id: "project",
        }
    }

    fn plan(content: &str) -> SendReplayPlan {
        SendReplayPlan {
            session_id: "session-1".to_owned(),
            source_id: Some("source-1".to_owned()),
            topic: "topic".to_owned(),
            to_actor: "codex-p2".to_owned(),
            focus: Some("review".to_owned()),
            done_when: Some("tests pass".to_owned()),
            content: content.to_owned(),
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
    fn replay_plan_survives_reopen_and_lists_as_queued() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let path = directory.path().join("send");
        let operation = {
            let mut store = RemoteSendOperationStore::open(path.clone())?;
            let operation = store.reserve_send("intent", "payload", meta())?;
            store.store_replay_plan("intent", &operation.operation_id, &plan("packet"))?;
            operation
        };
        let pending = RemoteSendOperationStore::open(path)?.pending(Some("codex-p1"))?;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation_id, operation.operation_id);
        assert_eq!(pending[0].state, SendOperationState::Queued);
        assert_eq!(pending[0].profile_url, "http://server.test");
        assert_eq!(pending[0].plan.content, "packet");
        Ok(())
    }

    #[test]
    fn actor_binding_and_last_error_survive_reopen() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let path = directory.path().join("send");
        let operation = {
            let mut store = RemoteSendOperationStore::open(path.clone())?;
            let operation = store.reserve_send("intent", "payload", meta())?;
            store.store_replay_plan("intent", &operation.operation_id, &plan("packet"))?;
            store.bind_actor("intent", &operation.operation_id, meta(), "actor-live")?;
            store.record_failure(
                "intent",
                &operation.operation_id,
                SendOperationState::Failed,
                "connection refused",
            )?;
            operation
        };
        let pending = RemoteSendOperationStore::open(path)?.pending(None)?;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation_id, operation.operation_id);
        assert_eq!(pending[0].actor_id.as_deref(), Some("actor-live"));
        assert_eq!(pending[0].state, SendOperationState::Failed);
        assert_eq!(pending[0].last_error.as_deref(), Some("connection refused"));
        Ok(())
    }

    #[test]
    fn changed_profile_generation_fails_closed() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("send"))?;
        let operation = store.reserve_send("intent", "payload", meta())?;
        let changed = SendOperationMeta {
            profile_added_at: 43,
            ..meta()
        };
        let error = store
            .bind_actor("intent", &operation.operation_id, changed, "actor-live")
            .expect_err("profile replacement must fail closed");
        assert!(error.to_string().contains("profile identity changed"));
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
    fn changed_replay_plan_fails_closed() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("send"))?;
        let operation = store.reserve_send("intent", "payload", meta())?;
        store.store_replay_plan("intent", &operation.operation_id, &plan("packet-a"))?;
        let error = store
            .store_replay_plan("intent", &operation.operation_id, &plan("packet-b"))
            .expect_err("changed replay data must fail");
        assert!(error.to_string().contains("replay data changed"));
        Ok(())
    }

    #[test]
    fn committed_exact_send_is_recovered_but_not_pending() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let path = directory.path().join("send");
        let mut store = RemoteSendOperationStore::open(path)?;
        let first = store.reserve_send("intent", "payload", meta())?;
        store.store_replay_plan("intent", &first.operation_id, &plan("packet"))?;
        store.bind_actor("intent", &first.operation_id, meta(), "actor-live")?;
        store.mark_committed("intent", &first.operation_id)?;
        let second = store.reserve_send("intent", "payload", meta())?;
        assert!(second.reused_pending);
        assert_eq!(second.operation_id, first.operation_id);
        assert!(store.pending(None)?.is_empty());
        Ok(())
    }

    #[test]
    fn changed_committed_send_starts_a_new_assignment() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir()
            .map_err(|error| AideMemoError::Internal(format!("create test directory: {error}")))?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("send"))?;
        let first = store.reserve_send("intent", "payload-a", meta())?;
        store.store_replay_plan("intent", &first.operation_id, &plan("packet-a"))?;
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
