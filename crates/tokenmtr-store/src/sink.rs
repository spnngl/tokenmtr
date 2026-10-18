//! Sink bookkeeping: export runs (D41) and per-dataset cursors (D17).

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, named_params, params};
use tokenmtr_core::SinkId;

use crate::codec::{RowExt, to_i64, to_limit, to_us};
use crate::error::{CodecError, StoreError};

use crate::types::{Dataset, Revision};

/// One export batch attempt of one sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkRun {
    /// The sink.
    pub sink: SinkId,
    /// The dataset the batch belonged to.
    pub dataset: Dataset,
    /// When the attempt started.
    pub started: Timestamp,
    /// When it ended.
    pub finished: Timestamp,
    /// Rows delivered; zero on failure.
    pub rows: u64,
    /// The failure, when there was one.
    pub error: Option<String>,
}

const INSERT_RUN: &str = "INSERT INTO sink_run \
    (sink, dataset, started_at_us, finished_at_us, row_count, error) \
    VALUES (:sink, :dataset, :started_at_us, :finished_at_us, :row_count, :error)";

const SET_CURSOR: &str = "INSERT INTO sink_cursor (sink, dataset, revision) \
    VALUES (?1, ?2, ?3) ON CONFLICT (sink, dataset) DO UPDATE SET revision = excluded.revision";

/// Appends one run to the history.
pub(crate) fn record_run(conn: &Connection, run: &SinkRun) -> Result<(), StoreError> {
    conn.prepare_cached(INSERT_RUN)?.execute(named_params! {
        ":sink": run.sink.as_str(),
        ":dataset": run.dataset.as_str(),
        ":started_at_us": to_us(run.started),
        ":finished_at_us": to_us(run.finished),
        ":row_count": to_i64(run.rows, "row_count")?,
        ":error": run.error,
    })?;
    Ok(())
}

/// The last revision `sink` acknowledged for `dataset`; [`Revision::ZERO`] before the first.
pub(crate) fn cursor(
    conn: &Connection,
    sink: &SinkId,
    dataset: Dataset,
) -> Result<Revision, StoreError> {
    let revision = conn
        .prepare_cached("SELECT revision FROM sink_cursor WHERE sink = ?1 AND dataset = ?2")?
        .query_row(params![sink.as_str(), dataset.as_str()], |row| row.get(0))
        .optional()?;
    Ok(revision.unwrap_or(Revision::ZERO))
}

/// Sets the cursor. Moving it backwards re-exports rows (D17).
pub(crate) fn set_cursor(
    conn: &Connection,
    sink: &SinkId,
    dataset: Dataset,
    revision: Revision,
) -> Result<(), StoreError> {
    conn.prepare_cached(SET_CURSOR)?
        .execute(params![sink.as_str(), dataset.as_str(), revision])?;
    Ok(())
}

/// The newest runs of `sink`, newest first.
pub(crate) fn runs(
    conn: &Connection,
    sink: &SinkId,
    limit: usize,
) -> Result<Vec<SinkRun>, StoreError> {
    conn.prepare_cached(
        "SELECT dataset, started_at_us, finished_at_us, row_count, error FROM sink_run \
         WHERE sink = ?1 ORDER BY started_at_us DESC, id DESC LIMIT ?2",
    )?
    .query_and_then(params![sink.as_str(), to_limit(limit)?], |row| {
        let dataset: String = row.get("dataset")?;
        Ok(SinkRun {
            sink: sink.clone(),
            dataset: Dataset::from_name(&dataset).ok_or_else(|| CodecError::Value {
                column: "dataset",
                value: dataset,
            })?,
            started: row.ts("started_at_us")?,
            finished: row.ts("finished_at_us")?,
            rows: row.count("row_count")?,
            error: row.get("error")?,
        })
    })?
    .collect()
}
