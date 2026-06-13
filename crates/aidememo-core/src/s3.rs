//! Feature-gated S3 manifest and segment transport.
//!
//! The implementation uses a local filesystem bucket mirror so the protocol
//! is usable in tests and offline environments. The manifest format is JSONL
//! and segment payloads are compressed as `.jsonl.zst`.
//!
//! TODO(phase6): swap the filesystem mirror for a real S3 client (aws-sdk-s3
//! or rusoto). Current callers only exercise the local path — there is no
//! retry, signing, or multi-region story yet.
//! TODO(phase6): manifest compaction in `flush_segments_to_manifest` runs
//! per-flush only. A scheduled background compactor (daemon side) would
//! reclaim space without blocking ingest.

use crate::error::{AideMemoError, Result};
use crate::wal::{WALSegment, wal_compact};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;
use zstd::stream::{decode_all, encode_all};

pub type SegmentId = Ulid;
pub type Manifest = Vec<ManifestEntry>;

const MANIFEST_FILE: &str = "manifest.jsonl";
const SEGMENTS_DIR: &str = "segments";
const MAX_SEGMENTS_BEFORE_COMPACTION: usize = 100;
const MAX_SEGMENT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub segment_id: SegmentId,
    pub uploaded_at: u64,
    pub fact_count: usize,
}

#[derive(Debug, Clone)]
pub struct S3Manifest {
    bucket: String,
    root: PathBuf,
    manifest: Manifest,
}

impl S3Manifest {
    pub fn open(bucket: impl Into<String>) -> Result<Self> {
        let bucket = bucket.into();
        let root = bucket_root(&bucket);
        std::fs::create_dir_all(root.join(SEGMENTS_DIR)).map_err(|source| {
            AideMemoError::StoreOpen {
                path: root.clone(),
                source: Box::new(source),
            }
        })?;

        let manifest_path = root.join(MANIFEST_FILE);
        let manifest = if manifest_path.exists() {
            load_manifest(&manifest_path)?
        } else {
            Vec::new()
        };

        Ok(Self {
            bucket,
            root,
            manifest,
        })
    }

    pub fn append_entry(&mut self, entry: ManifestEntry) -> Result<()> {
        self.manifest.push(entry);
        persist_manifest(&self.root.join(MANIFEST_FILE), &self.manifest)
    }

    pub fn manifest(&self) -> &[ManifestEntry] {
        &self.manifest
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }
}

pub fn download_segment(segment_id: SegmentId) -> Result<String> {
    let root = current_bucket_root();
    let compressed = std::fs::read(segment_path(&root, segment_id)).map_err(|source| {
        AideMemoError::StoreRead {
            table: "segments",
            key: segment_id.to_string(),
            source: Box::new(source),
        }
    })?;

    let decompressed = decode_all(&compressed[..]).map_err(|source| {
        AideMemoError::Internal(format!(
            "failed to decompress segment {segment_id}: {source}"
        ))
    })?;
    String::from_utf8(decompressed).map_err(|source| {
        AideMemoError::Internal(format!("segment {segment_id} is not valid UTF-8: {source}"))
    })
}

pub fn flush_segments_to_manifest(segments: Vec<WALSegment>) -> Result<ManifestEntry> {
    let root = current_bucket_root();
    flush_segments_to_manifest_at(&root, segments)
}

fn flush_segments_to_manifest_at(root: &Path, segments: Vec<WALSegment>) -> Result<ManifestEntry> {
    if segments.is_empty() {
        return Err(AideMemoError::InvalidInput(
            "cannot flush an empty set of WAL segments".to_string(),
        ));
    }

    let needs_compaction = segments.len() > MAX_SEGMENTS_BEFORE_COMPACTION
        || segments
            .iter()
            .any(|segment| segment_size_bytes(segment).unwrap_or(0) > MAX_SEGMENT_BYTES);

    let segment = if needs_compaction {
        let ids = segments.iter().map(|segment| segment.segment_id).collect();
        wal_compact(ids)?
    } else {
        let mut search_sessions = Vec::new();
        let mut search_feedback = Vec::new();
        for wal in segments {
            search_sessions.extend(wal.search_sessions);
            search_feedback.extend(wal.search_feedback);
        }
        WALSegment::from_records(search_sessions, search_feedback)
    };

    persist_segment(root, &segment)?;

    let entry = ManifestEntry {
        segment_id: segment.segment_id,
        uploaded_at: now_ms(),
        fact_count: segment.record_count(),
    };

    let mut manifest = S3Manifest::open(root.to_string_lossy().to_string())?;
    manifest.append_entry(entry.clone())?;
    Ok(entry)
}

