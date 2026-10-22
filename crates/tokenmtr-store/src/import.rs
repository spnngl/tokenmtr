//! Rows imported from the SQLite exports of other machines, and the cursors and runs of those
//! imports.
//!
//! Imported rows live in `usage_event` and `tool_call` beside local ones (see the crate doc,
//! "Imported rows"). They are keyed by the ids the origin exported: `dedup_key` and
//! `tool_use_id` hold `import:<id hex>`, the columns the export lacks get neutral values, and a
//! row is replaced only by a row of a higher `origin_revision`.

use camino::Utf8Path;
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, named_params, params};
use tokenmtr_core::{EventId, MachineId, SourceId, UsageEvent};
use uuid::Uuid;

use crate::codec::{RowExt, to_count, to_i64, to_limit, to_u64, to_us};
use crate::error::{CodecError, StoreError};
use crate::tool_calls::ProfiledToolCall;
use crate::types::{Dataset, Revision, UpsertStats};
use crate::{events, tool_calls, versioned};

/// A usage event as another machine exported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedEvent {
    /// The event. `id` is the exported `event_id`; `dedup_key`, `source_file`, `subagent` and
    /// `is_sidechain` do not exist in the export and are replaced by neutral values.
    pub event: UsageEvent,
    /// The flag the origin set on a stats-cache aggregate that transcripts cover.
    pub superseded: bool,
    /// The `revision` of the row in the origin's export.
    pub origin_revision: Revision,
}

/// A tool call as another machine exported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedToolCall {
    /// The exported `tool_call_id`, which keys the row here.
    pub id: EventId,
    /// The call; its `tool_use_id` is replaced by a key derived from `id`.
    pub call: ProfiledToolCall,
    /// The `revision` of the row in the origin's export.
    pub origin_revision: Revision,
}

/// One page of one export file, written together with its cursor.
#[derive(Debug, Clone, Copy)]
pub struct ImportPage<'a> {
    /// The source that read the file.
    pub source: &'a SourceId,
    /// The export file.
    pub path: &'a Utf8Path,
    /// The machine the file belongs to.
    pub machine: MachineId,
    /// Events of the page, in origin revision order.
    pub events: &'a [ImportedEvent],
    /// Tool calls of the page, in origin revision order.
    pub tool_calls: &'a [ImportedToolCall],
}

/// What [`ImportPage`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportPageStats {
    /// Event rows; `unchanged` includes rows skipped for a lower or equal origin revision.
    pub events: UpsertStats,
    /// Tool-call rows, counted the same way.
    pub tool_calls: UpsertStats,
}

/// What a finished read of a file looked like: when it matches the file again, nothing is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportFingerprint {
    /// Size in bytes.
    pub size: u64,
    /// Modification time.
    pub mtime: Timestamp,
    /// `_schema` version of the export.
    pub schema_version: u32,
}

/// Where one export file stands for one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportCursor {
    /// The machine the rows read so far came from.
    pub machine: MachineId,
    /// Last origin revision read of `usage_events`; [`Revision::ZERO`] before the first.
    pub usage_events: Revision,
    /// Last origin revision read of `tool_calls`.
    pub tool_calls: Revision,
    /// Set when the whole file was read, cleared by the next page written.
    pub fingerprint: Option<ImportFingerprint>,
}

/// One attempt to read a file of a source, or the failure that kept it from trying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRun {
    /// The source.
    pub source: SourceId,
    /// The file; `None` when the run failed before it had one.
    pub path: Option<String>,
    /// The dataset read; `None` when the run failed before reading.
    pub dataset: Option<Dataset>,
    /// When the attempt started.
    pub started: Timestamp,
    /// When it ended.
    pub finished: Timestamp,
    /// Rows read; zero on failure.
    pub rows: u64,
    /// The failure, when there was one.
    pub error: Option<String>,
}

/// Rows one source imported, or that forgetting it deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportedRows {
    /// Usage events.
    pub events: u64,
    /// Tool calls.
    pub tool_calls: u64,
}

