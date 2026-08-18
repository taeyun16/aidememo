//! Durable identity reservation for remote handoff sends.
//!
//! A transport can fail after the server committed a handoff but before the
//! caller observed the receipt. Keep one pending operation per sender/receiver
//! session route so an identical retry reuses the same handoff/context IDs.
//! Completed operations are replaceable, which still permits intentionally
//! sending the same assignment again later.

use aidememo_core::AideMemoError;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{path::PathBuf, time::Duration};

const STATE_PENDING: &str = "pending";
const STATE_COMMITTED: &str = "committed";

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

pub(crate) struct RemoteSendOperationStore {
    connection: Connection,
}

impl RemoteSendOperationStore {
    pub(crate) fn open_default() -> Result<Self, AideMemoError> {
        let home = std::env::var("HOME").map_err(|_| {
            AideMemoError::InvalidInput(
                "HOME env var not set — can't resolve the remote outbox database".to_owned(),
            )
        })?;
        let directory = PathBuf::from(home).join(".aidememo");
        std::fs::create_dir_all(&directory).map_err(|error| {
            AideMemoError::Internal(format!(
                "create remote outbox directory {}: {error}",
                directory.display()
            ))
        })?;
        Self::open(directory.join("remote-outbox.sqlite"))
    }

    fn open(path: PathBuf) -> Result<Self, AideMemoError> {
        let connection = Connection::open(&path).map_err(|error| {
            AideMemoError::Internal(format!(
                "open remote outbox database {}: {error}",
                path.display()
            ))
        })?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(sqlite_error("configure remote outbox busy timeout"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;\
                 CREATE TABLE IF NOT EXISTS remote_send_operations (\
                   intent_key TEXT PRIMARY KEY NOT NULL,\
                   profile_name TEXT NOT NULL,\
                   project_id TEXT NOT NULL,\
                   actor_id TEXT NOT NULL,\
                   payload_hash TEXT NOT NULL,\
                   operation_id TEXT NOT NULL,\
                   handoff_id TEXT NOT NULL,\
                   context_id TEXT NOT NULL,\
                   state TEXT NOT NULL CHECK (state IN ('pending', 'committed')),\
                   created_at_ms INTEGER NOT NULL,\
                   updated_at_ms INTEGER NOT NULL\
                 );",
            )
            .map_err(sqlite_error("initialize remote outbox schema"))?;
        Ok(Self { connection })
    }

    pub(crate) fn reserve_send(
        &mut self,
        intent_key: &str,
        payload_hash: &str,
        meta: SendOperationMeta<'_>,
    ) -> Result<ReservedSendOperation, AideMemoError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error("begin remote send reservation"))?;

        let existing = transaction
            .query_row(
                "SELECT profile_name, project_id, actor_id, payload_hash, operation_id,\
                        handoff_id, context_id, state\
                   FROM remote_send_operations WHERE intent_key = ?1",
                params![intent_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error("read remote send reservation"))?;

        if let Some((
            profile_name,
            project_id,
            actor_id,
            existing_payload_hash,
            operation_id,
            handoff_id,
            context_id,
            state,
        )) = existing
        {
            if profile_name != meta.profile_name
                || project_id != meta.project_id
                || actor_id != meta.actor_id
            {
                return Err(AideMemoError::InvalidInput(
                    "remote send operation identity collision detected".to_owned(),
                ));
            }
            if state == STATE_PENDING {
                if existing_payload_hash != payload_hash {
                    return Err(AideMemoError::InvalidInput(
                        "a pending remote send for this session route has different evidence; retry the original request or inspect the sender outbox before sending a changed assignment"
                            .to_owned(),
                    ));
                }
                transaction
                    .commit()
                    .map_err(sqlite_error("finish remote send recovery lookup"))?;
                return Ok(ReservedSendOperation {
                    operation_id,
                    handoff_id,
                    context_id,
                    reused_pending: true,
                });
            }
            if state != STATE_COMMITTED {
                return Err(AideMemoError::Internal(format!(
                    "remote send operation has unsupported state {state}"
                )));
            }
        }

        let now = now_ms()?;
        let operation_id = generated_id("operation");
        let handoff_id = generated_id("handoff");
        let context_id = generated_id("context");
        transaction
            .execute(
                "INSERT INTO remote_send_operations (\
                     intent_key, profile_name, project_id, actor_id, payload_hash,\
                     operation_id, handoff_id, context_id, state, created_at_ms, updated_at_ms\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)\
                 ON CONFLICT(intent_key) DO UPDATE SET\
                     profile_name = excluded.profile_name,\
                     project_id = excluded.project_id,\
                     actor_id = excluded.actor_id,\
                     payload_hash = excluded.payload_hash,\
                     operation_id = excluded.operation_id,\
                     handoff_id = excluded.handoff_id,\
                     context_id = excluded.context_id,\
                     state = excluded.state,\
                     created_at_ms = excluded.created_at_ms,\
                     updated_at_ms = excluded.updated_at_ms",
                params![
                    intent_key,
                    meta.profile_name,
                    meta.project_id,
                    meta.actor_id,
                    payload_hash,
                    operation_id,
                    handoff_id,
                    context_id,
                    STATE_PENDING,
                    now,
                ],
            )
            .map_err(sqlite_error("persist remote send reservation"))?;
        transaction
            .commit()
            .map_err(sqlite_error("commit remote send reservation"))?;

        Ok(ReservedSendOperation {
            operation_id,
            handoff_id,
            context_id,
            reused_pending: false,
        })
    }

