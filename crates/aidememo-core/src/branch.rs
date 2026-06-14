//! Branch log push / merge for cloud-deployed agents.
//!
//! Backups provide a baseline snapshot. Branch logs provide each agent's
//! append-only delta (or full-state fallback) after that baseline. Merge applies
//! those logs through the existing idempotent sync import path.

use crate::backup::{BackupManifest, BackupSyncCursor};
use crate::sync::{SyncExportOpts, SyncImportStats, cursor_from_jsonl};
use crate::{AideMemo, AideMemoError, Config, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use ulid::Ulid;

const BRANCHES_DIR: &str = "branches";
const SEGMENTS_DIR: &str = "segments";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchSegmentManifest {
    pub schema: u32,
    pub branch_id: String,
    pub segment_id: String,
    pub created_at_ms: u64,
    pub base_backup_id: Option<String>,
    pub export_mode: BranchExportMode,
    pub base_cursor: Option<BackupSyncCursor>,
    pub end_cursor: Option<BackupSyncCursor>,
    pub records: usize,
    pub object: String,
    pub compression: String,
    pub stored_bytes: u64,
    pub stored_sha256: String,
    pub payload_bytes: u64,
    pub payload_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BranchExportMode {
    Full,
    SinceBase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchPushReport {
    pub branch_id: String,
    pub segment_id: String,
    pub destination: String,
    pub manifest_uri: String,
    pub segment_uri: String,
    pub records_exported: usize,
    pub export_mode: BranchExportMode,
    pub base_backup_id: Option<String>,
    pub end_cursor: Option<BackupSyncCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BranchMergeReport {
    pub source: String,
    pub branch: Option<String>,
    pub segments_merged: usize,
    pub entities_inserted: usize,
    pub entities_skipped: usize,
    pub facts_inserted: usize,
    pub facts_skipped: usize,
    pub relations_inserted: usize,
    pub relations_skipped: usize,
    pub errors: usize,
}

pub fn push_local_branch(
    store_path: &Path,
    config: Config,
    branch_id: &str,
    base: Option<&BackupManifest>,
    destination_dir: &Path,
) -> Result<BranchPushReport> {
    let wiki = AideMemo::open(store_path, config)?;
    push_local_branch_for_wiki(&wiki, branch_id, base, destination_dir)
}

pub fn push_local_branch_for_wiki(
    wiki: &AideMemo,
    branch_id: &str,
    base: Option<&BackupManifest>,
    destination_dir: &Path,
) -> Result<BranchPushReport> {
    let branch_id = validate_branch_id(branch_id)?;
    let export = export_branch_segment(wiki, &branch_id, base)?;
    let branch_dir = destination_dir
        .join(BRANCHES_DIR)
        .join(&branch_id)
        .join(SEGMENTS_DIR);
    std::fs::create_dir_all(&branch_dir).map_err(|source| AideMemoError::StoreOpen {
        path: branch_dir.clone(),
        source: Box::new(source),
    })?;

    let segment_path = branch_dir.join(&export.manifest.object);
    std::fs::write(&segment_path, &export.stored).map_err(|source| AideMemoError::StoreWrite {
        table: "branch_segment",
        key: segment_path.display().to_string(),
        source: Box::new(source),
    })?;
    let manifest_path = branch_dir.join(manifest_file_name(&export.manifest.segment_id));
    write_manifest(&manifest_path, &export.manifest)?;

    Ok(BranchPushReport {
        branch_id,
        segment_id: export.manifest.segment_id.clone(),
        destination: destination_dir.display().to_string(),
        manifest_uri: manifest_path.display().to_string(),
        segment_uri: segment_path.display().to_string(),
        records_exported: export.manifest.records,
        export_mode: export.manifest.export_mode,
        base_backup_id: export.manifest.base_backup_id,
        end_cursor: export.manifest.end_cursor,
    })
}

pub fn merge_local_branches(
    store_path: &Path,
    config: Config,
    source_dir: &Path,
    branch: Option<&str>,
) -> Result<BranchMergeReport> {
    let wiki = AideMemo::open(store_path, config)?;
    merge_local_branches_for_wiki(&wiki, source_dir, branch)
}

pub fn merge_local_branches_for_wiki(
    wiki: &AideMemo,
    source_dir: &Path,
    branch: Option<&str>,
) -> Result<BranchMergeReport> {
    let branch = branch.map(validate_branch_id).transpose()?;
    let mut manifest_paths = local_manifest_paths(source_dir, branch.as_deref())?;
    manifest_paths.sort();

    let mut report = BranchMergeReport {
        source: source_dir.display().to_string(),
        branch,
        ..Default::default()
    };
    for path in manifest_paths {
        let manifest = read_manifest(&path)?;
        let segment_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&manifest.object);
        let stored = std::fs::read(&segment_path).map_err(|source| AideMemoError::StoreRead {
            table: "branch_segment",
            key: segment_path.display().to_string(),
            source: Box::new(source),
        })?;
        let payload = validate_and_decode(&manifest, &stored)?;
        let jsonl = std::str::from_utf8(&payload)
            .map_err(|source| AideMemoError::Internal(format!("branch segment UTF-8: {source}")))?;
        let stats = wiki.sync_import(jsonl)?;
        report.apply_stats(stats);
        report.segments_merged += 1;
    }
    Ok(report)
}

#[cfg(feature = "s3")]
pub async fn push_s3_branch(
    store_path: &Path,
    config: Config,
    branch_id: &str,
    base: Option<&BackupManifest>,
    destination: &str,
) -> Result<BranchPushReport> {
    let branch_id = validate_branch_id(branch_id)?;
    let export = export_branch_segment_compressed(store_path, config, &branch_id, base)?;
    let location = S3Location::parse(destination)?;
    let branch_prefix = location
        .join(BRANCHES_DIR)
        .join(&branch_id)
        .join(SEGMENTS_DIR);
    let segment_key = branch_prefix.object_key(&export.manifest.object);
    let manifest_key = branch_prefix.object_key(&manifest_file_name(&export.manifest.segment_id));
    let manifest_bytes =
        serde_json::to_vec_pretty(&export.manifest).map_err(|source| AideMemoError::Serialize {
            context: "branch segment manifest".to_string(),
            source,
        })?;
    let client = s3_client().await;
    put_s3_object(&client, &location.bucket, &segment_key, export.stored).await?;
    put_s3_object(&client, &location.bucket, &manifest_key, manifest_bytes).await?;

    Ok(BranchPushReport {
        branch_id,
        segment_id: export.manifest.segment_id.clone(),
        destination: destination.to_string(),
        manifest_uri: format!("s3://{}/{}", location.bucket, manifest_key),
        segment_uri: format!("s3://{}/{}", location.bucket, segment_key),
        records_exported: export.manifest.records,
        export_mode: export.manifest.export_mode,
        base_backup_id: export.manifest.base_backup_id,
        end_cursor: export.manifest.end_cursor,
    })
}

#[cfg(feature = "s3")]
pub async fn merge_s3_branches(
    store_path: &Path,
    config: Config,
    source: &str,
    branch: Option<&str>,
) -> Result<BranchMergeReport> {
    let branch = branch.map(validate_branch_id).transpose()?;
    let location = S3Location::parse(source)?;
    let prefix = if let Some(branch_id) = branch.as_deref() {
        location
            .join(BRANCHES_DIR)
            .join(branch_id)
            .join(SEGMENTS_DIR)
            .prefix
    } else {
        location.join(BRANCHES_DIR).prefix
    };
    let client = s3_client().await;
    let mut manifest_keys = list_s3_manifest_keys(&client, &location.bucket, &prefix).await?;
    manifest_keys.sort();

    let wiki = AideMemo::open(store_path, config)?;
    let mut report = BranchMergeReport {
        source: source.to_string(),
        branch,
        ..Default::default()
    };
    for manifest_key in manifest_keys {
        let manifest_bytes = get_s3_object(&client, &location.bucket, &manifest_key).await?;
        let manifest: BranchSegmentManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|source| {
                AideMemoError::Deserialize {
                    context: "branch segment manifest".to_string(),
                    source,
                }
            })?;
        let parent = manifest_key.rsplit_once('/').map(|(parent, _)| parent);
        let segment_key = if let Some(parent) = parent {
            format!("{parent}/{}", manifest.object)
        } else {
            manifest.object.clone()
        };
        let stored = get_s3_object(&client, &location.bucket, &segment_key).await?;
        let payload = validate_and_decode(&manifest, &stored)?;
        let jsonl = std::str::from_utf8(&payload)
            .map_err(|source| AideMemoError::Internal(format!("branch segment UTF-8: {source}")))?;
        let stats = wiki.sync_import(jsonl)?;
        report.apply_stats(stats);
        report.segments_merged += 1;
    }
    Ok(report)
}

struct BranchExport {
    manifest: BranchSegmentManifest,
    stored: Vec<u8>,
}

fn export_branch_segment(
    wiki: &AideMemo,
    branch_id: &str,
    base: Option<&BackupManifest>,
) -> Result<BranchExport> {
    export_branch_segment_with_compression(wiki, branch_id, base, false)
}

#[cfg(feature = "s3")]
fn export_branch_segment_compressed(
    store_path: &Path,
    config: Config,
    branch_id: &str,
    base: Option<&BackupManifest>,
) -> Result<BranchExport> {
    let wiki = AideMemo::open(store_path, config)?;
    export_branch_segment_with_compression(&wiki, branch_id, base, true)
}

fn export_branch_segment_with_compression(
    wiki: &AideMemo,
    branch_id: &str,
    base: Option<&BackupManifest>,
    compress: bool,
) -> Result<BranchExport> {
    let base_cursor = base.and_then(|manifest| manifest.sync_cursor.clone());
    let since = base_cursor
        .as_ref()
        .map(BackupSyncCursor::to_sync)
        .transpose()?
        .unwrap_or_default();
    let export_mode = if base_cursor.is_some() {
        BranchExportMode::SinceBase
    } else {
        BranchExportMode::Full
    };
    let mut payload = Vec::new();
    let records = wiki.sync_export(
        SyncExportOpts {
            since,
            include_relations: true,
            ..Default::default()
        },
        &mut payload,
    )?;
    let jsonl = std::str::from_utf8(&payload)
        .map_err(|source| AideMemoError::Internal(format!("branch segment UTF-8: {source}")))?;
    let end_cursor = Some(BackupSyncCursor::from_sync(cursor_from_jsonl(jsonl)?));
    let payload_bytes = payload.len() as u64;
    let payload_sha256 = sha256_hex(&payload);

    let (object, compression, stored) = if compress {
        #[cfg(feature = "s3")]
        {
            let stored = zstd::stream::encode_all(&payload[..], 0).map_err(|source| {
                AideMemoError::Internal(format!("branch segment compression failed: {source}"))
            })?;
            (
                format!("{}.jsonl.zst", Ulid::new()),
                "zstd".to_string(),
                stored,
            )
        }
        #[cfg(not(feature = "s3"))]
        {
            return Err(AideMemoError::InvalidInput(
                "compressed branch segments require the `s3` feature".to_string(),
            ));
        }
    } else {
        (
            format!("{}.jsonl", Ulid::new()),
            "none".to_string(),
            payload,
        )
    };
    let segment_id = object
        .split('.')
        .next()
        .unwrap_or(object.as_str())
        .to_string();
    let stored_sha256 = sha256_hex(&stored);
    let manifest = BranchSegmentManifest {
        schema: 1,
        branch_id: branch_id.to_string(),
        segment_id,
        created_at_ms: crate::time::current_epoch_ms(),
        base_backup_id: base.map(|manifest| manifest.backup_id.clone()),
        export_mode,
        base_cursor,
        end_cursor,
        records,
        object,
        compression,
        stored_bytes: stored.len() as u64,
        stored_sha256,
        payload_bytes,
        payload_sha256,
    };
    Ok(BranchExport { manifest, stored })
}

fn local_manifest_paths(source_dir: &Path, branch: Option<&str>) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    if let Some(branch) = branch {
        collect_manifest_paths(
            &source_dir
                .join(BRANCHES_DIR)
                .join(branch)
                .join(SEGMENTS_DIR),
            &mut paths,
        )?;
    } else {
        let branches = source_dir.join(BRANCHES_DIR);
        if !branches.exists() {
            return Ok(paths);
        }
        for entry in std::fs::read_dir(&branches).map_err(|source| AideMemoError::StoreRead {
            table: "branch",
            key: branches.display().to_string(),
            source: Box::new(source),
        })? {
            let entry = entry.map_err(|source| AideMemoError::StoreRead {
                table: "branch",
                key: branches.display().to_string(),
                source: Box::new(source),
            })?;
            let path = entry.path().join(SEGMENTS_DIR);
            collect_manifest_paths(&path, &mut paths)?;
        }
    }
    Ok(paths)
}

fn collect_manifest_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(|source| AideMemoError::StoreRead {
        table: "branch_segment",
        key: dir.display().to_string(),
        source: Box::new(source),
    })? {
        let entry = entry.map_err(|source| AideMemoError::StoreRead {
            table: "branch_segment",
            key: dir.display().to_string(),
            source: Box::new(source),
        })?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".manifest.json"))
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn validate_and_decode(manifest: &BranchSegmentManifest, stored: &[u8]) -> Result<Vec<u8>> {
    if manifest.schema != 1 {
        return Err(AideMemoError::InvalidInput(format!(
            "unsupported branch segment schema {}",
            manifest.schema
        )));
    }
    if manifest.stored_bytes != stored.len() as u64 {
        return Err(AideMemoError::InvalidInput(
            "branch segment stored size mismatch".to_string(),
        ));
    }
    if manifest.stored_sha256 != sha256_hex(stored) {
        return Err(AideMemoError::InvalidInput(
            "branch segment stored checksum mismatch".to_string(),
        ));
    }
    let payload = decode_payload(&manifest.compression, stored)?;
    if manifest.payload_bytes != payload.len() as u64 {
        return Err(AideMemoError::InvalidInput(
            "branch segment payload size mismatch".to_string(),
        ));
    }
    if manifest.payload_sha256 != sha256_hex(&payload) {
        return Err(AideMemoError::InvalidInput(
            "branch segment payload checksum mismatch".to_string(),
        ));
    }
    Ok(payload)
}

