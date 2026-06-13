//! Write-ahead log for search sessions and feedback.
//!
//! The WAL is stored in a local SQLite database as a staging area before S3
//! manifest flushes. When the `s3` feature is disabled this module is not
//! compiled.

use crate::error::{AideMemoError, Result};
use crate::types::{SearchFeedback, SearchSession};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

pub type SegmentId = Ulid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
enum WALLine {
    SearchSession(SearchSession),
    SearchFeedback(SearchFeedback),
}

/// Serialized batch of search sessions and feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WALSegment {
    pub segment_id: SegmentId,
    pub search_sessions: Vec<SearchSession>,
    pub search_feedback: Vec<SearchFeedback>,
    pub created_at: u64,
}

impl WALSegment {
    pub fn new(search_sessions: Vec<SearchSession>, search_feedback: Vec<SearchFeedback>) -> Self {
        Self {
            segment_id: SegmentId::new(),
            search_sessions,
            search_feedback,
            created_at: now_ms(),
        }
    }

    pub fn record_count(&self) -> usize {
        self.search_sessions.len() + self.search_feedback.len()
    }

    pub fn jsonl(&self) -> Result<String> {
        let mut lines = Vec::with_capacity(self.record_count());
        for session in &self.search_sessions {
            lines.push(
                serde_json::to_string(&WALLine::SearchSession(session.clone())).map_err(
                    |source| AideMemoError::Serialize {
                        context: "wal segment search session".to_string(),
                        source,
                    },
                )?,
            );
        }
        for feedback in &self.search_feedback {
            lines.push(
                serde_json::to_string(&WALLine::SearchFeedback(feedback.clone())).map_err(
                    |source| AideMemoError::Serialize {
                        context: "wal segment search feedback".to_string(),
                        source,
                    },
                )?,
            );
        }
        Ok(lines.join("\n"))
    }

    pub fn from_jsonl(segment_id: SegmentId, created_at: u64, content: &str) -> Result<Self> {
        let mut search_sessions = Vec::new();
        let mut search_feedback = Vec::new();

        for (idx, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: WALLine =
                serde_json::from_str(line).map_err(|source| AideMemoError::Deserialize {
                    context: format!("wal line {}", idx + 1),
                    source,
                })?;
            match entry {
                WALLine::SearchSession(session) => search_sessions.push(session),
                WALLine::SearchFeedback(feedback) => search_feedback.push(feedback),
            }
        }

        Ok(Self {
            segment_id,
            search_sessions,
            search_feedback,
            created_at,
        })
    }

    pub fn from_records(
        search_sessions: Vec<SearchSession>,
        search_feedback: Vec<SearchFeedback>,
    ) -> Self {
        Self::new(search_sessions, search_feedback)
    }
}

pub fn wal_append(segment: WALSegment) -> Result<SegmentId> {
    let conn = wal_db()?;
    let bytes = serde_json::to_vec(&segment).map_err(|source| AideMemoError::Serialize {
        context: "wal segment".to_string(),
        source,
    })?;
    conn.execute(
        "INSERT OR REPLACE INTO wal_segments (id, created_at, record_json)
         VALUES (?1, ?2, ?3)",
        params![
            segment.segment_id.to_string(),
            segment.created_at as i64,
            bytes
        ],
    )
    .map_err(|source| sqlite_write("wal_segments", &segment.segment_id.to_string(), source))?;
    Ok(segment.segment_id)
}

pub fn wal_segments() -> Result<Vec<WALSegment>> {
    let conn = wal_db()?;
    let mut stmt = conn
        .prepare("SELECT record_json FROM wal_segments ORDER BY id ASC")
        .map_err(|source| sqlite_read("wal_segments", "<prepare>", source))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|source| sqlite_read("wal_segments", "<iter>", source))?;
    let mut segments = Vec::new();
    for row in rows {
        let bytes = row.map_err(|source| sqlite_read("wal_segments", "<row>", source))?;
        let segment: WALSegment =
            serde_json::from_slice(&bytes).map_err(|source| AideMemoError::Deserialize {
                context: "wal segment".to_string(),
                source,
            })?;
        segments.push(segment);
    }

    segments.sort_by_key(|segment| segment.segment_id);
    Ok(segments)
}

