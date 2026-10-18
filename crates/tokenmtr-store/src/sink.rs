//! Sink bookkeeping: export runs and per-dataset cursors.

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

/// Runs kept per sink; older ones are deleted as new ones arrive (health reads the newest).
pub(crate) const RUN_HISTORY: usize = 1000;

const PRUNE_RUNS: &str = "DELETE FROM sink_run WHERE id IN (SELECT id FROM sink_run WHERE sink = ?1 \
    ORDER BY started_at_us DESC, id DESC LIMIT -1 OFFSET ?2)";

const SET_CURSOR: &str = "INSERT INTO sink_cursor (sink, dataset, revision) \
    VALUES (?1, ?2, ?3) ON CONFLICT (sink, dataset) DO UPDATE SET revision = excluded.revision";

/// Appends one run to the history and drops the sink's runs beyond the newest `keep`.
pub(crate) fn record_run(conn: &Connection, run: &SinkRun, keep: usize) -> Result<(), StoreError> {
    conn.prepare_cached(INSERT_RUN)?.execute(named_params! {
        ":sink": run.sink.as_str(),
        ":dataset": run.dataset.as_str(),
        ":started_at_us": to_us(run.started),
        ":finished_at_us": to_us(run.finished),
        ":row_count": to_i64(run.rows, "row_count")?,
        ":error": run.error,
    })?;
    conn.prepare_cached(PRUNE_RUNS)?
        .execute(params![run.sink.as_str(), to_limit(keep)?])?;
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

/// Sets the cursor. Moving it backwards re-delivers rows, which sinks drop when they hold
/// them already; to rebuild a sink, also delete its output.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_run_keeps_the_newest_runs_of_each_sink() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::migrations::migrations()
            .to_latest(&mut conn)
            .unwrap();
        let run = |sink: &str, started: i64| SinkRun {
            sink: SinkId::try_from(sink.to_owned()).unwrap(),
            dataset: Dataset::FxRates,
            started: Timestamp::from_second(started).unwrap(),
            finished: Timestamp::from_second(started + 1).unwrap(),
            rows: u64::try_from(started).unwrap(),
            error: None,
        };
        record_run(&conn, &run("other", 1), 2).unwrap();
        for started in [10, 30, 20, 40] {
            record_run(&conn, &run("a", started), 2).unwrap();
        }
        let a = SinkId::try_from("a".to_owned()).unwrap();
        let kept: Vec<_> = runs(&conn, &a, 10)
            .unwrap()
            .iter()
            .map(|r| r.rows)
            .collect();
        assert_eq!(kept, [40, 30]);
        let other = SinkId::try_from("other".to_owned()).unwrap();
        assert_eq!(runs(&conn, &other, 10).unwrap().len(), 1);
    }
}