/// The key an imported usage event is stored under.
fn event_key(id: &EventId) -> String {
    format!("import:{id}")
}

/// Writes the rows of `page` whose origin revision is higher than the stored one, and moves the
/// file's cursor to the last revision of each dataset. The caller has put `conn` inside a write
/// transaction. The cursor loses its fingerprint: the file is not fully read until
/// [`set_fingerprint`].
pub(crate) fn page(
    conn: &Connection,
    now: Timestamp,
    page: &ImportPage<'_>,
) -> Result<ImportPageStats, StoreError> {
    let machine = page.machine.uuid();
    let source = page.source.as_str();
    let events = events_page(conn, now, page, machine.as_bytes(), source)?;
    let tool_calls = tool_calls_page(conn, now, page, machine.as_bytes(), source)?;
    let stored = cursor(conn, page.source, page.path)?.filter(|c| c.machine == page.machine);
    let from = |read: fn(&ImportCursor) -> Revision| stored.as_ref().map_or(Revision::ZERO, read);
    let events_to = page
        .events
        .iter()
        .map(|e| e.origin_revision)
        .fold(from(|c| c.usage_events), Ord::max);
    let calls_to = page
        .tool_calls
        .iter()
        .map(|c| c.origin_revision)
        .fold(from(|c| c.tool_calls), Ord::max);
    conn.prepare_cached(
        "INSERT INTO import_cursor \
         (source, path, machine_id, usage_events_revision, tool_calls_revision) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT (source, path) DO UPDATE SET machine_id = excluded.machine_id, \
         usage_events_revision = excluded.usage_events_revision, \
         tool_calls_revision = excluded.tool_calls_revision, \
         size = NULL, mtime_us = NULL, schema_version = NULL",
    )?
    .execute(params![
        source,
        page.path.as_str(),
        machine.as_bytes(),
        events_to,
        calls_to
    ])?;
    Ok(ImportPageStats { events, tool_calls })
}

fn events_page(
    conn: &Connection,
    now: Timestamp,
    page: &ImportPage<'_>,
    machine: &[u8; 16],
    source: &str,
) -> Result<UpsertStats, StoreError> {
    let mut stored = conn.prepare_cached(
        "SELECT origin_revision FROM usage_event WHERE agent = ?1 AND dedup_key = ?2",
    )?;
    let mut newer = Vec::new();
    for imported in page.events {
        let key = event_key(&imported.event.id);
        let known: Option<Option<Revision>> = stored
            .query_row(params![imported.event.agent.as_str(), key], |row| {
                row.get(0)
            })
            .optional()?;
        if known.flatten().is_none_or(|r| r < imported.origin_revision) {
            newer.push((imported, key));
        }
    }
    let rows = versioned::last_wins(&newer, |(e, key)| (e.event.agent, key.clone()));
    let mut stats = versioned::run(
        conn,
        Dataset::UsageEvents,
        &events::UPSERT_IMPORTED,
        now,
        &rows,
        |upsert, (imported, key), revision, now_us| {
            let neutral = UsageEvent {
                subagent: None,
                is_sidechain: false,
                source_file: None,
                ..imported.event.clone()
            };
            events::bind(upsert, &neutral, key, revision, now_us)?;
            upsert.bind(named_params! {
                ":superseded": imported.superseded,
                ":origin_machine": machine,
                ":origin_source": source,
                ":origin_revision": imported.origin_revision,
            })
        },
    )?;
    stats.unchanged += to_count(page.events.len()).saturating_sub(to_count(rows.len()));
    Ok(stats)
}

