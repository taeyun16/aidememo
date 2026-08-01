//! `aidememo replica` — exact-read cache for an authenticated remote SSOT.

use aidememo_client::{HttpReplicaClient, RemoteProfile, ReplicaStore, pull_to_current};
use aidememo_core::AideMemoError;
use aidememo_domain::{ProjectId, ResourceId, ResourceKind, ResourceRef, ResourceState};
use bpaf::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::cmd::{Command, auth};

#[derive(Debug, Clone)]
pub enum ReplicaSub {
    Pull {
        remote_profile: String,
        replica_path: Option<PathBuf>,
        limit: Option<usize>,
    },
    Status {
        replica_path: Option<PathBuf>,
    },
    Get {
        replica_path: Option<PathBuf>,
        resource_kind: String,
        resource_id: String,
    },
    Reset {
        replica_path: Option<PathBuf>,
        force: bool,
    },
}

pub fn replica_command() -> impl Parser<Command> {
    let remote_profile = long("remote-profile")
        .help("Named authenticated SSOT profile")
        .argument::<String>("NAME");
    let replica_path_parser = || {
        long("replica-path")
            .help("Exact-read replica database (default: <store>.replica.sqlite)")
            .argument::<PathBuf>("PATH")
            .optional()
    };
    let limit = long("limit")
        .help("Change-feed entries per server batch (1..=1000, default 100)")
        .argument::<usize>("N")
        .optional();
    let replica_path = replica_path_parser();
    let pull = construct!(ReplicaSub::Pull {
        remote_profile,
        replica_path,
        limit,
    })
    .to_options()
    .command("pull")
    .help("Bootstrap or incrementally catch up a remote exact-read replica");

    let replica_path = replica_path_parser();
    let status = construct!(ReplicaSub::Status { replica_path })
        .to_options()
        .command("status")
        .help("Inspect the durable local cursor without contacting the server");

    let resource_kind = positional::<String>("KIND")
        .help("Canonical resource kind, such as session, fact, or handoff");
    let resource_id = positional::<String>("ID").help("Canonical resource ID");
    let replica_path = replica_path_parser();
    let get = construct!(ReplicaSub::Get {
        replica_path,
        resource_kind,
        resource_id,
    })
    .to_options()
    .command("get")
    .help("Read one cached canonical resource while offline");

    let force = long("force")
        .help("Confirm removal of cached scope, cursor, resources, and tombstones")
        .switch();
    let replica_path = replica_path_parser();
    let reset = construct!(ReplicaSub::Reset {
        replica_path,
        force,
    })
    .to_options()
    .command("reset")
    .help("Explicitly clear a replica after project restore or reassignment");

    construct!([pull, status, get, reset])
        .map(Command::Replica)
        .to_options()
        .command("replica")
        .help("Authenticated remote SSOT exact-read replica lifecycle")
}