fn decode_payload(compression: &str, stored: &[u8]) -> Result<Vec<u8>> {
    match compression {
        "none" => Ok(stored.to_vec()),
        #[cfg(feature = "s3")]
        "zstd" => zstd::stream::decode_all(stored).map_err(|source| {
            AideMemoError::Internal(format!("branch segment decompression failed: {source}"))
        }),
        #[cfg(not(feature = "s3"))]
        "zstd" => Err(AideMemoError::InvalidInput(
            "zstd branch segment requires a build with the `s3` feature".to_string(),
        )),
        other => Err(AideMemoError::InvalidInput(format!(
            "unsupported branch segment compression `{other}`"
        ))),
    }
}

fn write_manifest(path: &Path, manifest: &BranchSegmentManifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|source| AideMemoError::Serialize {
        context: "branch segment manifest".to_string(),
        source,
    })?;
    std::fs::write(path, bytes).map_err(|source| AideMemoError::StoreWrite {
        table: "branch_segment_manifest",
        key: path.display().to_string(),
        source: Box::new(source),
    })
}

fn read_manifest(path: &Path) -> Result<BranchSegmentManifest> {
    let bytes = std::fs::read(path).map_err(|source| AideMemoError::StoreRead {
        table: "branch_segment_manifest",
        key: path.display().to_string(),
        source: Box::new(source),
    })?;
    serde_json::from_slice(&bytes).map_err(|source| AideMemoError::Deserialize {
        context: "branch segment manifest".to_string(),
        source,
    })
}