fn tool_calls_page(
    conn: &Connection,
    now: Timestamp,
    page: &ImportPage<'_>,
    machine: &[u8; 16],
    source: &str,
) -> Result<UpsertStats, StoreError> {
    let mut stored = conn.prepare_cached(
        "SELECT origin_revision FROM tool_call WHERE agent = ?1 AND tool_use_id = ?2",
    )?;
    let mut newer = Vec::new();
    for imported in page.tool_calls {
        let key = format!("import:{}", imported.id);
        let known: Option<Option<Revision>> = stored
            .query_row(params![imported.call.call.agent.as_str(), key], |row| {
                row.get(0)
            })
            .optional()?;
        if known.flatten().is_none_or(|r| r < imported.origin_revision) {
            newer.push((imported, key));
        }
    }
    let rows = versioned::last_wins(&newer, |(c, key)| (c.call.call.agent, key.clone()));
    let mut stats = versioned::run(
        conn,
        Dataset::ToolCalls,
        &tool_calls::UPSERT_IMPORTED,
        now,
        &rows,
        |upsert, (imported, key), revision, now_us| {
            tool_calls::bind(upsert, &imported.call, key, revision, now_us)?;
            upsert.bind(named_params! {
                ":origin_machine": machine,
                ":origin_source": source,
                ":origin_revision": imported.origin_revision,
            })
        },
    )?;
    stats.unchanged += to_count(page.tool_calls.len()).saturating_sub(to_count(rows.len()));
    Ok(stats)
}

/// The cursor of `path` for `source`.
pub(crate) fn cursor(
    conn: &Connection,
    source: &SourceId,
    path: &Utf8Path,
) -> Result<Option<ImportCursor>, StoreError> {
    conn.prepare_cached(
        "SELECT machine_id, usage_events_revision, tool_calls_revision, size, mtime_us, \
         schema_version FROM import_cursor WHERE source = ?1 AND path = ?2",
    )?
    .query_and_then(params![source.as_str(), path.as_str()], |row| {
        let machine: [u8; 16] = row.get("machine_id")?;
        let size: Option<i64> = row.get("size")?;
        let schema_version: Option<i64> = row.get("schema_version")?;
        let fingerprint = match (size, row.ts_opt("mtime_us")?, schema_version) {
            (Some(size), Some(mtime), Some(version)) => Some(ImportFingerprint {
                size: to_u64(size, "size")?,
                mtime,
                schema_version: u32::try_from(version).map_err(|_| CodecError::Value {
                    column: "schema_version",
                    value: version.to_string(),
                })?,
            }),
            _ => None,
        };
        Ok(ImportCursor {
            machine: MachineId::try_from(Uuid::from_bytes(machine)).map_err(CodecError::from)?,
            usage_events: row.get("usage_events_revision")?,
            tool_calls: row.get("tool_calls_revision")?,
            fingerprint,
        })
    })?
    .next()
    .transpose()
}

/// Records that the whole file was read as `fingerprint` describes, creating the cursor when no
/// page was written (an empty export).
pub(crate) fn set_fingerprint(
    conn: &Connection,
    source: &SourceId,
    path: &Utf8Path,
    machine: MachineId,
    fingerprint: &ImportFingerprint,
) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO import_cursor (source, path, machine_id, size, mtime_us, schema_version) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (source, path) DO UPDATE SET machine_id = excluded.machine_id, \
         size = excluded.size, mtime_us = excluded.mtime_us, \
         schema_version = excluded.schema_version",
    )?
    .execute(params![
        source.as_str(),
        path.as_str(),
        machine.uuid().as_bytes(),
        to_i64(fingerprint.size, "size")?,
        to_us(fingerprint.mtime),
        fingerprint.schema_version,
    ])?;
    Ok(())
}

/// Forgets where `path` stood: the next run reads it from the start.
pub(crate) fn reset(
    conn: &Connection,
    source: &SourceId,
    path: &Utf8Path,
) -> Result<(), StoreError> {
    conn.prepare_cached("DELETE FROM import_cursor WHERE source = ?1 AND path = ?2")?
        .execute(params![source.as_str(), path.as_str()])?;
    Ok(())
}