pub fn run_replica(
    store_path: &Path,
    sub: ReplicaSub,
    json_output: bool,
) -> Result<String, AideMemoError> {
    match sub {
        ReplicaSub::Pull {
            remote_profile,
            replica_path,
            limit,
        } => {
            let path = resolve_replica_path(store_path, replica_path);
            let stored = auth::load_remote_profile(&remote_profile)?;
            let project_id = ProjectId::try_from(stored.project_id.as_str())
                .map_err(|error| AideMemoError::InvalidInput(error.to_string()))?;
            let profile =
                RemoteProfile::new(stored.url, project_id, stored.token).map_err(client_error)?;
            let client = HttpReplicaClient::new(profile);
            let mut store = ReplicaStore::open(&path).map_err(client_error)?;
            let report =
                pull_to_current(&client, &mut store, limit.unwrap_or(100)).map_err(client_error)?;
            if json_output {
                return serde_json::to_string_pretty(&json!({
                    "replica_path": path,
                    "remote_profile": remote_profile,
                    "report": report,
                }))
                .map_err(serialize_error);
            }
            let verb = if report.bootstrapped {
                "bootstrapped"
            } else {
                "updated"
            };
            Ok(format!(
                "replica {verb}: {} profile={} actor={} seq={} changes={} resources={} tombstones={}",
                path.display(),
                remote_profile,
                report.identity.actor_id,
                report.after_seq,
                report.changes,
                report.resource_count,
                report.tombstone_count,
            ))
        }
        ReplicaSub::Status { replica_path } => {
            let path = resolve_replica_path(store_path, replica_path);
            let store = ReplicaStore::open(&path).map_err(client_error)?;
            let status = store.status().map_err(client_error)?;
            if json_output {
                return serde_json::to_string_pretty(&json!({
                    "replica_path": path,
                    "status": status,
                }))
                .map_err(serialize_error);
            }
            if let (Some(scope), Some(epoch)) = (&status.scope, &status.project_epoch) {
                Ok(format!(
                    "replica status: {} tenant={} project={} epoch={} seq={} resources={} tombstones={} updated_at_ms={}",
                    path.display(),
                    scope.tenant_id,
                    scope.project_id,
                    epoch,
                    status.after_seq,
                    status.resource_count,
                    status.tombstone_count,
                    status.updated_at_ms.unwrap_or_default(),
                ))
            } else {
                Ok(format!(
                    "replica status: {} uninitialized (run `aidememo replica pull --remote-profile NAME`)",
                    path.display()
                ))
            }
        }
        ReplicaSub::Get {
            replica_path,
            resource_kind,
            resource_id,
        } => {
            let path = resolve_replica_path(store_path, replica_path);
            let coordinate = ResourceRef {
                kind: ResourceKind::try_from(resource_kind.as_str())
                    .map_err(|error| AideMemoError::InvalidInput(error.to_string()))?,
                id: ResourceId::try_from(resource_id.as_str())
                    .map_err(|error| AideMemoError::InvalidInput(error.to_string()))?,
            };
            let store = ReplicaStore::open(&path).map_err(client_error)?;
            let resource = store
                .resource(&coordinate)
                .map_err(client_error)?
                .ok_or_else(|| {
                    AideMemoError::InvalidInput(format!(
                        "cached resource {resource_kind}/{resource_id} was not found"
                    ))
                })?;
            let value = resource_json(&resource)?;
            if json_output {
                serde_json::to_string_pretty(&value).map_err(serialize_error)
            } else {
                Ok(value.to_string())
            }
        }
        ReplicaSub::Reset {
            replica_path,
            force,
        } => {
            if !force {
                return Err(AideMemoError::InvalidInput(
                    "replica reset removes cached scope, cursor, resources, and tombstones; pass --force"
                        .to_owned(),
                ));
            }
            let path = resolve_replica_path(store_path, replica_path);
            let mut store = ReplicaStore::open(&path).map_err(client_error)?;
            store.reset().map_err(client_error)?;
            if json_output {
                Ok(json!({"replica_path": path, "reset": true}).to_string())
            } else {
                Ok(format!("replica reset: {}", path.display()))
            }
        }
    }
}

fn resolve_replica_path(store_path: &Path, explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| {
        let mut path = store_path.to_path_buf();
        let name = store_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("wiki");
        path.set_file_name(format!("{name}.replica.sqlite"));
        path
    })
}

fn resource_json(resource: &aidememo_domain::CanonicalResource) -> Result<Value, AideMemoError> {
    let state = match &resource.state {
        ResourceState::Present { body } => json!({
            "state": "present",
            "body": serde_json::from_slice::<Value>(body).map_err(|error| {
                AideMemoError::InvalidInput(format!("cached resource body is not JSON: {error}"))
            })?,
        }),
        ResourceState::Deleted => json!({"state": "deleted"}),
    };
    Ok(json!({
        "scope": resource.scope,
        "resource": resource.resource,
        "revision": resource.revision,
        "state": state,
    }))
}

fn client_error(error: aidememo_client::ClientError) -> AideMemoError {
    let message = error.to_string();
    match error {
        aidememo_client::ClientError::Domain(_)
        | aidememo_client::ClientError::Remote { .. }
        | aidememo_client::ClientError::Protocol(_)
        | aidememo_client::ClientError::ScopeMismatch { .. }
        | aidememo_client::ClientError::EpochMismatch { .. }
        | aidememo_client::ClientError::CursorMismatch { .. } => {
            AideMemoError::InvalidInput(message)
        }
        aidememo_client::ClientError::Storage { .. }
        | aidememo_client::ClientError::Filesystem { .. }
        | aidememo_client::ClientError::Transport(_) => AideMemoError::Internal(message),
    }
}

fn serialize_error(error: serde_json::Error) -> AideMemoError {
    AideMemoError::Serialize {
        context: "replica output".to_owned(),
        source: error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_replica_path_does_not_replace_embedded_store_suffix() {
        assert_eq!(
            resolve_replica_path(Path::new("/tmp/wiki.sqlite"), None),
            PathBuf::from("/tmp/wiki.sqlite.replica.sqlite")
        );
    }
}