fn persist_segment(root: &Path, segment: &WALSegment) -> Result<PathBuf> {
    let compressed = encode_all(segment.jsonl()?.as_bytes(), 0).map_err(|source| {
        AideMemoError::Internal(format!(
            "failed to compress segment {}: {source}",
            segment.segment_id
        ))
    })?;
    std::fs::create_dir_all(root.join(SEGMENTS_DIR)).map_err(|source| {
        AideMemoError::StoreOpen {
            path: root.join(SEGMENTS_DIR),
            source: Box::new(source),
        }
    })?;
    let path = segment_path(root, segment.segment_id);
    std::fs::write(&path, compressed).map_err(|source| AideMemoError::StoreWrite {
        table: "segments",
        key: segment.segment_id.to_string(),
        source: Box::new(source),
    })?;
    Ok(path)
}

fn load_manifest(path: &Path) -> Result<Manifest> {
    let content = std::fs::read_to_string(path).map_err(|source| AideMemoError::StoreRead {
        table: "manifest",
        key: MANIFEST_FILE.to_string(),
        source: Box::new(source),
    })?;

    let mut manifest = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: ManifestEntry =
            serde_json::from_str(line).map_err(|source| AideMemoError::Deserialize {
                context: format!("manifest line {}", idx + 1),
                source,
            })?;
        manifest.push(entry);
    }
    Ok(manifest)
}

fn persist_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    let mut output = String::new();
    for entry in manifest {
        let line = serde_json::to_string(entry).map_err(|source| AideMemoError::Serialize {
            context: "manifest entry".to_string(),
            source,
        })?;
        output.push_str(&line);
        output.push('\n');
    }
    std::fs::write(path, output).map_err(|source| AideMemoError::StoreWrite {
        table: "manifest",
        key: MANIFEST_FILE.to_string(),
        source: Box::new(source),
    })
}

fn bucket_root(bucket: &str) -> PathBuf {
    if let Some(rest) = bucket.strip_prefix("s3://") {
        return std::env::temp_dir()
            .join("aidememo-s3")
            .join(sanitize_bucket(rest));
    }

    let path = PathBuf::from(bucket);
    if path.is_absolute() || path.components().count() > 1 {
        return path;
    }

    std::env::temp_dir()
        .join("aidememo-s3")
        .join(sanitize_bucket(bucket))
}

fn current_bucket_root() -> PathBuf {
    std::env::var("AIDEMEMO_STORAGE")
        .map(|value| bucket_root(&value))
        .unwrap_or_else(|_| std::env::temp_dir().join("aidememo-s3").join("default"))
}

fn sanitize_bucket(bucket: &str) -> String {
    bucket
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn segment_path(root: &Path, segment_id: SegmentId) -> PathBuf {
    root.join(SEGMENTS_DIR)
        .join(format!("{segment_id}.jsonl.zst"))
}

fn segment_size_bytes(segment: &WALSegment) -> Result<usize> {
    Ok(segment.jsonl()?.len())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::WALSegment;

    #[test]
    fn manifest_append_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let mut manifest = S3Manifest::open(tmp.path().to_string_lossy().to_string()).unwrap();
        let entry = ManifestEntry {
            segment_id: SegmentId::new(),
            uploaded_at: 1,
            fact_count: 2,
        };
        manifest.append_entry(entry.clone()).unwrap();
        assert_eq!(manifest.manifest().len(), 1);
        assert_eq!(manifest.manifest()[0], entry);
    }

    #[test]
    fn flush_writes_segment() {
        let tmp = tempfile::tempdir().unwrap();
        let segment = WALSegment::from_records(Vec::new(), Vec::new());
        let entry = flush_segments_to_manifest_at(tmp.path(), vec![segment]).unwrap();
        assert_eq!(entry.fact_count, 0);
    }
}