/// Every file of `source` with its cursor, by path.
pub(crate) fn cursors(
    conn: &Connection,
    source: &SourceId,
) -> Result<Vec<(String, ImportCursor)>, StoreError> {
    let paths = conn
        .prepare_cached("SELECT path FROM import_cursor WHERE source = ?1 ORDER BY path")?
        .query_map([source.as_str()], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut found = Vec::with_capacity(paths.len());
    for path in paths {
        if let Some(cursor) = cursor(conn, source, Utf8Path::new(&path))? {
            found.push((path, cursor));
        }
    }
    Ok(found)
}

/// Runs kept per source; older ones are deleted as new ones arrive (health reads the newest).
pub(crate) const RUN_HISTORY: usize = 1000;

/// Appends one run to the history and drops the source's runs beyond the newest `keep`.
pub(crate) fn record_run(
    conn: &Connection,
    run: &ImportRun,
    keep: usize,
) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO import_run \
         (source, path, dataset, started_at_us, finished_at_us, row_count, error) \
         VALUES (:source, :path, :dataset, :started_at_us, :finished_at_us, :row_count, :error)",
    )?
    .execute(named_params! {
        ":source": run.source.as_str(),
        ":path": run.path,
        ":dataset": run.dataset.map(Dataset::as_str),
        ":started_at_us": to_us(run.started),
        ":finished_at_us": to_us(run.finished),
        ":row_count": to_i64(run.rows, "row_count")?,
        ":error": run.error,
    })?;
    conn.prepare_cached(
        "DELETE FROM import_run WHERE id IN (SELECT id FROM import_run WHERE source = ?1 \
         ORDER BY started_at_us DESC, id DESC LIMIT -1 OFFSET ?2)",
    )?
    .execute(params![run.source.as_str(), to_limit(keep)?])?;
    Ok(())
}

/// The newest runs of `source`, newest first.
pub(crate) fn runs(
    conn: &Connection,
    source: &SourceId,
    limit: usize,
) -> Result<Vec<ImportRun>, StoreError> {
    conn.prepare_cached(
        "SELECT path, dataset, started_at_us, finished_at_us, row_count, error FROM import_run \
         WHERE source = ?1 ORDER BY started_at_us DESC, id DESC LIMIT ?2",
    )?
    .query_and_then(params![source.as_str(), to_limit(limit)?], |row| {
        let dataset: Option<String> = row.get("dataset")?;
        Ok(ImportRun {
            source: source.clone(),
            path: row.get("path")?,
            dataset: dataset
                .map(|name| {
                    Dataset::from_name(&name).ok_or(CodecError::Value {
                        column: "dataset",
                        value: name,
                    })
                })
                .transpose()?,
            started: row.ts("started_at_us")?,
            finished: row.ts("finished_at_us")?,
            rows: row.count("row_count")?,
            error: row.get("error")?,
        })
    })?
    .collect()
}

/// Rows imported by `source`: usage events and tool calls.
pub(crate) fn counts(conn: &Connection, source: &SourceId) -> Result<ImportedRows, StoreError> {
    let count = |table: &str| -> Result<u64, StoreError> {
        let rows: i64 = conn
            .prepare_cached(&format!(
                "SELECT COUNT(*) FROM {table} WHERE origin_source = ?1"
            ))?
            .query_row([source.as_str()], |row| row.get(0))?;
        Ok(to_u64(rows, "count")?)
    };
    Ok(ImportedRows {
        events: count("usage_event")?,
        tool_calls: count("tool_call")?,
    })
}

/// Deletes the rows `source` imported, its cursors and its runs. Returns the rows deleted.
pub(crate) fn forget(conn: &Connection, source: &SourceId) -> Result<ImportedRows, StoreError> {
    let report = counts(conn, source)?;
    for table in ["usage_event", "tool_call"] {
        conn.prepare_cached(&format!("DELETE FROM {table} WHERE origin_source = ?1"))?
            .execute([source.as_str()])?;
    }
    for table in ["import_cursor", "import_run"] {
        conn.prepare_cached(&format!("DELETE FROM {table} WHERE source = ?1"))?
            .execute([source.as_str()])?;
    }
    Ok(report)
}
