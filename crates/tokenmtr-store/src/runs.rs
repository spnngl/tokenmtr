//! History of ingest passes and backups.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use jiff::Timestamp;
use rusqlite::{Connection, named_params, params};
use tokenmtr_core::AgentId;

use crate::codec::{RowExt, to_i64, to_limit, to_us};
use crate::error::CodecError;
use crate::error::StoreError;

/// One backup attempt (D37).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRun {
    /// When the attempt started.
    pub started: Timestamp,
    /// When it ended.
    pub finished: Timestamp,
    /// The backup file, when one was written.
    pub path: Option<Utf8PathBuf>,
    /// Size of the compressed file.
    pub size_bytes: Option<u64>,
    /// The failure, when there was one.
    pub error: Option<String>,
}

/// One ingest pass of one agent, for the health screen (D5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestRun {
    /// The agent.
    pub agent: AgentId,
    /// When the pass started.
    pub started: Timestamp,
    /// When it ended.
    pub finished: Timestamp,
    /// Files read.
    pub files_parsed: u64,
    /// Bytes read.
    pub bytes_parsed: u64,
    /// Lines read.
    pub lines_read: u64,
    /// Usage events found.
    pub events_found: u64,
    /// Events inserted or changed.
    pub events_changed: u64,
    /// Lines skipped, by reason.
    pub skipped: BTreeMap<String, u64>,
}

/// Appends a backup attempt.
pub(crate) fn record_backup(conn: &Connection, run: &BackupRun) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO backup_run (started_at_us, finished_at_us, path, size_bytes, error) \
         VALUES (:started_at_us, :finished_at_us, :path, :size_bytes, :error)",
    )?
    .execute(named_params! {
        ":started_at_us": to_us(run.started),
        ":finished_at_us": to_us(run.finished),
        ":path": run.path.as_ref().map(|p| p.as_str()),
        ":size_bytes": run.size_bytes.map(|n| to_i64(n, "size_bytes")).transpose()?,
        ":error": run.error,
    })?;
    Ok(())
}

/// Appends an ingest pass.
pub(crate) fn record_ingest(conn: &Connection, run: &IngestRun) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO ingest_run (agent, started_at_us, finished_at_us, files_parsed, \
         bytes_parsed, lines_read, events_found, events_changed, skipped) \
         VALUES (:agent, :started_at_us, :finished_at_us, :files_parsed, :bytes_parsed, \
         :lines_read, :events_found, :events_changed, :skipped)",
    )?
    .execute(named_params! {
        ":agent": run.agent.as_str(),
        ":started_at_us": to_us(run.started),
        ":finished_at_us": to_us(run.finished),
        ":files_parsed": to_i64(run.files_parsed, "files_parsed")?,
        ":bytes_parsed": to_i64(run.bytes_parsed, "bytes_parsed")?,
        ":lines_read": to_i64(run.lines_read, "lines_read")?,
        ":events_found": to_i64(run.events_found, "events_found")?,
        ":events_changed": to_i64(run.events_changed, "events_changed")?,
        ":skipped": serde_json::to_string(&run.skipped)?,
    })?;
    Ok(())
}

/// The newest ingest passes, newest first; of one agent when given.
pub(crate) fn ingest_runs(
    conn: &Connection,
    agent: Option<AgentId>,
    limit: usize,
) -> Result<Vec<IngestRun>, StoreError> {
    conn.prepare_cached(
        "SELECT agent, started_at_us, finished_at_us, files_parsed, bytes_parsed, lines_read, \
         events_found, events_changed, skipped FROM ingest_run \
         WHERE ?1 IS NULL OR agent = ?1 ORDER BY id DESC LIMIT ?2",
    )?
    .query_and_then(
        params![agent.map(AgentId::as_str), to_limit(limit)?],
        |row| {
            let skipped: String = row.get("skipped")?;
            Ok(IngestRun {
                agent: row.id::<AgentId>("agent")?,
                started: row.ts("started_at_us")?,
                finished: row.ts("finished_at_us")?,
                files_parsed: row.count("files_parsed")?,
                bytes_parsed: row.count("bytes_parsed")?,
                lines_read: row.count("lines_read")?,
                events_found: row.count("events_found")?,
                events_changed: row.count("events_changed")?,
                skipped: serde_json::from_str(&skipped).map_err(CodecError::from)?,
            })
        },
    )?
    .collect()
}