pub fn wal_compact(segment_ids: Vec<SegmentId>) -> Result<WALSegment> {
    let mut combined_sessions = Vec::new();
    let mut combined_feedback = Vec::new();
    let segments = wal_segments()?;
    let mut matched = 0usize;

    for segment in segments {
        if segment_ids.is_empty() || segment_ids.contains(&segment.segment_id) {
            matched += 1;
            combined_sessions.extend(segment.search_sessions);
            combined_feedback.extend(segment.search_feedback);
        }
    }

    if matched == 0 {
        return Err(AideMemoError::InvalidInput(
            "no WAL segments matched compaction request".to_string(),
        ));
    }

    let compacted = WALSegment::new(combined_sessions, combined_feedback);
    let mut conn = wal_db()?;
    let tx = conn
        .transaction()
        .map_err(|source| sqlite_write("wal_segments", "begin", source))?;
    for id in &segment_ids {
        let _ = tx.execute(
            "DELETE FROM wal_segments WHERE id = ?1",
            params![id.to_string()],
        );
    }
    let bytes = serde_json::to_vec(&compacted).map_err(|source| AideMemoError::Serialize {
        context: "compacted wal segment".to_string(),
        source,
    })?;
    tx.execute(
        "INSERT OR REPLACE INTO wal_segments (id, created_at, record_json)
         VALUES (?1, ?2, ?3)",
        params![
            compacted.segment_id.to_string(),
            compacted.created_at as i64,
            bytes
        ],
    )
    .map_err(|source| sqlite_write("wal_segments", &compacted.segment_id.to_string(), source))?;
    tx.commit().map_err(|source| {
        sqlite_write("wal_segments", &compacted.segment_id.to_string(), source)
    })?;

    Ok(compacted)
}

fn wal_db() -> Result<Connection> {
    let path = wal_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| AideMemoError::StoreOpen {
            path: path.clone(),
            source: Box::new(source),
        })?;
    }
    let conn = Connection::open(&path).map_err(|source| AideMemoError::StoreOpen {
        path: path.clone(),
        source: Box::new(source),
    })?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|source| sqlite_write("wal_pragma", "journal_mode", source))?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|source| sqlite_write("wal_pragma", "synchronous", source))?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS wal_segments (
            id TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            record_json BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS wal_meta (
            key TEXT PRIMARY KEY,
            value BLOB NOT NULL
        );
        "#,
    )
    .map_err(|source| sqlite_write("wal_schema", "<batch>", source))?;
    Ok(conn)
}

fn wal_path() -> PathBuf {
    if let Ok(storage) = std::env::var("AIDEMEMO_STORAGE") {
        let storage = PathBuf::from(storage);
        if storage.extension().is_some() {
            return storage.with_extension("wal.sqlite");
        }
        return storage.join("wal.sqlite");
    }

    std::env::temp_dir().join("aidememo-wal.sqlite")
}

fn sqlite_read(table: &'static str, key: &str, source: rusqlite::Error) -> AideMemoError {
    AideMemoError::StoreRead {
        table,
        key: key.to_string(),
        source: Box::new(source),
    }
}

fn sqlite_write(table: &'static str, key: &str, source: rusqlite::Error) -> AideMemoError {
    AideMemoError::StoreWrite {
        table,
        key: key.to_string(),
        source: Box::new(source),
    }
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

    #[test]
    fn jsonl_roundtrip() {
        let segment = WALSegment::from_records(
            vec![SearchSession {
                id: "01H".to_string(),
                query: "hello".to_string(),
                timestamp: 1,
                result_count: 2,
            }],
            vec![SearchFeedback {
                session_id: "01H".to_string(),
                fact_id: crate::types::FactId::new(),
                helpful: true,
                timestamp: 2,
            }],
        );
        let jsonl = segment.jsonl().unwrap();
        let roundtrip =
            WALSegment::from_jsonl(segment.segment_id, segment.created_at, &jsonl).unwrap();
        assert_eq!(roundtrip.record_count(), 2);
        assert_eq!(roundtrip.search_sessions.len(), 1);
        assert_eq!(roundtrip.search_feedback.len(), 1);
    }
}