    pub(crate) fn mark_committed(
        &mut self,
        intent_key: &str,
        operation_id: &str,
    ) -> Result<(), AideMemoError> {
        let changed = self
            .connection
            .execute(
                "UPDATE remote_send_operations\
                    SET state = ?1, updated_at_ms = ?2\
                  WHERE intent_key = ?3 AND operation_id = ?4 AND state = ?5",
                params![
                    STATE_COMMITTED,
                    now_ms()?,
                    intent_key,
                    operation_id,
                    STATE_PENDING
                ],
            )
            .map_err(sqlite_error("mark remote send committed"))?;
        if changed == 1 {
            Ok(())
        } else {
            Err(AideMemoError::Internal(
                "remote send reservation changed before commit acknowledgement".to_owned(),
            ))
        }
    }
}

fn generated_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::new())
}

fn now_ms() -> Result<i64, AideMemoError> {
    i64::try_from(aidememo_core::time::current_epoch_ms()).map_err(|_| {
        AideMemoError::Internal("current time exceeds SQLite INTEGER range".to_owned())
    })
}

fn sqlite_error(
    operation: &'static str,
) -> impl FnOnce(rusqlite::Error) -> AideMemoError {
    move |error| AideMemoError::Internal(format!("{operation}: {error}"))
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
        let directory = tempfile::tempdir().map_err(|error| {
            AideMemoError::Internal(format!("create test directory: {error}"))
        })?;
        let path = directory.path().join("outbox.sqlite");
        let first = {
            let mut store = RemoteSendOperationStore::open(path.clone())?;
            store.reserve_send("intent", "payload", meta())?
        };
        let second = RemoteSendOperationStore::open(path)?.reserve_send(
            "intent",
            "payload",
            meta(),
        )?;
        assert!(second.reused_pending);
        assert_eq!(second.operation_id, first.operation_id);
        assert_eq!(second.handoff_id, first.handoff_id);
        assert_eq!(second.context_id, first.context_id);
        Ok(())
    }

    #[test]
    fn changed_pending_payload_fails_closed() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir().map_err(|error| {
            AideMemoError::Internal(format!("create test directory: {error}"))
        })?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("outbox.sqlite"))?;
        store.reserve_send("intent", "payload-a", meta())?;
        let error = store
            .reserve_send("intent", "payload-b", meta())
            .expect_err("changed pending evidence must fail");
        assert!(error.to_string().contains("different evidence"));
        Ok(())
    }

    #[test]
    fn committed_intent_can_start_a_new_assignment() -> Result<(), AideMemoError> {
        let directory = tempfile::tempdir().map_err(|error| {
            AideMemoError::Internal(format!("create test directory: {error}"))
        })?;
        let mut store = RemoteSendOperationStore::open(directory.path().join("outbox.sqlite"))?;
        let first = store.reserve_send("intent", "payload", meta())?;
        store.mark_committed("intent", &first.operation_id)?;
        let second = store.reserve_send("intent", "payload", meta())?;
        assert!(!second.reused_pending);
        assert_ne!(second.operation_id, first.operation_id);
        assert_ne!(second.handoff_id, first.handoff_id);
        assert_ne!(second.context_id, first.context_id);
        Ok(())
    }
}