fn manifest_file_name(segment_id: &str) -> String {
    format!("{segment_id}.manifest.json")
}

fn validate_branch_id(branch_id: &str) -> Result<String> {
    let trimmed = branch_id.trim();
    if trimmed.is_empty() {
        return Err(AideMemoError::InvalidInput(
            "branch id must not be empty".to_string(),
        ));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(AideMemoError::InvalidInput(
            "branch id may only contain ASCII letters, digits, '.', '-', or '_'".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

impl BranchMergeReport {
    fn apply_stats(&mut self, stats: SyncImportStats) {
        self.entities_inserted += stats.entities_inserted;
        self.entities_skipped += stats.entities_skipped;
        self.facts_inserted += stats.facts_inserted;
        self.facts_skipped += stats.facts_skipped;
        self.relations_inserted += stats.relations_inserted;
        self.relations_skipped += stats.relations_skipped;
        self.errors += stats.errors;
    }
}

#[cfg(feature = "s3")]
#[derive(Debug, Clone)]
struct S3Location {
    bucket: String,
    prefix: String,
}

#[cfg(feature = "s3")]
impl S3Location {
    fn parse(uri: &str) -> Result<Self> {
        let Some(rest) = uri.strip_prefix("s3://") else {
            return Err(AideMemoError::InvalidInput(format!(
                "expected s3:// URI, got {uri}"
            )));
        };
        let mut parts = rest.splitn(2, '/');
        let bucket = parts.next().unwrap_or_default().trim();
        if bucket.is_empty() {
            return Err(AideMemoError::InvalidInput(
                "S3 branch URI must include a bucket".to_string(),
            ));
        }
        let prefix = parts
            .next()
            .unwrap_or_default()
            .trim_matches('/')
            .to_string();
        Ok(Self {
            bucket: bucket.to_string(),
            prefix,
        })
    }

    fn join(&self, child: &str) -> Self {
        let child = child.trim_matches('/');
        let prefix = if self.prefix.is_empty() {
            child.to_string()
        } else {
            format!("{}/{}", self.prefix, child)
        };
        Self {
            bucket: self.bucket.clone(),
            prefix,
        }
    }

    fn object_key(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", self.prefix, name.trim_start_matches('/'))
        }
    }
}

#[cfg(feature = "s3")]
async fn s3_client() -> aws_sdk_s3::Client {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    aws_sdk_s3::Client::new(&config)
}

#[cfg(feature = "s3")]
async fn put_s3_object(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    bytes: Vec<u8>,
) -> Result<()> {
    use aws_sdk_s3::primitives::ByteStream;
    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(bytes))
        .send()
        .await
        .map_err(|source| {
            AideMemoError::Internal(format!("S3 put s3://{bucket}/{key} failed: {source}"))
        })?;
    Ok(())
}

#[cfg(feature = "s3")]
async fn get_s3_object(client: &aws_sdk_s3::Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    let output = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(|source| {
            AideMemoError::Internal(format!("S3 get s3://{bucket}/{key} failed: {source}"))
        })?;
    let bytes = output.body.collect().await.map_err(|source| {
        AideMemoError::Internal(format!("S3 read s3://{bucket}/{key} failed: {source}"))
    })?;
    Ok(bytes.into_bytes().to_vec())
}

#[cfg(feature = "s3")]
async fn list_s3_manifest_keys(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut continuation = None;
    loop {
        let mut req = client.list_objects_v2().bucket(bucket).prefix(prefix);
        if let Some(token) = continuation {
            req = req.continuation_token(token);
        }
        let out = req.send().await.map_err(|source| {
            AideMemoError::Internal(format!("S3 list s3://{bucket}/{prefix} failed: {source}"))
        })?;
        for object in out.contents() {
            if let Some(key) = object.key() {
                if key.ends_with(".manifest.json") {
                    keys.push(key.to_string());
                }
            }
        }
        continuation = out.next_continuation_token().map(str::to_string);
        if continuation.is_none() {
            break;
        }
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntityInput, EntityType, FactInput, FactType};

    #[test]
    #[cfg(feature = "sqlite")]
    fn local_branch_push_merge_applies_delta_from_backup_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let base_store = dir.path().join("base.sqlite");
        let agent_store = dir.path().join("agent.sqlite");
        let merged_store = dir.path().join("merged.sqlite");
        let backups = dir.path().join("backups");
        let branches = dir.path().join("branch-log");

        let mut config = Config::default();
        config.store.backend = "sqlite".to_string();
        let base = AideMemo::open(&base_store, config.clone()).unwrap();
        let entity = base
            .entity_add(EntityInput {
                name: "Branching".to_string(),
                entity_type: Some(EntityType::Concept),
                ..Default::default()
            })
            .unwrap();
        base.fact_add(FactInput {
            content: "Base memory fact".to_string(),
            entity_ids: Some(vec![entity]),
            fact_type: Some(FactType::Claim),
            ..Default::default()
        })
        .unwrap();

        let backup = crate::backup::create_local_backup(&base_store, "sqlite", &backups).unwrap();
        let backup_dir = PathBuf::from(&backup.destination);
        crate::backup::restore_local_backup(&backup_dir, &agent_store, "sqlite", false).unwrap();
        crate::backup::restore_local_backup(&backup_dir, &merged_store, "sqlite", false).unwrap();

        let agent = AideMemo::open(&agent_store, config.clone()).unwrap();
        let branch_entity = agent.entity_get("Branching").unwrap().id;
        agent
            .fact_add(FactInput {
                content: "Agent branch memory fact".to_string(),
                entity_ids: Some(vec![branch_entity]),
                fact_type: Some(FactType::Lesson),
                ..Default::default()
            })
            .unwrap();

        let push = push_local_branch(
            &agent_store,
            config.clone(),
            "agent-a",
            Some(&backup.manifest),
            &branches,
        )
        .unwrap();
        assert_eq!(push.export_mode, BranchExportMode::SinceBase);
        assert!(push.records_exported >= 1);

        let merge = merge_local_branches(&merged_store, config.clone(), &branches, Some("agent-a"))
            .unwrap();
        assert_eq!(merge.segments_merged, 1);
        assert_eq!(merge.facts_inserted, 1);

        let merged = AideMemo::open(&merged_store, config).unwrap();
        let stats = merged.stats().unwrap();
        assert_eq!(stats.fact_count, 2);
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn local_branch_merge_can_select_winning_experiment_and_discard_others() {
        let dir = tempfile::tempdir().unwrap();
        let base_store = dir.path().join("base.sqlite");
        let candidate_a_store = dir.path().join("candidate-a.sqlite");
        let candidate_b_store = dir.path().join("candidate-b.sqlite");
        let selected_store = dir.path().join("selected.sqlite");
        let merged_all_store = dir.path().join("merged-all.sqlite");
        let backups = dir.path().join("backups");
        let branches = dir.path().join("branches");

        let mut config = Config::default();
        config.store.backend = "sqlite".to_string();
        let base = AideMemo::open(&base_store, config.clone()).unwrap();
        let entity = base
            .entity_add(EntityInput {
                name: "WhatIf".to_string(),
                entity_type: Some(EntityType::Concept),
                ..Default::default()
            })
            .unwrap();
        base.fact_add(FactInput {
            content: "Baseline memory before experiments".to_string(),
            entity_ids: Some(vec![entity]),
            fact_type: Some(FactType::Claim),
            ..Default::default()
        })
        .unwrap();

        let backup = crate::backup::create_local_backup(&base_store, "sqlite", &backups).unwrap();
        let backup_dir = PathBuf::from(&backup.destination);
        for store in [
            &candidate_a_store,
            &candidate_b_store,
            &selected_store,
            &merged_all_store,
        ] {
            crate::backup::restore_local_backup(&backup_dir, store, "sqlite", false).unwrap();
        }

        add_fact_to_existing_entity(
            &candidate_a_store,
            config.clone(),
            "WhatIf",
            "Candidate A tried broad context and produced noisy results",
        );
        add_fact_to_existing_entity(
            &candidate_b_store,
            config.clone(),
            "WhatIf",
            "Candidate B tried focused context and produced best results",
        );

        let push_a = push_local_branch(
            &candidate_a_store,
            config.clone(),
            "candidate-a",
            Some(&backup.manifest),
            &branches,
        )
        .unwrap();
        let push_b = push_local_branch(
            &candidate_b_store,
            config.clone(),
            "candidate-b",
            Some(&backup.manifest),
            &branches,
        )
        .unwrap();
        assert_eq!(push_a.export_mode, BranchExportMode::SinceBase);
        assert_eq!(push_b.export_mode, BranchExportMode::SinceBase);
        assert_eq!(push_a.records_exported, 1);
        assert_eq!(push_b.records_exported, 1);

        let selected_merge = merge_local_branches(
            &selected_store,
            config.clone(),
            &branches,
            Some("candidate-b"),
        )
        .unwrap();
        assert_eq!(selected_merge.segments_merged, 1);
        assert_eq!(selected_merge.facts_inserted, 1);
        let selected = fact_contents(&selected_store, config.clone());
        assert_eq!(selected.len(), 2);
        assert!(
            selected
                .iter()
                .any(|content| content.contains("Candidate B"))
        );
        assert!(
            !selected
                .iter()
                .any(|content| content.contains("Candidate A"))
        );

        let repeated_merge = merge_local_branches(
            &selected_store,
            config.clone(),
            &branches,
            Some("candidate-b"),
        )
        .unwrap();
        assert_eq!(repeated_merge.segments_merged, 1);
        assert_eq!(repeated_merge.facts_inserted, 0);
        assert_eq!(fact_contents(&selected_store, config.clone()).len(), 2);

        let merge_all =
            merge_local_branches(&merged_all_store, config.clone(), &branches, None).unwrap();
        assert_eq!(merge_all.segments_merged, 2);
        assert_eq!(merge_all.facts_inserted, 2);
        let merged_all = fact_contents(&merged_all_store, config);
        assert_eq!(merged_all.len(), 3);
        assert!(
            merged_all
                .iter()
                .any(|content| content.contains("Candidate A"))
        );
        assert!(
            merged_all
                .iter()
                .any(|content| content.contains("Candidate B"))
        );
    }

    fn add_fact_to_existing_entity(
        store_path: &Path,
        config: Config,
        entity_name: &str,
        content: &str,
    ) {
        let wiki = AideMemo::open(store_path, config).unwrap();
        let entity = wiki.entity_get(entity_name).unwrap().id;
        wiki.fact_add(FactInput {
            content: content.to_string(),
            entity_ids: Some(vec![entity]),
            fact_type: Some(FactType::Lesson),
            ..Default::default()
        })
        .unwrap();
    }

    fn fact_contents(store_path: &Path, config: Config) -> Vec<String> {
        let wiki = AideMemo::open(store_path, config).unwrap();
        let mut contents: Vec<String> = wiki
            .fact_list(crate::FactListOpts::default())
            .unwrap()
            .into_iter()
            .map(|fact| fact.content)
            .collect();
        contents.sort();
        contents
    }
}
